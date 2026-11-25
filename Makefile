.PHONY: install upgrade run serve test format clean wasm cakes previews
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

# BUILD_DIR holds the cargo target, the WASM, and the replay outputs, so that agents building in
# parallel each name their own. vite.config.ts reads it too. Tools and cakes stay in build/.
BUILD_DIR ?= build
export BUILD_DIR
export CARGO_TARGET_DIR := $(abspath $(BUILD_DIR))/target
WASM_DIR := $(abspath $(BUILD_DIR))/wasm
WASM_BINDGEN := build/cargo/bin/wasm-bindgen
WASM_MODULE := $(CARGO_TARGET_DIR)/wasm32-unknown-unknown/release/cake_wasm.wasm
TSCONFIG := $(abspath $(BUILD_DIR))/tsconfig.json

# The client's tsconfig with @wasm pointed at WASM_DIR, since tsconfig.json cannot read BUILD_DIR.
define TSCONFIG_JSON
{
  "extends": "$(CURDIR)/src/client/tsconfig.json",
  "compilerOptions": {
    "typeRoots": ["$(CURDIR)/src/client/node_modules"],
    "paths": { "@wasm/*": ["$(WASM_DIR)/*"] }
  }
}
endef

install: build/.gitignore
	bash tools/install.sh

upgrade: build/.gitignore
	bash tools/upgrade.sh
	$(MAKE) test

run: wasm cakes
	cd src/client && npm run dev

# Where the multiplayer server listens: plain HTTP, behind a reverse proxy that terminates HTTPS.
SERVE_LISTEN ?= 127.0.0.1:8080

serve: wasm cakes
	cd src/client && npx vite build
	cargo run --release -p cake-server -- --listen $(SERVE_LISTEN) --client-dir $(BUILD_DIR)/client

# Cakes the determinism test replays beside the placeholder: the Marble Bust and Work Boot, whose
# native and WASM worlds once parted on platform math in their mass properties.
REPLAY_CAKES := fc6e731a0131471ba8e45511c7ea9996 d9506d9a318e4c56ac3d8ab273bd2540

test: wasm
	cargo fmt --check
	cargo clippy --all-targets -- -D warnings
	cargo test
	$(file > $(TSCONFIG),$(TSCONFIG_JSON))
	cd src/client && npm run check -- -p $(TSCONFIG)
	node tools/check-cakes.mjs
	node tools/check-libraries.mjs
	uv run --locked --script tools/test-repair-model.py
	cargo run -q --release -p cake-core --example replay -- $(REPLAY_CAKES) > $(BUILD_DIR)/replay-native.txt
	node tools/replay-wasm.mjs $(WASM_DIR) $(REPLAY_CAKES) > $(BUILD_DIR)/replay-wasm.txt
	diff $(BUILD_DIR)/replay-native.txt $(BUILD_DIR)/replay-wasm.txt

format:
	cargo fmt

clean:
	rm -rf build src/client/node_modules

wasm: build/.gitignore
	cargo build --release --target wasm32-unknown-unknown -p cake-wasm
	$(WASM_BINDGEN) --target web --out-dir $(WASM_DIR) $(WASM_MODULE)

cakes: build/.gitignore
	node tools/cake-pack.mjs sync assets/cakes build/public/cakes
	rm -rf build/public/previews
	mkdir build/public/previews
	cp assets/previews/*.webp build/public/previews/
	rm -rf build/public/rules
	mkdir build/public/rules
	cp assets/rules/*.webp build/public/rules/

# Rerenders the lobby picker's previews of cakes rebaked since their last render.
previews: wasm
	node tools/render-previews.mjs

build/.gitignore:
	mkdir -p build
	echo '*' > $@
