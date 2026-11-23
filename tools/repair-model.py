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
from typing import NamedTuple

import click
import igl
import manifold3d
import numpy as np
import pymeshfix
import trimesh
import xatlas
from PIL import Image, ImageFilter

__all__ = ["Repaired", "repair_model", "sample_bilinear"]

# The vertex snapping grid, as a share of the model's longest side.
_WELD_RATIO = 1e-4
# A solid needs at least a tetrahedron; fewer triangles are slivers to ignore.
_MIN_SOLID_FACES = 4
# A repaired piece must keep at least this share of the size it had.
_KEPT_SIZE_RATIO = 0.99
# A piece under this share of the model's volume is debris, dropped when it floats free.
_DEBRIS_VOLUME_RATIO = 0.01
# The default triangle budget: the bake's collider decomposition takes minutes on a model far over it.
_DEFAULT_MAX_TRIANGLES = 40000
# The side of the baked texture in texels. At 2048 a 0.24 m model gets texels of about 0.2 mm,
# finer than the screen resolves it at any zoom the camera allows and about as fine as the sources.
_DEFAULT_TEXTURE_SIZE = 2048
# Empty texels between charts, so a mipmapped lookup near a chart's edge reads only that chart.
_CHART_PADDING_TEXELS = 4
# How far each chart's colors spread into the gutter around it, so filtering at a seam never
# reads the fill between charts.
_GUTTER_TEXELS = 2 * _CHART_PADDING_TEXELS


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


def _build_solid(parts: list[trimesh.Trimesh], should_keep_largest: bool) -> manifold3d.Manifold:
    components = [part for part in _weld(_merge(parts)).split(only_watertight=False) if len(part.faces) >= _MIN_SOLID_FACES]
    solids = [solid for solid in map(_to_solid, components) if solid is not None]
    if not solids:
        raise click.ClickException("no piece of the model encloses any volume")
    union = manifold3d.Manifold.batch_boolean(solids, manifold3d.OpType.Add)
    parts = _to_trimesh(union).split(only_watertight=False)
    if len(parts) == 1:
        return union
    volumes = np.array([abs(part.volume) for part in parts])
    is_debris = volumes < _DEBRIS_VOLUME_RATIO * volumes.sum()
    if not should_keep_largest and (~is_debris).sum() > 1:
        raise click.ClickException(
            f"{len(parts)} separate pieces remain after merging, with volume shares "
            f"{np.round(volumes / volumes.sum(), 3).tolist()}; a cake needs one (--keep-largest keeps the biggest)"
        )
    solid = _to_solid(parts[int(volumes.argmax())])
    assert solid is not None, "the largest piece of a solid encloses volume"
    return solid


def _decimate(solid: manifold3d.Manifold, max_triangles: int) -> manifold3d.Manifold:
    """The solid with its triangles merged down to `max_triangles` by quadric edge collapse, which must stay one closed solid."""
    mesh = _to_trimesh(solid)
    if len(mesh.faces) <= max_triangles:
        return solid
    simplified = mesh.simplify_quadric_decimation(face_count=max_triangles)
    trimesh.repair.fix_inversion(simplified)
    decimated = _build_manifold(simplified)
    if decimated.status() != manifold3d.Error.NoError or decimated.is_empty() or decimated.volume() <= 0:
        raise click.ClickException(f"decimating {len(mesh.faces)} triangles to {max_triangles} broke the solid")
    return decimated


def _srgb_u8_to_linear(srgb: np.ndarray) -> np.ndarray:
    unit = srgb / 255.0
    return np.where(unit <= 0.04045, unit / 12.92, ((unit + 0.055) / 1.055) ** 2.4)


def _linear_to_srgb_u8(linear: np.ndarray) -> np.ndarray:
    clipped = np.clip(linear, 0.0, 1.0)
    srgb = np.where(clipped <= 0.0031308, 12.92 * clipped, 1.055 * clipped ** (1 / 2.4) - 0.055)
    return np.round(srgb * 255).astype(np.uint8)


def _fetch_texture(part: trimesh.Trimesh) -> Image.Image | None:
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
    texture = _fetch_texture(part)
    if texture is not None:
        uvs = np.einsum("ni,nij->nj", bary, visual.uv[part.faces[face_ids]])
        texels = sample_bilinear(_prefilter_texture(part, texture, spacing), uvs)
        return _linear_to_srgb_u8(_srgb_u8_to_linear(texels) * _get_color_factor(part))
    if isinstance(visual, trimesh.visual.TextureVisuals):
        return np.tile(_linear_to_srgb_u8(_get_color_factor(part)), (len(points), 1))
    corner_colors = np.asarray(visual.vertex_colors, dtype=np.float64)[part.faces[face_ids]][:, :, :3]
    return np.round(np.einsum("ni,nij->nj", np.clip(bary, 0.0, 1.0), corner_colors)).astype(np.uint8)


def _sample_colors(parts: list[trimesh.Trimesh], points: np.ndarray, spacing: float) -> np.ndarray:
    """The sRGB color of the source model, averaged over `spacing`, at the surface point nearest each of `points`."""
    merged = _merge(parts)
    _, face_ids, closest = igl.point_mesh_squared_distance(
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
    return colors


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


def _bake_texture(parts: list[trimesh.Trimesh], positions: np.ndarray, atlas: _Atlas, texture_size: int) -> Image.Image:
    """The source model's color at each texel of the atlas laid over `positions`, read from the
    nearest point of the source surface at the source's full detail."""
    corners = atlas.uvs[atlas.faces] * texture_size
    corners[..., 1] = texture_size - corners[..., 1]
    texel_ids, tri_ids, bary = _rasterize(corners, texture_size)
    points = np.einsum("ni,nij->nj", bary, positions[atlas.faces[tri_ids]])
    texel_len = math.sqrt(float(trimesh.triangles.area(positions[atlas.faces]).sum()) / len(texel_ids))
    flat_image = np.zeros((texture_size * texture_size, 3), dtype=np.uint8)
    flat_image[texel_ids] = _sample_colors(parts, points, texel_len)
    is_covered = np.zeros(texture_size * texture_size, dtype=bool)
    is_covered[texel_ids] = True
    shape = (texture_size, texture_size)
    return Image.fromarray(_fill_gutter(flat_image.reshape(*shape, 3), is_covered.reshape(shape)))


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
) -> Repaired:
    """Writes `destination` as a closed, outward-wound, single-piece textured glb of at most about `max_triangles` triangles."""
    parts = _fetch_parts(source)
    solid = _to_trimesh(_decimate(_build_solid(parts, should_keep_largest), max_triangles))
    atlas = _build_atlas(solid, texture_size)
    positions = np.asarray(solid.vertices)[atlas.source_vertex_ids]
    texture = _bake_texture(parts, positions, atlas, texture_size)
    material = trimesh.visual.material.PBRMaterial(baseColorTexture=texture, metallicFactor=0.0)
    visual = trimesh.visual.TextureVisuals(uv=atlas.uvs, material=material)
    textured = trimesh.Trimesh(positions, atlas.faces, visual=visual, process=False)
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
def main(source: Path, destination: Path, size_m: float, keep_largest: bool, max_triangles: int, texture_size: int) -> None:
    solid = repair_model(source, destination, size_m, keep_largest, max_triangles, texture_size).solid
    extents = np.round(solid.extents, 3).tolist()
    click.echo(f"{destination}: {len(solid.faces)} triangles, extents {extents} m, volume {solid.volume:.6f} m3")


if __name__ == "__main__":
    main()
