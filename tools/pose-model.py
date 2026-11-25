#!/usr/bin/env -S uv run --locked --script
# /// script
# requires-python = ">=3.13"
# dependencies = ["click", "numpy"]
# ///
"""Freezes an animated .glb at one moment of one of its animations into a static .glb, for
`tools/repair-model.py`, which reads meshes in their node pose and ignores skins and animation.

Every mesh comes out as a child of the scene root, carrying its posed world transform; a skinned
mesh has its vertices moved to the pose and an identity transform, as the glTF spec places a
skinned mesh by its joints alone. Positions, normals, and tangents are posed; UVs, materials,
and images are copied unchanged. Skins and animations are dropped.

    uv run --locked --script tools/pose-model.py --time-s 5 <source.glb> <posed.glb>

Run `uv lock --script tools/pose-model.py` after changing the dependencies above.
"""

import json
import struct
from pathlib import Path

import click
import numpy as np

__all__ = ["pose_model"]

_GLB_MAGIC = b"glTF"
_GLB_VERSION = 2
_JSON_CHUNK = 0x4E4F534A
_BIN_CHUNK = 0x004E4942
_FLOAT = 5126
_COMPONENT_DTYPES = {5120: np.int8, 5121: np.uint8, 5122: np.int16, 5123: np.uint16, 5125: np.uint32, _FLOAT: np.float32}
_TYPE_WIDTHS = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4, "MAT4": 16}
# Extensions that store vertex data where a plain accessor read cannot find it.
_COMPRESSION_EXTENSIONS = {"KHR_draco_mesh_compression", "EXT_meshopt_compression"}
# A turned normal or tangent shorter than this share of the length its blend's scale gives it
# belongs to a vertex the blend collapsed.
_COLLAPSED_RATIO = 1e-9


def _decode_glb(data: bytes) -> tuple[dict, bytes]:
    magic, version, _ = struct.unpack_from("<4sII", data, 0)
    if magic != _GLB_MAGIC or version != _GLB_VERSION:
        raise click.ClickException("not a glTF 2 binary (.glb)")
    json_len, json_type = struct.unpack_from("<II", data, 12)
    if json_type != _JSON_CHUNK:
        raise click.ClickException("the glb's first chunk is not its JSON")
    doc = json.loads(data[20 : 20 + json_len])
    bin_start = 20 + json_len
    if bin_start >= len(data):
        return doc, b""
    bin_len, bin_type = struct.unpack_from("<II", data, bin_start)
    if bin_type != _BIN_CHUNK:
        raise click.ClickException("the glb's second chunk is not its binary buffer")
    return doc, data[bin_start + 8 : bin_start + 8 + bin_len]


def _encode_glb(doc: dict, binary: bytes | bytearray) -> bytes:
    json_bytes = json.dumps(doc, separators=(",", ":")).encode()
    json_bytes += b" " * (-len(json_bytes) % 4)
    bin_bytes = bytes(binary) + b"\0" * (-len(binary) % 4)
    total_len = 12 + 8 + len(json_bytes) + 8 + len(bin_bytes)
    header = struct.pack("<4sII", _GLB_MAGIC, _GLB_VERSION, total_len)
    return header + struct.pack("<II", len(json_bytes), _JSON_CHUNK) + json_bytes + struct.pack("<II", len(bin_bytes), _BIN_CHUNK) + bin_bytes


def _validate_supported(doc: dict) -> None:
    buffers = doc.get("buffers", [])
    if len(buffers) != 1 or "uri" in buffers[0]:
        raise click.ClickException("only a glb whose data is all in its one embedded buffer is supported")
    compressed = _COMPRESSION_EXTENSIONS.intersection(doc.get("extensionsUsed", []))
    if compressed:
        raise click.ClickException(f"compressed vertex data is not supported: {sorted(compressed)}")
    if not doc.get("animations"):
        raise click.ClickException("the model has no animation to pose")
    if not 0 <= doc.get("scene", 0) < len(doc.get("scenes", [])):
        raise click.ClickException("the model has no scene to pose")


def _read_accessor(doc: dict, binary: bytes, accessor_idx: int) -> np.ndarray:
    """The accessor's elements as float64 rows, normalized integers scaled to [-1, 1] or [0, 1]."""
    accessor = doc["accessors"][accessor_idx]
    if "sparse" in accessor or "bufferView" not in accessor:
        raise click.ClickException(f"accessor {accessor_idx}: sparse or bufferless accessors are not supported")
    dtype = np.dtype(_COMPONENT_DTYPES[accessor["componentType"]])
    width = _TYPE_WIDTHS[accessor["type"]]
    count = accessor["count"]
    view = doc["bufferViews"][accessor["bufferView"]]
    item_size = dtype.itemsize * width
    stride = view.get("byteStride", item_size)
    start = view.get("byteOffset", 0) + accessor.get("byteOffset", 0)
    raw = np.frombuffer(binary, dtype=np.uint8, count=stride * (count - 1) + item_size, offset=start)
    rows = np.lib.stride_tricks.as_strided(raw, shape=(count, item_size), strides=(stride, 1))
    values = np.ascontiguousarray(rows).view(dtype).reshape(count, width).astype(np.float64)
    if not accessor.get("normalized", False):
        return values
    if np.issubdtype(dtype, np.signedinteger):
        return np.maximum(values / np.iinfo(dtype).max, -1.0)
    return values / np.iinfo(dtype).max


def _append_accessor(doc: dict, out: bytearray, values: np.ndarray, accessor_type: str) -> int:
    """Appends float rows to the output buffer as a new accessor and returns its index."""
    out.extend(b"\0" * (-len(out) % 4))
    stored = np.ascontiguousarray(values, dtype=np.float32)
    data = stored.tobytes()
    doc["bufferViews"].append({"buffer": 0, "byteOffset": len(out), "byteLength": len(data)})
    out.extend(data)
    doc["buffers"][0]["byteLength"] = len(out)
    accessor = {"bufferView": len(doc["bufferViews"]) - 1, "componentType": _FLOAT, "count": len(values), "type": accessor_type}
    if accessor_type == "VEC3":
        accessor |= {"min": stored.min(axis=0).tolist(), "max": stored.max(axis=0).tolist()}
    doc["accessors"].append(accessor)
    return len(doc["accessors"]) - 1


def _quaternion_to_matrix(x: float, y: float, z: float, w: float) -> np.ndarray:
    return np.array(
        [
            [1 - 2 * (y * y + z * z), 2 * (x * y - z * w), 2 * (x * z + y * w)],
            [2 * (x * y + z * w), 1 - 2 * (x * x + z * z), 2 * (y * z - x * w)],
            [2 * (x * z - y * w), 2 * (y * z + x * w), 1 - 2 * (x * x + y * y)],
        ]
    )


def _slerp(start: np.ndarray, end: np.ndarray, u: float) -> np.ndarray:
    if np.dot(start, end) < 0:
        end = -end
    cos_angle = min(float(np.dot(start, end)), 1.0)
    angle = np.arccos(cos_angle)
    if angle < 1e-6:
        blended = (1 - u) * start + u * end
    else:
        blended = (np.sin((1 - u) * angle) * start + np.sin(u * angle) * end) / np.sin(angle)
    return blended / np.linalg.norm(blended)


def _sample_channel(doc: dict, binary: bytes, sampler: dict, path: str, time_s: float) -> np.ndarray:
    """The channel's value at `time_s`, held at its first or last key outside its keys' span."""
    interpolation = sampler.get("interpolation", "LINEAR")
    if interpolation not in ("LINEAR", "STEP"):
        raise click.ClickException(f"{interpolation} animation interpolation is not supported")
    times = _read_accessor(doc, binary, sampler["input"])[:, 0]
    values = _read_accessor(doc, binary, sampler["output"])
    if time_s <= times[0]:
        return values[0]
    if time_s >= times[-1]:
        return values[-1]
    idx = int(np.searchsorted(times, time_s, side="right")) - 1
    if interpolation == "STEP":
        return values[idx]
    u = (time_s - times[idx]) / (times[idx + 1] - times[idx])
    if path == "rotation":
        return _slerp(values[idx], values[idx + 1], u)
    return (1 - u) * values[idx] + u * values[idx + 1]


def _build_local_matrices(doc: dict, binary: bytes, animation_idx: int, time_s: float) -> list[np.ndarray]:
    nodes = doc["nodes"]
    trs = [{"translation": np.array(node.get("translation", [0.0, 0.0, 0.0])), "rotation": np.array(node.get("rotation", [0.0, 0.0, 0.0, 1.0])), "scale": np.array(node.get("scale", [1.0, 1.0, 1.0]))} for node in nodes]
    animation = doc["animations"][animation_idx]
    for channel in animation["channels"]:
        target = channel["target"]
        if "node" not in target:
            continue
        path = target["path"]
        if path not in ("translation", "rotation", "scale"):
            raise click.ClickException(f"animating {path} is not supported")
        if "matrix" in nodes[target["node"]]:
            raise click.ClickException(f"node {target['node']} is animated but placed by a matrix, which glTF forbids")
        trs[target["node"]][path] = _sample_channel(doc, binary, animation["samplers"][channel["sampler"]], path, time_s)
    matrices = []
    for node, parts in zip(nodes, trs):
        if "matrix" in node:
            matrices.append(np.array(node["matrix"], dtype=np.float64).reshape(4, 4).T)
            continue
        matrix = np.eye(4)
        matrix[:3, :3] = _quaternion_to_matrix(*parts["rotation"]) * parts["scale"]
        matrix[:3, 3] = parts["translation"]
        matrices.append(matrix)
    return matrices


def _build_world_matrices(doc: dict, local: list[np.ndarray]) -> dict[int, np.ndarray]:
    """The world matrix of every node the default scene reaches."""
    world: dict[int, np.ndarray] = {}
    stack = [(idx, np.eye(4)) for idx in doc["scenes"][doc.get("scene", 0)]["nodes"]]
    while stack:
        idx, parent = stack.pop()
        world[idx] = parent @ local[idx]
        stack.extend((child, world[idx]) for child in doc["nodes"][idx].get("children", []))
    return world


def _calc_vertex_skins(doc: dict, binary: bytes, attributes: dict, joint_matrices: np.ndarray) -> np.ndarray:
    """Each vertex's blend of its joints' matrices, by its weights."""
    blended = None
    set_idx = 0
    while f"JOINTS_{set_idx}" in attributes:
        joints = _read_accessor(doc, binary, attributes[f"JOINTS_{set_idx}"]).astype(np.int64)
        weights = _read_accessor(doc, binary, attributes[f"WEIGHTS_{set_idx}"])
        part = np.einsum("vk,vkij->vij", weights, joint_matrices[joints])
        blended = part if blended is None else blended + part
        set_idx += 1
    if blended is None:
        raise click.ClickException("a skinned primitive has no JOINTS_0 weights")
    return blended


def _calc_normal_matrices(linear: np.ndarray, is_mirrored: np.ndarray) -> np.ndarray:
    """The matrices that turn normals as `linear` turns positions: each one's cofactor matrix, its
    inverse transpose times its determinant, which stays defined where it has no inverse, negated
    where it mirrors so normals keep facing out."""
    columns = linear.transpose(0, 2, 1)
    crosses = [np.cross(columns[:, 1], columns[:, 2]), np.cross(columns[:, 2], columns[:, 0]), np.cross(columns[:, 0], columns[:, 1])]
    return np.stack(crosses, axis=2) * np.where(is_mirrored, -1.0, 1.0)[:, None, None]


def _turn_directions(matrices: np.ndarray, directions: np.ndarray, full_lengths: np.ndarray) -> np.ndarray:
    """Each direction turned by its matrix, as a unit vector, or left as it is where the turned one
    is under `_COLLAPSED_RATIO` of its `full_lengths`: the blend collapsed that vertex to a point or
    a line, such as a joint scaled to nothing to hide a part, so it has no direction of its own."""
    turned = np.einsum("vij,vj->vi", matrices, directions)
    lengths = np.linalg.norm(turned, axis=1, keepdims=True)
    is_collapsed = lengths <= _COLLAPSED_RATIO * full_lengths[:, None]
    return np.where(is_collapsed, directions, turned / np.where(is_collapsed, 1.0, lengths))


def _pose_skinned_primitive(doc: dict, binary: bytes, out: bytearray, primitive: dict, joint_matrices: np.ndarray) -> None:
    attributes = primitive["attributes"]
    if primitive.get("targets"):
        raise click.ClickException("morph targets are not supported")
    skins = _calc_vertex_skins(doc, binary, attributes, joint_matrices)
    linear = skins[:, :3, :3]
    is_mirrored = np.linalg.det(linear) < 0
    squared_scales = np.einsum("vij,vij->v", linear, linear)
    positions = _read_accessor(doc, binary, attributes["POSITION"])
    attributes["POSITION"] = _append_accessor(doc, out, np.einsum("vij,vj->vi", linear, positions) + skins[:, :3, 3], "VEC3")
    if "NORMAL" in attributes:
        normals = _turn_directions(_calc_normal_matrices(linear, is_mirrored), _read_accessor(doc, binary, attributes["NORMAL"]), squared_scales)
        attributes["NORMAL"] = _append_accessor(doc, out, normals, "VEC3")
    if "TANGENT" in attributes:
        tangents = _read_accessor(doc, binary, attributes["TANGENT"])
        directions = _turn_directions(linear, tangents[:, :3], np.sqrt(squared_scales))
        # A mirroring blend reverses which way the bitangent, normal x tangent, points.
        handedness = np.where(is_mirrored, -tangents[:, 3], tangents[:, 3])
        attributes["TANGENT"] = _append_accessor(doc, out, np.column_stack([directions, handedness]), "VEC4")
    for name in [name for name in attributes if name.startswith(("JOINTS_", "WEIGHTS_"))]:
        del attributes[name]


def _pose_skinned_node(doc: dict, binary: bytes, out: bytearray, node: dict, world: dict[int, np.ndarray]) -> None:
    skin = doc["skins"][node.pop("skin")]
    joints = skin["joints"]
    if "inverseBindMatrices" in skin:
        inverse_binds = _read_accessor(doc, binary, skin["inverseBindMatrices"]).reshape(-1, 4, 4).transpose(0, 2, 1)
    else:
        inverse_binds = np.tile(np.eye(4), (len(joints), 1, 1))
    joint_matrices = np.stack([world[joint] for joint in joints]) @ inverse_binds
    for primitive in doc["meshes"][node["mesh"]]["primitives"]:
        _pose_skinned_primitive(doc, binary, out, primitive, joint_matrices)


def _validate_skinned_meshes_unshared(doc: dict) -> None:
    users: dict[int, int] = {}
    for node in doc["nodes"]:
        if "mesh" in node:
            users[node["mesh"]] = users.get(node["mesh"], 0) + 1
    shared = sorted({node["mesh"] for node in doc["nodes"] if "skin" in node and users[node["mesh"]] > 1})
    if shared:
        raise click.ClickException(f"skinned meshes {shared} are used by several nodes, which is not supported")


def pose_model(source: bytes, time_s: float, animation_idx: int) -> bytes:
    """The glb `source` frozen at `time_s` seconds into its animation `animation_idx`, as a glb
    whose meshes hang straight off its one scene's root, unskinned and unanimated."""
    doc, binary = _decode_glb(source)
    _validate_supported(doc)
    if not 0 <= animation_idx < len(doc["animations"]):
        raise click.ClickException(f"no animation {animation_idx}; the model has {len(doc['animations'])}")
    _validate_skinned_meshes_unshared(doc)
    world = _build_world_matrices(doc, _build_local_matrices(doc, binary, animation_idx, time_s))
    mesh_node_ids = sorted(idx for idx in world if "mesh" in doc["nodes"][idx])
    if not mesh_node_ids:
        raise click.ClickException("the scene reaches no mesh")
    out = bytearray(binary)
    for idx in mesh_node_ids:
        node = doc["nodes"][idx]
        for key in ("matrix", "translation", "rotation", "scale"):
            node.pop(key, None)
        if "skin" in node:
            _pose_skinned_node(doc, binary, out, node, world)
        else:
            node["matrix"] = world[idx].T.flatten().tolist()
    moved = set(mesh_node_ids)
    for node in doc["nodes"]:
        children = [child for child in node.get("children", []) if child not in moved]
        node.pop("children", None)
        if children:
            node["children"] = children
    doc["scenes"] = [{"nodes": mesh_node_ids}]
    doc["scene"] = 0
    for key in ("skins", "animations"):
        doc.pop(key, None)
    for node in doc["nodes"]:
        node.pop("skin", None)
    return _encode_glb(doc, out)


@click.command()
@click.argument("source", type=click.Path(exists=True, dir_okay=False, path_type=Path))
@click.argument("destination", type=click.Path(dir_okay=False, path_type=Path))
@click.option("--time-s", type=click.FloatRange(min=0), required=True, help="The moment of the animation to freeze, in seconds from its start.")
@click.option("--animation", "animation_idx", type=click.IntRange(min=0), default=0, show_default=True, help="Which of the model's animations to pose, by its index.")
def main(source: Path, destination: Path, time_s: float, animation_idx: int) -> None:
    """Freezes the animated SOURCE glb at --time-s into the static DESTINATION glb."""
    destination.write_bytes(pose_model(source.read_bytes(), time_s, animation_idx))


if __name__ == "__main__":
    main()
