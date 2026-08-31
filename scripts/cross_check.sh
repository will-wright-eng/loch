#!/usr/bin/env bash
# Compare loch's TOTAL row for one commit against the tokei CLI run on a fresh
# checkout of that commit (design doc §9.2).
#
# Usage: scripts/cross_check.sh [REPO] [REF]
#   REPO  path to a git repository or any directory inside it (default: .)
#   REF   commit-ish to compare (default: HEAD)
# Requires git, perl, loch on PATH, and tokei 14.0.0 (the version pinned in
# Cargo.toml, so CLI and library share one languages.json).
#
# Exit status: 0 match (or tolerance off, see below), 1 mismatch, 2 usage error.
set -euo pipefail

repo=${1:-.}
ref=${2:-HEAD}

tokei_version=$(tokei --version 2>/dev/null || true)
if [[ "$tokei_version" != *"14.0.0"* ]]; then
    echo "error: need tokei 14.0.0 on PATH, found '${tokei_version:-nothing}'" >&2
    echo "hint: cargo install tokei --version 14.0.0 --locked" >&2
    exit 2
fi
command -v loch >/dev/null || { echo "error: loch not on PATH (run via 'make cross-check')" >&2; exit 2; }

git_dir=$(git -C "$repo" rev-parse --absolute-git-dir)
sha=$(git -C "$repo" rev-parse --verify "$ref^{commit}")

scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
co="$scratch/checkout"
mkdir -p "$co" "$scratch/home"

# Materialize the commit through a throwaway index: never touches the source
# index or worktree, and unlike `git clone` it works from a shallow source
# (CI checkouts) and unlike `git archive` it ignores export-ignore attributes.
GIT_INDEX_FILE="$scratch/index" git --git-dir="$git_dir" --work-tree="$co" read-tree "$sha"
GIT_INDEX_FILE="$scratch/index" git --git-dir="$git_dir" --work-tree="$co" checkout-index -a -f

# Preconditions for zero tolerance (design §8/§9.2). loch skips symlinks and
# >10 MiB blobs, so those differ by design; invalid UTF-8 may differ by ±1 line
# per file after lossy decoding. Notebooks are handled below.
symlinks=$(find "$co" -type l | wc -l | tr -d ' ')
notebooks=$(find "$co" -type f -name '*.ipynb' | wc -l | tr -d ' ')
huge=$(find "$co" -type f -size +10485760c | wc -l | tr -d ' ')
invalid_utf8=$(find "$co" -type f -print0 | perl -0 -ne '
    open my $fh, "<:raw", $_ or next;
    local $/; my $bytes = <$fh>; close $fh;
    next if index(substr($bytes, 0, 8192), "\0") >= 0;   # binary: skipped by loch anyway
    $n++ unless utf8::decode($bytes);
    END { print $n // 0 }')

# tokei reads tokei.toml/.tokeirc from the XDG config dir, $HOME and the cwd —
# never from the target — so point all three at an empty directory.
tokei_table=$(cd "$scratch/home" && HOME="$scratch/home" XDG_CONFIG_HOME="$scratch/home" \
    tokei --hidden --no-ignore "$co")
# tokei's Jupyter parser already folds cell stats into the notebook's own row,
# and its grand total adds the "|-" child rows on top — every notebook line is
# counted twice (design §4.3). loch reports true counts, so subtract the child
# rows from tokei's Total. Fields are taken from the right because language
# names may contain spaces.
tokei_row=$(awk '
    /^ Jupyter Notebooks/ { in_jupyter = 1; next }
    in_jupyter && /^ \|-/ { jc += $(NF-2); jm += $(NF-1); jb += $NF; next }
    in_jupyter            { in_jupyter = 0 }
    /^ Total/ { files = $(NF-4); code = $(NF-2); comments = $(NF-1); blanks = $NF; found = 1 }
    END { if (found) print files "," code - jc "," comments - jm "," blanks - jb "," jc "," jm "," jb }
' <<<"$tokei_table")
IFS=, read -r t_files t_code t_comments t_blanks jup_code jup_comments jup_blanks <<<"$tokei_row"
tokei_row="$t_files,$t_code,$t_comments,$t_blanks"
if [[ -z "$t_files" ]]; then
    echo "error: could not find the ' Total' row in tokei's output; table format changed?" >&2
    echo "$tokei_table" >&2
    exit 2
fi

# Root (index 0) and tip are always emitted; the huge stride skips everything
# between, so this costs one tree walk rather than the whole history.
loch_rows=$(loch "$repo" -r "$sha" -n 1000000000 --per-language 2>"$scratch/loch.err" | grep ",$sha,")
loch_row=$(grep ',TOTAL,' <<<"$loch_rows" | cut -d, -f4-)

echo "cross-check: $repo @ $sha"
echo "  preconditions: symlinks=$symlinks notebooks=$notebooks huge=$huge invalid_utf8=$invalid_utf8"
echo "  loch  TOTAL (files,code,comments,blanks): $loch_row"
echo "  tokei Total (files,code,comments,blanks): $tokei_row"
if (( notebooks > 0 )); then
    echo "  (tokei Total reduced by $jup_code,$jup_comments,$jup_blanks: Jupyter child rows double-count notebook lines, design §4.3)"
fi

verdict() {
    echo "  result: $1"
    echo
    echo "--- loch per-language rows (informational: embedded code is folded into the container language)"
    echo "$loch_rows" | cut -d, -f3-
    echo
    echo "--- tokei"
    echo "$tokei_table"
    if [[ -s "$scratch/loch.err" ]]; then
        echo
        echo "--- loch stderr"
        cat "$scratch/loch.err"
    fi
}

if (( symlinks + huge > 0 )); then
    verdict "TOLERANCE OFF (symlinks or >10 MiB files present; comparison is informational)"
    exit 0
fi

IFS=, read -r l_files l_code l_comments l_blanks <<<"$loch_row"
abs() { echo $(( $1 < 0 ? -$1 : $1 )); }
if (( l_files == t_files
      && $(abs $((l_code - t_code))) <= invalid_utf8
      && $(abs $((l_comments - t_comments))) <= invalid_utf8
      && $(abs $((l_blanks - t_blanks))) <= invalid_utf8 )); then
    if (( invalid_utf8 > 0 )); then
        verdict "MATCH within ±$invalid_utf8 lines (invalid UTF-8 files)"
    else
        verdict "MATCH"
    fi
    exit 0
fi

verdict "MISMATCH"
exit 1
