#!/usr/bin/env bash
# Perf smoke test (design doc §9.4): time a full first-parent walk of REPO at
# REF with and without memoization, then assert two bounds.
#
# Usage: scripts/perf.sh REPO REF MAX_SECONDS MIN_SPEEDUP
#   MAX_SECONDS  absolute ceiling for the cached run — catches gross regressions
#   MIN_SPEEDUP  cached must beat --no-cache by this factor — catches a broken
#                cache, which an absolute bound on a medium repo cannot see
# Requires loch on PATH and perl (for millisecond timing on macOS and Linux).
set -euo pipefail

repo=$1
ref=$2
max_seconds=$3
min_speedup=$4

now_ms() { perl -MTime::HiRes=time -e 'printf "%d", time * 1000'; }
time_run() {
    local start end
    start=$(now_ms)
    loch "$repo" -r "$ref" -o /dev/null "$@" 2>/dev/null
    end=$(now_ms)
    echo $(( end - start ))
}

commits=$(git -C "$repo" rev-list --count --first-parent "$ref")
# Best of three: the cached run is sub-second, so a single sample is at the
# mercy of runner noise, which would make the speedup floor flaky.
cached_ms=$(for _ in 1 2 3; do time_run; done | sort -n | head -1)
uncached_ms=$(time_run --no-cache)
speedup=$(( uncached_ms / (cached_ms > 0 ? cached_ms : 1) ))

echo "perf: $repo @ $ref ($commits first-parent commits)"
echo "  cached:   ${cached_ms} ms, best of 3 (max ${max_seconds} s)"
echo "  no-cache: ${uncached_ms} ms"
echo "  speedup:  ${speedup}x (min ${min_speedup}x)"

status=0
if (( cached_ms > max_seconds * 1000 )); then
    echo "  FAIL: cached run exceeded ${max_seconds} s" >&2
    status=1
fi
if (( speedup < min_speedup )); then
    echo "  FAIL: memoization speedup below ${min_speedup}x" >&2
    status=1
fi
(( status == 0 )) && echo "  result: PASS"
exit $status
