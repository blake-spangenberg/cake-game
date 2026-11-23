#!/usr/bin/env -S uv run --locked --script
# /// script
# requires-python = ">=3.13"
# dependencies = ["click", "fast-simplification", "manifold3d", "networkx", "numpy", "pillow", "pymeshfix", "pytest", "rtree", "scipy", "trimesh"]
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
_repair = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_repair)


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


def test_box_comes_out_scaled_and_resting_on_the_table_centered_on_the_stand(tmp_path: Path) -> None:
    source = _write_box(tmp_path / "box.glb", (4.0, 2.0, 1.0), (10.0, 20.0, 30.0))
    repaired = _repair.repair_model(source, tmp_path / "out.glb", 0.2, False)
    assert np.allclose(repaired.extents, [0.2, 0.1, 0.05], atol=1e-4)
    assert np.allclose(repaired.bounds, [[-0.1, 0.0, -0.025], [0.1, 0.1, 0.025]], atol=1e-4)


def test_box_missing_its_bottom_is_closed(tmp_path: Path) -> None:
    box = _box((1.0, 1.0, 1.0), (0.0, 0.0, 0.0))
    open_bottom = trimesh.Trimesh(box.vertices, box.faces[box.triangles_center[:, 1] > -0.5], process=True)
    assert not open_bottom.is_watertight
    source = _write_scene(tmp_path / "open.glb", [open_bottom])
    repaired = _repair.repair_model(source, tmp_path / "out.glb", 0.2, False)
    assert repaired.is_watertight
    assert repaired.volume == pytest.approx(0.2**3, rel=1e-3)


def test_overlapping_boxes_merge_into_one_solid_of_their_union(tmp_path: Path) -> None:
    source = _write_scene(tmp_path / "l.glb", [_box((2.0, 1.0, 1.0), (0.0, 0.0, 0.0)), _box((1.0, 2.0, 1.0), (-0.5, 0.5, 0.0))])
    repaired = _repair.repair_model(source, tmp_path / "out.glb", 0.2, False)
    scale = 0.2 / 2.0
    assert repaired.volume == pytest.approx(3.0 * scale**3, rel=1e-3)


def test_two_separate_boxes_are_rejected_unless_the_largest_is_asked_for(tmp_path: Path) -> None:
    source = _write_scene(tmp_path / "pair.glb", [_box((2.0, 1.0, 1.0), (0.0, 0.0, 0.0)), _box((1.0, 1.0, 1.0), (5.0, 0.0, 0.0))])
    with pytest.raises(click.ClickException, match="2 separate pieces"):
        _repair.repair_model(source, tmp_path / "out.glb", 0.2, False)
    repaired = _repair.repair_model(source, tmp_path / "out.glb", 0.2, True)
    assert np.allclose(repaired.extents, [0.2, 0.1, 0.1], atol=1e-4)


def test_a_speck_floating_beside_the_model_is_dropped(tmp_path: Path) -> None:
    source = _write_scene(tmp_path / "speck.glb", [_box((2.0, 1.0, 1.0), (0.0, 0.0, 0.0)), _box((0.1, 0.1, 0.1), (5.0, 0.0, 0.0))])
    repaired = _repair.repair_model(source, tmp_path / "out.glb", 0.2, False)
    assert np.allclose(repaired.extents, [0.2, 0.1, 0.1], atol=1e-4)


def test_a_model_over_the_triangle_budget_is_decimated_to_one_closed_solid_of_nearly_its_volume(tmp_path: Path) -> None:
    sphere = trimesh.creation.icosphere(subdivisions=5)
    source = _write_scene(tmp_path / "sphere.glb", [sphere])
    repaired = _repair.repair_model(source, tmp_path / "out.glb", 0.2, False, max_triangles=2000)
    assert len(repaired.faces) <= 2100
    assert repaired.is_watertight and repaired.is_winding_consistent
    assert repaired.volume == pytest.approx(4 / 3 * np.pi * 0.1**3, rel=0.02)


def _color_of(mesh: trimesh.Trimesh) -> np.ndarray:
    return np.asarray(mesh.visual.vertex_colors)[:, :3]


def test_a_source_with_no_colors_comes_out_one_uniform_color(tmp_path: Path) -> None:
    source = _write_box(tmp_path / "box.glb", (1.0, 1.0, 1.0), (0.0, 0.0, 0.0))
    repaired = _repair.repair_model(source, tmp_path / "out.glb", 0.2, False)
    assert (_color_of(repaired) == _color_of(repaired)[0]).all()


def test_vertex_colors_carry_to_the_nearest_surface(tmp_path: Path) -> None:
    box = _box((2.0, 1.0, 1.0), (0.0, 0.0, 0.0))
    box = box.subdivide().subdivide()
    box.visual.vertex_colors = np.where(box.vertices[:, [0]] > 0, [255, 0, 0, 255], [0, 0, 255, 255]).astype(np.uint8)
    source = _write_scene(tmp_path / "halves.glb", [box])
    repaired = _repair.repair_model(source, tmp_path / "out.glb", 0.2, False)
    colors, x = _color_of(repaired), repaired.vertices[:, 0]
    assert (colors[x > 0.02] == [255, 0, 0]).all()
    assert (colors[x < -0.02] == [0, 0, 255]).all()


def _textured(uv: np.ndarray, image: Image.Image, color_factor: list[float] | None = None) -> trimesh.visual.TextureVisuals:
    material = trimesh.visual.material.PBRMaterial(baseColorTexture=image, baseColorFactor=color_factor)
    return trimesh.visual.TextureVisuals(uv=uv, material=material)


def test_a_texture_is_sampled_where_the_surface_maps_to_it(tmp_path: Path) -> None:
    box = _box((1.0, 1.0, 1.0), (0.0, 0.0, 0.0))
    image = Image.new("RGB", (2, 2), (0, 200, 0))
    box.visual = _textured(np.full((len(box.vertices), 2), 0.5), image)
    source = _write_scene(tmp_path / "textured.glb", [box])
    repaired = _repair.repair_model(source, tmp_path / "out.glb", 0.2, False)
    assert np.abs(_color_of(repaired).astype(int) - [0, 200, 0]).max() <= 2


def test_an_untextured_material_carries_its_color_factor_as_srgb(tmp_path: Path) -> None:
    box = _box((1.0, 1.0, 1.0), (0.0, 0.0, 0.0))
    material = trimesh.visual.material.PBRMaterial(baseColorFactor=[0.5, 0.0, 1.0, 1.0])
    box.visual = trimesh.visual.TextureVisuals(material=material)
    source = _write_scene(tmp_path / "factor.glb", [box])
    repaired = _repair.repair_model(source, tmp_path / "out.glb", 0.2, False)
    assert np.abs(_color_of(repaired).astype(int) - [188, 0, 255]).max() <= 2


def test_a_texture_is_tinted_by_its_material_color_factor(tmp_path: Path) -> None:
    box = _box((1.0, 1.0, 1.0), (0.0, 0.0, 0.0))
    image = Image.new("RGB", (2, 2), (200, 200, 200))
    box.visual = _textured(np.full((len(box.vertices), 2), 0.5), image, [1.0, 0.5, 0.0, 1.0])
    source = _write_scene(tmp_path / "tinted.glb", [box])
    repaired = _repair.repair_model(source, tmp_path / "out.glb", 0.2, False)
    assert np.abs(_color_of(repaired).astype(int) - [200, 146, 0]).max() <= 2


def _subdivided_box(subdivisions: int = 3) -> trimesh.Trimesh:
    box = trimesh.creation.box(extents=(1.0, 1.0, 1.0))
    for _ in range(subdivisions):
        box = box.subdivide()
    return box


def test_a_pattern_finer_than_the_vertices_averages_out_instead_of_aliasing(tmp_path: Path) -> None:
    box = _subdivided_box()
    checker = (np.indices((256, 256)).sum(axis=0) % 2 * 255).astype(np.uint8)
    box.visual = _textured(np.stack([box.vertices[:, 0] + 0.5, box.vertices[:, 2] + 0.5], axis=1), Image.fromarray(checker).convert("RGB"))
    source = _write_scene(tmp_path / "checker.glb", [box])
    repaired = _repair.repair_model(source, tmp_path / "out.glb", 0.2, False, smooth_passes=0)
    assert np.abs(_color_of(repaired).astype(int) - 128).max() <= 30


def test_smoothing_calms_speckle_but_keeps_a_color_edge(tmp_path: Path) -> None:
    box = _subdivided_box()
    noise = np.random.default_rng(0).integers(-40, 41, size=len(box.vertices))
    red_side = box.vertices[:, 0] > 0
    base = np.where(red_side[:, None], [200, 40, 40], [40, 40, 200])
    box.visual.vertex_colors = np.clip(base + noise[:, None], 0, 255).astype(np.uint8)
    source = _write_scene(tmp_path / "noisy.glb", [box])
    raw = _repair.repair_model(source, tmp_path / "raw.glb", 0.2, False, smooth_passes=0)
    smooth = _repair.repair_model(source, tmp_path / "smooth.glb", 0.2, False, smooth_passes=4)
    is_red = raw.vertices[:, 0] > 0.01
    assert _color_of(smooth)[is_red, 0].std() < 0.5 * _color_of(raw)[is_red, 0].std()
    assert _color_of(smooth)[is_red, 2].max() < 100
    assert _color_of(smooth)[raw.vertices[:, 0] < -0.01, 2].min() > 150


def test_delighting_flattens_a_slow_brightness_gradient(tmp_path: Path) -> None:
    box = _subdivided_box(4)
    gradient = (60 + 140 * (box.vertices[:, 0] + 0.5)).astype(np.uint8)
    box.visual.vertex_colors = np.stack([gradient, gradient, gradient, np.full_like(gradient, 255)], axis=1)
    source = _write_scene(tmp_path / "lit.glb", [box])
    lit = _repair.repair_model(source, tmp_path / "lit.glb", 0.2, False, smooth_passes=0)
    delit = _repair.repair_model(source, tmp_path / "delit.glb", 0.2, False, smooth_passes=0, delight=1.0)
    assert _color_of(delit)[:, 0].std() < 0.7 * _color_of(lit)[:, 0].std()


if __name__ == "__main__":
    sys.exit(pytest.main([__file__, "-q"]))
