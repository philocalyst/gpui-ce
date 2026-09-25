//! Where Lightbox writes: `target/lightbox/<suite>/…`.
//!
//! Every artifact (shot, film, lint, golden, matrix) writes its own small JSON
//! record under `<suite>/records/`, so tests running in parallel threads and
//! processes never contend for a shared manifest. The `lightbox report`
//! command reads them all back.

use crate::manifest::Record;
use anyhow::{Context as _, Result};
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use std::{
    env, fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// The directory Lightbox writes into.
///
/// `$LIGHTBOX_DIR` if set, otherwise `lightbox/` inside the workspace's target
/// directory (`$CARGO_TARGET_DIR`, or `target/` next to the workspace's
/// `Cargo.lock`).
pub fn root() -> PathBuf {
    if let Some(dir) = env::var_os("LIGHTBOX_DIR") {
        return PathBuf::from(dir);
    }
    if let Some(dir) = env::var_os("CARGO_TARGET_DIR") {
        let dir = PathBuf::from(dir);
        let dir = if dir.is_relative() {
            workspace_root().join(dir)
        } else {
            dir
        };
        return dir.join("lightbox");
    }
    workspace_root().join("target").join("lightbox")
}

/// The workspace root: the nearest ancestor of the calling crate (or of the
/// current directory) that holds a `Cargo.lock`.
pub fn workspace_root() -> PathBuf {
    let start = env::var_os("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .or_else(|| env::current_dir().ok())
        .unwrap_or_default();
    start
        .ancestors()
        .find(|dir| dir.join("Cargo.lock").is_file())
        .map(Path::to_path_buf)
        .unwrap_or(start)
}

/// A file-name-safe version of `name`: lowercase ASCII letters, digits, `-`,
/// `_` and `.`; everything else becomes `-`.
pub fn slug(name: &str) -> String {
    let mut slug = String::with_capacity(name.len());
    for char in name.chars() {
        let char = char.to_ascii_lowercase();
        if char.is_ascii_alphanumeric() || matches!(char, '_' | '.') {
            slug.push(char);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "unnamed".into()
    } else {
        slug.into()
    }
}

/// A named group of artifacts, written to `target/lightbox/<suite>/` and shown
/// as one section (with one contact sheet) in the report.
#[derive(Clone, Debug)]
pub struct Suite {
    name: String,
    dir: PathBuf,
}

impl Suite {
    /// The suite `name` under [`root`].
    pub fn new(name: &str) -> Self {
        Self::at(name, root().join(slug(name)))
    }

    /// A suite writing into an explicit directory.
    pub fn at(name: &str, dir: impl Into<PathBuf>) -> Self {
        Self {
            name: name.into(),
            dir: dir.into(),
        }
    }

    /// The suite's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The directory the suite writes into.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Describes the suite in one sentence, shown under its name in the
    /// contact sheet and the report.
    pub fn describe(&self, description: &str) -> Result<()> {
        fs::create_dir_all(&self.dir)?;
        let path = self.dir.join("suite.json");
        let json = serde_json::to_vec_pretty(&SuiteInfo {
            name: self.name.clone(),
            description: description.into(),
        })?;
        fs::write(&path, json).with_context(|| format!("writing {}", path.display()))
    }

    /// The suite's description, if one was given.
    pub fn description(&self) -> Option<String> {
        let bytes = fs::read(self.dir.join("suite.json")).ok()?;
        let info: SuiteInfo = serde_json::from_slice(&bytes).ok()?;
        Some(info.description)
    }

    /// The path of `file` inside the suite's directory.
    pub fn path(&self, file: &str) -> PathBuf {
        self.dir.join(file)
    }

    /// Writes `image` as `file` (a PNG) inside the suite's directory.
    pub fn write_png(&self, file: &str, image: &RgbaImage) -> Result<PathBuf> {
        let path = self.path(file);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        image
            .save_with_format(&path, image::ImageFormat::Png)
            .with_context(|| format!("writing {}", path.display()))?;
        Ok(path)
    }

    /// Writes `record` to `records/<kind>--<name>.json`, replacing an earlier
    /// record of the same kind and name.
    pub fn write_record(&self, record: &Record) -> Result<()> {
        let dir = self.dir.join("records");
        fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let path = dir.join(format!("{}--{}.json", record.kind(), slug(record.name())));
        let json = serde_json::to_vec_pretty(&SuiteRecord {
            suite: self.name.clone(),
            record: record.clone(),
        })?;
        fs::write(&path, json).with_context(|| format!("writing {}", path.display()))
    }

    /// Every record written to this suite, sorted by kind and name.
    pub fn records(&self) -> Result<Vec<Record>> {
        let dir = self.dir.join("records");
        let mut records = Vec::new();
        if !dir.is_dir() {
            return Ok(records);
        }
        let mut paths = fs::read_dir(&dir)?
            .filter_map(|entry| Some(entry.ok()?.path()))
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .collect::<Vec<_>>();
        paths.sort();
        for path in paths {
            let bytes = fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
            let record: SuiteRecord = serde_json::from_slice(&bytes)
                .with_context(|| format!("parsing {}", path.display()))?;
            records.push(record.record);
        }
        Ok(records)
    }

    /// Every suite under `root` that has records, sorted by name.
    pub fn all(root: &Path) -> Result<Vec<Suite>> {
        let mut suites = Vec::new();
        if !root.is_dir() {
            return Ok(suites);
        }
        for entry in fs::read_dir(root)? {
            let dir = entry?.path();
            let first_record = dir.join("records").read_dir().ok().and_then(|mut entries| {
                entries.find_map(|entry| {
                    let path = entry.ok()?.path();
                    let bytes = fs::read(path).ok()?;
                    serde_json::from_slice::<SuiteRecord>(&bytes).ok()
                })
            });
            if let Some(record) = first_record {
                suites.push(Suite::at(&record.suite, dir));
            }
        }
        suites.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(suites)
    }
}

/// A suite's `suite.json`.
#[derive(Serialize, Deserialize)]
struct SuiteInfo {
    name: String,
    description: String,
}

/// A record together with the name of the suite it belongs to, as stored on disk.
#[derive(Serialize, Deserialize)]
struct SuiteRecord {
    suite: String,
    #[serde(flatten)]
    record: Record,
}

/// Milliseconds since the Unix epoch.
pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

/// Facts about the build that produced a result, for comparing runs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RunInfo {
    /// Abbreviated commit hash of `HEAD`, or `unknown`.
    pub git_sha: String,
    /// The checked-out branch, if any.
    pub git_branch: Option<String>,
    /// When the run happened, RFC 3339 in UTC.
    pub timestamp: String,
    /// `debug` or `release`: whether debug assertions were on.
    pub profile: String,
}

impl RunInfo {
    /// Facts about the current build and checkout.
    pub fn current() -> Self {
        let (git_sha, git_branch) = git_head(&workspace_root()).unwrap_or_default();
        Self {
            git_sha: if git_sha.is_empty() {
                "unknown".into()
            } else {
                git_sha
            },
            git_branch,
            timestamp: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            profile: if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            }
            .into(),
        }
    }
}

/// Reads `HEAD` from the repository containing `dir` without running git:
/// follows worktree `.git` files, loose refs and `packed-refs`.
fn git_head(dir: &Path) -> Option<(String, Option<String>)> {
    let dot_git = dir
        .ancestors()
        .map(|dir| dir.join(".git"))
        .find(|path| path.exists())?;
    let git_dir = if dot_git.is_file() {
        let pointer = fs::read_to_string(&dot_git).ok()?;
        let target = PathBuf::from(pointer.strip_prefix("gitdir:")?.trim());
        if target.is_relative() {
            dot_git.parent()?.join(target)
        } else {
            target
        }
    } else {
        dot_git
    };
    let common_dir = fs::read_to_string(git_dir.join("commondir"))
        .ok()
        .map(|common| git_dir.join(common.trim()))
        .unwrap_or_else(|| git_dir.clone());

    let head = fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    let Some(reference) = head.strip_prefix("ref:").map(str::trim) else {
        return Some((abbreviate(head), None));
    };
    let branch = reference.strip_prefix("refs/heads/").map(str::to_string);
    let sha = [&git_dir, &common_dir]
        .iter()
        .find_map(|dir| fs::read_to_string(dir.join(reference)).ok())
        .or_else(|| {
            let packed = fs::read_to_string(common_dir.join("packed-refs")).ok()?;
            packed.lines().find_map(|line| {
                let (sha, name) = line.split_once(' ')?;
                (name == reference).then(|| sha.to_string())
            })
        })?;
    Some((abbreviate(sha.trim()), branch))
}

fn abbreviate(sha: &str) -> String {
    sha.chars().take(10).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_file_safe() {
        assert_eq!(slug("Card / Dark 2×"), "card-dark-2");
        assert_eq!(slug("hover_state.v2"), "hover_state.v2");
        assert_eq!(slug("   "), "unnamed");
    }

    #[test]
    fn reads_the_current_commit() {
        let info = RunInfo::current();
        assert_ne!(info.git_sha, "unknown", "the crate lives in a git checkout");
        assert!(info.git_sha.chars().all(|char| char.is_ascii_hexdigit()));
        assert!(!info.timestamp.is_empty());
    }
}
