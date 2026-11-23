#!/usr/bin/env bash
# Ensures the Rust toolchain, Node, uv for the model tools, the pinned wasm-bindgen CLI, and the client's packages.
set -euo pipefail
shopt -s nullglob
SCRIPT_DIR=$( cd -- "$( dirname -- "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )
TOP_DIR=$( git -C "$SCRIPT_DIR" rev-parse --show-toplevel )
cd "$TOP_DIR"

ensure_winget_package() {
    local command=$1 package_id=$2
    command -v "$command" > /dev/null && return
    if [[ "${OS:-}" != "Windows_NT" ]]; then
        echo "$command: not found on PATH" >&2
        exit 1
    fi
    winget install --exact --id "$package_id" --silent --accept-package-agreements --accept-source-agreements
    echo "$command: installed by winget; open a new terminal so PATH includes it, then rerun make install" >&2
    exit 1
}

ensure_wasm_bindgen() {
    local version
    version=$(sed -n 's/^wasm-bindgen = "=\(.*\)"$/\1/p' src/wasm/Cargo.toml)
    if [[ -z $version ]]; then
        echo "src/wasm/Cargo.toml: no exact wasm-bindgen = \"=<version>\" pin to match the CLI to" >&2
        exit 1
    fi
    if [[ "$(build/cargo/bin/wasm-bindgen --version 2> /dev/null)" == "wasm-bindgen $version" ]]; then
        return
    fi
    cargo install wasm-bindgen-cli --version "$version" --locked --root build/cargo
}

ensure_winget_package rustup Rustlang.Rustup
ensure_winget_package node OpenJS.NodeJS.LTS
ensure_winget_package uv astral-sh.uv
rustup toolchain install
ensure_wasm_bindgen
(cd src/client && npm install --no-audit --no-fund)
