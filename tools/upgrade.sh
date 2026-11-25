#!/usr/bin/env bash
# Bumps every dependency to its latest release, past semver and exact pins: the Rust toolchain to
# the newest dated nightly that ships every component and target the build uses, the Cargo
# workspace's crates (rapier3d and wasm-bindgen included), the client's npm packages,
# and the uv locks of the tools/ scripts. Then reinstalls the tools that track those pins.
set -euo pipefail
shopt -s nullglob
SCRIPT_DIR=$( cd -- "$( dirname -- "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )
TOP_DIR=$( git -C "$SCRIPT_DIR" rev-parse --show-toplevel )
cd "$TOP_DIR"

# Days back from today to look for a nightly, since a day's nightly can lack clippy or rustfmt.
NIGHTLY_SEARCH_DAYS=14

# Whether the channel manifest `$1` marks package `$2` available for target `$3`.
is_available() {
    local header="[pkg.$2.target.$3]"
    awk -v header="$header" '$0 == header { found = 1; next } found && /^available = / { print $3; exit }' <<< "$1" \
        | grep -qx true
}

upgrade_toolchain() {
    local host day date manifest
    host=$(rustc -vV | sed -n 's/^host: //p')
    for ((day = 0; day < NIGHTLY_SEARCH_DAYS; day++)); do
        date=$(date -u -d "-$day day" +%Y-%m-%d)
        manifest=$(curl -sSf "https://static.rust-lang.org/dist/$date/channel-rust-nightly.toml") || continue
        if is_available "$manifest" rustfmt-preview "$host" && is_available "$manifest" clippy-preview "$host" \
            && is_available "$manifest" rust-std wasm32-unknown-unknown; then
            sed -i "s/^channel = \".*\"$/channel = \"nightly-$date\"/" rust-toolchain.toml
            rustup toolchain install
            return
        fi
    done
    echo "channel-rust-nightly.toml: no nightly of the last $NIGHTLY_SEARCH_DAYS days ships rustfmt and clippy for $host and rust-std for wasm32-unknown-unknown" >&2
    exit 1
}

upgrade_crates() {
    cargo install cargo-edit --locked --root build/cargo
    PATH="$TOP_DIR/build/cargo/bin:$PATH" cargo upgrade --incompatible allow --pinned allow
    cargo update
}

list_latest() {
    node -p "Object.keys(require('./package.json').$1 ?? {}).map((name) => name + '@latest').join(' ')"
}

upgrade_npm_packages() {
    # shellcheck disable=SC2046 # one word per package
    npm install --no-audit --no-fund --save-prod $(list_latest dependencies)
    # shellcheck disable=SC2046
    npm install --no-audit --no-fund --save-dev $(list_latest devDependencies)
    npm update --no-audit --no-fund
}

upgrade_script_locks() {
    local lock
    for lock in tools/*.py.lock; do
        uv lock --upgrade --script "${lock%.lock}"
    done
}

# The toolchain goes first: the latest cargo-edit, and cargo's choice of crate versions, follow the active rustc.
upgrade_toolchain
upgrade_crates
(cd src/client && upgrade_npm_packages)
upgrade_script_locks
bash tools/install.sh
