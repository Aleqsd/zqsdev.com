# Makefile for ZQS Terminal

WASM_TARGET = wasm32-unknown-unknown
STATIC_DIR = static
PKG_DIR = pkg
STATIC_PKG = $(STATIC_DIR)/pkg
HOST ?= 0.0.0.0
SERVER_PORT ?= 3000
STATIC_PORT ?= 8765
SERVER_MANIFEST = server/Cargo.toml
NETLIFY_BIN ?= netlify
NETLIFY_FLAGS ?=
PROJECT_VERSION := $(shell cat VERSION 2>/dev/null)
NETLIFY_MESSAGE ?= Deploy $(PROJECT_VERSION)
AUTOTEST_FLAGS ?=

.PHONY: build clean check fmt serve serve-static test autotest deploy-preview deploy-prod deploy update backend-log rag

build:
	@command -v wasm-pack >/dev/null 2>&1 || { echo "wasm-pack not found. Install with 'cargo install wasm-pack'."; exit 1; }
	rustup target add $(WASM_TARGET) >/dev/null 2>&1 || true
	wasm-pack build --target web --release
	@if command -v wasm-opt >/dev/null 2>&1; then \
		wasm-opt -Oz $(PKG_DIR)/zqs_terminal_bg.wasm -o $(PKG_DIR)/zqs_terminal_bg.wasm; \
	else \
		echo "wasm-opt not found. Install binaryen to optimize the WebAssembly output."; \
	fi
	mkdir -p $(STATIC_PKG)
	cp -r $(PKG_DIR)/* $(STATIC_PKG)/
	python3 scripts/minify_css.py $(STATIC_DIR)/style.css -o $(STATIC_DIR)/style.min.css
	@if [ "$(SKIP_RAG)" = "1" ]; then \
		echo "Skipping RAG bundle rebuild because SKIP_RAG=1"; \
	else \
		$(MAKE) rag; \
	fi

rag:
	@command -v python3 >/dev/null 2>&1 || { echo "python3 not found. Install Python 3 to continue."; exit 1; }
	python3 scripts/build_rag.py $(RAG_FLAGS)

rag-inspect:
	@python3 scripts/inspect_rag.py $(RAG_INSPECT_FLAGS)

test:
	@command -v wasm-pack >/dev/null 2>&1 || { echo "wasm-pack not found. Install with 'cargo install wasm-pack'."; exit 1; }
	rustup target add $(WASM_TARGET) >/dev/null 2>&1 || true
	wasm-pack test --node
	cargo test --manifest-path $(SERVER_MANIFEST)

autotest:
	@command -v python3 >/dev/null 2>&1 || { echo "python3 not found. Install Python 3 to continue."; exit 1; }
	@python3 -c "import requests" >/dev/null 2>&1 || { echo "Python package 'requests' not found. Install with 'pip install requests'."; exit 1; }
	python3 scripts/live_smoke_test.py $(AUTOTEST_FLAGS)

check:
	cargo check --target $(WASM_TARGET)

fmt:
	cargo fmt

serve: build
	@echo "Starting Rust proxy server on http://$(HOST):$(SERVER_PORT)"
	HOST=$(HOST) PORT=$(SERVER_PORT) STATIC_DIR=$(STATIC_DIR) cargo run --manifest-path $(SERVER_MANIFEST)

serve-static: build
	@python3 scripts/serve.py --root $(STATIC_DIR) --host $(HOST) --port $(STATIC_PORT)

deploy-preview: build
	@command -v $(NETLIFY_BIN) >/dev/null 2>&1 || { echo "netlify CLI not found. Install with 'npm install -g netlify-cli'."; exit 1; }
	$(NETLIFY_BIN) deploy --dir $(STATIC_DIR) --message "$(NETLIFY_MESSAGE)" $(NETLIFY_FLAGS)

deploy-prod: build
	@command -v $(NETLIFY_BIN) >/dev/null 2>&1 || { echo "netlify CLI not found. Install with 'npm install -g netlify-cli'."; exit 1; }
	$(NETLIFY_BIN) deploy --dir $(STATIC_DIR) --prod --message "$(NETLIFY_MESSAGE)" $(NETLIFY_FLAGS)

deploy: deploy-prod

update:
	@git pull --rebase
	$(MAKE) build
	@sudo systemctl restart zqs-terminal.service

backend-log:
	@sudo tail -f backend.log

clean:
	cargo clean
	rm -rf $(PKG_DIR) $(STATIC_PKG)
