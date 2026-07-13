use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use anyhow::{Context, Result};
use gix::ObjectId;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::stats::{Counts, LangTotals};

pub struct Writer {
    out: Out,
}

enum Out {
    Csv(Box<csv::Writer<Box<dyn Write>>>),
    Jsonl(Box<dyn Write>),
}

impl Writer {
    pub fn new(jsonl: bool, path: Option<&Path>) -> Result<Self> {
        let sink: Box<dyn Write> = match path {
            Some(path) => Box::new(BufWriter::new(File::create(path).with_context(|| {
                format!("failed to create output file '{}'", path.display())
            })?)),
            None => Box::new(std::io::stdout()),
        };
        let out = if jsonl {
            Out::Jsonl(sink)
        } else {
            let mut writer = csv::Writer::from_writer(sink);
            writer.write_record([
                "timestamp",
                "sha",
                "language",
                "files",
                "code",
                "comments",
                "blanks",
            ])?;
            Out::Csv(Box::new(writer))
        };
        Ok(Self { out })
    }

    /// One TOTAL row per commit; with `per_language`, name-sorted language rows precede it.
    /// Flushes after every commit so an interrupted run leaves a valid, parseable prefix.
    pub fn emit(
        &mut self,
        committer_seconds: i64,
        sha: &ObjectId,
        totals: &LangTotals,
        per_language: bool,
    ) -> Result<()> {
        // RFC 3339 can only express years 0000–9999; git accepts timestamps beyond
        // that, so clamp rather than abort the run mid-output.
        const MIN_RFC3339_SECONDS: i64 = -62_167_219_200; // 0000-01-01T00:00:00Z
        const MAX_RFC3339_SECONDS: i64 = 253_402_300_799; // 9999-12-31T23:59:59Z
        let clamped = committer_seconds.clamp(MIN_RFC3339_SECONDS, MAX_RFC3339_SECONDS);
        if clamped != committer_seconds {
            eprintln!(
                "warning: commit {sha} committer timestamp ({committer_seconds}s) is outside the RFC 3339 range; clamped"
            );
        }
        let timestamp = OffsetDateTime::from_unix_timestamp(clamped)
            .context("commit timestamp out of range")?
            .format(&Rfc3339)?;
        let sha = sha.to_string();

        let mut grand_total = Counts::default();
        let mut rows: Vec<(&str, &Counts)> = Vec::with_capacity(totals.len());
        for (lang, counts) in totals {
            grand_total.add(counts);
            rows.push((lang.name(), counts));
        }
        rows.sort_by_key(|(name, _)| *name);

        if per_language {
            for (name, counts) in rows {
                self.write_row(&timestamp, &sha, name, counts)?;
            }
        }
        self.write_row(&timestamp, &sha, "TOTAL", &grand_total)?;
        self.flush()
    }

    fn write_row(&mut self, timestamp: &str, sha: &str, language: &str, c: &Counts) -> Result<()> {
        match &mut self.out {
            Out::Csv(writer) => {
                writer.write_record([
                    timestamp,
                    sha,
                    language,
                    &c.files.to_string(),
                    &c.code.to_string(),
                    &c.comments.to_string(),
                    &c.blanks.to_string(),
                ])?;
            }
            Out::Jsonl(sink) => {
                let row = serde_json::json!({
                    "timestamp": timestamp,
                    "sha": sha,
                    "language": language,
                    "files": c.files,
                    "code": c.code,
                    "comments": c.comments,
                    "blanks": c.blanks,
                });
                writeln!(sink, "{row}")?;
            }
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        match &mut self.out {
            Out::Csv(writer) => writer.flush()?,
            Out::Jsonl(sink) => sink.flush()?,
        }
        Ok(())
    }

    pub fn finish(&mut self) -> Result<()> {
        self.flush()
    }
}
