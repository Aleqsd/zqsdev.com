# Makefile for ZQS Terminal

WASM_TARGET = wasm32-unknown-unknown
STATIC_DIR = static
PKG_DIR = pkg
STATIC_PKG = $(STATIC_DIR)/pkg
HOST ?= 0.0.0.0
STATIC_PORT ?= 8765
API_MANIFEST = api/Cargo.toml
WRANGLER ?= npx wrangler
PROJECT_VERSION := $(shell cat VERSION 2>/dev/null)
LOCAL_COMMIT := $(shell git rev-parse --short HEAD 2>/dev/null)
BUILD_ID := $(or $(LOCAL_COMMIT),$(PROJECT_VERSION),dev)
AUTOTEST_FLAGS ?=
VERSION_URL ?= https://zqsdev.com/api/version
# `make deploy-api` checks the new release here, then rolls back on failure (VERIFY_FLAGS=--no-rollback only reports).
API_VERIFY_URL ?= https://zqsdev.com
VERIFY_FLAGS ?=
# Local development (README, "Local development"): site, API and OpenAI stub ports.
DEV_SITE_PORT ?= 8787
DEV_API_PORT ?= 8788
STUB_PORT ?= 9911
# Without the stub, empty this to call the real OpenAI with the key of api/.dev.vars (about $0.0005 a question).
DEV_API_FLAGS ?= --var OPENAI_BASE_URL:http://127.0.0.1:$(STUB_PORT)/v1
# Local fault injection (scripts/test_api_local.py) must never reach a deploy.
NO_FAULTS = @test -z "$$ZQS_FAULT_INJECTION" || { echo "Unset ZQS_FAULT_INJECTION first: it compiles in local fault injection."; exit 1; }

.PHONY: build build-frontend clean check fmt serve-static test autotest deploy-preview deploy-prod deploy dev dev-site dev-api dev-stub test-api-local api-dry-run deploy-api api-tail rag rag-inspect version-check bump-version ensure-version-bumped

# Frontend bundle into static/, then a check that the API Worker still builds: it embeds
# static/data at build time (64 KB knowledge guard, prompt template).
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
	@echo "window.__BUILD_ID__ = \"$(BUILD_ID)\";" > $(STATIC_DIR)/build_id.js
	python3 scripts/validate_knowledge.py
	cargo check --manifest-path $(API_MANIFEST) --target $(WASM_TARGET)

build-frontend:
	@$(MAKE) build

rag:
	@command -v python3 >/dev/null 2>&1 || { echo "python3 not found. Install Python 3 to continue."; exit 1; }
	python3 scripts/build_rag.py $(RAG_FLAGS)

rag-inspect:
	@python3 scripts/inspect_rag.py $(RAG_INSPECT_FLAGS)

test:
	@command -v wasm-pack >/dev/null 2>&1 || { echo "wasm-pack not found. Install with 'cargo install wasm-pack'."; exit 1; }
	@command -v python3 >/dev/null 2>&1 || { echo "python3 not found. Install Python 3 to continue."; exit 1; }
	rustup target add $(WASM_TARGET) >/dev/null 2>&1 || true
	python3 -m unittest scripts.test_knowledge
	python3 -m unittest scripts.test_cv_site
	python3 -m unittest scripts.test_cv_discovery
	python3 -m unittest scripts.test_ops_scripts
	wasm-pack test --node
	cargo test --manifest-path $(API_MANIFEST) --lib

autotest:
	@command -v python3 >/dev/null 2>&1 || { echo "python3 not found. Install Python 3 to continue."; exit 1; }
	@python3 -c "import requests" >/dev/null 2>&1 || { echo "Python package 'requests' not found. Install with 'pip install requests'."; exit 1; }
	python3 scripts/live_smoke_test.py $(AUTOTEST_FLAGS)

check:
	cargo check --target $(WASM_TARGET)
	cargo clippy --manifest-path $(API_MANIFEST) --target $(WASM_TARGET) -- -D warnings

fmt:
	cargo fmt
	cargo fmt --manifest-path $(API_MANIFEST) --all

serve-static: build
	@python3 scripts/serve.py --root $(STATIC_DIR) --host $(HOST) --port $(STATIC_PORT)

# Site and API Worker in one wrangler session (experimental multi-config; the three-terminal form below is
# the reference). It runs from api/ because wrangler starts the API's custom build in its own working directory.
dev:
	cd api && $(WRANGLER) dev -c ../wrangler.jsonc -c wrangler.jsonc --infer-origin-from-routes=false

# The three-terminal local stack: `make dev-stub`, `make dev-api`, `make dev-site` (its API binding connects to dev-api).
dev-stub:
	python3 scripts/openai_stub.py --port $(STUB_PORT)

dev-api:
	cd api && $(WRANGLER) dev --local --port $(DEV_API_PORT) $(DEV_API_FLAGS)

dev-site:
	$(WRANGLER) dev --local --port $(DEV_SITE_PORT) --infer-origin-from-routes=false

# Starts the stub and both Workers itself, runs every check, stops everything.
test-api-local:
	python3 scripts/test_api_local.py --site-port $(DEV_SITE_PORT) --api-port $(DEV_API_PORT) --stub-port $(STUB_PORT)

# Serve the site Worker locally (send x-test-host to act as another hostname).
deploy-preview: build
	$(WRANGLER) dev --infer-origin-from-routes=false

# Workers Builds deploys main on every push; this is the manual equivalent for the site Worker.
deploy-prod: build
	$(WRANGLER) deploy

deploy: deploy-prod

# Build and validate the API Worker without uploading (prints Total Upload and the bindings).
api-dry-run:
	$(NO_FAULTS)
	cd api && $(WRANGLER) deploy --dry-run --outdir dist

# Deploy the API Worker from a developer machine (Workers Builds has no Rust), then verify it.
deploy-api:
	$(NO_FAULTS)
	@git diff --quiet HEAD -- || { echo "Commit first: /api/version reports the HEAD commit."; exit 1; }
	cargo test --manifest-path $(API_MANIFEST) --lib
	cd api && ZQS_BUILD_COMMIT=$$(git rev-parse HEAD) $(WRANGLER) deploy
	python3 scripts/verify_api_deploy.py --base-url $(API_VERIFY_URL) $(VERIFY_FLAGS)

api-tail:
	cd api && $(WRANGLER) tail

version-check:
	@echo "Local frontend version: $(PROJECT_VERSION)"
	@echo "Local commit: $(LOCAL_COMMIT)"
	@VERSION_URL="$(VERSION_URL)" python3 scripts/version_check.py

bump-version:
	@python3 scripts/bump_version.py $(BUMP_ARGS)

ensure-version-bumped:
	@git diff --cached --quiet VERSION && { echo "\n[ensure-version-bumped] VERSION is not staged. Run 'git add VERSION' after bumping."; exit 1; } || echo "[ensure-version-bumped] VERSION bump detected in staged changes."

clean:
	cargo clean
	cargo clean --manifest-path $(API_MANIFEST)
	rm -rf $(PKG_DIR) $(STATIC_PKG) api/build api/dist
