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
REF ?= HEAD
PERF_REPO ?= /tmp/loch-perf/tokei
# Pinned so perf timings and cross-check results stay comparable across runs.
PERF_SHA ?= fa44e5194060305576514d59b850353643afbfc8
# Regression guards on a medium repo, not the design §7 laptop target. The
# absolute bound catches gross slowdowns; the speedup floor catches a broken
# cache (locally ~0.2 s cached vs ~4 s --no-cache, i.e. ~18x). See design §9.
PERF_MAX_SECONDS ?= 20
PERF_MIN_SPEEDUP ?= 5

.PHONY: help check build release test fmt fmt-check lint run install doc clean perf cross-check validate plot ci

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

perf: release ## Time full-history runs of tokei's repo at PERF_SHA; fail above PERF_MAX_SECONDS or below PERF_MIN_SPEEDUP
	@test -d $(PERF_REPO) || git clone --quiet --no-checkout https://github.com/XAMPPRocky/tokei $(PERF_REPO)
	@git -C $(PERF_REPO) cat-file -e $(PERF_SHA)^{commit} 2>/dev/null || git -C $(PERF_REPO) fetch --quiet origin
	PATH="$(CURDIR)/target/release:$$PATH" ./scripts/perf.sh $(PERF_REPO) $(PERF_SHA) $(PERF_MAX_SECONDS) $(PERF_MIN_SPEEDUP)

cross-check: release ## Compare a commit's TOTAL row with the tokei CLI on a fresh checkout: make cross-check REPO=... REF=...
	@command -v tokei >/dev/null || cargo install tokei --version 14.0.0 --locked
	PATH="$(CURDIR)/target/release:$$PATH" ./scripts/cross_check.sh $(REPO) $(REF)

validate: perf ## Run the design §9 validation suite (perf bound + cross-checks); CI runs this
	$(MAKE) cross-check REPO=$(PERF_REPO) REF=$(PERF_SHA)
	$(MAKE) cross-check REPO=. REF=HEAD

plot: release ## Chart a repo's language history: make plot REPO=/path/to/repo
	./target/release/loch $(REPO) --per-language -o loch.csv
	./scripts/loch_plot.py loch.csv

ci: fmt-check lint test ## Everything a CI gate should run
