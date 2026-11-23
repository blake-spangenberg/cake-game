#!/usr/bin/env -S uv run --locked --script
# /// script
# requires-python = ">=3.13"
# dependencies = ["click", "fast-simplification", "manifold3d", "networkx", "numpy", "pillow", "pymeshfix", "rtree", "scipy", "trimesh"]
# ///
"""Repairs a sourced .glb into one closed, outward-wound solid resting on y = 0 and centered
on the stand, scaled so its longest side is `--size-m`, carrying the source's coloring as
per-vertex sRGB colors. `bake-cake` turns the result into a playable cake, so rejection of what
this cannot fix happens there and here.

Run `uv lock --script tools/repair-model.py` after changing the dependencies above.
"""

import math
from pathlib import Path

import click
import manifold3d
import numpy as np
import pymeshfix
import scipy.sparse
import trimesh
from PIL import Image, ImageFilter

__all__ = ["repair_model"]

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
_DEFAULT_SMOOTH_PASSES = 2
# Two vertex colors this far apart (distance in sRGB, 0 to 1 per channel) barely smooth into each other.
_SMOOTH_COLOR_SIGMA = 0.15
# Lighting baked into a scan varies slowly across a surface, while detail (a seam, an eye) is finer than
# the first radius, so delighting keeps the brightness relative to this neighborhood's average and sets
# the neighborhood to the whole region's average, which is about as wide as the model.
_DELIGHT_FINE_RADIUS_M = 0.02
_DELIGHT_REGION_RADIUS_M = 0.25
# Brightness steps beyond this many natural-log units stop the delighting average, so a real change of
# material (a white sole under a black upper) bounds a region instead of blending as if it were lighting.
_DELIGHT_EDGE_SIGMA = 0.3
# The most delighting may brighten or darken a vertex, as a factor.
_DELIGHT_MAX_GAIN = 2.0
_LUMA_WEIGHTS = np.array([0.2126, 0.7152, 0.0722])


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
    """The texture blurred to the footprint of one output vertex, `spacing` apart in the part's units.

    Reading a texture at one point per vertex aliases any pattern finer than the vertex spacing into
    blotches, so the texture is low-passed first, as a mipmap would be. The blur is sized by how many
    texels cover a unit of surface, which is the part's surface area over its UV area.
    """
    uvs = part.visual.uv[part.faces]
    legs = uvs[:, 1:] - uvs[:, :1]
    uv_area = 0.5 * np.abs(legs[:, 0, 0] * legs[:, 1, 1] - legs[:, 0, 1] * legs[:, 1, 0]).sum()
    if uv_area <= 0:
        return texture
    texel_len = math.sqrt(part.area / (uv_area * texture.width * texture.height))
    return texture.filter(ImageFilter.GaussianBlur(radius=0.5 * spacing / texel_len))


def _sample_part_colors(part: trimesh.Trimesh, face_ids: np.ndarray, points: np.ndarray, spacing: float) -> np.ndarray:
    """The sRGB color of `part`, averaged over `spacing`, at `points`, each on the face of the same row in `face_ids`."""
    visual = part.visual
    bary = trimesh.triangles.points_to_barycentric(part.triangles[face_ids], points)
    texture = _fetch_texture(part)
    if texture is not None:
        uvs = np.einsum("ni,nij->nj", bary, visual.uv[part.faces[face_ids]])
        texels = trimesh.visual.color.uv_to_interpolated_color(uvs, _prefilter_texture(part, texture, spacing))[:, :3]
        return _linear_to_srgb_u8(_srgb_u8_to_linear(texels) * _get_color_factor(part))
    if isinstance(visual, trimesh.visual.TextureVisuals):
        return np.tile(_linear_to_srgb_u8(_get_color_factor(part)), (len(points), 1))
    corner_colors = np.asarray(visual.vertex_colors, dtype=np.float64)[part.faces[face_ids]][:, :, :3]
    return np.round(np.einsum("ni,nij->nj", bary, corner_colors)).astype(np.uint8)


def _sample_colors(parts: list[trimesh.Trimesh], points: np.ndarray, spacing: float) -> np.ndarray:
    """The sRGB color of the source model, averaged over `spacing`, at the surface point nearest each of `points`."""
    closest, _, face_ids = trimesh.proximity.closest_point(_merge(parts), points)
    face_counts = [len(part.faces) for part in parts]
    face_starts = np.cumsum([0, *face_counts])
    part_ids = np.repeat(np.arange(len(parts)), face_counts)[face_ids]
    colors = np.empty((len(points), 3), dtype=np.uint8)
    for part_idx, part in enumerate(parts):
        is_part = part_ids == part_idx
        if is_part.any():
            colors[is_part] = _sample_part_colors(part, face_ids[is_part] - face_starts[part_idx], closest[is_part], spacing)
    return colors


def _build_affinity(mesh: trimesh.Trimesh, features: np.ndarray, sigma: float) -> scipy.sparse.csr_matrix:
    """How alike each pair of edge-adjacent vertices count: 1 for equal `features`, falling to 0 over a few `sigma`."""
    flat = features.reshape(len(features), -1)
    edges = mesh.edges_unique
    gaps = np.linalg.norm(flat[edges[:, 0]] - flat[edges[:, 1]], axis=1)
    weights = np.exp(-(gaps**2) / (2 * sigma**2))
    half = scipy.sparse.coo_matrix((weights, (edges[:, 0], edges[:, 1])), shape=(len(mesh.vertices),) * 2)
    return (half + half.T).tocsr()


def _diffuse(values: np.ndarray, affinity: scipy.sparse.csr_matrix, passes: int) -> np.ndarray:
    """Replaces each vertex's value with the affinity-weighted mean of itself and its neighbors, `passes` times."""
    weight_sums = 1.0 + np.asarray(affinity.sum(axis=1)).ravel()
    for _ in range(passes):
        values = (values + affinity @ values) / weight_sums[:, None]
    return values


def _delight(mesh: trimesh.Trimesh, colors: np.ndarray, strength: float, meters_per_unit: float) -> np.ndarray:
    """The colors with the slow brightness trend divided out, by `strength` from 0 to 1.

    A scan bakes its lighting into the texture as soft gradients and shadows far wider than the detail
    it should show. Brightness is averaged over a fine neighborhood and over a region as wide as the
    model, neither crossing a sharp brightness step, and each vertex is scaled by the ratio of the region
    average to its neighborhood average. A region that shades from light to dark comes out one brightness
    with its detail intact. The radii are approximate: a pass of the average spreads about one edge length.
    """
    linear = _srgb_u8_to_linear(colors.astype(np.float64))
    luma = np.maximum(linear @ _LUMA_WEIGHTS, 1e-3)
    affinity = _build_affinity(mesh, np.log(luma), _DELIGHT_EDGE_SIGMA)
    edge_m = float(mesh.edges_unique_length.mean()) * meters_per_unit
    fine_passes, region_passes = (math.ceil((radius_m / edge_m) ** 2) for radius_m in (_DELIGHT_FINE_RADIUS_M, _DELIGHT_REGION_RADIUS_M))
    fine = _diffuse(luma[:, None], affinity, fine_passes)
    region = _diffuse(fine, affinity, region_passes - fine_passes)
    gain = np.clip((region / fine) ** strength, 1 / _DELIGHT_MAX_GAIN, _DELIGHT_MAX_GAIN)
    return _linear_to_srgb_u8(linear * gain)


def _smooth_colors(mesh: trimesh.Trimesh, colors: np.ndarray, passes: int) -> np.ndarray:
    """The colors averaged with those of similar-colored neighbors, so speckle goes and a real color edge stays."""
    srgb = colors / 255.0
    smoothed = _diffuse(srgb, _build_affinity(mesh, srgb, _SMOOTH_COLOR_SIGMA), passes)
    return np.round(np.clip(smoothed, 0.0, 1.0) * 255).astype(np.uint8)


def _rest_on_stand(mesh: trimesh.Trimesh, size_m: float) -> trimesh.Trimesh:
    """Scaled so the longest side is `size_m`, centered over the origin, with its lowest point at y = 0."""
    placed = mesh.copy()
    lower, upper = placed.bounds
    placed.apply_translation(-np.array([(lower[0] + upper[0]) / 2, lower[1], (lower[2] + upper[2]) / 2]))
    placed.apply_scale(size_m / float(placed.extents.max()))
    return placed


def repair_model(
    source: Path,
    destination: Path,
    size_m: float,
    should_keep_largest: bool,
    max_triangles: int = _DEFAULT_MAX_TRIANGLES,
    smooth_passes: int = _DEFAULT_SMOOTH_PASSES,
    delight: float = 0.0,
) -> trimesh.Trimesh:
    """Writes `destination` as a closed, outward-wound, single-piece glb of at most about `max_triangles` triangles and returns the mesh."""
    parts = _fetch_parts(source)
    solid = _decimate(_build_solid(parts, should_keep_largest), max_triangles)
    colored = _to_trimesh(solid)
    colors = _sample_colors(parts, colored.vertices, float(colored.edges_unique_length.mean()))
    if delight > 0:
        colors = _delight(colored, colors, delight, size_m / float(colored.extents.max()))
    colored.visual = trimesh.visual.ColorVisuals(colored, vertex_colors=_smooth_colors(colored, colors, smooth_passes))
    repaired = _rest_on_stand(colored, size_m)
    if not (repaired.is_watertight and repaired.is_winding_consistent and repaired.volume > 0):
        raise click.ClickException(f"{source}: repair left a mesh that is not a closed, outward-wound solid")
    destination.parent.mkdir(parents=True, exist_ok=True)
    repaired.export(destination, file_type="glb")
    return repaired


@click.command()
@click.argument("source", type=click.Path(exists=True, dir_okay=False, path_type=Path))
@click.argument("destination", type=click.Path(dir_okay=False, path_type=Path))
@click.option("--size-m", default=0.24, show_default=True, help="Longest side of the repaired model, in meters.")
@click.option("--keep-largest", is_flag=True, help="Keep only the biggest piece when several remain, such as one of a pair.")
@click.option("--max-triangles", default=_DEFAULT_MAX_TRIANGLES, show_default=True, help="Decimate a model with more triangles down to this.")
@click.option("--smooth-colors", "smooth_passes", default=_DEFAULT_SMOOTH_PASSES, show_default=True, help="Edge-preserving smoothing passes over the vertex colors, 0 for none.")
@click.option("--delight", type=click.FloatRange(0.0, 1.0), default=0.0, show_default=True, help="How much of the lighting baked into a scan's texture to remove, 0 for none and 1 for all of it.")
def main(
    source: Path, destination: Path, size_m: float, keep_largest: bool, max_triangles: int, smooth_passes: int, delight: float
) -> None:
    repaired = repair_model(source, destination, size_m, keep_largest, max_triangles, smooth_passes, delight)
    extents = np.round(repaired.extents, 3).tolist()
    click.echo(f"{destination}: {len(repaired.faces)} triangles, extents {extents} m, volume {repaired.volume:.6f} m3")


if __name__ == "__main__":
    main()
