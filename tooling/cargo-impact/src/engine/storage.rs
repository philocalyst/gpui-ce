//! Owned work directories, snapshot boundaries and storage policy.

use crate::model::ImpactRequest;
use fs2::FileExt;
use std::{
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
    time::Instant,
};
use tempfile::TempDir;

pub(super) struct WorkArea {
    root: PathBuf,
    _temporary: Option<TempDir>,
    _lock: File,
}

impl WorkArea {
    pub(super) fn open(path: Option<&Path>) -> io::Result<Self> {
        let temporary = if path.is_none() {
            Some(TempDir::new()?)
        } else {
            None
        };
        let root = path
            .or_else(|| temporary.as_ref().map(TempDir::path))
            .ok_or_else(|| io::Error::other("missing work directory"))?;
        fs::create_dir_all(root)?;
        let root = root.canonicalize()?;
        let marker = root.join("cargo-impact-work-v1");
        if fs::symlink_metadata(&marker).is_ok_and(|m| m.is_symlink())
            || fs::symlink_metadata(root.join("scan.lock")).is_ok_and(|m| m.is_symlink())
        {
            return Err(io::Error::other(
                "work directory marker and lock must not be symlinks",
            ));
        }
        if !marker.exists() {
            // Accept the previous version's known managed layout, but never mark
            // a repository or arbitrary user directory as safe to prune.
            for entry in fs::read_dir(&root)? {
                let name = entry?.file_name();
                if ![
                    "home",
                    "scan.lock",
                    "upstream",
                    "downstreams",
                    "checkouts",
                    "targets",
                    "cache",
                    "gate-target",
                    "gate-worker",
                    "gate-sources",
                    "workers",
                ]
                .iter()
                .any(|allowed| name == *allowed)
                {
                    return Err(io::Error::other(
                        "choose a dedicated work directory: this directory contains data not owned by cargo-impact",
                    ));
                }
            }
            fs::write(marker, b"cargo-impact managed work directory v1\n")?;
        } else if fs::read(&marker)? != b"cargo-impact managed work directory v1\n" {
            return Err(io::Error::other(
                "invalid cargo-impact work directory marker",
            ));
        }
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join("scan.lock"))?;
        lock.try_lock_exclusive()
            .map_err(|e| io::Error::other(format!("another scan owns this work directory: {e}")))?;
        Ok(Self {
            root,
            _temporary: temporary,
            _lock: lock,
        })
    }
    pub(super) fn path(&self) -> &Path {
        &self.root
    }
}

impl Drop for WorkArea {
    fn drop(&mut self) {
        // Explicitly release the shared open-file-description lock before a retry.
        // Relying on descriptor closure can leave it briefly held by an exiting child.
        let _ = FileExt::unlock(&self._lock);
    }
}

/// Count only this managed tree. Symlinks are never followed into user files.
pub(crate) fn work_bytes(root: &Path) -> io::Result<u64> {
    let mut pending = vec![root.to_owned()];
    let mut bytes = 0u64;
    while let Some(path) = pending.pop() {
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if metadata.is_dir() {
            let entries = match fs::read_dir(path) {
                Ok(entries) => entries,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            for entry in entries {
                match entry {
                    Ok(entry) => pending.push(entry.path()),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error),
                }
            }
        } else {
            bytes = bytes.saturating_add(metadata.len());
        }
    }
    Ok(bytes)
}

pub(super) fn prepare_storage(
    root: &Path,
    execution: &crate::model::ExecutionOptions,
) -> io::Result<()> {
    // A lock is held and no workers have started. Pruning never races live builds.
    if execution.prune_before_run {
        for directory in [
            "targets",
            "workers",
            "gate-target",
            "gate-worker",
            "gate-sources",
            "cache",
        ] {
            let path = root.join(directory);
            if fs::symlink_metadata(&path).is_ok() {
                fs::remove_dir_all(path)?;
            }
        }
    }
    let bytes = work_bytes(root)?;
    if execution
        .max_work_bytes
        .is_some_and(|budget| bytes > budget)
    {
        return Err(io::Error::other(format!(
            "work directory uses {bytes} bytes and exceeds the configured storage budget; enable execution.prune_before_run or choose a new work directory"
        )));
    }
    Ok(())
}

pub(super) fn snapshot_in(
    request: &ImpactRequest,
    root: &Path,
    source: &Path,
    destination: &Path,
    deadline: Instant,
) -> io::Result<String> {
    crate::runner::safe_directory(root, destination)?;
    let available = request
        .execution
        .max_work_bytes
        .map(|budget| work_bytes(root).map(|used| budget.saturating_sub(used)))
        .transpose()?
        .unwrap_or(2 * 1024 * 1024 * 1024);
    crate::source::snapshot_before(
        source,
        destination,
        available,
        Some(deadline),
        request
            .execution
            .max_work_bytes
            .map(|budget| (root, budget)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_accounting_tolerates_parallel_snapshot_replacement() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().to_owned();
        std::thread::scope(|scope| {
            let root = &root;
            scope.spawn(move || {
                for _ in 0..200 {
                    let directory = root.join("changing/nested");
                    fs::create_dir_all(&directory).unwrap();
                    fs::write(directory.join("file"), "temporary snapshot").unwrap();
                    fs::remove_dir_all(root.join("changing")).unwrap();
                }
            });
            for _ in 0..500 {
                work_bytes(root).unwrap();
            }
        });
    }
}
