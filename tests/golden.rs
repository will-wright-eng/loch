use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use tokei::{Config, LanguageType};

fn git(dir: &Path, args: &[&str], day: u8) {
    let date = format!("2024-01-{day:02}T12:00:00+00:00");
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env("GIT_AUTHOR_DATE", &date)
        .env("GIT_COMMITTER_DATE", &date)
        .output()
        .expect("failed to run git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn git_stdout(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("failed to run git");
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn write_file(root: &Path, rel: &str, contents: &[u8], executable: bool) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, contents).unwrap();
    if executable {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
}

/// One first-parent commit and the countable file model at that point.
/// `files` maps path -> (content, detected language); None = skipped
/// (binary / huge / unknown extension), so it contributes nothing.
struct CommitRec {
    sha: String,
    timestamp: String,
    files: BTreeMap<String, (String, Option<LanguageType>)>,
}

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    commits: Vec<CommitRec>,
}

const MAIN_RS_V1: &str = "// entry\nfn main() {\n    println!(\"hi\");\n}\n";
const MAIN_RS_V2: &str =
    "// entry\nfn main() {\n    println!(\"hi\");\n}\n\nfn helper() -> u32 {\n    41\n}\n";
const CONFIG_JSON_V1: &str = "{\n  \"a\": 1\n}\n";
const CONFIG_JSON_V2: &str = "{\n  \"a\": 1,\n  \"b\": 2\n}\n";
const LIB_PY: &str = "# lib\n\ndef f():\n    return 1\n";
const BIN_RUN: &str = "#!/usr/bin/env python3\nprint('x')\n";
const FEATURE_RS: &str = "/// doc\npub fn feat() {}\n";
const TEMP_PY: &str = "print('temp')\n";
const TOP_PY: &str = "x = 1\n";
const GEN_PY: &str = "# generated\ny = 2\n";
const RUNME: &str = " #!/bin/sh\necho hi\n"; // leading whitespace before the shebang
const NOTEBOOK: &str = r##"{
 "cells": [
  {"cell_type": "code", "execution_count": null, "metadata": {}, "outputs": [], "source": ["x = 1\n", "y = 2\n", "z = 3\n"]},
  {"cell_type": "markdown", "metadata": {}, "source": ["# Title\n", "prose\n"]}
 ],
 "metadata": {"kernelspec": {"language": "python", "name": "python3"}, "language_info": {"name": "python", "file_extension": ".py"}},
 "nbformat": 4,
 "nbformat_minor": 5
}
"##;

fn build_fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let mut model: BTreeMap<String, (String, Option<LanguageType>)> = BTreeMap::new();
    let mut commits: Vec<CommitRec> = Vec::new();

    git(&root, &["init", "-q", "-b", "main"], 1);

    let record = |root: &Path, model: &BTreeMap<String, (String, Option<LanguageType>)>,
                      commits: &mut Vec<CommitRec>,
                      day: u8| {
        commits.push(CommitRec {
            sha: git_stdout(root, &["rev-parse", "HEAD"]),
            timestamp: format!("2024-01-{day:02}T12:00:00Z"),
            files: model.clone(),
        });
    };

    // c1: Rust + JSON
    write_file(&root, "src/main.rs", MAIN_RS_V1.as_bytes(), false);
    write_file(&root, "config.json", CONFIG_JSON_V1.as_bytes(), false);
    model.insert("src/main.rs".into(), (MAIN_RS_V1.into(), Some(LanguageType::Rust)));
    model.insert("config.json".into(), (CONFIG_JSON_V1.into(), Some(LanguageType::Json)));
    git(&root, &["add", "."], 1);
    git(&root, &["commit", "-q", "-m", "c1"], 1);
    record(&root, &model, &mut commits, 1);

    // c2: Python + modified Rust + a notebook (regression: no Jupyter double-count)
    write_file(&root, "src/lib.py", LIB_PY.as_bytes(), false);
    write_file(&root, "src/main.rs", MAIN_RS_V2.as_bytes(), false);
    write_file(&root, "nb.ipynb", NOTEBOOK.as_bytes(), false);
    model.insert("src/lib.py".into(), (LIB_PY.into(), Some(LanguageType::Python)));
    model.insert("src/main.rs".into(), (MAIN_RS_V2.into(), Some(LanguageType::Rust)));
    model.insert("nb.ipynb".into(), (NOTEBOOK.into(), Some(LanguageType::Jupyter)));
    git(&root, &["add", "."], 2);
    git(&root, &["commit", "-q", "-m", "c2"], 2);
    record(&root, &model, &mut commits, 2);

    // c3: executable shebang script, unknown extension, and two identical
    // directory trees (a/ and b/ share tree OIDs — the exclude/cache case)
    write_file(&root, "bin/run", BIN_RUN.as_bytes(), true);
    write_file(&root, "notes.xyzzy", b"who knows\n", false);
    // shebang content but unknown extension: tokei skips it, so must we
    write_file(&root, "deploy.xyzzy", b"#!/bin/sh\necho hi\n", false);
    // extensionless with whitespace-led shebang: tokei counts it, so must we
    write_file(&root, "runme", RUNME.as_bytes(), false);
    model.insert("deploy.xyzzy".into(), (String::new(), None));
    model.insert("runme".into(), (RUNME.into(), Some(LanguageType::Sh)));
    for d in ["a", "b"] {
        write_file(&root, &format!("{d}/top.py"), TOP_PY.as_bytes(), false);
        write_file(&root, &format!("{d}/sub/gen.py"), GEN_PY.as_bytes(), false);
        model.insert(format!("{d}/top.py"), (TOP_PY.into(), Some(LanguageType::Python)));
        model.insert(format!("{d}/sub/gen.py"), (GEN_PY.into(), Some(LanguageType::Python)));
    }
    model.insert("bin/run".into(), (BIN_RUN.into(), Some(LanguageType::Python)));
    model.insert("notes.xyzzy".into(), (String::new(), None));
    git(&root, &["add", "."], 3);
    git(&root, &["commit", "-q", "-m", "c3"], 3);
    record(&root, &model, &mut commits, 3);

    // c4: binary blob with a known extension, and a huge (>10 MB) text blob
    write_file(&root, "texture.c", b"\x89BIN\x00\x01\x02data", false);
    let big = "# x\n".repeat(3_000_000); // 12 MB
    write_file(&root, "big.py", big.as_bytes(), false);
    model.insert("texture.c".into(), (String::new(), None));
    model.insert("big.py".into(), (String::new(), None));
    git(&root, &["add", "."], 4);
    git(&root, &["commit", "-q", "-m", "c4"], 4);
    record(&root, &model, &mut commits, 4);

    // feature branch commit (day 5) — must NOT appear in the first-parent walk
    git(&root, &["checkout", "-q", "-b", "feature"], 5);
    write_file(&root, "feature.rs", FEATURE_RS.as_bytes(), false);
    git(&root, &["add", "."], 5);
    git(&root, &["commit", "-q", "-m", "feat"], 5);
    git(&root, &["checkout", "-q", "main"], 5);

    // c5 on main: modify JSON
    write_file(&root, "config.json", CONFIG_JSON_V2.as_bytes(), false);
    model.insert("config.json".into(), (CONFIG_JSON_V2.into(), Some(LanguageType::Json)));
    git(&root, &["add", "."], 6);
    git(&root, &["commit", "-q", "-m", "c5"], 6);
    record(&root, &model, &mut commits, 6);

    // c6: merge commit — feature.rs arrives on the first-parent line
    git(&root, &["merge", "-q", "--no-ff", "feature", "-m", "merge feature"], 7);
    model.insert("feature.rs".into(), (FEATURE_RS.into(), Some(LanguageType::Rust)));
    record(&root, &model, &mut commits, 7);

    // c7: add a file...
    write_file(&root, "temp.py", TEMP_PY.as_bytes(), false);
    model.insert("temp.py".into(), (TEMP_PY.into(), Some(LanguageType::Python)));
    git(&root, &["add", "."], 8);
    git(&root, &["commit", "-q", "-m", "c7"], 8);
    record(&root, &model, &mut commits, 8);

    // c8: ...and revert it
    git(&root, &["revert", "--no-edit", "HEAD"], 9);
    model.remove("temp.py");
    record(&root, &model, &mut commits, 9);

    Fixture { _dir: dir, root, commits }
}

fn run(repo: &Path, args: &[&str]) -> (String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_repo-stats"))
        .arg(repo)
        .args(args)
        .output()
        .expect("failed to run repo-stats");
    assert!(
        out.status.success(),
        "repo-stats {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    (
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn excluded(path: &str, excludes: &[&str]) -> bool {
    let comps: Vec<&str> = path.split('/').collect();
    excludes.iter().any(|pattern| {
        let pat: Vec<&str> = pattern.split('/').filter(|c| !c.is_empty()).collect();
        pat.len() <= comps.len() && pat.iter().zip(&comps).all(|(a, b)| a == b)
    })
}

/// Independent reimplementation of the aggregation, with tokei as the line-count oracle.
fn expected_csv(commits: &[CommitRec], excludes: &[&str], per_language: bool, every: usize) -> String {
    let config = Config::default();
    let mut out = String::from("timestamp,sha,language,files,code,comments,blanks\n");
    let last = commits.len() - 1;
    for (i, commit) in commits.iter().enumerate() {
        if i % every != 0 && i != last {
            continue;
        }
        let mut by_lang: BTreeMap<&'static str, (u64, u64, u64, u64)> = BTreeMap::new();
        for (path, (content, lang)) in &commit.files {
            let Some(lang) = lang else { continue };
            if excluded(path, excludes) {
                continue;
            }
            let parsed = lang.parse_from_str(content.as_str(), &config);
            let stats = if *lang == LanguageType::Jupyter { parsed } else { parsed.summarise() };
            let entry = by_lang.entry(lang.name()).or_default();
            entry.0 += 1;
            entry.1 += stats.code as u64;
            entry.2 += stats.comments as u64;
            entry.3 += stats.blanks as u64;
        }
        let mut total = (0u64, 0u64, 0u64, 0u64);
        for (_, (f, c, m, b)) in &by_lang {
            total.0 += f;
            total.1 += c;
            total.2 += m;
            total.3 += b;
        }
        if per_language {
            for (name, (f, c, m, b)) in &by_lang {
                out.push_str(&format!(
                    "{},{},{name},{f},{c},{m},{b}\n",
                    commit.timestamp, commit.sha
                ));
            }
        }
        out.push_str(&format!(
            "{},{},TOTAL,{},{},{},{}\n",
            commit.timestamp, commit.sha, total.0, total.1, total.2, total.3
        ));
    }
    out
}

#[test]
fn default_totals_match_model() {
    let fx = build_fixture();
    let (stdout, stderr) = run(&fx.root, &[]);
    assert_eq!(stdout, expected_csv(&fx.commits, &[], false, 1));
    assert!(
        stderr.contains("skipped 2 unrecognized, 1 binary, 1 huge"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn per_language_rows_match_model() {
    let fx = build_fixture();
    let (stdout, _) = run(&fx.root, &["--per-language"]);
    assert_eq!(stdout, expected_csv(&fx.commits, &[], true, 1));
}

#[test]
fn excludes_apply_and_cache_stays_correct() {
    let fx = build_fixture();
    // a/ and b/ have identical tree OIDs; only a/sub must be pruned.
    let args = ["--per-language", "-e", "a/sub", "-e", "notes.xyzzy"];
    let (cached, _) = run(&fx.root, &args);
    assert_eq!(cached, expected_csv(&fx.commits, &["a/sub", "notes.xyzzy"], true, 1));

    let mut no_cache_args = args.to_vec();
    no_cache_args.push("--no-cache");
    let (uncached, _) = run(&fx.root, &no_cache_args);
    assert_eq!(cached, uncached, "cache-on and cache-off output must be byte-identical");
}

#[test]
fn no_cache_is_byte_identical_without_excludes() {
    let fx = build_fixture();
    let (cached, _) = run(&fx.root, &["--per-language"]);
    let (uncached, _) = run(&fx.root, &["--per-language", "--no-cache"]);
    assert_eq!(cached, uncached);
}

#[test]
fn sampling_keeps_stride_and_tip() {
    let fx = build_fixture();
    let (stdout, _) = run(&fx.root, &["-n", "3"]);
    // 8 first-parent commits: indices 0, 3, 6 plus the tip (7).
    assert_eq!(stdout, expected_csv(&fx.commits, &[], false, 3));
    assert_eq!(stdout.lines().count(), 1 + 4);
}

#[test]
fn jsonl_mirrors_csv_fields() {
    let fx = build_fixture();
    let (jsonl, _) = run(&fx.root, &["--per-language", "-f", "jsonl"]);
    let (csv_out, _) = run(&fx.root, &["--per-language"]);

    let csv_rows: Vec<String> = csv_out.lines().skip(1).map(str::to_string).collect();
    let json_rows: Vec<String> = jsonl
        .lines()
        .map(|line| {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            format!(
                "{},{},{},{},{},{},{}",
                v["timestamp"].as_str().unwrap(),
                v["sha"].as_str().unwrap(),
                v["language"].as_str().unwrap(),
                v["files"],
                v["code"],
                v["comments"],
                v["blanks"]
            )
        })
        .collect();
    assert_eq!(csv_rows, json_rows);
}

#[test]
fn ref_flag_walks_annotated_and_lightweight_tags() {
    let fx = build_fixture();
    let target = fx.commits[3].sha.clone();
    git(&fx.root, &["tag", "-a", "v-mid", "-m", "midpoint", &target], 10);
    git(&fx.root, &["tag", "light", &target], 10);
    let expected = expected_csv(&fx.commits[..4], &[], false, 1);
    for tag in ["v-mid", "light"] {
        let (stdout, _) = run(&fx.root, &["-r", tag]);
        assert_eq!(stdout, expected, "walking -r {tag}");
    }
}

#[test]
fn empty_repo_exits_one_with_clean_message() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"], 1);
    let out = Command::new(env!("CARGO_BIN_EXE_repo-stats"))
        .arg(dir.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("empty repository or unborn ref"),
        "stderr: {stderr}"
    );
}

#[test]
fn shallow_clone_warns_and_stops_at_boundary() {
    let fx = build_fixture();
    let dir = tempfile::tempdir().unwrap();
    let shallow = dir.path().join("shallow");
    git(
        dir.path(),
        &[
            "clone",
            "-q",
            "--depth",
            "2",
            &format!("file://{}", fx.root.display()),
            shallow.to_str().unwrap(),
        ],
        10,
    );
    let (stdout, stderr) = run(&shallow, &[]);
    assert!(stderr.contains("shallow clone"), "stderr: {stderr}");
    let last_two = &fx.commits[fx.commits.len() - 2..];
    assert_eq!(stdout, expected_csv(last_two, &[], false, 1));
}

#[test]
fn jupyter_top_level_already_includes_cells() {
    let config = Config::default();
    let plain = LanguageType::Jupyter.parse_from_str(NOTEBOOK, &config);
    assert!(plain.code > 0, "fixture notebook must be parseable by tokei");
    let folded = LanguageType::Jupyter.parse_from_str(NOTEBOOK, &config).summarise();
    // if this ever fails, tokei changed parse_jupyter and stats.rs's special case
    // (and this model's) should be re-examined
    assert_eq!(folded.code, 2 * plain.code);
}

#[test]
fn output_file_is_truncated_not_appended() {
    let fx = build_fixture();
    let out_path = fx.root.join("stats.csv");
    std::fs::write(&out_path, "stale content\nstale content\nstale content\n").unwrap();
    run(&fx.root, &["-o", out_path.to_str().unwrap()]);
    let written = std::fs::read_to_string(&out_path).unwrap();
    assert_eq!(written, expected_csv(&fx.commits, &[], false, 1));
}
