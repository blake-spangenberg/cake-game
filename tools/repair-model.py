#!/usr/bin/env -S uv run --locked --script
# /// script
# requires-python = ">=3.13"
# dependencies = ["click", "manifold3d", "networkx", "numpy", "pymeshfix", "scipy", "trimesh"]
# ///
"""Repairs a sourced .glb into one closed, outward-wound solid resting on y = 0 and centered
on the stand, scaled so its longest side is `--size-m`. `bake-cake` turns the result into a
playable cake, so rejection of what this cannot fix happens there and here.

Run `uv lock --script tools/repair-model.py` after changing the dependencies above.
"""

from pathlib import Path

import click
import manifold3d
import numpy as np
import pymeshfix
import trimesh

__all__ = ["repair_model"]

# The vertex snapping grid, as a share of the model's longest side.
_WELD_RATIO = 1e-4
# A solid needs at least a tetrahedron; fewer triangles are slivers to ignore.
_MIN_SOLID_FACES = 4
# A repaired piece must keep at least this share of the size it had.
_KEPT_SIZE_RATIO = 0.99
# A piece under this share of the model's volume is debris, dropped when it floats free.
_DEBRIS_VOLUME_RATIO = 0.01


def _fetch_merged(path: Path) -> trimesh.Trimesh:
    scene = trimesh.load(path, force="scene")
    meshes = [mesh for mesh in scene.dump() if isinstance(mesh, trimesh.Trimesh) and len(mesh.faces) > 0]
    if not meshes:
        raise click.ClickException(f"{path}: holds no triangles")
    merged = trimesh.util.concatenate(meshes)
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


def _build_solid(mesh: trimesh.Trimesh, should_keep_largest: bool) -> manifold3d.Manifold:
    components = [part for part in _weld(mesh).split(only_watertight=False) if len(part.faces) >= _MIN_SOLID_FACES]
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


def _rest_on_stand(mesh: trimesh.Trimesh, size_m: float) -> trimesh.Trimesh:
    """Scaled so the longest side is `size_m`, centered over the origin, with its lowest point at y = 0."""
    placed = mesh.copy()
    lower, upper = placed.bounds
    placed.apply_translation(-np.array([(lower[0] + upper[0]) / 2, lower[1], (lower[2] + upper[2]) / 2]))
    placed.apply_scale(size_m / float(placed.extents.max()))
    return placed


def repair_model(source: Path, destination: Path, size_m: float, should_keep_largest: bool) -> trimesh.Trimesh:
    """Writes `destination` as a closed, outward-wound, single-piece glb and returns the mesh."""
    solid = _build_solid(_fetch_merged(source), should_keep_largest)
    repaired = _rest_on_stand(_to_trimesh(solid), size_m)
    if not (repaired.is_watertight and repaired.is_winding_consistent and repaired.volume > 0):
        raise click.ClickException(f"{source}: repair left a mesh that is not a closed, outward-wound solid")
    destination.parent.mkdir(parents=True, exist_ok=True)
    repaired.export(destination, file_type="glb")
    return repaired


@click.command()
@click.argument("source", type=click.Path(exists=True, dir_okay=False, path_type=Path))
@click.argument("destination", type=click.Path(dir_okay=False, path_type=Path))
@click.option("--size-m", default=0.2, show_default=True, help="Longest side of the repaired model, in meters.")
@click.option("--keep-largest", is_flag=True, help="Keep only the biggest piece when several remain, such as one of a pair.")
def main(source: Path, destination: Path, size_m: float, keep_largest: bool) -> None:
    repaired = repair_model(source, destination, size_m, keep_largest)
    extents = np.round(repaired.extents, 3).tolist()
    click.echo(f"{destination}: {len(repaired.faces)} triangles, extents {extents} m, volume {repaired.volume:.6f} m3")


if __name__ == "__main__":
    main()
