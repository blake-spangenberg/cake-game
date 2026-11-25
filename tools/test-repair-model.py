#!/usr/bin/env -S uv run --locked --script
# /// script
# requires-python = ">=3.13"
# dependencies = ["click", "fast-simplification", "libigl", "manifold3d", "networkx", "numpy", "pillow", "pymeshfix", "pytest", "rtree", "scipy", "trimesh", "xatlas"]
# ///
"""Tests for repair-model.py and pose-model.py. Run with `uv run --locked --script tools/test-repair-model.py`."""

import importlib.util
import json
import struct
import sys
from pathlib import Path
from types import ModuleType

import click
import manifold3d
import numpy as np
import pytest
import trimesh
from click.testing import CliRunner
from PIL import Image

sys.dont_write_bytecode = True


def _load_tool(module_name: str, file_name: str) -> ModuleType:
    spec = importlib.util.spec_from_file_location(module_name, Path(__file__).with_name(file_name))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


_repair_module = _load_tool("repair_model", "repair-model.py")
_pose_module = _load_tool("pose_model", "pose-model.py")

# A small texture keeps the tests quick; the bake works the same at any size.
_TEST_TEXTURE_SIZE = 128
# A coarse grid keeps hollowing quick, and the 0.2 m test models need no finer.
_HOLLOW = _repair_module.Hollow(wall_m=0.02, floor_m=0.05, voxel_m=0.004)


def _box(extents: tuple[float, float, float], center: tuple[float, float, float]) -> trimesh.Trimesh:
    box = trimesh.creation.box(extents=extents)
    box.apply_translation(center)
    return box


def _write_scene(path: Path, meshes: list[trimesh.Trimesh]) -> Path:
    trimesh.Scene(meshes).export(path, file_type="glb")
    return path


def _write_box(path: Path, extents: tuple[float, float, float], center: tuple[float, float, float]) -> Path:
    return _write_scene(path, [_box(extents, center)])


def _write_manifold(path: Path, solid: manifold3d.Manifold) -> Path:
    mesh = solid.to_mesh()
    return _write_scene(path, [trimesh.Trimesh(mesh.vert_properties[:, :3], mesh.tri_verts)])


def _repair(source: Path, destination: Path, should_keep_largest: bool = False, **options: object) -> _repair_module.Repaired:
    return _repair_module.repair_model(source, destination, 0.2, should_keep_largest, **{"texture_size": _TEST_TEXTURE_SIZE, **options})


def _texture_of(repaired: _repair_module.Repaired) -> np.ndarray:
    return np.asarray(repaired.textured.visual.material.baseColorTexture.convert("RGB")).astype(int)


def _sample_baked(repaired: _repair_module.Repaired, points: np.ndarray) -> np.ndarray:
    """The baked texture's sRGB color at the repaired surface's point nearest each of `points`."""
    textured = repaired.textured
    closest, _, face_ids = trimesh.proximity.closest_point(textured, points)
    bary = trimesh.triangles.points_to_barycentric(textured.triangles[face_ids], closest)
    uvs = np.einsum("ni,nij->nj", bary, textured.visual.uv[textured.faces[face_ids]])
    return np.round(_repair_module.sample_bilinear(textured.visual.material.baseColorTexture, uvs)).astype(int)


def test_box_comes_out_scaled_and_resting_on_the_table_centered_on_the_stand(tmp_path: Path) -> None:
    source = _write_box(tmp_path / "box.glb", (4.0, 2.0, 1.0), (10.0, 20.0, 30.0))
    for mesh in _repair(source, tmp_path / "out.glb"):
        assert np.allclose(mesh.extents, [0.2, 0.1, 0.05], atol=1e-4)
        assert np.allclose(mesh.bounds, [[-0.1, 0.0, -0.025], [0.1, 0.1, 0.025]], atol=1e-4)


def test_box_missing_its_bottom_is_closed(tmp_path: Path) -> None:
    box = _box((1.0, 1.0, 1.0), (0.0, 0.0, 0.0))
    open_bottom = trimesh.Trimesh(box.vertices, box.faces[box.triangles_center[:, 1] > -0.5], process=True)
    assert not open_bottom.is_watertight
    solid = _repair(_write_scene(tmp_path / "open.glb", [open_bottom]), tmp_path / "out.glb").solid
    assert solid.is_watertight
    assert solid.volume == pytest.approx(0.2**3, rel=1e-3)


def test_overlapping_boxes_merge_into_one_solid_of_their_union(tmp_path: Path) -> None:
    source = _write_scene(tmp_path / "l.glb", [_box((2.0, 1.0, 1.0), (0.0, 0.0, 0.0)), _box((1.0, 2.0, 1.0), (-0.5, 0.5, 0.0))])
    scale = 0.2 / 2.0
    assert _repair(source, tmp_path / "out.glb").solid.volume == pytest.approx(3.0 * scale**3, rel=1e-3)


def test_two_separate_boxes_are_rejected_unless_the_largest_is_asked_for(tmp_path: Path) -> None:
    source = _write_scene(tmp_path / "pair.glb", [_box((2.0, 1.0, 1.0), (0.0, 0.0, 0.0)), _box((1.0, 1.0, 1.0), (5.0, 0.0, 0.0))])
    with pytest.raises(click.ClickException, match="2 separate pieces"):
        _repair(source, tmp_path / "out.glb")
    assert np.allclose(_repair(source, tmp_path / "out.glb", True).solid.extents, [0.2, 0.1, 0.1], atol=1e-4)


# A join gap of 0.01 mm, under the 0.02 mm a leg one weld step short stands off at 0.1 m per unit,
# so only stretching a piece within two weld steps can join the leg.
_HAIR_JOIN_GAP_M = 1e-5


def test_with_a_join_gap_a_leg_modeled_a_hair_short_of_the_body_it_holds_up_joins_it(tmp_path: Path) -> None:
    # The leg's top stops one weld step, a ten-thousandth of the 2-unit longest side, under the top.
    top = _box((2.0, 0.5, 1.0), (0.0, 1.25, 0.0))
    leg = _box((0.2, 1.0, 0.2), (0.0, 0.5 - 2e-4, 0.0))
    solid = _repair(_write_scene(tmp_path / "table.glb", [top, leg]), tmp_path / "out.glb", join_gap_m=_HAIR_JOIN_GAP_M).solid
    assert len(solid.split(only_watertight=False)) == 1
    scale = 0.2 / 2.0
    assert solid.volume == pytest.approx((1.0 + 0.04) * scale**3, rel=1e-3)


def test_without_a_join_gap_a_leg_a_hair_short_of_the_body_stays_a_separate_piece(tmp_path: Path) -> None:
    # The Electric Guitar shipped with such a piece left off; joining it changed its mesh.
    top = _box((2.0, 0.5, 1.0), (0.0, 1.25, 0.0))
    leg = _box((0.2, 1.0, 0.2), (0.0, 0.5 - 2e-4, 0.0))
    with pytest.raises(click.ClickException, match="2 separate pieces"):
        _repair(_write_scene(tmp_path / "table.glb", [top, leg]), tmp_path / "out.glb")


def test_a_leg_three_weld_steps_short_of_the_body_stays_a_separate_piece(tmp_path: Path) -> None:
    top = _box((2.0, 0.5, 1.0), (0.0, 1.25, 0.0))
    leg = _box((0.2, 1.0, 0.2), (0.0, 0.5 - 6e-4, 0.0))
    with pytest.raises(click.ClickException, match="2 separate pieces"):
        _repair(_write_scene(tmp_path / "table.glb", [top, leg]), tmp_path / "out.glb", join_gap_m=_HAIR_JOIN_GAP_M)


def test_parts_standing_a_short_way_off_the_body_move_onto_it_in_a_chain_and_lose_their_wires(tmp_path: Path) -> None:
    # At 0.1 m per unit: a leg 6 mm under the top, a foot 1 mm under the leg, and a wire 2 mm thick
    # through the leg, reaching 0.3 units past the top's front and back.
    top = _box((2.0, 0.5, 1.0), (0.0, 1.25, 0.0))
    leg = _box((0.2, 1.0, 0.2), (0.0, 0.44, 0.0))
    foot = _box((0.6, 0.1, 0.2), (0.0, -0.12, 0.0))
    wire = _box((0.02, 0.02, 1.6), (0.0, 0.44, 0.0))
    source = _write_scene(tmp_path / "table.glb", [top, leg, foot, wire])
    solid = _repair(source, tmp_path / "out.glb", join_gap_m=0.008).solid
    assert len(solid.split(only_watertight=False)) == 1
    # The leg rises 0.06 units and the foot with it, then 0.01 more, each sinking 0.0004 units (two
    # weld steps) into what it joins, so the model's bottom rises from -0.17 to -0.0992.
    assert np.allclose(solid.extents, [0.2, 0.15992, 0.1], atol=1e-5)
    assert solid.volume == pytest.approx((1.0 + 0.04 + 0.012) * 0.1**3, rel=1e-3)


def test_a_part_standing_farther_off_than_the_join_gap_stays_a_separate_piece(tmp_path: Path) -> None:
    top = _box((2.0, 0.5, 1.0), (0.0, 1.25, 0.0))
    leg = _box((0.2, 1.0, 0.2), (0.0, 0.44, 0.0))
    with pytest.raises(click.ClickException, match="2 separate pieces"):
        _repair(_write_scene(tmp_path / "table.glb", [top, leg]), tmp_path / "out.glb", join_gap_m=0.005)


def test_trimming_below_a_height_share_cuts_away_the_legs_and_leaves_a_flat_bottom(tmp_path: Path) -> None:
    # The model is 1.5 tall, so a cut at 0.7 of its height, y = 1.05, takes the legs and 0.05 of the top.
    top = _box((2.0, 0.5, 1.0), (0.0, 1.25, 0.0))
    legs = [_box((0.2, 1.1, 0.2), (x, 0.55, 0.0)) for x in (-0.8, 0.8)]
    solid = _repair(_write_scene(tmp_path / "table.glb", [top, *legs]), tmp_path / "out.glb", trim_below_share=0.7).solid
    scale = 0.2 / 2.0
    assert np.allclose(solid.extents, [0.2, 0.45 * scale, 0.1], atol=1e-4)
    assert solid.volume == pytest.approx(2.0 * 0.45 * 1.0 * scale**3, rel=1e-3)


def test_a_speck_floating_beside_the_model_is_dropped(tmp_path: Path) -> None:
    source = _write_scene(tmp_path / "speck.glb", [_box((2.0, 1.0, 1.0), (0.0, 0.0, 0.0)), _box((0.1, 0.1, 0.1), (5.0, 0.0, 0.0))])
    assert np.allclose(_repair(source, tmp_path / "out.glb").solid.extents, [0.2, 0.1, 0.1], atol=1e-4)


def test_a_sealed_void_inside_the_model_is_filled(tmp_path: Path) -> None:
    cube = manifold3d.Manifold.cube
    source = _write_manifold(tmp_path / "void.glb", cube((1.0, 1.0, 1.0), center=True) - cube((0.5, 0.5, 0.5), center=True))
    assert _repair(source, tmp_path / "out.glb").solid.volume == pytest.approx(0.2**3, rel=1e-3)


def _open_fin(width: float, height: float, center: tuple[float, float, float]) -> trimesh.Trimesh:
    """A thin open sheet bent into a shallow V, as trim such as a fish's fin is modeled."""
    xs = np.linspace(-width / 2, width / 2, 9)
    ys = np.linspace(-height / 2, height / 2, 9)
    grid_x, grid_y = np.meshgrid(xs, ys, indexing="ij")
    vertices = np.stack([grid_x.ravel(), grid_y.ravel(), 0.05 * np.abs(grid_x.ravel())], axis=1) + center
    idx = np.arange(81).reshape(9, 9)
    corners = idx[:-1, :-1].ravel(), idx[1:, :-1].ravel(), idx[1:, 1:].ravel(), idx[:-1, 1:].ravel()
    faces = np.concatenate([np.stack([corners[0], corners[1], corners[2]], axis=1), np.stack([corners[0], corners[2], corners[3]], axis=1)])
    return trimesh.Trimesh(vertices, faces, process=False)


def test_a_thin_open_piece_that_will_not_close_is_dropped_as_trim(tmp_path: Path) -> None:
    source = _write_scene(tmp_path / "fin.glb", [_box((2.0, 1.0, 1.0), (0.0, 0.0, 0.0)), _open_fin(0.5, 0.5, (1.5, 0.0, 0.0))])
    assert np.allclose(_repair(source, tmp_path / "out.glb").solid.extents, [0.2, 0.1, 0.1], atol=1e-4)


def test_an_open_piece_that_will_not_close_becomes_its_convex_hull_when_asked(tmp_path: Path) -> None:
    # The fin reaches 0.45 past the box's end, so its hull lengthens the model from 2 to 2.45.
    source = _write_scene(tmp_path / "fin.glb", [_box((2.0, 1.0, 1.0), (0.0, 0.0, 0.0)), _open_fin(0.5, 0.5, (1.2, 0.0, 0.0))])
    solid = _repair(source, tmp_path / "out.glb", should_hull_open=True).solid
    assert solid.extents[0] / solid.extents[1] == pytest.approx(2.45, rel=1e-3)


def test_a_model_over_the_triangle_budget_is_decimated_to_one_closed_solid_of_nearly_its_volume(tmp_path: Path) -> None:
    source = _write_scene(tmp_path / "sphere.glb", [trimesh.creation.icosphere(subdivisions=5)])
    solid = _repair(source, tmp_path / "out.glb", max_triangles=2000).solid
    assert len(solid.faces) <= 2100
    assert solid.is_watertight and solid.is_winding_consistent
    assert solid.volume == pytest.approx(4 / 3 * np.pi * 0.1**3, rel=0.02)


def test_a_decimation_that_breaks_the_shape_gives_way_to_simplifying_within_a_tolerance(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    # fast-simplification once grew a piano's volume by half and threw a sheet past its bounds.
    def swell(mesh: trimesh.Trimesh, face_count: int) -> trimesh.Trimesh:
        return trimesh.creation.icosphere(subdivisions=2, radius=2.0)

    monkeypatch.setattr(trimesh.Trimesh, "simplify_quadric_decimation", swell)
    source = _write_scene(tmp_path / "sphere.glb", [trimesh.creation.icosphere(subdivisions=5)])
    solid = _repair(source, tmp_path / "out.glb", max_triangles=2000).solid
    assert len(solid.faces) <= 2000
    assert solid.volume == pytest.approx(4 / 3 * np.pi * 0.1**3, rel=0.02)


def test_a_decimation_that_breaks_the_shape_once_runs_again_on_the_solid_merged_within_a_small_tolerance(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    # Edge collapse broke the Grand Piano, and held its shape once its edges were merged within 1e-4.
    collapse = trimesh.Trimesh.simplify_quadric_decimation
    calls = []

    def swell_the_first_time(mesh: trimesh.Trimesh, face_count: int) -> trimesh.Trimesh:
        calls.append(face_count)
        return trimesh.creation.icosphere(subdivisions=2, radius=2.0) if len(calls) == 1 else collapse(mesh, face_count=face_count)

    monkeypatch.setattr(trimesh.Trimesh, "simplify_quadric_decimation", swell_the_first_time)
    source = _write_scene(tmp_path / "sphere.glb", [trimesh.creation.icosphere(subdivisions=5)])
    solid = _repair(source, tmp_path / "out.glb", max_triangles=2000).solid
    assert len(solid.faces) == pytest.approx(2000, rel=0.05)
    assert solid.volume == pytest.approx(4 / 3 * np.pi * 0.1**3, rel=0.02)


def _break_edge_collapse(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(trimesh.Trimesh, "simplify_quadric_decimation", lambda mesh, face_count: trimesh.creation.icosphere(subdivisions=2, radius=2.0))


# The test sphere's longest side is 2, so merging edges starts at a tolerance of 2e-4.
_FIRST_SIMPLIFY_TOLERANCE = 2e-4


def test_decimating_fails_when_no_tolerance_the_shape_allows_removes_triangles(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    # Where coarser tolerances kept leaving as many triangles, the doubling ran on without end.
    _break_edge_collapse(monkeypatch)
    monkeypatch.setattr(manifold3d.Manifold, "simplify", lambda solid, tolerance: solid)
    source = _write_scene(tmp_path / "sphere.glb", [trimesh.creation.icosphere(subdivisions=5)])
    with pytest.raises(click.ClickException, match="at best 20480 triangles"):
        _repair(source, tmp_path / "out.glb", max_triangles=2000)


def test_decimating_fails_when_merging_edges_within_a_tolerance_breaks_the_shape(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    _break_edge_collapse(monkeypatch)
    speck = manifold3d.Manifold.sphere(0.1, 16)

    def shrink_past_the_first_tolerance(solid: manifold3d.Manifold, tolerance: float) -> manifold3d.Manifold:
        return solid if tolerance <= _FIRST_SIMPLIFY_TOLERANCE * 1.5 else speck

    monkeypatch.setattr(manifold3d.Manifold, "simplify", shrink_past_the_first_tolerance)
    source = _write_scene(tmp_path / "sphere.glb", [trimesh.creation.icosphere(subdivisions=5)])
    with pytest.raises(click.ClickException, match="broke the shape"):
        _repair(source, tmp_path / "out.glb", max_triangles=2000)


def _split_off_a_speck(mesh: trimesh.Trimesh, face_count: int) -> trimesh.Trimesh:
    """A stand-in for edge collapse that leaves a unit sphere with a speck 0.02 across beside it,
    within the sphere's bounds so the shape check passes."""
    speck = trimesh.creation.icosphere(subdivisions=1, radius=0.01)
    speck.apply_translation((0.75, 0.75, 0.0))
    return trimesh.util.concatenate([trimesh.creation.icosphere(subdivisions=4), speck])


def test_a_speck_the_collapse_after_merging_edges_splits_off_is_dropped(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    # Edge collapse on the merged Grand Piano split slivers off its thin lid.
    calls = []

    def swell_then_split_off_a_speck(mesh: trimesh.Trimesh, face_count: int) -> trimesh.Trimesh:
        calls.append(face_count)
        return trimesh.creation.icosphere(subdivisions=2, radius=2.0) if len(calls) == 1 else _split_off_a_speck(mesh, face_count)

    monkeypatch.setattr(trimesh.Trimesh, "simplify_quadric_decimation", swell_then_split_off_a_speck)
    source = _write_scene(tmp_path / "sphere.glb", [trimesh.creation.icosphere(subdivisions=5)])
    solid = _repair(source, tmp_path / "out.glb", max_triangles=6000).solid
    assert len(solid.split(only_watertight=False)) == 1
    assert np.allclose(solid.extents, [0.2, 0.2, 0.2], atol=1e-3)


def test_a_speck_the_first_edge_collapse_splits_off_stays_beside_the_solid(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    # The Common Raven shipped with 12 such specks, and dropping them changed its mesh.
    monkeypatch.setattr(trimesh.Trimesh, "simplify_quadric_decimation", _split_off_a_speck)
    source = _write_scene(tmp_path / "sphere.glb", [trimesh.creation.icosphere(subdivisions=5)])
    solid = _repair(source, tmp_path / "out.glb", max_triangles=6000).solid
    assert len(solid.split(only_watertight=False)) == 2
    assert np.allclose(solid.extents, [0.2, 0.2, 0.2], atol=1e-3)


def _read_solid_ids(path: Path) -> np.ndarray:
    loaded = trimesh.load(path, force="mesh", process=False)
    return np.asarray(loaded.vertex_attributes["_SOLID_VERTEX"]).ravel().astype(int)


def test_numbering_by_first_use_numbers_the_solid_in_the_order_the_glb_first_copies_it_and_keeps_the_surface(tmp_path: Path) -> None:
    source = _write_scene(tmp_path / "sphere.glb", [trimesh.creation.icosphere(subdivisions=3)])
    kept = _repair(source, tmp_path / "kept.glb")
    renumbered = _repair(source, tmp_path / "renumbered.glb", should_number_by_first_use=True)
    kept_ids, renumbered_ids = _read_solid_ids(tmp_path / "kept.glb"), _read_solid_ids(tmp_path / "renumbered.glb")
    _, first_idx = np.unique(renumbered_ids, return_index=True)
    assert renumbered_ids[np.sort(first_idx)].tolist() == list(range(642))
    assert not np.array_equal(kept_ids, renumbered_ids), "manifold3d's order is not the first-use order"
    assert np.array_equal(renumbered.textured.vertices, kept.textured.vertices)
    assert np.array_equal(renumbered.textured.faces, kept.textured.faces)
    assert np.array_equal(renumbered.textured.visual.uv, kept.textured.visual.uv)
    assert np.array_equal(renumbered.solid.vertices[renumbered_ids], kept.solid.vertices[kept_ids])


def test_the_glb_names_the_solid_vertex_each_seam_split_vertex_copies(tmp_path: Path) -> None:
    source = _write_scene(tmp_path / "sphere.glb", [trimesh.creation.icosphere(subdivisions=3)])
    repaired = _repair(source, tmp_path / "out.glb")
    loaded = trimesh.load(tmp_path / "out.glb", force="mesh", process=False)
    assert isinstance(loaded.visual, trimesh.visual.TextureVisuals)
    solid_ids = np.asarray(loaded.vertex_attributes["_SOLID_VERTEX"]).ravel().astype(int)
    assert len(loaded.vertices) > len(repaired.solid.vertices), "the seams split some vertices"
    rejoined = trimesh.Trimesh(repaired.solid.vertices, solid_ids[loaded.faces], process=False)
    assert rejoined.is_watertight
    assert np.allclose(loaded.vertices, repaired.solid.vertices[solid_ids], atol=1e-6)


def test_hollowing_leaves_walls_and_a_floor_of_the_asked_thickness(tmp_path: Path) -> None:
    source = _write_box(tmp_path / "box.glb", (1.0, 1.0, 1.0), (0.0, 0.0, 0.0))
    solid = _repair(source, tmp_path / "out.glb", hollow=_HOLLOW).solid
    assert solid.is_watertight
    hollow_volume = 0.16 * 0.16 * (0.2 - 0.05 - 0.02)
    assert solid.volume == pytest.approx(0.2**3 - hollow_volume, rel=0.05)


def test_hollowing_opens_into_a_narrow_opening_instead_of_walling_it_off(tmp_path: Path) -> None:
    block = manifold3d.Manifold.cube((1.0, 1.0, 1.0), center=True) - manifold3d.Manifold.cube((0.3, 0.6, 0.3), center=True).translate((0.0, 0.4, 0.0))
    solid = _repair(_write_manifold(tmp_path / "well.glb", block), tmp_path / "out.glb", hollow=_HOLLOW).solid
    assert len(solid.split(only_watertight=False)) == 1, "an inner surface apart from the outer one would be a sealed void"
    assert solid.volume < 0.6 * 0.2**3


def test_hollowing_keeps_attached_a_part_its_walls_alone_would_cut_free(tmp_path: Path) -> None:
    # An island on top of a block, ringed by a moat narrow enough to be bridged over. Walls measured
    # from the bridged top end at the moat's floor, so they alone would leave the island's top loose.
    cube = manifold3d.Manifold.cube
    moat = cube((0.4, 0.15, 0.4), center=True) - cube((0.3, 0.15, 0.3), center=True)
    source = _write_manifold(tmp_path / "moat.glb", cube((1.0, 1.0, 1.0), center=True) - moat.translate((0.0, 0.425, 0.0)))
    solid = _repair(source, tmp_path / "out.glb", hollow=_HOLLOW).solid
    assert len(solid.split(only_watertight=False)) == 1
    assert solid.volume < 0.7 * 0.2**3, "the rest of the block is still hollowed"


def test_hollowing_paints_the_walls_it_carves_with_the_inside_color(tmp_path: Path) -> None:
    box = _box((1.0, 1.0, 1.0), (0.0, 0.0, 0.0)).subdivide().subdivide()
    box.visual.vertex_colors = np.tile([200, 0, 0, 255], (len(box.vertices), 1)).astype(np.uint8)
    hollow = _HOLLOW._replace(inside_color=(0, 150, 0))
    repaired = _repair(_write_scene(tmp_path / "red.glb", [box]), tmp_path / "out.glb", hollow=hollow)
    carved_floor, outer_side = _sample_baked(repaired, np.array([[0.0, 0.06, 0.0], [0.1, 0.1, 0.0]]))
    assert np.abs(carved_floor - [0, 150, 0]).max() <= 2
    assert np.abs(outer_side - [200, 0, 0]).max() <= 2


def _red(mesh: trimesh.Trimesh) -> trimesh.Trimesh:
    mesh.visual.vertex_colors = np.tile([200, 0, 0, 255], (len(mesh.vertices), 1)).astype(np.uint8)
    return mesh


def _write_perch(path: Path) -> Path:
    """A block held up by a post off its center, as a bird perches on a branch: 1.6 x 2.0 x 0.6
    units, its center of mass 0.0222 units toward the post."""
    block = _red(_box((1.6, 0.5, 0.6), (0.0, 1.75, 0.0)))
    post = _red(_box((0.2, 1.6, 0.2), (0.2, 0.8, 0.0)))
    return _write_scene(path, [block, post])


def _slice(solid: trimesh.Trimesh, height: float) -> manifold3d.CrossSection:
    """The solid's horizontal cross-section at `height`, with x and -z as its axes."""
    mesh = manifold3d.Mesh(vert_properties=np.asarray(solid.vertices, dtype=np.float32), tri_verts=np.asarray(solid.faces, dtype=np.uint32))
    return manifold3d.Manifold(mesh).rotate((90.0, 0.0, 0.0)).slice(height)


def test_a_plinth_holds_the_model_up_centered_under_its_center_of_mass_and_counts_in_its_size(tmp_path: Path) -> None:
    plinth = _repair_module.Plinth(radius_m=0.04, thickness_m=0.02, offset_m=(0.01, 0.0), sink_m=0.003)
    repaired = _repair(_write_perch(tmp_path / "perch.glb"), tmp_path / "out.glb", plinth=plinth)
    solid = repaired.solid
    assert len(solid.split(only_watertight=False)) == 1
    # The 0.2 m height holds the plinth's 0.017 m above the post's foot, so a unit is 0.183 / 2.0 m,
    # and the center of mass sits 0.0222 units, 0.00203 m, toward the post.
    assert solid.extents[1] == pytest.approx(0.2, abs=1e-4)
    foot = _slice(solid, 0.001)
    assert foot.area() == pytest.approx(np.pi * 0.04**2, rel=1e-3)
    assert np.allclose(np.reshape(foot.bounds(), (2, 2)).mean(axis=0), [0.00203 + 0.01, 0.0], atol=1e-4)
    # The plinth's top steps in by half its thickness, and above it stands only the 0.2 unit post.
    assert _slice(solid, 0.019).area() == pytest.approx(np.pi * 0.03**2, rel=1e-3)
    assert _slice(solid, 0.021).area() == pytest.approx((0.2 * 0.183 / 2.0) ** 2, rel=1e-3)
    plinth_side, post_side = _sample_baked(repaired, np.array([[0.01203 + 0.04, 0.002, 0.0], [0.0275, 0.05, 0.0]]))
    assert np.abs(plinth_side - [112, 72, 46]).max() <= 16, "the plinth is wood"
    assert np.abs(post_side - [200, 0, 0]).max() <= 2


def test_a_plinth_that_misses_the_model_s_foot_is_rejected(tmp_path: Path) -> None:
    plinth = _repair_module.Plinth(radius_m=0.02, thickness_m=0.02, offset_m=(-0.05, 0.0))
    with pytest.raises(click.ClickException, match="does not reach"):
        _repair(_write_perch(tmp_path / "perch.glb"), tmp_path / "out.glb", plinth=plinth)


@pytest.mark.parametrize(
    "plinth_options",
    [
        pytest.param(["--plinth", "0.05", "0.003"], id="no thicker than the foot sinks"),
        pytest.param(["--plinth", "0.005", "0.02"], id="narrower than its edge steps in"),
        pytest.param(["--plinth", "0.12", "0.02"], id="as wide as half the size"),
        pytest.param(["--plinth", "0.05", "0.02", "--plinth-offset", "0.0", "-0.07"], id="moved out to half the size"),
        pytest.param(["--plinth-offset", "0.01", "0.0"], id="an offset without a plinth"),
    ],
)
def test_plinth_options_that_cannot_mount_the_model_are_rejected_before_repairing(tmp_path: Path, plinth_options: list[str]) -> None:
    source = _write_box(tmp_path / "box.glb", (1.0, 1.0, 1.0), (0.0, 0.0, 0.0))
    result = CliRunner().invoke(_repair_module.main, [*plinth_options, str(source), str(tmp_path / "out.glb")])
    assert result.exit_code == 2
    assert not (tmp_path / "out.glb").exists()


def _box_colored_by_facing(down_color: list[int], other_color: list[int]) -> trimesh.Trimesh:
    box = _box((1.0, 1.0, 1.0), (0.0, 0.0, 0.0)).subdivide().subdivide()
    box = trimesh.Trimesh(box.vertices[box.faces].reshape(-1, 3), np.arange(len(box.faces) * 3).reshape(-1, 3), process=False)
    is_down = np.repeat(box.face_normals[:, 1] < -0.5, 3)
    box.visual.vertex_colors = np.where(is_down[:, None], down_color + [255], other_color + [255]).astype(np.uint8)
    return box


def test_an_unlit_underside_takes_the_shaded_color_of_the_surface_above(tmp_path: Path) -> None:
    source = _write_scene(tmp_path / "unlit.glb", [_box_colored_by_facing([10, 10, 10], [200, 150, 100])])
    finish = _repair_module.Finish(should_fill_unlit_underside=True)
    repaired = _repair(source, tmp_path / "out.glb", finish=finish)
    bottom = _sample_baked(repaired, np.array([[0.0, 0.0, 0.0], [0.05, 0.0, -0.05]]))
    assert np.abs(bottom - [160, 120, 80]).max() <= 3


def test_recoloring_turns_saturated_colors_and_leaves_grays(tmp_path: Path) -> None:
    box = _box((2.0, 1.0, 1.0), (0.0, 0.0, 0.0)).subdivide().subdivide()
    box.visual.vertex_colors = np.where(box.vertices[:, [0]] > 0, [255, 0, 0, 255], [255, 255, 255, 255]).astype(np.uint8)
    finish = _repair_module.Finish(recolor=_repair_module.Recolor(hue_shift_deg=120, saturation_scale=1.0, value_scale=0.5))
    repaired = _repair(_write_scene(tmp_path / "halves.glb", [box]), tmp_path / "out.glb", finish=finish)
    red_side, white_side = _sample_baked(repaired, np.array([[0.08, 0.05, 0.0], [-0.08, 0.05, 0.0]]))
    assert np.abs(red_side - [0, 128, 0]).max() <= 4
    assert np.abs(white_side - [255, 255, 255]).max() <= 2


def test_a_source_with_no_colors_comes_out_one_uniform_color(tmp_path: Path) -> None:
    texture = _texture_of(_repair(_write_box(tmp_path / "box.glb", (1.0, 1.0, 1.0), (0.0, 0.0, 0.0)), tmp_path / "out.glb"))
    assert (texture == texture[0, 0]).all()


def test_vertex_colors_carry_to_the_nearest_surface(tmp_path: Path) -> None:
    box = _box((2.0, 1.0, 1.0), (0.0, 0.0, 0.0)).subdivide().subdivide()
    box.visual.vertex_colors = np.where(box.vertices[:, [0]] > 0, [255, 0, 0, 255], [0, 0, 255, 255]).astype(np.uint8)
    repaired = _repair(_write_scene(tmp_path / "halves.glb", [box]), tmp_path / "out.glb")
    probes = np.array([[0.08, 0.05, 0.0], [0.05, 0.0, 0.02], [-0.08, 0.05, 0.0], [-0.05, 0.1, -0.02]])
    assert (_sample_baked(repaired, probes) == [[255, 0, 0], [255, 0, 0], [0, 0, 255], [0, 0, 255]]).all()


def _textured(uv: np.ndarray, image: Image.Image, color_factor: list[float] | None = None) -> trimesh.visual.TextureVisuals:
    material = trimesh.visual.material.PBRMaterial(baseColorTexture=image, baseColorFactor=color_factor)
    return trimesh.visual.TextureVisuals(uv=uv, material=material)


def _assert_uniform(texture: np.ndarray, color: list[int]) -> None:
    assert np.abs(texture - color).max() <= 2


def test_a_texture_is_sampled_where_the_surface_maps_to_it(tmp_path: Path) -> None:
    box = _box((1.0, 1.0, 1.0), (0.0, 0.0, 0.0))
    box.visual = _textured(np.full((len(box.vertices), 2), 0.5), Image.new("RGB", (2, 2), (0, 200, 0)))
    _assert_uniform(_texture_of(_repair(_write_scene(tmp_path / "textured.glb", [box]), tmp_path / "out.glb")), [0, 200, 0])


def test_an_untextured_material_carries_its_color_factor_as_srgb(tmp_path: Path) -> None:
    box = _box((1.0, 1.0, 1.0), (0.0, 0.0, 0.0))
    box.visual = trimesh.visual.TextureVisuals(material=trimesh.visual.material.PBRMaterial(baseColorFactor=[0.5, 0.0, 1.0, 1.0]))
    _assert_uniform(_texture_of(_repair(_write_scene(tmp_path / "factor.glb", [box]), tmp_path / "out.glb")), [188, 0, 255])


def test_a_texture_is_tinted_by_its_material_color_factor(tmp_path: Path) -> None:
    box = _box((1.0, 1.0, 1.0), (0.0, 0.0, 0.0))
    box.visual = _textured(np.full((len(box.vertices), 2), 0.5), Image.new("RGB", (2, 2), (200, 200, 200)), [1.0, 0.5, 0.0, 1.0])
    _assert_uniform(_texture_of(_repair(_write_scene(tmp_path / "tinted.glb", [box]), tmp_path / "out.glb")), [200, 146, 0])


def _checkered_box(cells: int) -> trimesh.Trimesh:
    """A unit box whose top and bottom are a black and white checkerboard of `cells` squares a side."""
    box = trimesh.creation.box(extents=(1.0, 1.0, 1.0))
    for _ in range(3):
        box = box.subdivide()
    checker = (np.indices((cells, cells)).sum(axis=0) % 2 * 255).astype(np.uint8)
    box.visual = _textured(np.stack([box.vertices[:, 0] + 0.5, box.vertices[:, 2] + 0.5], axis=1), Image.fromarray(checker).convert("RGB"))
    return box


def test_texture_detail_finer_than_the_vertices_comes_through_sharp(tmp_path: Path) -> None:
    cells = 16
    repaired = _repair(_write_scene(tmp_path / "checker.glb", [_checkered_box(cells)]), tmp_path / "out.glb", texture_size=512)
    centers = ((np.arange(cells) + 0.5) / cells - 0.5) * 0.2
    xs, zs = np.meshgrid(centers, centers, indexing="ij")
    luma = _sample_baked(repaired, np.stack([xs.ravel(), np.full(xs.size, 0.2), zs.ravel()], axis=1)).mean(axis=1)
    assert np.abs(luma - 127.5).min() > 100, "every square of the top reads clearly black or white"
    assert 0.4 < (luma > 127.5).mean() < 0.6


def test_a_pattern_finer_than_the_baked_texels_averages_out_instead_of_aliasing(tmp_path: Path) -> None:
    texture = _texture_of(_repair(_write_scene(tmp_path / "checker.glb", [_checkered_box(1024)]), tmp_path / "out.glb"))
    assert np.abs(texture - 128).max() <= 30


_GLB_JSON_CHUNK = 0x4E4F534A
_GLB_BIN_CHUNK = 0x004E4942
_GLB_FLOAT = 5126
_GLB_UNSIGNED_BYTE = 5121


def _encode_glb(doc: dict, binary: bytes) -> bytes:
    json_bytes = json.dumps(doc).encode()
    json_bytes += b" " * (-len(json_bytes) % 4)
    binary += b"\0" * (-len(binary) % 4)
    chunks = struct.pack("<II", len(json_bytes), _GLB_JSON_CHUNK) + json_bytes + struct.pack("<II", len(binary), _GLB_BIN_CHUNK) + binary
    return struct.pack("<4sII", b"glTF", 2, 12 + len(chunks)) + chunks


def _decode_glb(data: bytes) -> tuple[dict, bytes]:
    json_len = struct.unpack_from("<I", data, 12)[0]
    bin_start = 20 + json_len
    bin_len = struct.unpack_from("<I", data, bin_start)[0]
    return json.loads(data[20:bin_start]), data[bin_start + 8 : bin_start + 8 + bin_len]


def _read_floats(doc: dict, binary: bytes, accessor_idx: int) -> np.ndarray:
    """A tightly packed float accessor's rows, as the posed glb writes them."""
    accessor = doc["accessors"][accessor_idx]
    view = doc["bufferViews"][accessor["bufferView"]]
    width = {"VEC3": 3, "VEC4": 4}[accessor["type"]]
    return np.frombuffer(binary, dtype=np.float32, count=accessor["count"] * width, offset=view["byteOffset"]).reshape(-1, width)


def _translation(x: float, y: float, z: float) -> np.ndarray:
    return trimesh.transformations.translation_matrix((x, y, z))


def _write_skinned_glb(path: Path, joint_scale: tuple[float, float, float] = (1.0, 1.0, 1.0)) -> Path:
    """A triangle skinned to two joints, and a static triangle hung under the second joint.

    The root joint stands at (0, 1, 0), and the second joint 1 unit along x from it, scaled by
    `joint_scale`, turning about z from no turn at 0 s to a quarter turn at 2 s. The skinned triangle,
    `skinned`, has corners (0, 0, 0), (2, 1, 0), and (0, 2, 0), weighted wholly to the root, wholly
    to the second joint, and half to each, each with normal +x and tangent +y. Its node stands at
    (5, 0, 0), which glTF ignores for a skinned mesh. The static triangle, `static`, has corners
    one unit along each axis.
    """
    doc: dict = {"asset": {"version": "2.0"}, "buffers": [{}], "bufferViews": [], "accessors": []}
    binary = bytearray()

    def add_accessor(values: np.ndarray, accessor_type: str) -> int:
        binary.extend(b"\0" * (-len(binary) % 4))
        doc["bufferViews"].append({"buffer": 0, "byteOffset": len(binary), "byteLength": values.nbytes})
        binary.extend(values.tobytes())
        component_type = _GLB_UNSIGNED_BYTE if values.dtype == np.uint8 else _GLB_FLOAT
        doc["accessors"].append({"bufferView": len(doc["bufferViews"]) - 1, "componentType": component_type, "count": len(values), "type": accessor_type})
        return len(doc["accessors"]) - 1

    def floats(rows: list[list[float]]) -> np.ndarray:
        return np.array(rows, dtype=np.float32)

    skinned_attributes = {
        "POSITION": add_accessor(floats([[0, 0, 0], [2, 1, 0], [0, 2, 0]]), "VEC3"),
        "NORMAL": add_accessor(floats([[1, 0, 0]] * 3), "VEC3"),
        "TANGENT": add_accessor(floats([[0, 1, 0, 1]] * 3), "VEC4"),
        "JOINTS_0": add_accessor(np.array([[0, 0, 0, 0], [1, 0, 0, 0], [0, 1, 0, 0]], dtype=np.uint8), "VEC4"),
        "WEIGHTS_0": add_accessor(floats([[1, 0, 0, 0], [1, 0, 0, 0], [0.5, 0.5, 0, 0]]), "VEC4"),
    }
    static_attributes = {"POSITION": add_accessor(floats([[1, 0, 0], [0, 1, 0], [0, 0, 1]]), "VEC3")}
    # The inverse of each joint's world matrix in the bind pose, column-major as glTF stores it.
    inverse_binds = np.stack([_translation(0, -1, 0).T, _translation(-1, -1, 0).T]).astype(np.float32).reshape(2, 16)
    times = add_accessor(floats([[0], [2]]), "SCALAR")
    rotations = add_accessor(floats([[0, 0, 0, 1], [0, 0, np.sqrt(0.5), np.sqrt(0.5)]]), "VEC4")
    doc |= {
        "meshes": [{"name": "skinned", "primitives": [{"attributes": skinned_attributes}]}, {"name": "static", "primitives": [{"attributes": static_attributes}]}],
        "nodes": [
            {"translation": [0, 1, 0], "children": [1]},
            {"translation": [1, 0, 0], "scale": list(joint_scale), "children": [3]},
            {"mesh": 0, "skin": 0, "translation": [5, 0, 0]},
            {"mesh": 1},
        ],
        "skins": [{"joints": [0, 1], "inverseBindMatrices": add_accessor(inverse_binds, "MAT4")}],
        "animations": [{"channels": [{"sampler": 0, "target": {"node": 1, "path": "rotation"}}], "samplers": [{"input": times, "output": rotations}]}],
        "scenes": [{"nodes": [0, 2]}],
        "scene": 0,
    }
    doc["buffers"][0]["byteLength"] = len(binary)
    path.write_bytes(_encode_glb(doc, bytes(binary)))
    return path


def _pose(source: Path, time_s: float) -> tuple[dict, bytes]:
    return _decode_glb(_pose_module.pose_model(source.read_bytes(), time_s, 0))


def _load_posed_vertices(posed: bytes, path: Path) -> dict[str, np.ndarray]:
    """Each mesh's vertices in world space, by mesh name, as trimesh reads the posed glb."""
    path.write_bytes(posed)
    scene = trimesh.load(path, force="scene", process=False)
    vertices = {}
    for node in scene.graph.nodes_geometry:
        transform, geometry_name = scene.graph[node]
        vertices[geometry_name] = trimesh.transform_points(scene.geometry[geometry_name].vertices, transform)
    return vertices


def test_posing_moves_each_skinned_vertex_by_its_weighted_joints_at_the_asked_time(tmp_path: Path) -> None:
    # At 1 s the second joint has turned an eighth of a turn about its place, (1, 1, 0).
    posed = _pose_module.pose_model(_write_skinned_glb(tmp_path / "skinned.glb").read_bytes(), 1.0, 0)
    half = np.sqrt(0.5)
    vertices = _load_posed_vertices(posed, tmp_path / "posed.glb")
    assert np.allclose(vertices["skinned"], [[0, 0, 0], [1 + half, 1 + half, 0], [0.5 - half, 1.5, 0]], atol=1e-6)


def test_posing_holds_the_last_key_past_the_animation_s_end(tmp_path: Path) -> None:
    posed = _pose_module.pose_model(_write_skinned_glb(tmp_path / "skinned.glb").read_bytes(), 9.0, 0)
    assert np.allclose(_load_posed_vertices(posed, tmp_path / "posed.glb")["skinned"], [[0, 0, 0], [1, 2, 0], [0, 1, 0]], atol=1e-6)


def test_posing_turns_normals_and_tangents_with_the_joints(tmp_path: Path) -> None:
    doc, binary = _pose(_write_skinned_glb(tmp_path / "skinned.glb"), 2.0)
    attributes = doc["meshes"][0]["primitives"][0]["attributes"]
    half = np.sqrt(0.5)
    assert np.allclose(_read_floats(doc, binary, attributes["NORMAL"]), [[1, 0, 0], [0, 1, 0], [half, half, 0]], atol=1e-6)
    assert np.allclose(_read_floats(doc, binary, attributes["TANGENT"]), [[0, 1, 0, 1], [-1, 0, 0, 1], [-half, half, 0, 1]], atol=1e-6)


def test_posing_hangs_a_static_mesh_off_the_root_at_its_animated_place(tmp_path: Path) -> None:
    posed = _pose_module.pose_model(_write_skinned_glb(tmp_path / "skinned.glb").read_bytes(), 2.0, 0)
    vertices = _load_posed_vertices(posed, tmp_path / "posed.glb")
    assert np.allclose(vertices["static"], [[1, 2, 0], [0, 1, 0], [1, 1, 1]], atol=1e-6)
    doc, _ = _decode_glb(posed)
    assert doc["scenes"] == [{"nodes": [2, 3]}]
    assert "children" not in doc["nodes"][1]


def test_the_posed_glb_holds_no_skin_animation_or_joint_weights(tmp_path: Path) -> None:
    doc, _ = _pose(_write_skinned_glb(tmp_path / "skinned.glb"), 1.0)
    assert "skins" not in doc and "animations" not in doc
    assert all("skin" not in node for node in doc["nodes"])
    assert set(doc["meshes"][0]["primitives"][0]["attributes"]) == {"POSITION", "NORMAL", "TANGENT"}


def test_a_skinned_node_outside_the_posed_scene_keeps_no_reference_to_the_dropped_skins(tmp_path: Path) -> None:
    doc, binary = _decode_glb(_write_skinned_glb(tmp_path / "skinned.glb").read_bytes())
    doc["meshes"].append({"name": "unused", "primitives": [{"attributes": {"POSITION": 0}}]})
    doc["nodes"].append({"mesh": 2, "skin": 0})
    posed, _ = _decode_glb(_pose_module.pose_model(_encode_glb(doc, binary), 1.0, 0))
    assert all("skin" not in node for node in posed["nodes"])


def test_posed_positions_declare_the_bounds_of_the_values_stored(tmp_path: Path) -> None:
    # glTF requires a POSITION accessor's min and max to match its float32 values exactly.
    doc, binary = _pose(_write_skinned_glb(tmp_path / "skinned.glb"), 2.0)
    accessor_idx = doc["meshes"][0]["primitives"][0]["attributes"]["POSITION"]
    stored = _read_floats(doc, binary, accessor_idx).astype(np.float64)
    assert doc["accessors"][accessor_idx]["min"] == stored.min(axis=0).tolist()
    assert doc["accessors"][accessor_idx]["max"] == stored.max(axis=0).tolist()


def test_a_joint_mirrored_by_a_negative_scale_flips_its_vertices_tangent_handedness(tmp_path: Path) -> None:
    doc, binary = _pose(_write_skinned_glb(tmp_path / "skinned.glb", joint_scale=(-1.0, 1.0, 1.0)), 2.0)
    attributes = doc["meshes"][0]["primitives"][0]["attributes"]
    assert np.allclose(_read_floats(doc, binary, attributes["POSITION"])[1], [1, 0, 0], atol=1e-6)
    assert np.allclose(_read_floats(doc, binary, attributes["TANGENT"])[1], [-1, 0, 0, -1], atol=1e-6)


def test_a_vertex_on_a_joint_scaled_to_nothing_collapses_onto_it_and_keeps_its_normal_and_tangent(tmp_path: Path) -> None:
    # Rigs hide a part by scaling its joint to zero, which leaves its vertices' blend with no inverse.
    doc, binary = _pose(_write_skinned_glb(tmp_path / "skinned.glb", joint_scale=(0.0, 0.0, 0.0)), 2.0)
    attributes = doc["meshes"][0]["primitives"][0]["attributes"]
    assert np.allclose(_read_floats(doc, binary, attributes["POSITION"]), [[0, 0, 0], [1, 1, 0], [0.5, 1.5, 0]], atol=1e-6)
    assert np.allclose(_read_floats(doc, binary, attributes["NORMAL"]), [[1, 0, 0]] * 3, atol=1e-6)
    assert np.allclose(_read_floats(doc, binary, attributes["TANGENT"]), [[0, 1, 0, 1]] * 3, atol=1e-6)


def test_a_model_with_no_animation_is_rejected(tmp_path: Path) -> None:
    doc, binary = _decode_glb(_write_skinned_glb(tmp_path / "skinned.glb").read_bytes())
    del doc["animations"]
    with pytest.raises(click.ClickException, match="no animation"):
        _pose_module.pose_model(_encode_glb(doc, binary), 1.0, 0)


if __name__ == "__main__":
    sys.exit(pytest.main([__file__, "-q"]))
