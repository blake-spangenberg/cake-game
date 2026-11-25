#!/usr/bin/env -S uv run --locked --script
# /// script
# requires-python = ">=3.13"
# dependencies = ["click", "fast-simplification", "libigl", "manifold3d", "networkx", "numpy", "pillow", "pymeshfix", "pytest", "rtree", "scipy", "trimesh", "xatlas"]
# ///
"""Tests for repair-model.py. Run with `uv run --locked --script tools/test-repair-model.py`."""

import importlib.util
import sys
from pathlib import Path

import click
import manifold3d
import numpy as np
import pytest
import trimesh
from PIL import Image

sys.dont_write_bytecode = True
_spec = importlib.util.spec_from_file_location("repair_model", Path(__file__).with_name("repair-model.py"))
_repair_module = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_repair_module)

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


def test_a_leg_modeled_a_hair_short_of_the_body_it_holds_up_joins_it(tmp_path: Path) -> None:
    # The leg's top stops one weld step, a ten-thousandth of the 2-unit longest side, under the top.
    top = _box((2.0, 0.5, 1.0), (0.0, 1.25, 0.0))
    leg = _box((0.2, 1.0, 0.2), (0.0, 0.5 - 2e-4, 0.0))
    solid = _repair(_write_scene(tmp_path / "table.glb", [top, leg]), tmp_path / "out.glb").solid
    assert len(solid.split(only_watertight=False)) == 1
    scale = 0.2 / 2.0
    assert solid.volume == pytest.approx((1.0 + 0.04) * scale**3, rel=1e-3)


def test_a_leg_three_weld_steps_short_of_the_body_stays_a_separate_piece(tmp_path: Path) -> None:
    top = _box((2.0, 0.5, 1.0), (0.0, 1.25, 0.0))
    leg = _box((0.2, 1.0, 0.2), (0.0, 0.5 - 6e-4, 0.0))
    with pytest.raises(click.ClickException, match="2 separate pieces"):
        _repair(_write_scene(tmp_path / "table.glb", [top, leg]), tmp_path / "out.glb")


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


if __name__ == "__main__":
    sys.exit(pytest.main([__file__, "-q"]))
