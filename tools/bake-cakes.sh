#!/usr/bin/env bash
# Rebakes the cakes src/client/src/cakes.ts lists, each from its source build/candidates/objaverse/<id>.glb
# with --keep-largest and its entry's repair options, into assets/cakes/<id>.cake.br. Run it after a
# change to the repair or the bake, or to a cake's options. BAKE_JOBS sets how many bake at once.
# Usage: tools/bake-cakes.sh [id...]   (every listed cake when no id is given)
set -euo pipefail
SCRIPT_DIR=$( cd -- "$( dirname -- "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )
TOP_DIR=$( git -C "$SCRIPT_DIR" rev-parse --show-toplevel )
cd "$TOP_DIR"

# One line per cake to bake: its id, then its repair options.
recipes=$(node --input-type=module -e '
import { CAKES } from "./src/client/src/cakes.ts";
const wanted = process.argv.slice(1);
const unknown = wanted.filter((id) => !CAKES.some((cake) => cake.id === id));
if (unknown.length > 0) throw new Error(`not in src/client/src/cakes.ts: ${unknown.join(" ")}`);
for (const cake of CAKES) {
  if (wanted.length === 0 || wanted.includes(cake.id)) console.log([cake.id, ...(cake.repair ?? [])].join(" "));
}' "$@")

bake_one() {
    local id=$1
    shift
    local source=build/candidates/objaverse/$id.glb
    if [[ ! -f $source ]]; then
        echo "$id: missing $source; download it from https://sketchfab.com/3d-models/$id" >&2
        return 1
    fi
    if bash tools/bake-cake.sh "$source" "assets/cakes/$id.cake.br" --keep-largest "$@" > "build/repaired/$id.log" 2>&1; then
        echo "$id: baked"
    else
        echo "$id: failed, see build/repaired/$id.log" >&2
        return 1
    fi
}
export -f bake_one
mkdir -p build/repaired
echo "$recipes" | xargs -P "${BAKE_JOBS:-3}" -L 1 bash -c 'bake_one "$@"' _
