.PHONY: install run test format clean wasm
.DEFAULT_GOAL := run

ifeq ($(OS),Windows_NT)
    # Guard via BASH, not SHELL: make silently falls back to cmd.exe on an empty SHELL.
    BASH_CACHE := build/.bash-path
    BASH := $(file < $(BASH_CACHE))
    space := $() $()
    ifeq ($(wildcard $(subst $(space),\$(space),$(BASH))),)
        BASH := $(shell powershell -NoProfile -ExecutionPolicy Bypass -File tools/find-bash.ps1)
        ifeq ($(BASH),)
            $(error Git bash not found by tools/find-bash.ps1)
        endif
        ifneq ($(wildcard build),)
            $(file > $(BASH_CACHE),$(BASH))
        endif
    endif
    SHELL := $(BASH)
else
    SHELL := bash
endif
.SHELLFLAGS := -euo pipefail -c

WASM_BINDGEN := build/cargo/bin/wasm-bindgen
WASM_MODULE := build/target/wasm32-unknown-unknown/release/cake_wasm.wasm

install: build/.gitignore
	bash tools/install.sh

run: wasm
	cd src/client && npm run dev

test: wasm
	cargo fmt --check
	cargo clippy --all-targets -- -D warnings
	cargo test
	cd src/client && npm run check
	uv run --locked --script tools/test-repair-model.py
	cargo run -q --release -p cake-core --example replay > build/replay-native.txt
	node tools/replay-wasm.mjs > build/replay-wasm.txt
	diff build/replay-native.txt build/replay-wasm.txt

format:
	cargo fmt

clean:
	rm -rf build src/client/node_modules

wasm: build/.gitignore
	cargo build --release --target wasm32-unknown-unknown -p cake-wasm
	$(WASM_BINDGEN) --target web --out-dir build/wasm $(WASM_MODULE)

build/.gitignore:
	mkdir -p build
	echo '*' > $@
