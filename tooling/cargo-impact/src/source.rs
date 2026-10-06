//! Snapshot inputs without modifying them; immutable revisions make results reproducible.

use std::{
    collections::BTreeSet,
    fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

use sha2::{Digest, Sha256};

use crate::{
    forge::Repository,
    model::DownstreamSource,
    process::{self, Capture},
    runner::clean_command,
};

pub(crate) fn key(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

pub(crate) fn checkout(
    source: &DownstreamSource,
    destination: &Path,
    timeout: Duration,
) -> io::Result<(PathBuf, Option<String>)> {
    let DownstreamSource::Git { url, revision, .. } = source else {
        let DownstreamSource::Local { path } = source else {
            unreachable!()
        };
        return Ok((path.canonicalize()?, None));
    };
    Repository::parse(url, None).map_err(io::Error::other)?;
    if revision.is_empty() || revision.starts_with('-') || revision.chars().any(char::is_whitespace)
    {
        return Err(io::Error::other("invalid git revision"));
    }
    if !destination.exists() {
        fs::create_dir_all(
            destination
                .parent()
                .ok_or_else(|| io::Error::other("missing checkout parent"))?,
        )?;
        git(
            destination.parent().unwrap(),
            &[
                "clone",
                "--quiet",
                "--no-checkout",
                "--",
                url,
                destination
                    .to_str()
                    .ok_or_else(|| io::Error::other("non-UTF8 checkout path"))?,
            ],
            timeout,
        )?;
    }
    git(
        destination,
        &["fetch", "--quiet", "origin", revision],
        timeout,
    )?;
    let sha = git(
        destination,
        &["rev-parse", "--verify", "FETCH_HEAD^{commit}"],
        timeout,
    )?;
    let sha = sha.trim().to_owned();
    if sha.len() != 40 && sha.len() != 64 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(io::Error::other("git returned invalid commit SHA"));
    }
    // Discard only changes in our managed checkout, never the user's original working tree.
    git(
        destination,
        &["checkout", "--quiet", "--force", "--detach", &sha, "--"],
        timeout,
    )?;
    git(destination, &["clean", "-ffdqx"], timeout)?;
    Ok((destination.to_owned(), Some(sha)))
}

fn git(cwd: &Path, args: &[&str], timeout: Duration) -> io::Result<String> {
    let mut command = clean_command("git");
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_LFS_SKIP_SMUDGE", "1")
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "protocol.file.allow=never",
            "-c",
            "credential.helper=",
        ])
        .args(args)
        .current_dir(cwd);
    let output = process::run(&mut command, timeout, Capture::Bytes)?;
    if !output.success {
        return Err(io::Error::other(format!("git failed: {}", output.log)));
    }
    if output.data_truncated {
        return Err(io::Error::other("git output exceeded limit"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub(crate) fn snapshot(source: &Path, destination: &Path) -> io::Result<String> {
    let source = source.canonicalize()?;
    if let Ok(relative) = destination.strip_prefix(&source)
        && relative
            .components()
            .next()
            .is_none_or(|c| c.as_os_str() != ".cargo-impact")
    {
        return Err(io::Error::other(
            "work directory must be outside source trees, or named .cargo-impact",
        ));
    }
    fs::create_dir_all(
        destination
            .parent()
            .ok_or_else(|| io::Error::other("snapshot has no parent"))?,
    )?;
    let staged = tempfile::TempDir::new_in(destination.parent().unwrap())?;
    let mut snapshot = Snapshot {
        root: &source,
        previous: destination,
        hash: Sha256::new(),
        visited: BTreeSet::new(),
        bytes: 0,
        files: 0,
    };
    snapshot.copy(&source, staged.path(), Path::new(""))?;
    let hash = format!("{:x}", snapshot.hash.finalize());
    if fs::symlink_metadata(destination).is_ok() {
        fs::remove_dir_all(destination)?;
    }
    fs::rename(staged.keep(), destination)?;
    Ok(hash)
}

struct Snapshot<'a> {
    root: &'a Path,
    previous: &'a Path,
    hash: Sha256,
    visited: BTreeSet<PathBuf>,
    bytes: u64,
    files: usize,
}

impl Snapshot<'_> {
    fn copy(&mut self, source: &Path, destination: &Path, relative: &Path) -> io::Result<()> {
        let resolved = source.canonicalize()?;
        if !resolved.starts_with(self.root) {
            return Err(io::Error::other(format!(
                "symlink escapes source tree: {}",
                relative.display()
            )));
        }
        if !self.visited.insert(resolved.clone()) {
            return Err(io::Error::other("source tree has a symlink cycle"));
        }
        fs::create_dir_all(destination)?;
        let mut entries = fs::read_dir(source)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let name = entry.file_name();
            if [".git", "target", ".cargo-impact", "node_modules"]
                .iter()
                .any(|v| name == *v)
            {
                continue;
            }
            let path = entry.path();
            let metadata = fs::metadata(&path)?;
            let relative = relative.join(&name);
            let destination = destination.join(&name);
            if metadata.is_dir() {
                self.copy(&path, &destination, &relative)?;
            } else if metadata.is_file() {
                let resolved = path.canonicalize()?;
                if !resolved.starts_with(self.root) {
                    return Err(io::Error::other(format!(
                        "symlink escapes source tree: {}",
                        relative.display()
                    )));
                }
                self.bytes += metadata.len();
                self.files += 1;
                if self.bytes > 2 * 1024 * 1024 * 1024 || self.files > 100_000 {
                    return Err(io::Error::other(
                        "source snapshot exceeds 2 GiB or 100,000 files",
                    ));
                }
                let bytes = fs::read(&path)?;
                self.hash.update(relative.as_os_str().as_encoded_bytes());
                self.hash.update([0]);
                self.hash.update((bytes.len() as u64).to_le_bytes());
                self.hash.update(&bytes);
                let previous = self.previous.join(&relative);
                let unchanged_time = previous
                    .canonicalize()
                    .ok()
                    .filter(|p| p.starts_with(self.previous))
                    .and_then(|_| fs::metadata(&previous).ok())
                    .filter(|m| m.is_file())
                    .and_then(|m| {
                        fs::read(&previous)
                            .ok()
                            .filter(|old| *old == bytes)
                            .and_then(|_| m.modified().ok())
                    });
                fs::copy(&path, &destination)?;
                if let Some(time) = unchanged_time {
                    fs::File::open(&destination)?
                        .set_times(fs::FileTimes::new().set_modified(time))?;
                }
            } else {
                return Err(io::Error::other("unsupported special file in source tree"));
            }
        }
        self.visited.remove(&resolved);
        Ok(())
    }
}

pub(crate) fn validate_manifest(manifest: &Path) -> io::Result<()> {
    if manifest.is_absolute()
        || manifest
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err(io::Error::other(
            "manifest must be a relative path without traversal",
        ));
    }
    Ok(())
}
