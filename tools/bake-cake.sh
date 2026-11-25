#!/usr/bin/env bash
# Turns a sourced .glb model into a playable .cake file: repairs it to one closed solid scaled to
# the cake size and resting on the table (tools/repair-model.py), then builds its collider hulls
# (the cake-bake crate). Extra arguments, such as --keep-largest or --size-m, go to the repair,
# except --pose-s, which first freezes an animated model at that many seconds into its first
# animation (tools/pose-model.py), since the repair ignores skins and animation, and
# --trim-overhangs, which goes to cake-bake.
# An output ending in .cake.br is brotli-packed for git (tools/cake-pack.mjs), which make unpacks.
# Usage: tools/bake-cake.sh <source.glb> <out.cake | out.cake.br> [--pose-s <seconds>] [--trim-overhangs] [repair options]
set -euo pipefail
SCRIPT_DIR=$( cd -- "$( dirname -- "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )
TOP_DIR=$( git -C "$SCRIPT_DIR" rev-parse --show-toplevel )
cd "$TOP_DIR"

if [[ $# -lt 2 ]]; then
    echo "usage: tools/bake-cake.sh <source.glb> <out.cake | out.cake.br> [--pose-s <seconds>] [--trim-overhangs] [repair options]" >&2
    exit 2
fi
source=$1
destination=$2
shift 2
pose_s=
repair_args=()
bake_args=()
while [[ $# -gt 0 ]]; do
    case $1 in
        --pose-s)
            [[ $# -ge 2 ]] || { echo "--pose-s needs a time in seconds" >&2; exit 2; }
            pose_s=$2
            shift 2
            ;;
        --trim-overhangs) bake_args+=("$1"); shift ;;
        *) repair_args+=("$1"); shift ;;
    esac
done

mkdir -p build/repaired "$(dirname "$destination")"
name=$(basename "${source%.*}")
repaired=build/repaired/$name.glb
if [[ -n $pose_s ]]; then
    posed=build/repaired/$name-posed.glb
    uv run --locked --script tools/pose-model.py --time-s "$pose_s" "$source" "$posed"
    source=$posed
fi
uv run --locked --script tools/repair-model.py "${repair_args[@]}" "$source" "$repaired"
if [[ $destination == *.cake.br ]]; then
    baked=build/repaired/$(basename "${destination%.br}")
    cargo run --release --quiet -p cake-bake -- "$repaired" "$baked" "${bake_args[@]}"
    node tools/cake-pack.mjs pack "$baked" "$destination"
else
    cargo run --release --quiet -p cake-bake -- "$repaired" "$destination" "${bake_args[@]}"
fi
