# Implementation Plan: Closing the `loch` Design-Doc Gaps

**Status:** Implemented 2026-08-30 (see §10) · **Date:** 2026-08-30 · **Companion:** [design-doc.md](design-doc.md) · **Estimate:** ~1 day

---

## 1. Summary

An audit of the prototype against the design doc (2026-08-30) found every functional
requirement implemented: the §4 algorithm, the §5 CLI surface and semantics, and all ten
§8 edge-case rows. `cargo build`, `cargo clippy --all-targets -D warnings`,
`cargo fmt --check`, and all 12 tests pass.

What is missing is concentrated in **validation (§9 / M5)** — nothing in the repo
demonstrates the tokei cross-check or the perf bound, and there is no CI — plus one
real CLI trap, one conventional-CLI gap, and a handful of doc↔code drift items.

This plan closes those gaps in five sequential phases, each landing as one conventional
commit. Phases 0–1 are small fixes; Phases 2–3 are the actual M5 deliverables; Phases
4–5 harden tests and bring the design doc back in sync.

## 2. Baseline (verified 2026-08-30)

| Check | Result |
|---|---|
| `cargo build --release` / `clippy -D warnings` / `fmt --check` | clean |
| `cargo test` | 12/12 pass (1 unit, 11 integration in `tests/golden.rs`) |
| `commit.time()` is committer time (design §5) | confirmed in gix 0.85.0 source |
| `--object-cache-mb 0` disables the cache | confirmed (`Some(0)` → `unset_object_cache`) |
| Binary links only `libSystem` + `libiconv` (design §2 "single static binary") | confirmed via `otool -L` |
| Perf data point: `~/repos/feynman`, 4,920 first-parent commits, 75k packed objects | **4.6 s** cached · **116.7 s** `--no-cache` (25×) |
| tokei CLI installed on this machine | **no** — §9.2 cannot have been run here |
| CI configuration | **none** (`.github/` absent) |

## 3. Scope

**In scope**

- §9.2 cross-check against the tokei CLI, as a reusable script + Makefile target.
- §9.4 perf smoke with an asserted upper bound, run in CI.
- CI pipeline running `make ci`, an MSRV check, and the two validation jobs above.
- `-e/--exclude` parsing trap; quiet exit on broken pipe.
- Test coverage for the §8 rows that currently have none.
- Design-doc sync (drift, status, recorded validation results).
- Rename leftovers, `.gitignore`, `Cargo.toml` hygiene.

**Out of scope** (unchanged from design §3 / §11): persistent caches, parallel counting,
churn metrics, embedded-language redistribution, language categories. Also deferred, see §8:
gix feature trimming and a README.

## 4. Work Items

### Phase 0 — Housekeeping (~15 min)

| # | Item | Detail |
|---|---|---|
| 0.1 | Stage the doc move | `git status` shows `D design-doc.md` + `?? docs/`. Run `git add -A design-doc.md docs/` so the rename is recorded as a rename. |
| 0.2 | Ignore `make plot` outputs | Append `/loch.csv` and `/loch.png` to `.gitignore`. |
| 0.3 | Remove `repo-stats` leftovers | `src/stats.rs:256` sentinel → `/__loch_no_such_dir__`; `tests/golden.rs:242` and `:245` messages → `loch`. |
| 0.4 | `Cargo.toml` | Delete the empty `[dev-dependencies]` table. |

Commit: `chore: tidy leftovers from the repo-stats rename`

### Phase 1 — CLI correctness (~1 h)

**1.1 `-e` silently swallows a stray token as `REPO_PATH`.**
`loch -e src tests` treats `tests` as the repo path; `gix::discover` walks up to the
repo, only `src` is excluded, exit 0. Verified: 5 files vs. the correct 4. The help text
(`src/main.rs:35`, "e.g. vendor/ node_modules/") and the design doc's `<PREFIX>...`
both invite this.

Decision: keep one value per flag (the standard clap pattern; a comma delimiter would
make comma-containing paths unrepresentable) and fix the affordances:

```rust
/// Repo-root-anchored path prefix to skip; repeat the flag for each one
/// (e.g. -e vendor -e node_modules)
#[arg(short, long = "exclude", value_name = "PREFIX")]
exclude: Vec<String>,
```

Update design §5 to `-e, --exclude <PREFIX>` (drop the `...`).

**1.2 `value_name` parity with design §5.**
Add `value_name` so `--help` renders `<FMT>`, `<FILE>`, `<N>` for `--format`,
`--output`, `--every`/`--object-cache-mb`, matching the design doc.

**1.3 Quiet exit on broken pipe.**
`loch | head` currently exits 1 with `Error: Broken pipe (os error 32)`. Follow the
ripgrep convention: exit 0 silently. Two facts shape the implementation:

- `csv::Error` implements `std::error::Error` **without** `source()` (csv 1.4.0,
  `src/error.rs:127`), so the io error is *not* on `anyhow::Error::chain()`. Detection
  must downcast to `csv::Error` and match `kind()` on `csv::ErrorKind::Io`.
- The JSONL path surfaces a bare `std::io::Error`, which *is* on the chain.

```rust
fn is_broken_pipe(err: &anyhow::Error) -> bool {
    use std::io::ErrorKind::BrokenPipe;
    err.chain().any(|cause| {
        if let Some(io) = cause.downcast_ref::<std::io::Error>() {
            return io.kind() == BrokenPipe;
        }
        matches!(
            cause.downcast_ref::<csv::Error>().map(csv::Error::kind),
            Some(csv::ErrorKind::Io(io)) if io.kind() == BrokenPipe
        )
    })
}
```

`main` stops returning `Result` and matches: `Ok` → exit 0; broken pipe → exit 0
silently; anything else → `eprintln!("Error: {err:?}")` (anyhow's alternate format, same
text as today) and exit 1. The existing `empty_repo_exits_one_with_clean_message` test
guards the exit-1 path.

Deterministic test (no race with a `| head` reader): create a pipe, drop the read end
*before* spawning, hand the write end to the child as stdout. The first flush (after
commit 0) hits `EPIPE`.

```rust
#[test]
fn broken_pipe_exits_quietly() {
    let fx = build_fixture();
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let out = Command::new(env!("CARGO_BIN_EXE_loch"))
        .arg(&fx.root)
        .stdout(writer)
        .output()
        .unwrap();
    assert!(out.status.success(), "status: {}", out.status);
    assert!(!String::from_utf8_lossy(&out.stderr).contains("Broken pipe"));
}
```

**1.4 MSRV → 1.87.**
`std::io::pipe` is stable since Rust 1.87. Bump `rust-version = "1.87"` in `Cargo.toml`
and design §6. This is not a real regression: the Makefile header already states the
lockfile only builds on 1.87 (`home`/`time`/`human_format` pins), so the 1.85 claim was
never true in practice. Phase 3 adds an MSRV CI job so the number stays honest.
Alternative if 1.85 must hold: `os_pipe` as a dev-dependency.

Commit: `fix(cli): clarify repeated -e usage, name option values, exit quietly on broken pipe`

### Phase 2 — §9.2 tokei cross-check (~2 h)

**2.1 `scripts/cross_check.sh REPO [REF]`** — compares loch's `TOTAL` row for a commit
against `tokei --hidden --no-ignore` on a fresh checkout of that commit.

Design points, each traceable to design §9.2:

| Concern | Handling |
|---|---|
| Fresh checkout without mutating the source repo | `git clone -q --shared --no-checkout "$repo" "$scratch/co"` then `git -C "$scratch/co" checkout -q --detach "$sha"`. `--shared` reuses the source ODB (no object copy); works from a bare source too. `git worktree add` is rejected because it writes into the source `.git`; `git archive` is rejected because it honors `export-ignore`. |
| No `tokei.toml` / `.tokeirc` on tokei's lookup path | tokei 14.0.0 reads config from the XDG config dir, `$HOME`, and the **cwd** — never the target (`src/config.rs:94`). Run tokei with `HOME` and `XDG_CONFIG_HOME` pointed at an empty scratch dir, from that dir. A config file inside the checkout is harmless. |
| `.tokeignore` | Covered: `--no-ignore` implies `--no-ignore-dot`, which disables `.ignore` and `.tokeignore` (`src/cli.rs:127`). |
| Preconditions for zero tolerance | Scan the checkout (pruning `.git`) and warn for each: symlinks (`-type l`), `*.ipynb`, files `-size +10M`. If any hit, print the diff as informational and exit 0 with a `TOLERANCE OFF` banner instead of failing. |
| Invalid UTF-8 (±1 line per file, §8) | Detect with `grep -rlaxv '.*'` (needs GNU grep; on macOS use `ggrep` from `brew install grep`, else skip detection and warn). Allow `TOTAL` code/comments/blanks to differ by at most the number of such files. |
| tokei totals | Parse the ` Total` row of the default table: `awk '/^ Total/ {print $2","$4","$5","$6}'` → files, code, comments, blanks. This row is the CLI's own grand total (the one design §4.3 references). |
| loch totals | `loch "$repo" -r "$sha" -n 1000000000 \| tail -1 \| cut -d, -f4-`. Root (index 0) and tip are always emitted; the huge stride skips everything between, so this is O(tip tree) not O(history). |
| Per-language rows | Print loch `--per-language` rows for the tip next to tokei's table. Informational only (embedded-language folding differs by design). |
| Exit code | 0 on match (or tolerance-off), 1 on mismatch with both rows printed. |

**2.2 Makefile target**

```make
cross-check: release ## Compare the tip TOTAL row with the tokei CLI on a checkout: make cross-check REPO=... REF=...
	@command -v tokei >/dev/null || cargo install tokei --version 14.0.0 --locked
	PATH="$(CURDIR)/target/release:$$PATH" ./scripts/cross_check.sh $(REPO) $(REF)
```

with `REF ?= HEAD`. The tokei version is pinned to match `Cargo.toml` so the CLI and
library agree on `languages.json`.

**2.3 Run and record.** Execute against loch itself and against tokei's repository at
the SHA pinned in Phase 3. Record the result (SHA, `TOTAL` rows, match/mismatch, any
tolerance banner) in the design doc's §9 (Phase 5).

Commit: `test: add tokei cross-check script and Makefile target (design §9.2)`

### Phase 3 — §9.4 perf bound and CI (~1.5 h)

**3.1 Assert an upper bound in `make perf`.**

```make
PERF_REPO ?= /tmp/loch-perf/tokei
PERF_SHA ?= <pin on first run>
PERF_MAX_SECONDS ?= 60

perf: release ## Time a full-history run against tokei's repo and fail above PERF_MAX_SECONDS
	@test -d $(PERF_REPO) || git clone --quiet --no-checkout https://github.com/XAMPPRocky/tokei $(PERF_REPO)
	@git -C $(PERF_REPO) cat-file -e $(PERF_SHA)^{commit} 2>/dev/null || git -C $(PERF_REPO) fetch --quiet origin
	@start=$$(date +%s); ./target/release/loch $(PERF_REPO) -r $(PERF_SHA) -o /dev/null; \
	 elapsed=$$(( $$(date +%s) - start )); echo "perf: $${elapsed}s (max $(PERF_MAX_SECONDS)s)"; \
	 test $$elapsed -le $(PERF_MAX_SECONDS)
```

- `--no-checkout` + `-r $(PERF_SHA)`: loch never needs a working tree, and pinning the
  ref makes the number comparable across runs.
- `date +%s` rather than `/usr/bin/time`: portable across macOS and GitHub's Ubuntu
  images (GNU `time` is not preinstalled there). Whole-second resolution is fine for a bound.
- The design §7 target (50k commits / 1M blobs < 60 s) is too large to clone per CI run;
  this bound is a **regression guard** on a medium repo. The big-repo target stays a
  manual `make perf PERF_REPO=<path> PERF_SHA=<sha>` check; record one run in §9 results.

**3.2 `.github/workflows/ci.yml`** — three jobs:

```yaml
name: ci
on: [push, pull_request]
jobs:
  ci:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with: { components: rustfmt, clippy }
      - uses: Swatinem/rust-cache@v2
      - run: make ci
  msrv:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@master
        with: { toolchain: "1.87" }
      - run: cargo check --locked --all-targets
  validate:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - uses: actions/cache@v4
        with: { path: ~/.cargo/bin/tokei, key: tokei-14.0.0-${{ runner.os }} }
      - run: make perf
      - run: make cross-check REPO=$PERF_REPO REF=$PERF_SHA
      - run: make cross-check REPO=.
```

`make cross-check REPO=.` runs on the CI checkout (shallow, depth 1) — loch warns and
emits the single reachable commit, which is exactly the tip the script compares.

**3.3 Calibrate.** Measure `make perf` locally and once in CI; set `PERF_MAX_SECONDS`
to ~3× the CI figure (rounded up) to absorb runner variance. Record both numbers in
the Makefile comment and design §9.

Commit: `ci: add GitHub Actions pipeline with MSRV, perf bound, and tokei cross-check`

### Phase 4 — §8 edge-case test coverage (~2 h)

All in `tests/golden.rs`; the `git()` helper gains a variant that accepts an explicit
date string (needed by 4.2).

| # | §8 row | Test |
|---|---|---|
| 4.1 | Invalid UTF-8 text | Add `latin1.py` = `b"# caf\xe9\nx = 1\n"` to the fixture (commit c2). The model stores `String::from_utf8_lossy` of the bytes; both sides go through the same lossy conversion, so the existing exact-match assertions cover it. |
| 4.2 | Committer time outside RFC 3339 | New repo, one commit with `GIT_COMMITTER_DATE="@253402300800 +0000"` (one second past 9999-12-31T23:59:59Z). Assert the row's timestamp is `9999-12-31T23:59:59Z` and stderr contains `clamped`. If `git commit` rejects the date, fall back to `git commit-tree` plumbing. |
| 4.3 | Empty tree (§5.1 "still emits its TOTAL row with zeros") | New repo, `git commit --allow-empty`. Assert output is the header plus exactly one `TOTAL,0,0,0,0` row. |
| 4.4 | Pathologically deep trees | Build the commit **in-process with gix** (a regular dependency, so usable from `tests/`): one blob, then `N` nested single-entry trees via `repo.write_object`, then `repo.commit`. Filesystem construction is impossible (`PATH_MAX`) and `git mktree` per level is too slow. Assert success at `N` = 20,000. During implementation, temporarily drop the 256 MiB stack in `main.rs` and confirm the test *fails* — otherwise raise `N` until it does, so the test actually protects the guard. |
| 4.5 | Broken pipe | Delivered in Phase 1. |

Commit: `test: cover §8 edge cases (invalid UTF-8, timestamp clamp, empty tree, deep trees)`

### Phase 5 — Design-doc sync (~30 min)

| Section | Change |
|---|---|
| Header | `Status: Draft` → `Status: Implemented (prototype)`. |
| §4.2 "Skips are memoized too" | Describe the actual scheme: binary/huge classification is content-only, so it is cached on blob OID alone (`class_cache`), separate from the `(oid, lang)` parse cache. |
| §5 | `-e, --exclude <PREFIX>` (no `...`); note "repeat per prefix". |
| §6 | MSRV 1.87 (Phase 1.4). |
| §8 | Add row: "Broken pipe on stdout — exit 0 silently (ripgrep convention)". |
| §9 | Add a **Results** subsection: cross-check SHAs and outcomes (Phase 2.3), perf numbers local + CI with the chosen bound (Phase 3.3), the feynman data point from §2 above. |
| §10 | Mark M1–M5 done with dates. |

Commit: `docs: sync design doc with implementation and record validation results`

## 5. Sequencing and Dependencies

```
Phase 0 ──► Phase 1 ──► Phase 2 ──► Phase 3 ──► Phase 4 ──► Phase 5
 (tidy)     (CLI fixes)  (cross-    (CI + perf)  (§8 tests)  (doc sync)
                          check)
```

- Phase 1.4 (MSRV) must precede Phase 3's `msrv` job.
- Phase 2's script must exist before Phase 3's `validate` job.
- Phase 3.3's pinned `PERF_SHA` is what Phase 2.3 records against; pin it first.
- Phase 4 is independent of 2–3 and can be pulled forward if CI setup stalls.
- Phase 5 is last so it records real numbers, not projections.

## 6. Acceptance Criteria

- [ ] `make ci` clean locally and on GitHub Actions for `stable`; `cargo check --locked` clean on 1.87.
- [ ] `loch --help` shows `-e, --exclude <PREFIX>` with the "repeat the flag" wording; design §5 matches.
- [ ] `loch . | head -1` prints the header, exit status 0, nothing on stderr; `broken_pipe_exits_quietly` passes deterministically.
- [ ] `make cross-check REPO=.` and `make cross-check REPO=$PERF_REPO REF=$PERF_SHA` exit 0 with matching `TOTAL` rows (or a documented tolerance banner).
- [ ] `make perf` prints a time and fails when `PERF_MAX_SECONDS` is set below it (verify once with `PERF_MAX_SECONDS=0`).
- [ ] Four new tests from Phase 4 pass; the deep-tree test fails without the 256 MiB stack (checked once, manually).
- [ ] No `repo-stats`/`repo_stats` strings remain outside git history.
- [ ] Design doc §4.2, §5, §6, §8, §9, §10 updated; §9 contains recorded results with SHAs.

## 7. Decisions

| Decision | Recommendation | Alternative considered |
|---|---|---|
| Broken-pipe exit status | **0, silent** (ripgrep, fd) | 141 to mimic `SIGPIPE`; or restoring `SIGPIPE` default via `libc` — rejected: extra dependency for no user-visible gain |
| `-e` multi-value | **Help text + `value_name` only** | `value_delimiter = ','` — rejected: commas are legal in paths, and the repeat-flag form is already standard |
| Deterministic pipe in tests | **`std::io::pipe`, MSRV → 1.87** | `os_pipe` dev-dep keeps 1.85 — acceptable if 1.85 matters; it currently does not build anyway |
| Cross-check tokei totals source | **Parse the table's ` Total` row** | `--output json` and re-implement `summarise()` in the script — more code to keep in sync with tokei's nesting rules |
| CI perf bound | **3× first CI measurement**, pinned repo SHA | Bound from the §7 laptop target — rejected: runner speed differs and the §7 repo size is not CI-affordable |
| Fresh checkout mechanism | **`git clone --shared` + detached checkout** | `git worktree add` (mutates source `.git`), `git archive` (honors `export-ignore`) |

## 8. Deferred (not in this plan)

- **gix feature trimming.** Default features compile `blame`, `status`, `mailmap`,
  `negotiate` that loch never uses. Build-time only; the binary already links nothing
  beyond libSystem. Revisit if build time becomes a complaint — determine the minimal
  feature set empirically (`revision` is required for `rev_parse_single`).
- **README.md.** Not required by the design doc. Worth adding once the CLI surface is
  final (after Phase 1): usage, the pandas densify recipe from §5.1, and `scripts/loch_plot.py`.
- **`-e` runtime guard.** Detecting "second bare token was probably meant as an exclude"
  is guesswork; the help-text fix is the honest solution.

## 9. Risks

| Risk | Mitigation |
|---|---|
| tokei CLI table format changes | Version is pinned to `=14.0.0` in both `Cargo.toml` and the `cargo install`; the script asserts `tokei --version` before parsing. |
| CI perf bound flaps on slow runners | 3× headroom; the job prints the measured time so drift is visible before it fails. |
| `git commit` rejects the year-10000 date (4.2) | Fall back to `git commit-tree`, which does not validate the date range. |
| Deep-tree test slow from 20k loose objects | gix writes loose objects quickly (~1 s for 20k); if it becomes a problem, write into a packfile or lower `N` to the smallest value that still overflows the default stack. |
| `std::io::pipe` on Windows | Design §3 excludes Windows testing; `From<PipeWriter> for Stdio` is implemented on all platforms regardless. |

## 10. Implementation Notes (2026-08-30)

All five phases landed. Deviations from the plan above, with the reason each was made:

| Plan said | Implemented | Why |
|---|---|---|
| Fresh checkout via `git clone --shared` + detached checkout (2.1, §7) | `git read-tree` + `checkout-index` through a throwaway `GIT_INDEX_FILE` | git refuses to clone *from* a shallow repository, which is exactly what a default CI checkout is; the temp-index route also never touches the source index or worktree |
| Notebooks switch the cross-check to "tolerance off" (2.1) | Script subtracts tokei's Jupyter child rows from its `Total` and compares at zero tolerance | On tokei's own repo the whole delta (528/333/115) was exactly those rows, so the adjustment turns an informational run into a real assertion |
| Absolute bound only, whole-second `date +%s` timing (3.1) | `scripts/perf.sh`: best-of-3 cached run in ms (perl `Time::HiRes`) plus a `--no-cache` run; asserts `PERF_MAX_SECONDS` (20) **and** `PERF_MIN_SPEEDUP` (5×) | Measured 0.22 s cached vs 4 s uncached on the tokei repo — an absolute bound loose enough for CI could never notice a broken cache; the ratio can |
| `PERF_MAX_SECONDS` calibrated at 3× the first CI run (3.3) | Set to 20 s ahead of the first CI run | With the speedup floor doing the real work, the absolute bound only needs to catch gross regressions; revisit after the first CI run |
| Deep-tree depth "raise `N` until it fails" (4.4) | Kept 20,000; measured that a debug build without the worker thread overflows between 2,500 and 5,000 | Gives ~4–8× margin so the test stays meaningful for release-profile frames; costs ~8 s |
| Item 0.1 (`git add` the doc move) | Already committed as `f9c9f3f` before Phase 0 started | — |
| Workflow sketch in 3.2 (`@v4`/`@v2` tags, `dtolnay/rust-toolchain`, default permissions) | Hardened with zizmor 1.29: every action pinned to a commit SHA with a version comment, `permissions: {}` at the workflow level and `contents: read` per job, `persist-credentials: false` on checkout, a workflow `concurrency` group, and plain `rustup` steps instead of the toolchain action; `.github/dependabot.yml` keeps the SHA pins current. Clean at `--persona pedantic`, offline and online | zizmor flagged 21 findings on the sketch; SHA pins and least-privilege tokens are the standard mitigations for action supply-chain and token-exfiltration risk |

Recorded results live in design-doc §9.1. Still open after this pass: the §7 large-repo target (needs a 50k-commit repo), the CI-side perf calibration (needs one run on GitHub Actions), and the deferred items in §8.

## 11. References

- [design-doc.md](design-doc.md) — the requirements this plan implements
- [gix 0.85.0 — `Repository::object_cache_size`](https://docs.rs/gix/0.85.0/gix/struct.Repository.html#method.object_cache_size)
- [tokei 14.0.0 — `Config::from_config_files`](https://docs.rs/tokei/14.0.0/tokei/struct.Config.html#method.from_config_files) (lookup path: XDG config dir, home, cwd)
- [tokei CLI flags](https://github.com/XAMPPRocky/tokei#options) — `--hidden`, `--no-ignore`
- [csv 1.4 — `Error::kind`](https://docs.rs/csv/1.4.0/csv/struct.Error.html#method.kind) — no `source()` chain
- [`std::io::pipe`](https://doc.rust-lang.org/std/io/fn.pipe.html) — stable since Rust 1.87
- [ripgrep broken-pipe handling](https://github.com/BurntSushi/ripgrep/blob/master/crates/core/main.rs) — exit 0 on `BrokenPipe`
- [clap derive — `value_name`](https://docs.rs/clap/latest/clap/_derive/index.html#arg-attributes)
- [Swatinem/rust-cache](https://github.com/Swatinem/rust-cache), [actions/checkout](https://github.com/actions/checkout), [actions/cache](https://github.com/actions/cache) — pinned to commit SHAs
- [zizmor](https://docs.zizmor.sh/) — GitHub Actions security linter; audits fixed: `unpinned-uses`, `excessive-permissions`, `artipacked`, `concurrency-limits`, `superfluous-actions`
- [Conventional Commits 1.0.0](https://www.conventionalcommits.org/en/v1.0.0/)
