SHELL := /bin/bash
PYTHON := python3
VENV := .venv
VENV_BIN := $(VENV)/bin

.PHONY: bootstrap verify-toolchains install install-node install-python fetch-rust format format-check lint typecheck test check clean

bootstrap: verify-toolchains install check

verify-toolchains:
	@node_version="$$(node --version 2>/dev/null || true)"; expected_node="v$$(cat .node-version)"; \
	  test "$$node_version" = "$$expected_node" || { echo "Expected Node $$expected_node, found $${node_version:-missing}."; exit 1; }
	@python_version="$$($(PYTHON) -c 'import platform; print(platform.python_version())' 2>/dev/null || true)"; expected_python="$$(cat .python-version)"; \
	  test "$$python_version" = "$$expected_python" || { echo "Expected Python $$expected_python, found $${python_version:-missing}."; exit 1; }
	@rust_version="$$(rustc --version 2>/dev/null | awk '{print $$2}' || true)"; \
	  test "$$rust_version" = "1.98.0" || { echo "Expected Rust 1.98.0, found $${rust_version:-missing}. Install with rustup; rust-toolchain.toml pins it."; exit 1; }

install: install-node install-python fetch-rust

install-node:
	corepack enable
	corepack prepare pnpm@11.22.0 --activate
	pnpm install --frozen-lockfile=false

install-python:
	$(PYTHON) -m venv $(VENV)
	$(VENV_BIN)/python -m pip install --upgrade pip
	$(VENV_BIN)/python -m pip install -r requirements-bootstrap.txt -e apps/api -e workers/ingest

fetch-rust:
	cargo fetch --locked

format:
	pnpm format
	$(VENV_BIN)/ruff format apps/api workers/ingest
	cargo fmt --all

format-check:
	pnpm format:check
	$(VENV_BIN)/ruff format --check apps/api workers/ingest
	cargo fmt --all -- --check

lint:
	pnpm lint
	$(VENV_BIN)/ruff check apps/api workers/ingest
	cargo clippy --workspace --all-targets --locked -- -D warnings

typecheck:
	pnpm typecheck
	$(VENV_BIN)/mypy --strict apps/api/src workers/ingest/src apps/api/tests workers/ingest/tests
	cargo check --workspace --all-targets --locked

test:
	pnpm test
	$(VENV_BIN)/pytest -q apps/api/tests workers/ingest/tests
	cargo test --workspace --all-targets --locked

check: format-check lint typecheck test

clean:
	rm -rf $(VENV) node_modules apps/web/node_modules packages/contracts/node_modules apps/web/.next target .pytest_cache .mypy_cache .ruff_cache
