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


def _write_box(path: Path, extents: tuple[float, float, float], center: tuple[float, float, float]) -> Path:
    box = trimesh.creation.box(extents=extents)
    box.apply_translation(center)
    box.export(path, file_type="glb")
    return path


def _write_scene(path: Path, meshes: list[trimesh.Trimesh]) -> Path:
    trimesh.Scene(meshes).export(path, file_type="glb")
    return path


def _box(extents: tuple[float, float, float], center: tuple[float, float, float]) -> trimesh.Trimesh:
    box = trimesh.creation.box(extents=extents)
    box.apply_translation(center)
    return box


def _repair(source: Path, destination: Path, should_keep_largest: bool = False, **options: int):
    return _repair_module.repair_model(source, destination, 0.2, should_keep_largest, **{"texture_size": _TEST_TEXTURE_SIZE, **options})


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


def test_a_speck_floating_beside_the_model_is_dropped(tmp_path: Path) -> None:
    source = _write_scene(tmp_path / "speck.glb", [_box((2.0, 1.0, 1.0), (0.0, 0.0, 0.0)), _box((0.1, 0.1, 0.1), (5.0, 0.0, 0.0))])
    assert np.allclose(_repair(source, tmp_path / "out.glb").solid.extents, [0.2, 0.1, 0.1], atol=1e-4)


def test_a_model_over_the_triangle_budget_is_decimated_to_one_closed_solid_of_nearly_its_volume(tmp_path: Path) -> None:
    source = _write_scene(tmp_path / "sphere.glb", [trimesh.creation.icosphere(subdivisions=5)])
    solid = _repair(source, tmp_path / "out.glb", max_triangles=2000).solid
    assert len(solid.faces) <= 2100
    assert solid.is_watertight and solid.is_winding_consistent
    assert solid.volume == pytest.approx(4 / 3 * np.pi * 0.1**3, rel=0.02)


def test_the_glb_holds_the_solid_split_only_along_texture_seams(tmp_path: Path) -> None:
    source = _write_scene(tmp_path / "sphere.glb", [trimesh.creation.icosphere(subdivisions=3)])
    repaired = _repair(source, tmp_path / "out.glb")
    loaded = trimesh.load(tmp_path / "out.glb", force="mesh", process=False)
    assert isinstance(loaded.visual, trimesh.visual.TextureVisuals)
    welded = trimesh.Trimesh(loaded.vertices, loaded.faces, process=True)
    assert welded.is_watertight
    assert welded.volume == pytest.approx(repaired.solid.volume, rel=1e-4)


def _texture_of(repaired) -> np.ndarray:
    return np.asarray(repaired.textured.visual.material.baseColorTexture.convert("RGB")).astype(int)


def _sample_baked(repaired, points: np.ndarray) -> np.ndarray:
    """The baked texture's sRGB color at the repaired surface's point nearest each of `points`."""
    textured = repaired.textured
    closest, _, face_ids = trimesh.proximity.closest_point(textured, points)
    bary = trimesh.triangles.points_to_barycentric(textured.triangles[face_ids], closest)
    uvs = np.einsum("ni,nij->nj", bary, textured.visual.uv[textured.faces[face_ids]])
    return np.round(_repair_module.sample_bilinear(textured.visual.material.baseColorTexture, uvs)).astype(int)


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
