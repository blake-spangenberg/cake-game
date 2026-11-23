#!/usr/bin/env -S uv run --locked --script
# /// script
# requires-python = ">=3.13"
# dependencies = ["click", "manifold3d", "networkx", "numpy", "pymeshfix", "pytest", "scipy", "trimesh"]
# ///
"""Tests for repair-model.py. Run with `uv run --locked --script tools/test-repair-model.py`."""

import importlib.util
import sys
from pathlib import Path

import click
import numpy as np
import pytest
import trimesh

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


if __name__ == "__main__":
    sys.exit(pytest.main([__file__, "-q"]))
