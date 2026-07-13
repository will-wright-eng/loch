# Development workflow for loch. All targets are phony (no file outputs);
# `make` alone prints this list.
#
# NOTE: Cargo.lock intentionally pins home/time/human_format below their latest
# versions for rustc 1.87 compatibility — a bare `cargo update` will undo that
# and break the build. Update dependencies one at a time with
# `cargo update <crate> --precise <version>`.

.DEFAULT_GOAL := help

ARGS ?=
REPO ?= .
PERF_REPO ?= /tmp/loch-perf/tokei

.PHONY: help check build release test fmt fmt-check lint run install doc clean perf plot ci

help: ## Show available targets
	@grep -E '^[a-zA-Z_-]+:.*?## ' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-10s\033[0m %s\n", $$1, $$2}'

check: ## Type-check without producing a binary (fastest feedback)
	cargo check --all-targets

build: ## Compile a debug binary (target/debug/loch)
	cargo build

release: ## Compile an optimized binary (target/release/loch)
	cargo build --release

test: ## Run the full test suite (unit + integration)
	cargo test

fmt: ## Reformat all source files in place
	cargo fmt

fmt-check: ## Fail if any file is not rustfmt-clean (CI mode)
	cargo fmt --check

lint: ## Run clippy, treating warnings as errors
	cargo clippy --all-targets -- -D warnings

run: ## Run the debug binary; pass flags via ARGS, e.g. make run ARGS="-n 10 --per-language"
	cargo run -- $(ARGS)

install: ## Install loch into ~/.cargo/bin
	cargo install --path . --locked

doc: ## Build and open API docs for this crate only
	cargo doc --no-deps --open

clean: ## Delete the target/ directory
	cargo clean

perf: release ## Time a full-history run against tokei's repo (clones it on first use)
	@test -d $(PERF_REPO) || git clone --quiet https://github.com/XAMPPRocky/tokei $(PERF_REPO)
	/usr/bin/time -p ./target/release/loch $(PERF_REPO) -o /dev/null

plot: release ## Chart a repo's language history: make plot REPO=/path/to/repo
	./target/release/loch $(REPO) --per-language -o loch.csv
	./scripts/loch_plot.py loch.csv

ci: fmt-check lint test ## Everything a CI gate should run
