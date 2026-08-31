use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write as _;
use std::path::Path;
use std::rc::Rc;

use anyhow::{Context, Result};
use gix::bstr::{BString, ByteSlice};
use gix::object::tree::EntryKind;
use gix::ObjectId;
use tokei::{Config, LanguageType};

pub const HUGE_BLOB_LIMIT: u64 = 10 * 1024 * 1024;
const BINARY_SNIFF_LEN: usize = 8 * 1024;

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Counts {
    pub files: u64,
    pub code: u64,
    pub comments: u64,
    pub blanks: u64,
}

impl Counts {
    pub fn add(&mut self, other: &Counts) {
        self.files += other.files;
        self.code += other.code;
        self.comments += other.comments;
        self.blanks += other.blanks;
    }
}

pub type LangTotals = BTreeMap<LanguageType, Counts>;

enum BlobClass {
    Text,
    Binary,
    Huge,
}

pub struct Counter<'repo> {
    repo: &'repo gix::Repository,
    config: Config,
    caching: bool,
    // Exclude patterns as repo-root-anchored path components (byte-exact, case-sensitive).
    excludes: Vec<Vec<BString>>,
    tree_cache: HashMap<ObjectId, Rc<LangTotals>>,
    parse_cache: HashMap<(ObjectId, LanguageType), Counts>,
    class_cache: HashMap<ObjectId, bool>, // true = huge, false = binary; text blobs land in parse_cache
    shebang_cache: HashMap<ObjectId, Option<LanguageType>>,
    skipped_unknown: HashSet<ObjectId>,
    skipped_binary: HashSet<ObjectId>,
    skipped_huge: HashSet<ObjectId>,
}

impl<'repo> Counter<'repo> {
    pub fn new(repo: &'repo gix::Repository, excludes: &[String], caching: bool) -> Self {
        Self {
            repo,
            config: Config::default(),
            caching,
            excludes: parse_excludes(excludes),
            tree_cache: HashMap::new(),
            parse_cache: HashMap::new(),
            class_cache: HashMap::new(),
            shebang_cache: HashMap::new(),
            skipped_unknown: HashSet::new(),
            skipped_binary: HashSet::new(),
            skipped_huge: HashSet::new(),
        }
    }

    pub fn stats_tree(&mut self, tree_id: ObjectId) -> Result<Rc<LangTotals>> {
        let active: Vec<(usize, usize)> = (0..self.excludes.len()).map(|i| (i, 0)).collect();
        self.stats_tree_inner(tree_id, &active)
    }

    /// `active` holds (exclude index, component offset) pairs for patterns whose consumed
    /// prefix matches the path of this tree. A tree is cacheable only when no exclude can
    /// apply beneath it — the same tree OID at another path must not inherit filtered totals.
    fn stats_tree_inner(
        &mut self,
        tree_id: ObjectId,
        active: &[(usize, usize)],
    ) -> Result<Rc<LangTotals>> {
        let cacheable = self.caching && active.is_empty();
        if cacheable {
            if let Some(hit) = self.tree_cache.get(&tree_id) {
                return Ok(Rc::clone(hit));
            }
        }

        // Decode entries into owned form before recursing so no tree buffer stays borrowed.
        let entries: Vec<(BString, ObjectId, EntryKind)> = {
            let tree = self
                .repo
                .find_object(tree_id)
                .with_context(|| format!("failed to read tree {tree_id}"))?
                .try_into_tree()
                .with_context(|| format!("object {tree_id} is not a tree"))?;
            let mut v = Vec::new();
            for entry in tree.iter() {
                let entry = entry.with_context(|| format!("corrupt tree {tree_id}"))?;
                v.push((
                    entry.filename().to_owned(),
                    entry.oid().to_owned(),
                    entry.mode().kind(),
                ));
            }
            v
        };

        let mut total = LangTotals::new();
        for (name, oid, kind) in entries {
            let mut pruned = false;
            let mut child_active: Vec<(usize, usize)> = Vec::new();
            for &(pat_idx, offset) in active {
                let pattern = &self.excludes[pat_idx];
                if pattern[offset].as_bstr() == name.as_bstr() {
                    if offset + 1 == pattern.len() {
                        pruned = true;
                        break;
                    }
                    child_active.push((pat_idx, offset + 1));
                }
            }
            if pruned {
                continue;
            }
            match kind {
                EntryKind::Tree => {
                    let child = self.stats_tree_inner(oid, &child_active)?;
                    for (lang, counts) in child.iter() {
                        total.entry(*lang).or_default().add(counts);
                    }
                }
                EntryKind::Blob | EntryKind::BlobExecutable => {
                    if let Some((lang, counts)) = self.stats_blob(&name, oid)? {
                        total.entry(lang).or_default().add(&counts);
                    }
                }
                EntryKind::Link | EntryKind::Commit => {}
            }
        }

        let total = Rc::new(total);
        if cacheable {
            self.tree_cache.insert(tree_id, Rc::clone(&total));
        }
        Ok(total)
    }

    fn stats_blob(
        &mut self,
        name: &BString,
        oid: ObjectId,
    ) -> Result<Option<(LanguageType, Counts)>> {
        let lang = match self.detect_language(name, oid)? {
            Some(lang) => lang,
            None => {
                // detect_by_shebang may have skipped it as huge already
                if !self.skipped_huge.contains(&oid) {
                    self.skipped_unknown.insert(oid);
                }
                return Ok(None);
            }
        };

        if self.caching {
            if let Some(counts) = self.parse_cache.get(&(oid, lang)) {
                return Ok(Some((lang, *counts)));
            }
            if let Some(&huge) = self.class_cache.get(&oid) {
                debug_assert!(
                    self.skipped_huge.contains(&oid) || self.skipped_binary.contains(&oid)
                );
                let _ = huge;
                return Ok(None);
            }
        }

        match self.classify_and_read(name, oid)? {
            (BlobClass::Huge, _) => {
                if self.caching {
                    self.class_cache.insert(oid, true);
                }
                Ok(None)
            }
            (BlobClass::Binary, _) => {
                if self.caching {
                    self.class_cache.insert(oid, false);
                }
                Ok(None)
            }
            (BlobClass::Text, data) => {
                let text = String::from_utf8_lossy(&data);
                let parsed = lang.parse_from_str(text.as_ref(), &self.config);
                // tokei's Jupyter parser already folds cell stats into the top level;
                // summarise() there would double-count every notebook line.
                let stats = if lang == LanguageType::Jupyter {
                    parsed
                } else {
                    parsed.summarise()
                };
                let counts = Counts {
                    files: 1,
                    code: stats.code as u64,
                    comments: stats.comments as u64,
                    blanks: stats.blanks as u64,
                };
                if self.caching {
                    self.parse_cache.insert((oid, lang), counts);
                }
                Ok(Some((lang, counts)))
            }
        }
    }

    /// Size guard via the object header (no payload decode), then a NUL sniff on the bytes.
    fn classify_and_read(&mut self, name: &BString, oid: ObjectId) -> Result<(BlobClass, Vec<u8>)> {
        let header = self
            .repo
            .find_header(oid)
            .with_context(|| format!("failed to read object header for blob {oid}"))?;
        if header.size() > HUGE_BLOB_LIMIT {
            if self.skipped_huge.insert(oid) {
                eprintln!(
                    "warning: skipping huge blob '{}' ({} bytes, limit {})",
                    name.to_str_lossy(),
                    header.size(),
                    HUGE_BLOB_LIMIT
                );
            }
            return Ok((BlobClass::Huge, Vec::new()));
        }
        let data = self
            .repo
            .find_object(oid)
            .with_context(|| format!("failed to read blob {oid}"))?
            .detach()
            .data;
        let sniff_len = data.len().min(BINARY_SNIFF_LEN);
        if data[..sniff_len].contains(&0) {
            self.skipped_binary.insert(oid);
            return Ok((BlobClass::Binary, Vec::new()));
        }
        Ok((BlobClass::Text, data))
    }

    /// Language from the entry filename/extension, with a shebang fallback for
    /// extensionless files. Detection depends only on the entry name and blob bytes,
    /// which is what keeps the OID-keyed caches sound.
    fn detect_language(&mut self, name: &BString, oid: ObjectId) -> Result<Option<LanguageType>> {
        // The sentinel directory cannot exist, so from_path's shebang fallback
        // (which opens the path) can never read a stray same-named local file.
        let lossy_name = name.to_str_lossy();
        let sentinel = Path::new("/__loch_no_such_dir__").join(lossy_name.as_ref());
        if let Some(lang) = LanguageType::from_path(&sentinel, &self.config) {
            return Ok(Some(lang));
        }
        // tokei consults the shebang only when the path has no extension; a file
        // with an unrecognized extension is skipped even if its content has one.
        if sentinel.extension().is_none() {
            self.detect_by_shebang(oid)
        } else {
            Ok(None)
        }
    }

    /// Mirrors the tokei CLI's shebang detection by writing the blob's first line to a
    /// temp file and letting tokei's own shebang table classify it (no table to vendor).
    fn detect_by_shebang(&mut self, oid: ObjectId) -> Result<Option<LanguageType>> {
        if self.caching {
            if let Some(cached) = self.shebang_cache.get(&oid) {
                return Ok(*cached);
            }
            if self.class_cache.contains_key(&oid) {
                return Ok(None);
            }
        }
        let header = self
            .repo
            .find_header(oid)
            .with_context(|| format!("failed to read object header for blob {oid}"))?;
        if header.size() > HUGE_BLOB_LIMIT {
            if self.skipped_huge.insert(oid) {
                eprintln!(
                    "warning: skipping huge blob {oid} ({} bytes, limit {HUGE_BLOB_LIMIT})",
                    header.size()
                );
            }
            if self.caching {
                self.class_cache.insert(oid, true);
            }
            return Ok(None);
        }
        let data = self
            .repo
            .find_object(oid)
            .with_context(|| format!("failed to read blob {oid}"))?
            .detach()
            .data;
        let first_line = data.split(|&b| b == b'\n').next().unwrap_or(&data);
        let first_line = &first_line[..first_line.len().min(4096)];
        // tokei tolerates leading whitespace before the '#!' token
        let lang = if first_line.trim_ascii_start().starts_with(b"#!") {
            let mut tmp = tempfile::NamedTempFile::new()?;
            tmp.write_all(first_line)?;
            tmp.write_all(b"\n")?;
            tmp.flush()?;
            LanguageType::from_path(tmp.path(), &self.config)
        } else {
            None
        };
        if self.caching {
            self.shebang_cache.insert(oid, lang);
        }
        Ok(lang)
    }

    pub fn report_skips(&self) {
        let (unknown, binary, huge) = (
            self.skipped_unknown.len(),
            self.skipped_binary.len(),
            self.skipped_huge.len(),
        );
        if unknown + binary + huge > 0 {
            eprintln!(
                "loch: skipped {unknown} unrecognized, {binary} binary, {huge} huge (>{HUGE_BLOB_LIMIT} bytes) unique blobs"
            );
        }
    }
}

fn parse_excludes(patterns: &[String]) -> Vec<Vec<BString>> {
    patterns
        .iter()
        .filter_map(|pattern| {
            let components: Vec<BString> = pattern
                .split('/')
                .filter(|c| !c.is_empty() && *c != ".")
                .map(BString::from)
                .collect();
            if components.is_empty() {
                None
            } else {
                Some(components)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::parse_excludes;

    #[test]
    fn excludes_normalize_slashes() {
        let parsed = parse_excludes(&[
            "vendor/".to_string(),
            "/src/generated".to_string(),
            "./docs".to_string(),
            "a//b/".to_string(),
            "/".to_string(),
            "".to_string(),
        ]);
        let as_strings: Vec<Vec<String>> = parsed
            .iter()
            .map(|p| p.iter().map(|c| c.to_string()).collect())
            .collect();
        assert_eq!(
            as_strings,
            vec![
                vec!["vendor".to_string()],
                vec!["src".to_string(), "generated".to_string()],
                vec!["docs".to_string()],
                vec!["a".to_string(), "b".to_string()],
            ]
        );
    }
}
