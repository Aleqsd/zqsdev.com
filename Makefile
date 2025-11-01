# Makefile for ZQS Terminal

WASM_TARGET = wasm32-unknown-unknown
STATIC_DIR = static
PKG_DIR = pkg
STATIC_PKG = $(STATIC_DIR)/pkg
HOST ?= 0.0.0.0
SERVER_PORT ?= 3000
STATIC_PORT ?= 8765
SERVER_MANIFEST = server/Cargo.toml

.PHONY: build clean check fmt serve serve-static test

build:
	@command -v wasm-pack >/dev/null 2>&1 || { echo "wasm-pack not found. Install with 'cargo install wasm-pack'."; exit 1; }
	rustup target add $(WASM_TARGET) >/dev/null 2>&1 || true
	wasm-pack build --target web --release
	mkdir -p $(STATIC_PKG)
	cp -r $(PKG_DIR)/* $(STATIC_PKG)/

test:
	@command -v wasm-pack >/dev/null 2>&1 || { echo "wasm-pack not found. Install with 'cargo install wasm-pack'."; exit 1; }
	rustup target add $(WASM_TARGET) >/dev/null 2>&1 || true
	wasm-pack test --node
	cargo test --manifest-path $(SERVER_MANIFEST)

check:
	cargo check --target $(WASM_TARGET)

fmt:
	cargo fmt

serve: build
	@echo "Starting Rust proxy server on http://$(HOST):$(SERVER_PORT)"
	HOST=$(HOST) PORT=$(SERVER_PORT) STATIC_DIR=$(STATIC_DIR) cargo run --manifest-path $(SERVER_MANIFEST)

serve-static: build
	@python3 scripts/serve.py --root $(STATIC_DIR) --host $(HOST) --port $(STATIC_PORT)

clean:
	cargo clean
	rm -rf $(PKG_DIR) $(STATIC_PKG)
