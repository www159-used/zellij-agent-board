.DEFAULT_GOAL := help
SHELL := /bin/bash

CARGO ?= cargo
PYTHON ?= python3
TARGET ?=
VERSION ?=
WASM ?= target/wasm32-wasip1/release/zellij-agent-board.wasm
TUI ?= target/$(if $(TARGET),$(TARGET)/)release/board-tui
# A relative TUI is not found by a bare `exec` from the shell, so run it as
# ./path; an absolute TUI (a release bundle) is left alone.
TUI_BIN = $(if $(filter /%,$(TUI)),$(TUI),./$(TUI))
OUT_DIR ?= target/packages
ASSET_DIR ?= target/release-assets
ARCHIVE ?= $(OUT_DIR)/zellij-agent-board-$(VERSION)-$(TARGET).tar.gz
ADAPTER ?= all
SCENE ?= e2e/scenes/slash-search-moves.scene
SCENARIO ?=
REPEAT ?=
EVENT ?=

# Pass values through the environment so paths and arguments stay quoted.
export TARGET VERSION WASM TUI OUT_DIR ASSET_DIR ARCHIVE ADAPTER SCENE SCENARIO REPEAT EVENT

.PHONY: help fmt fmt-check lint check test test-rust test-hooks test-harness \
	build wasm tui install install-hooks run stats scan reconcile catalog hook \
	e2e replay e2e-zellij fault package package-wasm test-package clean

help: ## List commands and their parameters
	@awk 'BEGIN {FS = ":.*## "} /^[a-zA-Z0-9_-]+:.*## / {printf "  %-18s %s\n", $$1, $$2}' Makefile

fmt: ## Format Rust code
	$(CARGO) fmt

fmt-check: ## Check Rust formatting
	$(CARGO) fmt --check

lint: ## Run Rust Clippy with warnings as errors
	$(CARGO) clippy --locked --lib --all-targets -- -D warnings

check: fmt-check lint test ## Run formatting, lint, and all unit tests

test: test-rust test-hooks test-harness ## Run Rust, hook, and Zellij harness unit tests

test-rust: ## Run Rust unit tests and board scenes
	$(CARGO) test --locked --lib --bin board-tui --test daemon

test-hooks: ## Test hook scripts with isolated state
	$(PYTHON) -B scripts/test-hooks.py

test-harness: ## Test the Zellij harness without launching Zellij
	PYTHONPATH=e2e/zellij $(PYTHON) -B -m unittest discover -s e2e/zellij/harness -p 'test_*.py'

build: wasm tui ## Build the release WASM bridge and host TUI

wasm: ## Build the WASM bridge
	$(CARGO) build --locked --target wasm32-wasip1 --release --features plugin --bin zellij-agent-board

tui: ## Build the host TUI (optional TARGET=Rust-triple)
	@set --; if [[ -n "$$TARGET" ]]; then set -- --target "$$TARGET"; fi; \
	$(CARGO) build --locked --release --bin board-tui "$$@"

# Release bundles install their own binaries without needing Rust.
ifeq ($(and $(wildcard zellij-agent-board.wasm),$(wildcard board-tui)),)
install: build
endif
install: ## Install both binaries (ZELLIJ_AGENT_BOARD_PLUGIN_PATH overrides destination)
	bash scripts/install.sh "$$WASM" "$$TUI"

install-hooks: ## Install hooks (ADAPTER=all or an adapter id)
	$(PYTHON) -B scripts/install-hooks.py "$$ADAPTER"

run: tui ## Open the host TUI in this terminal
	@$(TUI_BIN)

stats: tui ## Show local usage statistics
	@$(TUI_BIN) --stats

scan: ## Print the shell scanner's SCAN/HOOK/META snapshot
	@bash scripts/scan-agents.sh

reconcile: tui ## Refresh the host scan and title store once
	@$(TUI_BIN) --reconcile

catalog: ## Print the adapter catalog's scan index
	@$(PYTHON) -B scripts/lib/catalog.py dump

hook: ## Send one hook event (EVENT=name, JSON payload on stdin)
	@test -n "$$EVENT" || { echo 'usage: make hook EVENT=name < payload.json' >&2; exit 2; }
	@bash scripts/zellij-agent-board-hook.sh "$$EVENT"

e2e: ## Run all board scenes without a TTY
	$(CARGO) test --locked --lib scenes_in_e2e

replay: ## Replay one board scene (SCENE=path.scene)
	$(CARGO) run --locked --bin board-tui -- --replay "$$SCENE"

e2e-zellij: build ## Check board startup and dismissal in an isolated Zellij session
	bash scripts/e2e-zellij.sh "$$WASM" "$$TUI"

fault: build ## Run Zellij lifecycle experiments (optional SCENARIO=name REPEAT=count)
	@set --; if [[ -n "$$SCENARIO" ]]; then set -- "$$SCENARIO"; fi; \
	if [[ -n "$$REPEAT" ]]; then set -- "$$@" "$$REPEAT"; fi; \
	ZAB_FAULT_WASM="$$WASM" ZAB_FAULT_TUI="$$TUI" bash scripts/zab-fault.sh "$$@"

package: ## Bundle prebuilt binaries (VERSION=vX.Y.Z TARGET=triple, optional WASM/TUI/OUT_DIR)
	@test -n "$$VERSION" -a -n "$$TARGET" || { echo 'usage: make package VERSION=vX.Y.Z TARGET=triple' >&2; exit 2; }
	bash scripts/package-release.sh "$$VERSION" "$$TARGET" "$$WASM" "$$TUI" "$$OUT_DIR"

package-wasm: ## Package a prebuilt standalone WASM and checksum (VERSION=vX.Y.Z)
	@test -n "$$VERSION" || { echo 'usage: make package-wasm VERSION=vX.Y.Z' >&2; exit 2; }
	@. "$(CURDIR)/scripts/lib/release.sh"; \
	require_safe_name "$$VERSION" || exit 2; \
	mkdir -p "$$ASSET_DIR"; \
	cp "$$WASM" "$$ASSET_DIR/zellij-agent-board-$$VERSION.wasm" && \
	cd "$$ASSET_DIR" && \
	sha256_line "zellij-agent-board-$$VERSION.wasm" > "zellij-agent-board-$$VERSION.wasm.sha256"

test-package: ## Test installation from an archive (ARCHIVE=path, or VERSION and TARGET)
	bash scripts/test-release-package.sh "$$ARCHIVE"

clean: ## Remove Cargo build artifacts
	$(CARGO) clean
