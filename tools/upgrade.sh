#!/usr/bin/env bash
# Bumps every dependency to its latest release, past semver and exact pins: the Rust toolchain,
# the Cargo workspace's crates (rapier3d and wasm-bindgen included), the client's npm packages,
# and the uv locks of the tools/ scripts. Then reinstalls the tools that track those pins.
set -euo pipefail
shopt -s nullglob
SCRIPT_DIR=$( cd -- "$( dirname -- "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )
TOP_DIR=$( git -C "$SCRIPT_DIR" rev-parse --show-toplevel )
cd "$TOP_DIR"

upgrade_toolchain() {
    local version
    version=$(curl -sSf https://static.rust-lang.org/dist/channel-rust-stable.toml | sed -n '/^\[pkg\.rust\]$/,/^version/{s/^version = "\([0-9.]*\) .*/\1/p}')
    if [[ -z $version ]]; then
        echo "channel-rust-stable.toml: no [pkg.rust] version found" >&2
        exit 1
    fi
    sed -i "s/^channel = \".*\"$/channel = \"$version\"/" rust-toolchain.toml
    rustup toolchain install
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
