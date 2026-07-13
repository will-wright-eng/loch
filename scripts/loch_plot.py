#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.10"
# dependencies = ["pandas>=2", "matplotlib>=3.8"]
# ///
"""Summarize and chart per-language history produced by `loch --per-language`.

Reads the long-format CSV (timestamp,sha,language,files,code,comments,blanks),
prints a per-language summary as of the newest commit, and renders a
stacked-area chart of the chosen metric over time.

Usage:
    loch --per-language -o loch.csv && scripts/loch_plot.py loch.csv
    loch --per-language | scripts/loch_plot.py - --metric lines --top 10
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt
import pandas as pd

METRICS = ("code", "comments", "blanks", "files", "lines")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("csv", help="CSV from `loch --per-language` ('-' for stdin)")
    parser.add_argument("--metric", choices=METRICS, default="code")
    parser.add_argument(
        "--top",
        type=int,
        default=8,
        help="languages to chart individually; the rest fold into 'Other' (default 8)",
    )
    parser.add_argument("--out", help="chart path (default: <csv stem>.png)")
    parser.add_argument("--pivot", help="also write the wide commit x language table here")
    parser.add_argument("--no-plot", action="store_true", help="print the summary only")
    return parser.parse_args()


def load(source) -> pd.DataFrame:
    df = pd.read_csv(source, parse_dates=["timestamp"])
    expected = {"timestamp", "sha", "language", "files", "code", "comments", "blanks"}
    missing = expected - set(df.columns)
    if missing:
        sys.exit(f"error: input is missing columns {sorted(missing)}; expected loch CSV output")
    if df.empty:
        sys.exit("error: input contains no rows")
    df["lines"] = df["code"] + df["comments"] + df["blanks"]
    # loch emits commits oldest to newest; key on that order rather than the
    # timestamp, which can collide or run backwards under clock skew
    df["commit"] = pd.factorize(df["sha"])[0]
    return df


def widen(df: pd.DataFrame, metric: str) -> tuple[pd.DataFrame, pd.Series]:
    langs = df[df["language"] != "TOTAL"]
    if langs.empty:
        # totals-only input (loch without --per-language): chart TOTAL as one series
        langs = df.assign(language="TOTAL")
    wide = langs.pivot_table(
        index="commit", columns="language", values=metric, aggfunc="sum", fill_value=0
    )
    # commits where every language is absent (empty tree) still get a zero row
    wide = wide.reindex(range(df["commit"].max() + 1), fill_value=0)
    # column order: biggest at the newest commit first
    wide = wide[wide.iloc[-1].sort_values(ascending=False).index]
    times = df.drop_duplicates("commit").set_index("commit")["timestamp"].sort_index()
    return wide, times


def summarize(wide: pd.DataFrame, times: pd.Series, metric: str) -> str:
    final = wide.iloc[-1]
    total = final.sum()
    out = [
        f"{len(times)} commits, {times.iloc[0].date()} to {times.iloc[-1].date()}",
        f"{metric} at newest commit: {total:,}",
        "",
        f"{'language':<22}{metric:>10}   share  first seen",
    ]
    for lang, value in final.items():
        share = value / total if total else 0
        nonzero = wide[lang].to_numpy().nonzero()[0]
        first_seen = times.iloc[nonzero[0]].date() if len(nonzero) else "-"
        out.append(f"{lang:<22}{value:>10,}  {share:>6.1%}  {first_seen}")
    return "\n".join(out)


def plot(wide: pd.DataFrame, times: pd.Series, metric: str, top: int, out: Path) -> None:
    if len(wide.columns) > top:
        keep = list(wide.columns[:top])
        wide = wide[keep].assign(Other=wide.drop(columns=keep).sum(axis=1))
    # x axis is calendar time; stable sort keeps commit order for equal stamps
    order = times.sort_values(kind="stable").index
    times, wide = times.loc[order], wide.loc[order]

    fig, ax = plt.subplots(figsize=(12, 6))
    ax.stackplot(
        times.values,
        [wide[c].to_numpy() for c in wide.columns],
        labels=wide.columns,
        alpha=0.85,
    )
    ax.set_title(f"{metric} by language over {len(times)} commits")
    ax.set_ylabel(metric)
    ax.margins(x=0)
    ax.legend(loc="upper left", fontsize="small")
    fig.autofmt_xdate()
    fig.tight_layout()
    fig.savefig(out, dpi=150)
    print(f"\nchart written to {out}")


def main() -> None:
    args = parse_args()
    df = load(sys.stdin if args.csv == "-" else args.csv)
    wide, times = widen(df, args.metric)
    print(summarize(wide, times, args.metric))
    if args.pivot:
        wide.assign(timestamp=times).set_index("timestamp").to_csv(args.pivot)
        print(f"\npivot table written to {args.pivot}")
    if not args.no_plot:
        default = Path(args.csv).with_suffix(".png") if args.csv != "-" else Path("loch.png")
        plot(wide, times, args.metric, args.top, Path(args.out) if args.out else default)


if __name__ == "__main__":
    main()
