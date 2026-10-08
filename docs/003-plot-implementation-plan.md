# Implementation Plan: Native Charting with `--plot`

**Status:** Proposed · **Date:** 2026-10-07 · **Companion:** [design-doc.md](design-doc.md) §11 · **Estimate:** ~half a day

---

## 1. Summary

`scripts/loch_plot.py` renders a stacked-area chart of per-language history from `loch`
CSV output. It works, but it drags Python, `uv`, pandas, and matplotlib into a tool whose
design goal is a single static binary. This plan moves charting into `loch` itself behind
a `--plot <FILE>` flag, rendered with [`plotters`](https://docs.rs/plotters/0.3.7) to PNG or
SVG, and deletes the script.

The chart does not need to match the matplotlib output. It needs to be legible, build
without system libraries, and keep the binary linking only `libSystem` + `libiconv`
(design §2). Five phases, each landing as one conventional commit.

## 2. Baseline (verified 2026-10-07)

| Check | Result |
|---|---|
| Toolchain | cargo 1.99.0 locally; MSRV 1.87 enforced in CI |
| `plotters` latest | 0.3.7, MIT. Default features pull in `font-kit` (system fontconfig), `chrono`, GIF, and every series type |
| Font backends | `ttf` = font-kit (dynamic system libs); `ab_glyph` = pure Rust, fonts registered from `&'static [u8]` via `plotters::style::register_font` |
| Existing data flow | `run()` in `src/main.rs` streams one `LangTotals` per commit straight into `output::Writer`; nothing is retained across commits |
| Script features | summary table, `--metric`, `--top` with "Other" fold, `--pivot` wide CSV, `--no-plot`, stacked area over calendar time |
| Current `make plot` | release build → CSV → `scripts/loch_plot.py` |

## 3. Decisions

| Decision | Choice | Why |
|---|---|---|
| Crate | `plotters` 0.3.7, `default-features = false` | Mature, PNG + SVG, no runtime deps. Trimmed features avoid `font-kit` and `chrono` |
| Feature gate | Cargo feature `plot`, **on by default**, `cargo install` users get it; `--no-default-features` builds the slim tool | Chart is the headline feature of this change; opt-out beats opt-in for a CLI |
| Flag shape | `--plot <FILE>` is an *additional* sink. Row output still streams to stdout / `-o` | Lets `make plot` write CSV and PNG in one walk. `loch . --plot x.png > /dev/null` for chart-only |
| Format | By extension: `.svg` → `SVGBackend`, anything else → `BitMapBackend` PNG | No extra flag. SVG also makes integration tests inspectable |
| Data source | Always per-language from the in-memory `LangTotals`, independent of `--per-language` | The data is already there; no reason to tie chart content to row layout |
| Time axis | `i64` unix seconds on x, labels formatted with the `time` crate already in the tree | Avoids the `chrono` dependency. Sort stable by time so equal stamps keep commit order (same rule as the script) |
| Text | `ab_glyph` feature + one embedded OFL/Bitstream-licensed TTF registered at startup of `render()` | `font-kit` needs fontconfig on Linux and breaks the static-binary goal. Embedding costs ~0.7 MB |
| Dropped from script | Summary table, `--pivot`, `--no-plot` | Not charting. `--per-language` CSV already feeds any pivot; a summary belongs in a future `loch summary` if wanted |

### 3.1 CLI surface

```text
    --plot <FILE>           Render a stacked-area chart of per-language history (.svg, else PNG)
    --plot-metric <METRIC>  code | comments | blanks | files | lines [default: code]
    --plot-top <N>          Languages charted individually; the rest fold into "Other" [default: 8]
```

`--plot-metric` and `--plot-top` carry `requires = "plot"` so they error without `--plot`.
All three fields are `#[cfg(feature = "plot")]` so the slim build has no dead flags.

### 3.2 Chart spec

- Figure 1200×600 px (PNG at 1×; SVG uses the same logical size).
- Series order: value at the newest commit, descending. Top N kept; remainder summed into
  `Other`, drawn last in neutral grey.
- Stack by cumulative sum. Draw layers from the largest cumulative down with `AreaSeries`
  so each lower layer paints over the one above it.
- Fixed categorical palette of 8 colours plus grey for `Other`; no per-run colour shuffling.
- Title `"<metric> by language over <n> commits"`, y label = metric, x labels `YYYY-MM-DD`,
  legend upper-left.
- Degenerate inputs: one commit → widen x range by ±1 day; all-zero series → y max 1.

## 4. Work Items

### Phase 0 — Spike (~45 min, no commit unless it fails)

Validate the two things this plan assumes before touching real code.

| # | Item | Pass condition |
|---|---|---|
| 0.1 | Add `plotters` with the trimmed feature set in a scratch binary, render a two-layer `AreaSeries` stack to SVG and PNG | Both files open; PNG is non-trivial size |
| 0.2 | Confirm text behaviour under `ab_glyph` with and without `register_font` | Document which: panic, error, or silent no-text. Decide if `render()` must guard |
| 0.3 | `cargo check --locked` on rustc 1.87 with the new deps | Clean. If `image`/`ab_glyph` transitive pins break 1.87, pin with `cargo update <crate> --precise` per the Makefile note |
| 0.4 | `otool -L target/release/loch` | Still only `libSystem` + `libiconv` |
| 0.5 | Measure clean release build time and binary size before/after | Record in §7 |

Fallback if 0.2 shows `ab_glyph` is unworkable: SVG-only output via `SVGBackend` with no font
feature (SVG text is rendered by the viewer, not plotters).

### Phase 1 — Dependency and feature plumbing (~20 min)

| # | Item | Detail |
|---|---|---|
| 1.1 | `Cargo.toml` | `plotters = { version = "0.3.7", default-features = false, features = ["bitmap_backend", "bitmap_encoder", "svg_backend", "area_series", "ab_glyph"], optional = true }`; `[features] default = ["plot"]`, `plot = ["dep:plotters"]` |
| 1.2 | Lockfile | `cargo add` then inspect `git diff Cargo.lock`: only new crates may appear. The pinned `home`/`time`/`human_format` lines must not move |
| 1.3 | Font asset | `assets/fonts/<Font>.ttf` + its licence file. Candidates: DejaVu Sans (Bitstream Vera licence) or Inter (OFL). Both are GPL-compatible for bundling. Prefer a subset or the smallest weight to limit binary growth |
| 1.4 | `LICENSE` / `README.md` | One line noting the bundled font and its licence |

Commit: `build(plot): add plotters behind default-on plot feature`

### Phase 2 — `src/plot.rs` (~2 h)

| # | Item | Detail |
|---|---|---|
| 2.1 | `Metric` enum | `Code, Comments, Blanks, Files, Lines` with `clap::ValueEnum`; `fn pick(&self, c: &Counts) -> u64` where `Lines = code + comments + blanks` |
| 2.2 | `Collector` | `Vec<(i64, Rc<LangTotals>)>`. `push(seconds, totals)` called from `run()` right after `writer.emit`. `Rc` means retention is nearly free: cached subtrees are already shared |
| 2.3 | `shape(&[(i64, Rc<LangTotals>)], metric, top) -> Shaped` | Pure function. Produces `times: Vec<i64>` (stable-sorted by time), `series: Vec<(String, Vec<u64>)>` ordered by final value desc with `Other` folded, and cumulative stacks. Unit-testable without rendering |
| 2.4 | `render(shaped, path) -> Result<()>` | Dispatch on extension to `SVGBackend` / `BitMapBackend`. Call `register_font` once via `std::sync::Once` (or the `once_cell`-style `OnceLock`). Build chart per §3.2 |
| 2.5 | Error context | Wrap backend errors with the output path; plotters' `DrawingAreaErrorKind` does not name the file |
| 2.6 | Unit tests (in `plot.rs`) | top-N fold sums the tail into `Other`; ordering by final value; stable tie order for equal timestamps; `Lines` metric sums; empty-tree commit yields a zero column, not a missing one; single-commit x-range widening |

Commit: `feat(plot): add stacked-area chart module`

### Phase 3 — CLI wiring (~30 min)

| # | Item | Detail |
|---|---|---|
| 3.1 | `Args` | Add the three `#[cfg(feature = "plot")]` fields from §3.1 |
| 3.2 | `run()` | `let mut collector = args.plot.as_ref().map(\|_\| plot::Collector::default());` push per emitted commit; after `writer.finish()`, `collector.render(...)`. Chart renders after rows flush so an interrupted run still leaves CSV intact |
| 3.3 | Broken pipe | `is_broken_pipe` already covers CSV/JSONL. Rendering happens after the pipe is done, so no change expected; verify with `loch . --plot x.png \| head -1` |
| 3.4 | Stderr line | `chart written to <path>` to stderr, matching the warning channel. Stdout stays pure data |

Commit: `feat(cli): add --plot, --plot-metric, --plot-top`

### Phase 4 — Tests, Makefile, CI (~45 min)

| # | Item | Detail |
|---|---|---|
| 4.1 | `tests/golden.rs` | `plot_svg_lists_each_language`: run fixture with `--plot out.svg`, assert file starts with `<svg`, contains each fixture language name and the title text. `plot_top_folds_into_other`: `--plot-top 1` → SVG contains `Other`. `plot_png_has_magic_bytes`: first 8 bytes are the PNG signature and size > 1 KiB. `plot_flags_require_plot`: `--plot-top 3` alone exits non-zero |
| 4.2 | Gate tests | `#[cfg(feature = "plot")]` on the new tests so `--no-default-features` still passes `cargo test` |
| 4.3 | `Makefile` | `plot:` → `./target/release/loch $(REPO) --per-language -o loch.csv --plot loch.png`. Add `check-slim: cargo check --all-targets --no-default-features` and include it in `ci` |
| 4.4 | CI workflow | MSRV job: also run `cargo check --locked --all-targets --no-default-features`. Keep `make ci` as the stable gate |
| 4.5 | `.gitignore` | `/loch.png` already ignored; add `/loch.svg` |

Commit: `test(plot): cover chart output and slim build`

### Phase 5 — Remove the script and sync docs (~20 min)

| # | Item | Detail |
|---|---|---|
| 5.1 | Delete `scripts/loch_plot.py` | The `make plot` target no longer references it |
| 5.2 | `README.md` Usage | Add the three flags to the options block and one example: `loch ~/src/project --plot history.png` |
| 5.3 | `README.md` Install | Mention `--no-default-features` for the chart-free build |
| 5.4 | `docs/design-doc.md` §11 | Replace the "Plotting helper" bullet with a pointer to this plan and the shipped flag; add `--plot*` to the §5 CLI table |
| 5.5 | `docs/implementation-plan.md` line 322 | Note that the pandas recipe and script are superseded |
| 5.6 | This document | Set **Status:** Implemented, fill §7 |

Commit: `refactor(plot): drop Python plot script in favour of --plot`

## 5. Risks

| Risk | Mitigation |
|---|---|
| `ab_glyph` text behaviour without a registered font is undocumented | Phase 0.2 settles it; `render()` registers unconditionally so the question is moot in practice |
| Binary grows by the font plus `image` PNG encoder (~1–2 MB) | Acceptable for a CLI. The slim feature exists for anyone who cares |
| `cargo add` disturbs the 1.87 pins in `Cargo.lock` | Phase 1.2 diff review; fix with `--precise` |
| Retaining `Rc<LangTotals>` for every commit on a 100k-commit repo | Each entry is one `Rc` plus an `i64`; the maps are already alive in `tree_cache`. With `--no-cache` the maps are unique, roughly 1 KiB each, so 100 MB worst case. Document; do not optimise yet |
| Thousands of x points make `AreaSeries` polygons heavy in SVG | SVG is the test/debug format; PNG is the default. Note in README that `-n` sampling also thins the chart |

## 6. Out of Scope

Summary table and `--pivot` from the script, per-directory charts, interactive HTML output
(`charming`/plotly), Vega-Lite spec emission, and any colour-by-category logic. These stay in
design §11 if someone wants them.

## 7. Results

To be filled on completion: build-time delta, binary size delta, `otool -L` output, and the
`make plot` chart for this repository.

## References

- plotters 0.3.7 docs: https://docs.rs/plotters/0.3.7
- plotters `register_font` (feature `ab_glyph`): https://docs.rs/plotters/0.3.7/plotters/style/fn.register_font.html
- plotters `AreaSeries`: https://docs.rs/plotters/0.3.7/plotters/series/struct.AreaSeries.html
- plotters Cargo features (v0.3.7 manifest): https://github.com/plotters-rs/plotters/blob/v0.3.7/plotters/Cargo.toml
- DejaVu fonts licence: https://dejavu-fonts.github.io/License.html
- Inter font (OFL): https://github.com/rsms/inter
- Cargo features reference: https://doc.rust-lang.org/cargo/reference/features.html
