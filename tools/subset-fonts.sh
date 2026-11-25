#!/usr/bin/env bash
# Rebuilds the client's bundled fonts in src/client/src/fonts/: downloads Pacifico (titles and
# Cut!) and Nunito (all other text) from a pinned commit of Google's font repository, subsets
# each to the characters the game shows, and writes it as WOFF2 beside its SIL OFL license.
# Neither font has a Reserved Font Name, so the subsets may keep their names.
# Usage: tools/subset-fonts.sh
set -euo pipefail
SCRIPT_DIR=$( cd -- "$( dirname -- "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )
TOP_DIR=$( git -C "$SCRIPT_DIR" rev-parse --show-toplevel )

FONTS_COMMIT=83c62e30d87daa70216dab6b984d00d20d96d585
FONTTOOLS_VERSION=4.68.0
OUT_DIR=$TOP_DIR/src/client/src/fonts
# Titles and Cut! are fixed English text.
SCRIPT_UNICODES=U+0020-007E
# Player names may carry accented Latin letters and typographic punctuation; anything else falls back to a system font.
SANS_UNICODES=U+0020-007E,U+00A0-00FF,U+2013-2014,U+2018-201D,U+2022,U+2026

require_commands() {
    for cmd in "$@"; do
        command -v "$cmd" >/dev/null 2>&1 || { echo "Error: '$cmd' not found" >&2; exit 1; }
    done
}

# Downloads one file of Google's font repository into the temp dir.
fetch() {
    curl -sSfL -o "$tmp_dir/$2" "https://raw.githubusercontent.com/google/fonts/$FONTS_COMMIT/$1"
}

subset() {
    local source=$1 out=$2 unicodes=$3
    uv run --quiet --isolated --no-project --with "fonttools[woff]==$FONTTOOLS_VERSION" -- \
        python -I -m fontTools.subset "$source" --output-file="$out" --flavor=woff2 \
        --unicodes="$unicodes" --layout-features='*' --name-IDs='*' --name-languages='*'
    echo "$(basename "$out"): $(wc -c < "$out") bytes"
}

[[ $# -eq 0 ]] || { echo "Error: unknown argument '$1'" >&2; exit 1; }
require_commands curl uv

tmp_dir=$(mktemp -d)
trap 'rm -rf "$tmp_dir"' EXIT
fetch ofl/pacifico/Pacifico-Regular.ttf pacifico.ttf
fetch ofl/pacifico/OFL.txt pacifico-OFL.txt
fetch "ofl/nunito/Nunito%5Bwght%5D.ttf" nunito.ttf
fetch ofl/nunito/OFL.txt nunito-OFL.txt

mkdir -p "$OUT_DIR"
subset "$tmp_dir/pacifico.ttf" "$OUT_DIR/pacifico.woff2" "$SCRIPT_UNICODES"
subset "$tmp_dir/nunito.ttf" "$OUT_DIR/nunito.woff2" "$SANS_UNICODES"
cp "$tmp_dir/pacifico-OFL.txt" "$tmp_dir/nunito-OFL.txt" "$OUT_DIR/"
