# Design Doc: `repo-stats` — Per-Commit Codebase Statistics via gix + tokei

**Status:** Draft · **Scope:** Prototype · **Language:** Rust (2021 edition)

---

## 1. Summary

`repo-stats` is a small CLI tool that walks the first-parent history of a git branch and emits per-commit codebase statistics (files, lines of code, comments, blanks — broken down by language) without ever materializing a working tree. It reads blobs directly from the object database via [gix](https://github.com/GitoxideLabs/gitoxide) and counts them in memory with [tokei](https://github.com/XAMPPRocky/tokei) used as a library.

The key idea that makes full-history analysis fast is **memoizing statistics on git object IDs**. Because git storage is content-addressed, any directory (tree) or file (blob) that is unchanged between two commits has the same OID. Stats are computed recursively per tree and cached, so each commit after the first only pays for the paths that actually changed.

Expected cost: O(unique blobs in history), not O(commits × files). On a typical mid-size repo (tens of thousands of commits), a full run should complete in seconds to low minutes.

## 2. Goals

- Emit one row per commit on `main` (or a user-specified ref): timestamp, SHA, and per-language totals for code / comments / blanks / file count.
- Never touch the working tree; safe to run inside a dirty checkout or against a bare repo.
- Correct, independent computation per commit (no delta arithmetic that can drift).
- Single static binary; output as CSV (default) or JSON Lines.
- Fast enough that sampling is unnecessary for repos up to ~100k commits.

## 3. Non-Goals (prototype)

- Churn/authorship/ownership metrics (use `git log --numstat` or hercules).
- Historical `.gitignore` evaluation — files committed to the tree are counted; a static path-prefix exclude list is provided instead.
- Rename tracking (irrelevant under the memoization model).
- Complexity metrics (scc-style cyclomatic estimates); tokei does not compute these.
- Incremental persistence of the cache across runs (see Future Work).
- Windows-specific testing (should work, untested).

## 4. Architecture

```
┌────────────┐   rev walk    ┌──────────────┐   tree OID    ┌─────────────┐
│  gix repo  │ ────────────► │ commit loop  │ ────────────► │ stats_tree  │
└────────────┘  (1st parent, └──────────────┘               │ (recursive, │
                 oldest→new)        │                       │  memoized)  │
                                    │ emit row              └──────┬──────┘
                                    ▼                              │ blob OID miss
                              ┌──────────┐                  ┌──────▼──────┐
                              │ writer   │                  │ stats_blob  │
                              │ CSV/JSONL│                  │ tokei parse │
                              └──────────┘                  │ (memoized)  │
                                                            └─────────────┘
```

### 4.1 Components

| Component | Responsibility |
|---|---|
| `cli` | Argument parsing (clap): repo path, ref, output format, excludes, sampling interval. |
| `walk` | Resolve ref → first-parent, oldest-first commit list via `gix::Repository::rev_walk` with `first_parent_only()`. "Oldest-first" means the reversed tip→root first-parent chain (`git log --first-parent --reverse`); commit timestamps are ignored for ordering. gix offers no reverse sort, so the walk collects OIDs and iterates them reversed. |
| `stats_tree` | Recursive tree traversal with a `HashMap<ObjectId, LangTotals>` cache. |
| `stats_blob` | Language detection + counting via tokei, cached on `(ObjectId, LanguageType)`. |
| `output` | Streaming writer. Default: exactly one `TOTAL` pseudo-language row per commit. With `--per-language`: one row per detected language, followed by the `TOTAL` row. Schema is identical in both modes. |

### 4.2 Core algorithm

```
stats(tree_oid):
    if tree_oid in tree_cache: return cached
    total = {}
    for entry in read_tree(tree_oid):
        match entry.mode:
            Tree          -> total += stats(entry.oid)          # recurse
            Blob | BlobExe -> total += blob_stats(entry)
            Link | Commit -> skip                               # symlinks, submodules
    tree_cache[tree_oid] = total
    return total

blob_stats(entry):
    lang = tokei::LanguageType::from_path(entry.name)?          # else skip
    key = (entry.oid, lang)
    if key in blob_cache: return cached
    bytes = odb.find_blob(entry.oid)
    if looks_binary(bytes): skip                                # NUL-byte heuristic
    stats = lang.parse_from_str(lossy_utf8(bytes))
    blob_cache[key] = stats
    return stats

main:
    for commit in first_parent_oldest_first(ref):
        emit(commit.time, commit.id, stats(commit.root_tree))
```

Correctness note: every commit's totals are computed from its full tree, so merges, reverts, and history rewrites need no special handling — the cache only affects speed, never results.

**Excludes vs. the tree cache.** `--exclude` makes stats path-dependent, and the same tree OID can legitimately appear at both an excluded and a non-excluded path — so a naive OID-keyed cache would silently return filtered totals at the wrong path. Entries are pruned during traversal by repo-root-relative path, and the tree cache is bypassed (neither read nor written) for any tree whose path is a proper prefix of an exclude pattern; every other subtree keeps the plain OID key. With no excludes this degenerates to the pseudocode above, and cache-on vs. cache-off output stays byte-identical (test 9.3) in all cases.

**Skips are memoized too.** Binary and huge blobs are cached as zero-stat entries under the same `(oid, lang)` key, preserving the O(unique blobs) cost claim even when a large binary sits next to hot source files. The > 10 MB guard reads the object header (size only) before fetching bytes. The stderr summary reports *unique* skipped blobs by reason (unknown extension / binary / huge) — per-commit skip counts would be cache-dependent and meaningless.

### 4.3 Language detection nuance

Tokei detects language from the *path* (extension or well-known filename), but the cache key must include the detected language, not just the blob OID: identical bytes reachable as both `config.h` and `config.py` would count comments differently. Keying on `(blob_oid, LanguageType)` costs nothing and removes the collision.

Path context also means tree traversal must carry the entry name down to blob counting — the OID alone is insufficient.

Extensionless files (e.g. `bin/setup`) are detected by matching the blob's first line against tokei's shebang table, mirroring the tokei CLI so the §9 cross-check holds. `LanguageType::from_path` must never be handed a bare entry name for this — its shebang fallback opens the path on the local filesystem. Detection therefore remains a function of entry filename + blob bytes only, which keeps both the tree-OID and `(blob_oid, lang)` cache keys sound.

`parse_from_str` returns `CodeStats` with nested per-language stats for embedded code (JS/CSS inside HTML, fenced blocks in Markdown). These are folded into the path-detected container language via `CodeStats::summarise()`; redistribution to child languages is future work. Exception: tokei's Jupyter parser already includes cell stats in its top-level counts, so `.ipynb` results are taken as-is — folding again would double-count every notebook line (the tokei CLI's own grand total exhibits exactly that doubling; repo-stats deliberately reports true line counts instead).

## 5. CLI Interface

```
repo-stats [OPTIONS] [REPO_PATH]

OPTIONS:
  -r, --ref <REF>            Branch/ref to walk [default: HEAD]
  -f, --format <FMT>         csv | jsonl [default: csv]
  -o, --output <FILE>        Output path [default: stdout]
  -e, --exclude <PREFIX>...  Path prefixes to skip (e.g. vendor/ node_modules/)
  -n, --every <N>            Sample every Nth commit [default: 1]
      --per-language         Emit per-language rows (default: totals only)
      --object-cache-mb <N>  gix object decode cache size [default: 256]
      --no-cache             Disable tree/blob memoization (hidden debug flag; exists for test 9.3)
```

Semantics:

- `timestamp` is **committer time** (matches "state of main over time" and is near-monotonic under first-parent), normalized to UTC, RFC 3339 (`2024-03-01T12:34:56Z`). Not strictly monotonic — consumers should sort by it.
- `sha` is the **full 40-hex object id** (the abbreviated form in the §5.1 example is illustrative shorthand).
- `--exclude` prefixes are repo-root-anchored, case-sensitive, whole-path-component matches, applying to files and directories alike: `vendor` ≡ `vendor/`, both exclude `vendor/**`, neither matches `vendored/`.
- `--every N` keeps commits with `index % N == 0` on the oldest-first walk (root is always index 0, so sample points stay stable as history grows) and **always emits the tip commit**, even off-stride.
- `--output` creates or truncates the file; the writer flushes after each commit so an interrupted run leaves a valid, parseable prefix.

### 5.1 Output schema (CSV)

```
timestamp,sha,language,files,code,comments,blanks
2024-03-01T12:34:56Z,a1b2c3d,Rust,412,58210,4102,7315
2024-03-01T12:34:56Z,a1b2c3d,TOTAL,633,71455,6980,9922
```

The example above shows `--per-language` output. The 7-column schema is identical in all modes: by default each commit emits exactly one `TOTAL` row; with `--per-language`, one row per detected language precedes it (languages sorted by name for deterministic output). A language appears only in commits where it has files — absence means zero; consumers densify with e.g. pandas `pivot_table(index='timestamp', columns='language', values='code', fill_value=0)`. A commit with nothing countable (empty tree, all-binary, fully excluded) still emits its `TOTAL` row with zeros, so every walked commit is visible.

JSONL mirrors the same fields, one object per row. Rows stream as they are computed so partial output survives interruption.

## 6. Key Dependencies

| Crate | Purpose | Notes |
|---|---|---|
| `gix` | Repo open, rev walk, tree/blob access | API churns between versions — pinned to **`=0.85.0`** (MSRV 1.85). Verified against docs.rs: `rev_walk(..).first_parent_only()`, `Repository::object_cache_size()`, ODB blob access all present; no reverse/oldest-first sort mode (collect + reverse instead). |
| `tokei` | Language detection + line counting | Pinned to **`=14.0.0`** (stable; 13.0.0 ended the multi-year alpha series in Nov 2025). `LanguageType::from_path` and `parse_from_str` both take a `&tokei::Config` — one shared `Config::default()` is used everywhere; `tokei.toml`/`.tokeirc` are never read. |
| `clap` (derive) | CLI | |
| `serde` / `csv` / `serde_json` | Output | |
| `anyhow` | Error plumbing | Prototype-grade error handling. |

Fallback plan: if the pinned `gix` API proves painful, `git2` (libgit2) supports every operation used here with a stabler API at some cost in raw object-access speed — acceptable for a prototype since blob reads are cached anyway.

## 7. Performance Considerations

- **Cold start dominates.** The first commit's full-tree pass decodes many packed blobs (long delta chains for old objects). Setting gix's object cache to a few hundred MB substantially reduces redundant delta resolution.
- **Steady state is cheap.** Each later commit costs roughly (changed files) blob parses + (changed directories) tree reads.
- **Memory.** Caches store fixed-size counters, never file contents. Estimate ~100 bytes/entry ⇒ 1M unique blobs ≈ 100 MB. Acceptable for the prototype; an LRU bound is a follow-up if needed.
- **Parallelism (stretch).** Serial is expected to be sufficient. If not: collect cache-miss blob entries per tree and count with `rayon::par_iter`, caches behind `dashmap`. Do this only after profiling the serial version.

Prototype performance target: full history of a 50k-commit, 1M-unique-blob repo in < 60 s on a laptop.

## 8. Edge Cases

| Case | Handling |
|---|---|
| Binary blobs | Skip if a NUL byte appears in the first 8 KiB. |
| Invalid UTF-8 text | `String::from_utf8_lossy`; counts may differ ±1 line from disk-based tools — acceptable. |
| Symlinks / submodules | Skipped (tree entry mode `Link` / `Commit`). |
| Unknown extensions | Skipped, matching scc/tokei default behavior; count of skipped files reported to stderr at end. |
| Empty repo / unborn ref | Clean error message, exit 1. |
| Octopus merges / roots | First-parent walk handles both; each commit computed independently. |
| Huge blobs (> 10 MB) | Skip and warn — likely generated/vendored; avoids pathological parse times. |
| Shallow clones | Walk stops at the shallow boundary; warn that history is truncated. |
| Committer time outside RFC 3339 (year > 9999 or < 0) | Clamp to the 0000/9999 bounds, warn on stderr, keep emitting. |
| Pathologically deep trees (hand-crafted repos) | Traversal runs on a 256 MiB stack so depth is bounded by repo size, not the 8 MiB default. |

## 9. Testing Plan

1. **Golden test:** construct a tiny fixture repo in a tempdir (via `gix` or shelling out to `git`) with known file contents across ~10 commits including a merge and a revert; assert exact CSV output.
2. **Cross-check:** on a real repo, compare the final commit's `TOTAL` row against `tokei --hidden --no-ignore` run on a fresh checkout of that SHA, with no `tokei.toml`/`.tokeirc` on tokei's config lookup path. The flags matter: stock tokei skips hidden files/dirs (`.github/` alone breaks file counts) and honors ignore files, while repo-stats counts everything committed. Zero tolerance applies to the `TOTAL` row on a checkout free of symlinked sources, > 10 MB text files, invalid UTF-8 (the lossy-UTF-8 case allows ±1 line, per §8), and `.ipynb` notebooks (repo-stats corrects a double-count present in tokei's own notebook totals, §4.3, so those legitimately differ). Per-language rows are informational only — tokei's CLI reports embedded code nested under child languages while repo-stats folds it into the container language (§4.3).
3. **Cache-correctness:** run with `--no-cache` (hidden flag; disables both tree and blob caches, reproducing the M2 baseline) and diff outputs — must be byte-identical, including with `--exclude` patterns of depth > 1.
4. **Smoke perf:** time a full run on a medium public repo (e.g. tokei's own repo) in CI; assert an upper bound.

## 10. Milestones

1. **M1 — Walking skeleton (½ day):** open repo, first-parent walk, print `timestamp,sha` per commit.
2. **M2 — Counting (1 day):** tree recursion + blob counting, no caching; correct output on the fixture repo.
3. **M3 — Memoization (½ day):** add tree/blob caches; verify identical output; measure speedup.
4. **M4 — Polish (½ day):** excludes, sampling, JSONL, binary/huge-blob guards, stderr summary.
5. **M5 — Validation (½ day):** cross-check vs tokei-on-checkout; perf smoke test.

Roughly a 3-day prototype for someone comfortable in Rust, with M1–M3 delivering the interesting result.

## 11. Future Work

- Persist caches (sled/rocksdb or a flat file keyed by OID) for fast incremental re-runs as the repo grows.
- Parallel blob counting (rayon + dashmap) if profiling justifies it.
- Optional scc-style complexity estimates (would require vendoring scc's processor or reimplementing its heuristics).
- Churn metrics from tree diffs (added/removed lines per commit) as a second output table.
- Language-category tagging for `TOTAL` filtering (hand-maintained language→category map, or a `--no-data` deny-list), since tokei exposes no category API.
- Redistribute embedded-language stats (JS in HTML, Markdown fences) to child-language rows instead of folding into the container.
- `--all-parents` mode and per-directory rollups (stats per top-level directory over time).
- Plotting helper (`repo-stats plot stats.csv`) or a documented pandas/vega recipe.

## 12. Open Questions

Both resolved 2026-07-12:

- **Should `TOTAL` include data/markup languages (JSON, YAML, Markdown)?** Resolved: include everything tokei detects; the 7-column §5.1 schema ships unchanged. The original proposal — tag rows with tokei's language category — is unimplementable: tokei 14.0.0 exposes no category API (`LanguageType` has no such method and `languages.json` no such field; that taxonomy is GitHub linguist's). Consumers filter `--per-language` rows by name; a hand-maintained category map or `--no-data` deny-list moves to Future Work.
- **Is first-parent the right default?** Resolved: first-parent stays the only mode for the prototype — it matches "state of main over time," the stated use case.
