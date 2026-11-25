#!/usr/bin/env -S uv run --locked --script
# /// script
# requires-python = ">=3.13"
# dependencies = ["click", "fast-simplification", "libigl", "manifold3d", "networkx", "numpy", "pillow", "pymeshfix", "rtree", "scipy", "trimesh", "xatlas"]
# ///
"""Repairs a sourced .glb into one closed, outward-wound solid resting on y = 0 and centered
on the stand, scaled so its longest side is `--size-m`, carrying the source's coloring as a
texture baked at full detail onto a fresh UV atlas. `bake-cake` turns the result into a playable
cake, so rejection of what this cannot fix happens there and here.

Run `uv lock --script tools/repair-model.py` after changing the dependencies above.
"""

import math
from pathlib import Path
from typing import Literal, NamedTuple, get_args

import click
import igl
import manifold3d
import numpy as np
import pymeshfix
import trimesh
import xatlas
from PIL import Image, ImageFilter
from scipy import ndimage, spatial

__all__ = ["Finish", "Hollow", "Recolor", "Repaired", "repair_model", "sample_bilinear"]

# The vertex snapping grid, as a share of the model's longest side.
_WELD_RATIO = 1e-4
# A piece within this many weld steps of the model's largest piece touches it, and joins it.
_JOIN_WELD_STEPS = 2
# A solid needs at least a tetrahedron; fewer triangles are slivers to ignore.
_MIN_SOLID_FACES = 4
# A repaired piece must keep at least this share of the size it had.
_KEPT_SIZE_RATIO = 0.99
# A piece under this share of the model's volume is debris, dropped when it floats free.
_DEBRIS_VOLUME_RATIO = 0.01
# A piece that will not close is trim, dropped with a warning, when its convex hull holds under this
# share of the whole model's: a fin, a string, a propped-open lid, thin parts no cake could hold.
# A bulkier one fails the repair.
_TRIM_HULL_VOLUME_RATIO = 0.03
# The default triangle budget: the bake's collider decomposition takes minutes on a model far over it.
_DEFAULT_MAX_TRIANGLES = 40000
# A decimated solid whose volume changed by more than this share, or whose bounds moved by more
# than this share of its longest side, lost its shape.
_DECIMATED_VOLUME_CHANGE = 0.05
_DECIMATED_BOUNDS_SHIFT = 0.01
# Merging edges within a tolerance starts at this share of the longest side, which on a 317000
# triangle piano kept its volume to 0.2%.
_SIMPLIFY_START_RATIO = 1e-4
# The side of the baked texture in texels. At 2048 a 0.24 m model gets texels of about 0.2 mm,
# finer than the screen resolves it at any zoom the camera allows and about as fine as the sources.
_DEFAULT_TEXTURE_SIZE = 2048
# Empty texels between charts, so a mipmapped lookup near a chart's edge reads only that chart.
_CHART_PADDING_TEXELS = 4
# How far each chart's colors spread into the gutter around it, so filtering at a seam never
# reads the fill between charts.
_GUTTER_TEXELS = 2 * _CHART_PADDING_TEXELS
# The glb vertex attribute naming the solid vertex each vertex copies, so `cake-bake` rejoins the
# seams without matching positions, which two sheets of a hollowed wall can share.
_SOLID_VERTEX_ATTRIBUTE = "_SOLID_VERTEX"
# Hollowing finds the inside on a voxel grid this fine, in meters at cake size.
_HOLLOW_VOXEL_M = 0.001
# Openings up to about twice this wide, such as a shoe's collar, are bridged over when finding
# the inside to hollow, so the hollow joins the opening instead of stopping a wall short of it.
_HOLLOW_BRIDGE_M = 0.03
# A piece the hollow would cut free with fewer voxels than this is a speck, carved away with the hollow.
_HOLLOW_SPECK_VOXELS = 27
# A texel farther from the source surface than this share of the hollow's wall lies on a carved
# wall. Carved walls lie a whole wall under the bridged surface except where they meet an opening,
# and the decimated outer surface strays from the source far less.
_INSIDE_WALL_SHARE = 0.5
# Texels facing at least this far down (the downward part of their normal) count as underside.
_UNDERSIDE_MIN_DOWN = 0.3
# An underside texel darker than this share of the upper surface's median brightness counts as
# unlit, as a scan's floor-facing side comes out.
_UNDERSIDE_DARK_RATIO = 0.35
# Filled underside texels take the color above them at this share of its brightness, as a
# surface facing the table would be.
_UNDERSIDE_SHADE = 0.8
# Texels facing at least this far up are the upper surface an unlit underside borrows from.
_UPPER_MIN_UP = 0.3

_Axis = Literal["x", "y", "z"]


def _fetch_parts(path: Path) -> list[trimesh.Trimesh]:
    """The model's meshes with their node transforms applied, colors intact."""
    scene = trimesh.load(path, force="scene")
    parts = [mesh for mesh in scene.dump() if isinstance(mesh, trimesh.Trimesh) and len(mesh.faces) > 0]
    if not parts:
        raise click.ClickException(f"{path}: holds no triangles")
    return parts


def _merge(parts: list[trimesh.Trimesh]) -> trimesh.Trimesh:
    merged = trimesh.util.concatenate([trimesh.Trimesh(part.vertices, part.faces, process=False) for part in parts])
    return trimesh.Trimesh(vertices=merged.vertices, faces=merged.faces, process=False)


def _weld(mesh: trimesh.Trimesh) -> trimesh.Trimesh:
    """Snaps vertices to a grid of `_WELD_RATIO` of the longest side and merges those that land together."""
    welded = mesh.copy()
    grid = _WELD_RATIO * float(welded.extents.max())
    welded.vertices = np.round(welded.vertices / grid) * grid
    welded.merge_vertices()
    welded.update_faces(welded.nondegenerate_faces())
    welded.update_faces(welded.unique_faces())
    welded.remove_unreferenced_vertices()
    return welded


def _build_manifold(mesh: trimesh.Trimesh) -> manifold3d.Manifold:
    return manifold3d.Manifold(
        manifold3d.Mesh(
            vert_properties=np.asarray(mesh.vertices, dtype=np.float32), tri_verts=np.asarray(mesh.faces, dtype=np.uint32)
        )
    )


def _close_holes(component: trimesh.Trimesh) -> trimesh.Trimesh:
    """The component with its holes and self-intersections repaired, which must keep its overall size."""
    fixer = pymeshfix.MeshFix(np.asarray(component.vertices, dtype=np.float64), np.asarray(component.faces, dtype=np.int32))
    fixer.repair(joincomp=False, remove_smallest_components=False)
    fixed = trimesh.Trimesh(vertices=fixer.points, faces=fixer.faces, process=False)
    if len(fixed.faces) > 0 and np.any(fixed.extents < _KEPT_SIZE_RATIO * component.extents):
        raise click.ClickException(f"repairing a piece of {len(component.faces)} triangles cut away much of its size")
    return fixed


def _to_solid(component: trimesh.Trimesh) -> manifold3d.Manifold | None:
    """The component as a manifold solid, repaired only if it is not one already, or None if it encloses no volume."""
    patched = component.copy()
    trimesh.repair.fix_inversion(patched)
    solid = _build_manifold(patched)
    if solid.status() != manifold3d.Error.NoError:
        patched = _close_holes(component)
        if len(patched.faces) == 0:
            return None
        trimesh.repair.fix_inversion(patched)
        solid = _build_manifold(patched)
    if solid.status() != manifold3d.Error.NoError:
        raise click.ClickException(f"a piece of {len(component.faces)} triangles will not close: {solid.status().name}")
    return None if solid.is_empty() or solid.volume() <= 0 else solid


def _to_trimesh(solid: manifold3d.Manifold) -> trimesh.Trimesh:
    mesh = solid.to_mesh()
    return trimesh.Trimesh(vertices=mesh.vert_properties[:, :3], faces=mesh.tri_verts, process=False)


def _to_solid_or_drop_trim(component: trimesh.Trimesh, model_hull_volume: float, should_hull_open: bool) -> manifold3d.Manifold | None:
    """The component as a solid, or None when it encloses no volume or is thin trim that will not close.
    With `should_hull_open`, a piece that will not close becomes its convex hull instead, which suits
    a nearly convex part modeled as an open shell, such as a guitar's neck."""
    try:
        return _to_solid(component)
    except click.ClickException as error:
        if should_hull_open and len(component.vertices) >= 4 and component.convex_hull.volume > 0:
            return _build_manifold(component.convex_hull)
        if len(component.vertices) >= 4 and component.convex_hull.volume >= _TRIM_HULL_VOLUME_RATIO * model_hull_volume:
            raise
        click.echo(f"dropped a trim piece of {len(component.faces)} triangles: {error.message}", err=True)
        return None


def _find_pieces(solid: manifold3d.Manifold) -> list[manifold3d.Manifold]:
    """The solid's separate pieces. A sealed void decomposes into an inward-facing shell of its own,
    of negative volume, which is no piece."""
    return [piece for piece in solid.decompose() if piece.volume() > 0]


def _join_touching(union: manifold3d.Manifold, join_distance: float) -> manifold3d.Manifold:
    """The union with each piece that stops within `join_distance` of the largest stretched into it,
    by adding a copy moved `join_distance` past touching along the largest's surface normal. A part
    modeled as resting on another, such as a piano's leg under its body, can stop a hair short of
    it and come out a separate piece."""
    pieces = _find_pieces(union)
    main = max(pieces, key=lambda piece: piece.volume())
    main_mesh = _to_trimesh(main)
    main_vertices, main_faces = np.asarray(main_mesh.vertices, dtype=np.float64), np.asarray(main_mesh.faces, dtype=np.int64)
    bridges = []
    for piece in (piece for piece in pieces if piece is not main):
        vertices = np.asarray(_to_trimesh(piece).vertices, dtype=np.float64)
        squared_distances, face_ids, _ = igl.point_mesh_squared_distance(vertices, main_vertices, main_faces)
        nearest_idx = int(squared_distances.argmin())
        gap = math.sqrt(float(squared_distances[nearest_idx]))
        if gap <= join_distance:
            inward = -main_mesh.face_normals[face_ids[nearest_idx]]
            bridges.append(piece.translate(tuple((gap + join_distance) * inward)))
    return manifold3d.Manifold.batch_boolean([union, *bridges], manifold3d.OpType.Add) if bridges else union


def _calc_cut_height(parts: list[trimesh.Trimesh], height_share: float) -> float:
    """The height `height_share` of the way up the model."""
    lower, upper = _merge(parts).bounds[:, 1]
    return float(lower + height_share * (upper - lower))


def _build_solid(parts: list[trimesh.Trimesh], should_keep_largest: bool, should_hull_open: bool = False, cut_y: float | None = None) -> manifold3d.Manifold:
    welded = _weld(_merge(parts))
    components = [part for part in welded.split(only_watertight=False) if len(part.faces) >= _MIN_SOLID_FACES]
    model_hull_volume = float(welded.convex_hull.volume)
    solids = [solid for solid in (_to_solid_or_drop_trim(component, model_hull_volume, should_hull_open) for component in components) if solid is not None]
    if not solids:
        raise click.ClickException("no piece of the model encloses any volume")
    union = manifold3d.Manifold.batch_boolean(solids, manifold3d.OpType.Add)
    union = _join_touching(union, _JOIN_WELD_STEPS * _WELD_RATIO * float(welded.extents.max()))
    if cut_y is not None:
        union = union.trim_by_plane((0.0, 1.0, 0.0), cut_y)
        if union.is_empty():
            raise click.ClickException("--trim-below cut away the whole model")
    pieces = _to_trimesh(union).split(only_watertight=False)
    if len(pieces) == 1:
        return union
    volumes = np.array([abs(piece.volume) for piece in pieces])
    is_debris = volumes < _DEBRIS_VOLUME_RATIO * volumes.sum()
    if not should_keep_largest and (~is_debris).sum() > 1:
        raise click.ClickException(
            f"{len(pieces)} separate pieces remain after merging, with volume shares "
            f"{np.round(volumes / volumes.sum(), 3).tolist()}; a cake needs one (--keep-largest keeps the biggest)"
        )
    solid = _to_solid(pieces[int(volumes.argmax())])
    assert solid is not None, "the largest piece of a solid encloses volume"
    return solid


def _is_shaped_like(decimated: manifold3d.Manifold, solid: manifold3d.Manifold) -> bool:
    if decimated.status() != manifold3d.Error.NoError or decimated.is_empty():
        return False
    volume_change = abs(decimated.volume() / solid.volume() - 1)
    bounds, decimated_bounds = (np.asarray(manifold.bounding_box()).reshape(2, 3) for manifold in (solid, decimated))
    bounds_shift = np.abs(decimated_bounds - bounds).max() / (bounds[1] - bounds[0]).max()
    return volume_change <= _DECIMATED_VOLUME_CHANGE and bounds_shift <= _DECIMATED_BOUNDS_SHIFT


def _simplify(solid: manifold3d.Manifold, max_triangles: int) -> manifold3d.Manifold:
    """The solid with edges merged within a tolerance doubled until at most `max_triangles` remain."""
    bounds = np.asarray(solid.bounding_box()).reshape(2, 3)
    tolerance = _SIMPLIFY_START_RATIO * float((bounds[1] - bounds[0]).max())
    simplified = solid.simplify(tolerance)
    while simplified.num_tri() > max_triangles:
        tolerance *= 2
        simplified = solid.simplify(tolerance)
    return simplified


def _decimate(solid: manifold3d.Manifold, max_triangles: int) -> manifold3d.Manifold:
    """The solid with its triangles merged down to about `max_triangles`, still one closed solid
    of nearly the same shape. Quadric edge collapse keeps the shape best per triangle, but can
    break it, and then edges are merged within a tolerance instead."""
    mesh = _to_trimesh(solid)
    if len(mesh.faces) <= max_triangles:
        return solid
    simplified = mesh.simplify_quadric_decimation(face_count=max_triangles)
    trimesh.repair.fix_inversion(simplified)
    decimated = _build_manifold(simplified)
    if _is_shaped_like(decimated, solid):
        return decimated
    click.echo(f"decimating {len(mesh.faces)} triangles by edge collapse broke the shape, so merging within a tolerance instead", err=True)
    return _simplify(solid, max_triangles)


class Hollow(NamedTuple):
    """How to hollow a model whose real counterpart is hollow, such as a shoe, in meters at cake size."""

    wall_m: float
    """The least thickness left around the hollow."""
    floor_m: float
    """The height above the model's bottom the hollow stays clear of, such as a shoe's sole."""
    voxel_m: float = _HOLLOW_VOXEL_M
    """The grid the inside is found on; finer follows the model's shape more closely and takes longer."""
    inside_color: tuple[int, int, int] | None = None
    """The plain sRGB color the carved walls take, or None to give them the color of the source surface nearest them."""


def _is_joined(is_kept: np.ndarray, first_idx: tuple[int, ...], second_idx: tuple[int, ...]) -> bool:
    labels, _ = ndimage.label(is_kept)
    return labels[first_idx] == labels[second_idx]


def _find_joining_reach(is_inside: np.ndarray, is_carved: np.ndarray, distance: np.ndarray, piece_idx: tuple[int, ...], main_idx: tuple[int, ...], pitch: float) -> float:
    """The least reach, to within `pitch`, such that leaving the solid within `distance` reach
    uncarved joins the voxel at `piece_idx` to the one at `main_idx`."""

    def is_joined_at(reach: float) -> bool:
        return _is_joined(is_inside & ~(is_carved & (distance > reach)), piece_idx, main_idx)

    low, high = 0.0, float(distance[is_inside].max())
    if not is_joined_at(high):
        raise click.ClickException("hollowing cut a piece free that no part of the solid joins to the rest")
    while high - low > pitch:
        middle = (low + high) / 2
        low, high = (low, middle) if is_joined_at(middle) else (middle, high)
    return high


def _keep_pieces_attached(field: np.ndarray, is_inside: np.ndarray, pitch: float) -> np.ndarray:
    """The hollow's field, carved where positive, changed so the carving cuts no piece free of the
    rest: the solid within the least reach that rejoins a free piece stays uncarved, and a speck is
    carved away. Walls measured from bridged openings come out thinner than asked where the surface
    dips below the bridge, which can cut a part such as a shoe's tongue loose."""
    is_carved = field > 0
    labels, count = ndimage.label(is_inside & ~is_carved)
    if count <= 1:
        return field
    sizes = np.bincount(labels.ravel())[1:]
    main_label = 1 + int(sizes.argmax())
    main_idx = tuple(np.argwhere(labels == main_label)[0])
    kept_field = field.copy()
    for label in range(1, count + 1):
        if label == main_label:
            continue
        is_piece = labels == label
        if sizes[label - 1] < _HOLLOW_SPECK_VOXELS:
            kept_field[is_piece] = pitch
            continue
        distance = ndimage.distance_transform_edt(~is_piece) * pitch
        reach = _find_joining_reach(is_inside, is_carved, distance, tuple(np.argwhere(is_piece)[0]), main_idx, pitch)
        kept_field = np.minimum(kept_field, distance - reach)
    return kept_field


def _hollow(solid: manifold3d.Manifold, hollow: Hollow, unit_per_m: float) -> manifold3d.Manifold:
    """The solid with its inside cut away, leaving walls `hollow.wall_m` thick and the bottom
    `hollow.floor_m` whole, still in one piece. The inside is the solid with its openings bridged
    over, so the hollow runs out through an opening such as a shoe's collar, as the real one does."""
    mesh = _to_trimesh(solid)
    pitch, wall, floor, bridge = (value * unit_per_m for value in (hollow.voxel_m, hollow.wall_m, hollow.floor_m, _HOLLOW_BRIDGE_M))
    margin = bridge + 2 * pitch
    axes = [np.arange(low - margin, high + margin + pitch, pitch) for low, high in zip(*mesh.bounds, strict=True)]
    zs, ys, xs = np.meshgrid(axes[2], axes[1], axes[0], indexing="ij")
    points = np.stack([xs.ravel(), ys.ravel(), zs.ravel()], axis=1)
    winding = igl.fast_winding_number(np.asarray(mesh.vertices, dtype=np.float64), np.asarray(mesh.faces, dtype=np.int64), points)
    is_inside = (winding > 0.5).reshape(xs.shape)
    is_near = ndimage.distance_transform_edt(~is_inside) * pitch <= bridge
    is_bridged = ndimage.distance_transform_edt(is_near) * pitch > bridge
    depth = ndimage.distance_transform_edt(is_bridged) * pitch
    field = _keep_pieces_attached(np.minimum(depth - wall, ys - (mesh.bounds[0, 1] + floor)), is_inside, pitch)
    vertices, faces, _ = igl.marching_cubes(field.ravel(), points, len(axes[0]), len(axes[1]), len(axes[2]), 0.0)
    if len(faces) == 0:
        raise click.ClickException("the model has no inside deep enough to hollow")
    cut = _build_manifold(trimesh.Trimesh(vertices, faces, process=False))
    if cut.status() == manifold3d.Error.NoError and cut.volume() < 0:
        cut = _build_manifold(trimesh.Trimesh(vertices, faces[:, ::-1], process=False))
    if cut.status() != manifold3d.Error.NoError or cut.volume() <= 0:
        raise click.ClickException(f"the hollow did not come out a closed solid: {cut.status().name}")
    hollowed = solid - cut
    piece_count = len(_find_pieces(hollowed))
    if piece_count != 1:
        raise click.ClickException(f"hollowing left {piece_count} pieces instead of one")
    return hollowed


def _srgb_u8_to_linear(srgb: np.ndarray) -> np.ndarray:
    unit = srgb / 255.0
    return np.where(unit <= 0.04045, unit / 12.92, ((unit + 0.055) / 1.055) ** 2.4)


def _linear_to_srgb_u8(linear: np.ndarray) -> np.ndarray:
    clipped = np.clip(linear, 0.0, 1.0)
    srgb = np.where(clipped <= 0.0031308, 12.92 * clipped, 1.055 * clipped ** (1 / 2.4) - 0.055)
    return np.round(srgb * 255).astype(np.uint8)


def _find_texture(part: trimesh.Trimesh) -> Image.Image | None:
    visual = part.visual
    if not isinstance(visual, trimesh.visual.TextureVisuals) or visual.uv is None:
        return None
    material = visual.material
    texture = getattr(material, "baseColorTexture", None) or getattr(material, "image", None)
    return None if texture is None else texture.convert("RGBA")


def _get_color_factor(part: trimesh.Trimesh) -> np.ndarray:
    """The linear RGB the part's material multiplies its texture by, white when it has none."""
    factor = getattr(getattr(part.visual, "material", None), "baseColorFactor", None)
    return np.ones(3) if factor is None else np.asarray(factor, dtype=np.float64)[:3] / 255


def _prefilter_texture(part: trimesh.Trimesh, texture: Image.Image, spacing: float) -> Image.Image:
    """The texture blurred to the footprint of one output texel, `spacing` across in the part's units.

    Reading a texture at one point per output texel aliases any pattern finer than that texel into
    blotches, so a source finer than the output is low-passed first, as a mipmap would be, and one as
    coarse or coarser is read as is. The blur is sized by how many texels cover a unit of surface,
    which is the part's surface area over its UV area.
    """
    uvs = part.visual.uv[part.faces]
    legs = uvs[:, 1:] - uvs[:, :1]
    uv_area = 0.5 * np.abs(legs[:, 0, 0] * legs[:, 1, 1] - legs[:, 0, 1] * legs[:, 1, 0]).sum()
    if uv_area <= 0:
        return texture
    radius = 0.5 * spacing / math.sqrt(part.area / (uv_area * texture.width * texture.height))
    return texture if radius < 0.5 else texture.filter(ImageFilter.GaussianBlur(radius=radius))


def sample_bilinear(image: Image.Image, uvs: np.ndarray) -> np.ndarray:
    """The image's sRGB colors at `uvs` (trimesh's, with v up) filtered as a GPU does, texel centers
    at half-texel offsets and wrapping at the edges as glTF's default sampler repeats. trimesh's own
    sampler puts the image's edge texels' centers on the UV square's edges, which shifts every
    lookup by up to half a texel."""
    pixels = np.asarray(image.convert("RGB"), dtype=np.float64)
    rows, cols = pixels.shape[:2]
    x = uvs[:, 0] * cols - 0.5
    y = (1.0 - uvs[:, 1]) * rows - 0.5
    x0, y0 = np.floor(x), np.floor(y)
    fx, fy = (x - x0)[:, None], (y - y0)[:, None]
    x0, y0 = x0.astype(np.int64), y0.astype(np.int64)

    def texel(dx: int, dy: int) -> np.ndarray:
        return pixels[(y0 + dy) % rows, (x0 + dx) % cols]

    top = texel(0, 0) * (1 - fx) + texel(1, 0) * fx
    bottom = texel(0, 1) * (1 - fx) + texel(1, 1) * fx
    return top * (1 - fy) + bottom * fy


def _sample_part_colors(part: trimesh.Trimesh, face_ids: np.ndarray, points: np.ndarray, spacing: float) -> np.ndarray:
    """The sRGB color of `part`, averaged over `spacing`, at `points`, each on the face of the same row in `face_ids`."""
    visual = part.visual
    bary = trimesh.triangles.points_to_barycentric(part.triangles[face_ids], points)
    texture = _find_texture(part)
    if texture is not None:
        uvs = np.einsum("ni,nij->nj", bary, visual.uv[part.faces[face_ids]])
        texels = sample_bilinear(_prefilter_texture(part, texture, spacing), uvs)
        return _linear_to_srgb_u8(_srgb_u8_to_linear(texels) * _get_color_factor(part))
    if isinstance(visual, trimesh.visual.TextureVisuals):
        return np.tile(_linear_to_srgb_u8(_get_color_factor(part)), (len(points), 1))
    corner_colors = np.asarray(visual.vertex_colors, dtype=np.float64)[part.faces[face_ids]][:, :, :3]
    return np.round(np.einsum("ni,nij->nj", np.clip(bary, 0.0, 1.0), corner_colors)).astype(np.uint8)


def _sample_colors(parts: list[trimesh.Trimesh], points: np.ndarray, spacing: float) -> tuple[np.ndarray, np.ndarray]:
    """The sRGB color of the source model, averaged over `spacing`, at the surface point nearest
    each of `points`, and each point's distance to that surface point."""
    merged = _merge(parts)
    squared_distances, face_ids, closest = igl.point_mesh_squared_distance(
        np.asarray(points, dtype=np.float64), np.asarray(merged.vertices, dtype=np.float64), np.asarray(merged.faces, dtype=np.int64)
    )
    face_counts = [len(part.faces) for part in parts]
    face_starts = np.cumsum([0, *face_counts])
    part_ids = np.repeat(np.arange(len(parts)), face_counts)[face_ids]
    colors = np.empty((len(points), 3), dtype=np.uint8)
    for part_idx, part in enumerate(parts):
        is_part = part_ids == part_idx
        if is_part.any():
            colors[is_part] = _sample_part_colors(part, face_ids[is_part] - face_starts[part_idx], closest[is_part], spacing)
    return colors, np.sqrt(squared_distances)


class _Atlas(NamedTuple):
    """A mesh cut into charts laid flat in one square texture: the source vertex each output vertex
    copies, the triangles over the output vertices, and each output vertex's UV."""

    source_vertex_ids: np.ndarray
    faces: np.ndarray
    uvs: np.ndarray


def _build_atlas(mesh: trimesh.Trimesh, texture_size: int) -> _Atlas:
    atlas = xatlas.Atlas()
    atlas.add_mesh(np.asarray(mesh.vertices, dtype=np.float32), np.asarray(mesh.faces, dtype=np.uint32))
    pack_options = xatlas.PackOptions()
    pack_options.resolution = texture_size
    pack_options.padding = _CHART_PADDING_TEXELS
    pack_options.bilinear = True
    atlas.generate(pack_options=pack_options)
    if atlas.atlas_count != 1:
        raise click.ClickException(f"the texture atlas took {atlas.atlas_count} pages instead of one")
    return _Atlas(*atlas[0])


def _cross(u: np.ndarray, v: np.ndarray) -> np.ndarray:
    return u[:, 0] * v[:, 1] - u[:, 1] * v[:, 0]


def _rasterize(corners: np.ndarray, size: int) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
    """The texels of a `size` square image whose centers the triangles cover, as flat indices with
    row 0 at the top, the triangle covering each, and the texel center's barycentric coordinates in
    it. `corners` holds each triangle's three corners in texels, x to the right and y down."""
    lower = np.clip(np.ceil(corners.min(axis=1) - 0.5), 0, size - 1).astype(np.int64)
    upper = np.clip(np.floor(corners.max(axis=1) - 0.5), 0, size - 1).astype(np.int64)
    spans = np.maximum(upper - lower + 1, 0)
    counts = spans[:, 0] * spans[:, 1]
    tri_ids = np.repeat(np.arange(len(corners)), counts)
    offsets = np.arange(len(tri_ids)) - np.repeat(np.cumsum(counts) - counts, counts)
    cols = lower[tri_ids, 0] + offsets % spans[tri_ids, 0]
    rows = lower[tri_ids, 1] + offsets // spans[tri_ids, 0]
    centers = np.stack([cols + 0.5, rows + 0.5], axis=1)
    a, b, c = (corners[tri_ids, k] for k in range(3))
    with np.errstate(divide="ignore", invalid="ignore"):
        bary = np.stack([_cross(b - centers, c - centers), _cross(c - centers, a - centers), _cross(a - centers, b - centers)], axis=1)
        bary /= _cross(b - a, c - a)[:, None]
    is_inside = np.isfinite(bary).all(axis=1) & (bary >= -1e-6).all(axis=1)
    return rows[is_inside] * size + cols[is_inside], tri_ids[is_inside], bary[is_inside]


def _fill_gutter(image: np.ndarray, is_covered: np.ndarray) -> np.ndarray:
    """The image with each chart's edge colors spread `_GUTTER_TEXELS` into the empty texels around
    it, and the empty texels farther out set to the average color, which compresses best."""
    image, is_covered = image.astype(np.float64), is_covered.copy()
    rows, cols = is_covered.shape
    for _ in range(_GUTTER_TEXELS):
        padded = np.pad(image * is_covered[..., None], ((1, 1), (1, 1), (0, 0)))
        padded_covered = np.pad(is_covered, 1).astype(np.float64)
        sums, counts = np.zeros_like(image), np.zeros(is_covered.shape)
        for dy in range(3):
            for dx in range(3):
                sums += padded[dy : dy + rows, dx : dx + cols]
                counts += padded_covered[dy : dy + rows, dx : dx + cols]
        is_new = ~is_covered & (counts > 0)
        image[is_new] = sums[is_new] / counts[is_new, None]
        is_covered |= is_new
    image[~is_covered] = image[is_covered].mean(axis=0)
    return np.round(image).astype(np.uint8)


def _fill_unlit_underside(colors: np.ndarray, points: np.ndarray, normals: np.ndarray) -> np.ndarray:
    """The colors with the dark, unlit underside a scan leaves where it sat replaced by the color of
    the upper surface straight above each texel, shaded as a side facing the table would be."""
    luma = colors.astype(np.float64).mean(axis=1)
    is_upper = normals[:, 1] >= _UPPER_MIN_UP
    if not is_upper.any():
        raise click.ClickException("the model has no upper surface to fill its underside from")
    is_unlit = (normals[:, 1] <= -_UNDERSIDE_MIN_DOWN) & (luma < _UNDERSIDE_DARK_RATIO * np.median(luma[is_upper]))
    _, nearest = spatial.cKDTree(points[is_upper][:, [0, 2]]).query(points[is_unlit][:, [0, 2]])
    filled = colors.copy()
    filled[is_unlit] = np.round(colors[is_upper][nearest] * _UNDERSIDE_SHADE).astype(np.uint8)
    return filled


class Recolor(NamedTuple):
    """A change of a model's colors, which leaves grays (a white eye, a black sole) as they are."""

    hue_shift_deg: float
    """How far every color's hue turns."""
    saturation_scale: float
    value_scale: float
    """What a fully saturated color's brightness is multiplied by; a gray's is kept, and a color between moves in proportion."""


def _recolor(image: Image.Image, recolor: Recolor) -> Image.Image:
    hsv = np.asarray(image.convert("HSV"), dtype=np.float64)
    hue, saturation, value = hsv[..., 0], hsv[..., 1] / 255, hsv[..., 2]
    hue = (hue + recolor.hue_shift_deg * 256 / 360) % 256
    value = value * (1 + saturation * (recolor.value_scale - 1))
    saturation = saturation * recolor.saturation_scale
    recolored = np.stack([hue, np.clip(saturation * 255, 0, 255), np.clip(value, 0, 255)], axis=-1)
    return Image.fromarray(np.round(recolored).astype(np.uint8), mode="HSV").convert("RGB")


class Finish(NamedTuple):
    """Changes to the baked texture."""

    should_fill_unlit_underside: bool = False
    recolor: Recolor | None = None


class _PlainInside(NamedTuple):
    """The plain color of the surface a hollow carved, which lies farther than `min_distance` from the source surface."""

    min_distance: float
    color: tuple[int, int, int]


def _bake_texture(
    parts: list[trimesh.Trimesh], positions: np.ndarray, atlas: _Atlas, texture_size: int, finish: Finish, inside: _PlainInside | None
) -> Image.Image:
    """The source model's color at each texel of the atlas laid over `positions`, read from the
    nearest point of the source surface at the source's full detail, then finished."""
    corners = atlas.uvs[atlas.faces] * texture_size
    corners[..., 1] = texture_size - corners[..., 1]
    texel_ids, tri_ids, bary = _rasterize(corners, texture_size)
    triangles = positions[atlas.faces]
    points = np.einsum("ni,nij->nj", bary, triangles[tri_ids])
    texel_len = math.sqrt(float(trimesh.triangles.area(triangles).sum()) / len(texel_ids))
    colors, distances = _sample_colors(parts, points, texel_len)
    if finish.should_fill_unlit_underside:
        normals = np.cross(triangles[:, 1] - triangles[:, 0], triangles[:, 2] - triangles[:, 0])
        normals /= np.maximum(np.linalg.norm(normals, axis=1, keepdims=True), 1e-30)
        colors = _fill_unlit_underside(colors, points, normals[tri_ids])
    if inside is not None:
        colors[distances > inside.min_distance] = inside.color
    flat_image = np.zeros((texture_size * texture_size, 3), dtype=np.uint8)
    flat_image[texel_ids] = colors
    is_covered = np.zeros(texture_size * texture_size, dtype=bool)
    is_covered[texel_ids] = True
    shape = (texture_size, texture_size)
    image = Image.fromarray(_fill_gutter(flat_image.reshape(*shape, 3), is_covered.reshape(shape)))
    return image if finish.recolor is None else _recolor(image, finish.recolor)


def _place_on_stand(mesh: trimesh.Trimesh, size_m: float) -> np.ndarray:
    """The transform that scales the mesh's longest side to `size_m`, centers it over the origin,
    and puts its lowest point at y = 0."""
    lower, upper = mesh.bounds
    center = np.array([(lower[0] + upper[0]) / 2, lower[1], (lower[2] + upper[2]) / 2])
    scale = trimesh.transformations.scale_matrix(size_m / float(mesh.extents.max()))
    return scale @ trimesh.transformations.translation_matrix(-center)


class Repaired(NamedTuple):
    """The repaired model as one closed solid, and the same surface cut along its texture seams
    and textured, as the glb holds it."""

    solid: trimesh.Trimesh
    textured: trimesh.Trimesh


def repair_model(
    source: Path,
    destination: Path,
    size_m: float,
    should_keep_largest: bool,
    max_triangles: int = _DEFAULT_MAX_TRIANGLES,
    texture_size: int = _DEFAULT_TEXTURE_SIZE,
    hollow: Hollow | None = None,
    finish: Finish = Finish(),
    should_hull_open: bool = False,
    turn: tuple[_Axis, float] | None = None,
    trim_below_share: float | None = None,
) -> Repaired:
    """Writes `destination` as a closed, outward-wound, single-piece textured glb of at most about `max_triangles` triangles.
    With `trim_below_share`, everything lower than that share of the model's height (after `turn`) is cut away."""
    parts = _fetch_parts(source)
    if turn is not None:
        axis, degrees = turn
        rotation = trimesh.transformations.rotation_matrix(math.radians(degrees), np.eye(3)["xyz".index(axis)])
        parts = [part.copy().apply_transform(rotation) for part in parts]
    cut_y = None if trim_below_share is None else _calc_cut_height(parts, trim_below_share)
    solid = _build_solid(parts, should_keep_largest, should_hull_open, cut_y)
    unit_per_m = float(_to_trimesh(solid).extents.max()) / size_m
    inside = None
    if hollow is not None:
        solid = _hollow(solid, hollow, unit_per_m)
        if hollow.inside_color is not None:
            inside = _PlainInside(_INSIDE_WALL_SHARE * hollow.wall_m * unit_per_m, hollow.inside_color)
    solid = _to_trimesh(_decimate(solid, max_triangles))
    atlas = _build_atlas(solid, texture_size)
    positions = np.asarray(solid.vertices)[atlas.source_vertex_ids]
    texture = _bake_texture(parts, positions, atlas, texture_size, finish, inside)
    material = trimesh.visual.material.PBRMaterial(baseColorTexture=texture, metallicFactor=0.0)
    visual = trimesh.visual.TextureVisuals(uv=atlas.uvs, material=material)
    solid_ids = {_SOLID_VERTEX_ATTRIBUTE: atlas.source_vertex_ids.astype(np.uint32)}
    textured = trimesh.Trimesh(positions, atlas.faces, visual=visual, vertex_attributes=solid_ids, process=False)
    placement = _place_on_stand(solid, size_m)
    repaired = Repaired(solid.apply_transform(placement), textured.apply_transform(placement))
    if not (repaired.solid.is_watertight and repaired.solid.is_winding_consistent and repaired.solid.volume > 0):
        raise click.ClickException(f"{source}: repair left a mesh that is not a closed, outward-wound solid")
    destination.parent.mkdir(parents=True, exist_ok=True)
    repaired.textured.export(destination, file_type="glb", extension_webp=True)
    return repaired


@click.command()
@click.argument("source", type=click.Path(exists=True, dir_okay=False, path_type=Path))
@click.argument("destination", type=click.Path(dir_okay=False, path_type=Path))
@click.option("--size-m", default=0.24, show_default=True, help="Longest side of the repaired model, in meters.")
@click.option("--keep-largest", is_flag=True, help="Keep only the biggest piece when several remain, such as one of a pair.")
@click.option("--max-triangles", default=_DEFAULT_MAX_TRIANGLES, show_default=True, help="Decimate a model with more triangles down to this.")
@click.option("--texture-size", default=_DEFAULT_TEXTURE_SIZE, show_default=True, help="Side of the baked square texture, in texels.")
@click.option("--hollow", type=(float, float), default=None, metavar="WALL_M FLOOR_M", help="Hollow the inside, leaving walls WALL_M thick and the bottom FLOOR_M high whole, for a model whose counterpart is hollow, such as a shoe.")
@click.option("--inside-color", type=(click.IntRange(0, 255), click.IntRange(0, 255), click.IntRange(0, 255)), default=None, metavar="R G B", help="Color the walls --hollow carves plainly in this sRGB color, before any --recolor, rather than like the outside nearest them.")
@click.option("--turn", type=(click.Choice(get_args(_Axis)), float), default=None, metavar="AXIS DEGREES", help="Turn the model about an axis first, to rest it on another side, such as a skinned model trimesh loads in its node pose.")
@click.option("--trim-below", type=click.FloatRange(0, 1, min_open=True, max_open=True), default=None, metavar="HEIGHT_SHARE", help="Cut away everything lower than this share of the model's height, such as legs too thin to stand a cake on, leaving a flat bottom.")
@click.option("--hull-open-pieces", is_flag=True, help="Replace each piece that will not close with its convex hull, for nearly convex parts modeled as open shells.")
@click.option("--fill-unlit-underside", is_flag=True, help="Replace a scan's dark, unlit underside with the colors of the surface above it.")
@click.option("--recolor", type=(float, float, float), default=None, metavar="HUE_DEG SATURATION VALUE", help="Recolor the model: turn every hue by HUE_DEG and scale saturated colors' saturation and brightness, leaving grays as they are.")
def main(
    source: Path,
    destination: Path,
    size_m: float,
    keep_largest: bool,
    max_triangles: int,
    texture_size: int,
    hollow: tuple[float, float] | None,
    inside_color: tuple[int, int, int] | None,
    turn: tuple[_Axis, float] | None,
    trim_below: float | None,
    hull_open_pieces: bool,
    fill_unlit_underside: bool,
    recolor: tuple[float, float, float] | None,
) -> None:
    if inside_color is not None and hollow is None:
        raise click.BadParameter("needs --hollow, since only a hollow carves walls", param_hint="--inside-color")
    finish = Finish(fill_unlit_underside, None if recolor is None else Recolor(*recolor))
    hollowing = None if hollow is None else Hollow(*hollow, inside_color=inside_color)
    solid = repair_model(source, destination, size_m, keep_largest, max_triangles, texture_size, hollowing, finish, hull_open_pieces, turn, trim_below).solid
    extents = np.round(solid.extents, 3).tolist()
    click.echo(f"{destination}: {len(solid.faces)} triangles, extents {extents} m, volume {solid.volume:.6f} m3")


if __name__ == "__main__":
    main()
