//! A cache of normalized session text, so a search reads a few MB instead of
//! re-parsing a GB of JSONL. Entries are keyed by path, mtime and size and only
//! changed transcripts are re-read.

use crate::extract::{self, Field, NUM_FIELDS};
use crate::text::FieldBuilder;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Bump when extraction or normalization changes so old caches are rebuilt.
const CACHE_VERSION: u32 = 2;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Doc {
    pub path: String,
    pub sid: String,
    pub cwd: String,
    pub branches: Vec<String>,
    pub title: String,
    pub mtime_secs: i64,
    pub mtime_nanos: u32,
    pub size: u64,
    pub fields: [String; NUM_FIELDS],
    pub lens: [u32; NUM_FIELDS],
}

#[derive(Serialize, Deserialize)]
struct Cache {
    version: u32,
    docs: Vec<Doc>,
}

pub fn projects_dir() -> PathBuf {
    std::env::var_os("CCS_PROJECTS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".claude/projects"))
}

pub fn cache_path() -> PathBuf {
    std::env::var_os("CCS_CACHE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".cache/ccs"))
        .join("index.bin")
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

struct FileStat {
    path: PathBuf,
    secs: i64,
    nanos: u32,
    size: u64,
}

/// Top-level `<uuid>.jsonl` files only. Subagent transcripts live in
/// subdirectories and can't be resumed on their own.
fn list_sessions(dir: &Path) -> Vec<FileStat> {
    let mut out = vec![];
    let Ok(projects) = std::fs::read_dir(dir) else { return out };
    for project in projects.flatten() {
        let Ok(files) = std::fs::read_dir(project.path()) else { continue };
        for f in files.flatten() {
            let path = f.path();
            let is_session = path.extension().is_some_and(|e| e == "jsonl")
                && path.file_stem().and_then(|s| s.to_str()).is_some_and(is_uuid);
            if !is_session {
                continue;
            }
            let Ok(meta) = f.metadata() else { continue };
            let mt = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).unwrap_or_default();
            out.push(FileStat { path, secs: mt.as_secs() as i64, nanos: mt.subsec_nanos(), size: meta.len() });
        }
    }
    out
}

fn is_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36
        && b.iter().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => *c == b'-',
            _ => c.is_ascii_hexdigit() && !c.is_ascii_uppercase(),
        })
}

pub fn build_doc(path: &Path, secs: i64, nanos: u32, size: u64) -> Option<Doc> {
    let t = extract::read(path).ok()?;
    let mut fields: [FieldBuilder; NUM_FIELDS] = Default::default();

    if t.titles.is_empty() {
        fields[Field::Title as usize].push(&t.title);
    }
    for title in &t.titles {
        fields[Field::Title as usize].push(title);
    }
    fields[Field::Project as usize].push(&project_label(&t.cwd));
    for b in &t.branches {
        fields[Field::Project as usize].push(b);
    }
    for s in &t.segments {
        fields[s.field as usize].push(&s.text);
    }

    Some(Doc {
        path: path.to_string_lossy().into_owned(),
        sid: path.file_stem()?.to_string_lossy().into_owned(),
        cwd: t.cwd,
        branches: t.branches,
        title: t.title,
        mtime_secs: secs,
        mtime_nanos: nanos,
        size,
        lens: std::array::from_fn(|i| fields[i].len),
        fields: fields.map(|f| f.text),
    })
}

/// The last two path components, e.g. "repo-worktrees/mcp-toolkit-public-api".
/// Higher components like "Users/me/Documents" are shared by every session.
pub fn project_label(cwd: &str) -> String {
    let parts: Vec<&str> = cwd.split('/').filter(|p| !p.is_empty()).collect();
    parts[parts.len().saturating_sub(2)..].join("/")
}

/// Load the index, re-reading transcripts that changed since the cache was
/// written. With `persist`, write the refreshed cache back to disk.
pub fn load(persist: bool) -> Vec<Doc> {
    load_from(&projects_dir(), &cache_path(), persist)
}

pub fn load_from(projects: &Path, cache_file: &Path, persist: bool) -> Vec<Doc> {
    let mut cached: HashMap<String, Doc> = std::fs::read(cache_file)
        .ok()
        .and_then(|b| bincode::deserialize::<Cache>(&b).ok())
        .filter(|c| c.version == CACHE_VERSION)
        .map(|c| c.docs.into_iter().map(|d| (d.path.clone(), d)).collect())
        .unwrap_or_default();
    let cached_count = cached.len();

    let files = list_sessions(projects);
    let mut docs = Vec::with_capacity(files.len());
    let mut stale = vec![];
    for f in files {
        let key = f.path.to_string_lossy().into_owned();
        match cached.remove(&key) {
            Some(d) if d.mtime_secs == f.secs && d.mtime_nanos == f.nanos && d.size == f.size => docs.push(d),
            _ => stale.push(f),
        }
    }
    let changed = !stale.is_empty() || docs.len() != cached_count;
    docs.extend(stale.par_iter().filter_map(|f| build_doc(&f.path, f.secs, f.nanos, f.size)).collect::<Vec<_>>());

    if persist && changed {
        if let Err(e) = save(cache_file, &docs) {
            eprintln!("ccs: could not write cache {}: {e}", cache_file.display());
        }
    }
    docs
}

fn save(path: &Path, docs: &[Doc]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let cache = Cache { version: CACHE_VERSION, docs: docs.to_vec() };
    let bytes = bincode::serialize(&cache).map_err(std::io::Error::other)?;
    // Write then rename, so a concurrent reader never sees a partial file.
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_filter() {
        assert!(is_uuid("0fae4f6b-b154-4a06-8e66-5547850868f2"));
        assert!(!is_uuid("agent-a1b2c3"));
        assert!(!is_uuid("0fae4f6b-b154-4a06-8e66-5547850868f2x"));
    }

    #[test]
    fn project_label_keeps_last_two_components() {
        assert_eq!(project_label("/Users/me/stlabs/repo-worktrees/mcp-toolkit"), "repo-worktrees/mcp-toolkit");
        assert_eq!(project_label("/tmp"), "tmp");
        assert_eq!(project_label(""), "");
    }
}
