#!/usr/bin/env bash
# Turns a sourced .glb model into a playable .cake file: repairs it to one closed solid scaled to
# the cake size and resting on the table (tools/repair-model.py), then builds its collider hulls
# (the cake-bake crate). Extra arguments, such as --keep-largest or --size-m, go to the repair.
# An output ending in .cake.br is brotli-packed for git (tools/cake-pack.mjs), which make unpacks.
# Usage: tools/bake-cake.sh <source.glb> <out.cake | out.cake.br> [repair options]
set -euo pipefail
SCRIPT_DIR=$( cd -- "$( dirname -- "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )
TOP_DIR=$( git -C "$SCRIPT_DIR" rev-parse --show-toplevel )
cd "$TOP_DIR"

if [[ $# -lt 2 ]]; then
    echo "usage: tools/bake-cake.sh <source.glb> <out.cake | out.cake.br> [repair options]" >&2
    exit 2
fi
source=$1
destination=$2
shift 2

mkdir -p build/repaired "$(dirname "$destination")"
repaired=build/repaired/$(basename "${source%.*}").glb
uv run --locked --script tools/repair-model.py "$@" "$source" "$repaired"
if [[ $destination == *.cake.br ]]; then
    baked=build/repaired/$(basename "${destination%.br}")
    cargo run --release --quiet -p cake-bake -- "$repaired" "$baked"
    node tools/cake-pack.mjs pack "$baked" "$destination"
else
    cargo run --release --quiet -p cake-bake -- "$repaired" "$destination"
fi
