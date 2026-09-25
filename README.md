# loch

`loch` (**LOC** + **h**istory) walks the first-parent history of a git branch and
emits per-commit codebase statistics: files, lines of code, comments, and blanks,
broken down by language. It reads blobs straight from the object database with
[gix](https://github.com/GitoxideLabs/gitoxide) and counts them in memory with
[tokei](https://github.com/XAMPPRocky/tokei), so it never touches the working tree.
It is safe to run in a dirty checkout or against a bare repo.

Stats are memoized on tree and blob object IDs, so each commit after the first only
pays for the paths that changed. A full-history run costs O(unique blobs), not
O(commits × files).

## Install

Homebrew:

```bash
brew install will-wright-eng/tools/loch
```

From source (Rust 1.87+):

```bash
cargo install --git https://github.com/will-wright-eng/loch --locked
```

## Usage

```text
loch [OPTIONS] [REPO_PATH]

  -r, --ref <REF>            Branch/ref to walk [default: HEAD]
  -f, --format <FMT>         csv | jsonl [default: csv]
  -o, --output <FILE>        Output path [default: stdout]
  -e, --exclude <PREFIX>     Repo-root-anchored path prefix to skip; repeat per prefix
  -n, --every <N>            Sample every Nth commit; the tip is always emitted [default: 1]
      --per-language         Emit per-language rows before each commit's TOTAL row
      --object-cache-mb <N>  gix object decode cache size in MiB [default: 256]
```

```bash
loch ~/src/project --per-language -e vendor -o history.csv
```

Output has the same seven columns in every mode. By default each commit emits one
`TOTAL` row; `--per-language` adds one row per detected language before it:

```text
timestamp,sha,language,files,code,comments,blanks
2026-08-31T01:36:45Z,cfa30b0fd91ca0bc4496da0b08ed9a5219f6ee93,Rust,5,1093,69,99
2026-08-31T01:36:45Z,cfa30b0fd91ca0bc4496da0b08ed9a5219f6ee93,TOTAL,14,1493,537,297
```

`timestamp` is committer time in UTC (RFC 3339) and `sha` is the full object ID. JSON
Lines output has the same fields. Rows are flushed per commit, so an interrupted run
leaves a valid prefix.

## Development

`make` lists the targets. `make ci` runs formatting, clippy, and tests;
`make validate` runs the performance guards and the tokei cross-check.

`Cargo.lock` pins some transitive dependencies for rustc 1.87 compatibility. Don't run
a bare `cargo update`; see the note at the top of the [Makefile](Makefile).

Design and rationale: [docs/design-doc.md](docs/design-doc.md).

## License

Copyright (C) 2026 Will Wright

Licensed under the GNU General Public License, version 3 or (at your option) any
later version. See [LICENSE](LICENSE).

## References

- [gix](https://github.com/GitoxideLabs/gitoxide)
- [tokei](https://github.com/XAMPPRocky/tokei)
- [will-wright-eng/homebrew-tools](https://github.com/will-wright-eng/homebrew-tools)
