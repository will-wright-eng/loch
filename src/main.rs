mod output;
mod stats;
mod walk;

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Parser, ValueEnum};

#[derive(Parser)]
#[command(
    name = "loch",
    about = "Per-commit codebase statistics (LOC history) via gix + tokei, without materializing a working tree",
    version
)]
struct Args {
    /// Path to the git repository (or any directory inside it)
    #[arg(default_value = ".")]
    repo_path: PathBuf,

    /// Branch/ref to walk
    #[arg(short = 'r', long = "ref", default_value = "HEAD")]
    r#ref: String,

    /// Output format
    #[arg(short, long, value_enum, default_value_t = Format::Csv)]
    format: Format,

    /// Output path (stdout if omitted)
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Repo-root-anchored path prefixes to skip (e.g. vendor/ node_modules/)
    #[arg(short, long = "exclude")]
    exclude: Vec<String>,

    /// Sample every Nth commit, oldest first; the tip commit is always emitted
    #[arg(short = 'n', long = "every", default_value_t = 1, value_parser = clap::value_parser!(u64).range(1..))]
    every: u64,

    /// Emit per-language rows before each commit's TOTAL row
    #[arg(long)]
    per_language: bool,

    /// gix object decode cache size in MiB
    #[arg(long, default_value_t = 256)]
    object_cache_mb: usize,

    /// Disable tree/blob memoization (exists for cache-correctness testing)
    #[arg(long, hide = true)]
    no_cache: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Csv,
    Jsonl,
}

fn main() -> Result<()> {
    let args = Args::parse();
    // stats_tree recurses per directory level; a fat stack keeps pathologically
    // deep trees (hostile/hand-crafted repos) from overflowing the default stack.
    std::thread::Builder::new()
        .stack_size(256 * 1024 * 1024)
        .spawn(move || run(args))
        .context("failed to spawn worker thread")?
        .join()
        .map_err(|_| anyhow::anyhow!("worker thread panicked"))?
}

fn run(args: Args) -> Result<()> {
    let mut repo = gix::discover(&args.repo_path).with_context(|| {
        format!(
            "failed to open a git repository at '{}'",
            args.repo_path.display()
        )
    })?;
    repo.object_cache_size(Some(args.object_cache_mb.saturating_mul(1024 * 1024)));

    let commits = walk::first_parent_oldest_first(&repo, &args.r#ref)?;
    if commits.is_empty() {
        bail!("no commits reachable from '{}'", args.r#ref);
    }
    if repo.is_shallow() {
        eprintln!("warning: shallow clone — history is truncated at the shallow boundary");
    }

    let mut writer =
        output::Writer::new(matches!(args.format, Format::Jsonl), args.output.as_deref())?;
    let mut counter = stats::Counter::new(&repo, &args.exclude, !args.no_cache);

    let last = commits.len() - 1;
    for (i, id) in commits.iter().enumerate() {
        if i as u64 % args.every != 0 && i != last {
            continue;
        }
        let commit = repo
            .find_object(*id)
            .with_context(|| format!("failed to read commit {id}"))?
            .try_into_commit()
            .with_context(|| format!("object {id} is not a commit"))?;
        let seconds = commit.time()?.seconds;
        let tree_id = commit.tree_id()?.detach();
        let totals = counter.stats_tree(tree_id)?;
        writer.emit(seconds, id, &totals, args.per_language)?;
    }
    writer.finish()?;
    counter.report_skips();
    Ok(())
}
