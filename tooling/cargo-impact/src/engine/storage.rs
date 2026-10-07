//! Owned work directories, snapshot boundaries and storage policy.

use crate::model::{DownstreamSource, ImpactRequest};
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

/// Check known inputs before pruning any managed path. Missing inputs remain a
/// later preparation failure, but their existing ancestors still reveal aliases
/// and descendants of the work directory.
pub(super) fn validate_inputs(root: &Path, request: &ImpactRequest) -> io::Result<()> {
    validate_input(root, &request.baseline)?;
    validate_input(root, &request.candidate)?;
    for spec in &request.downstreams {
        if let DownstreamSource::Local { path } = &spec.source {
            validate_input(root, path)?;
        }
    }
    Ok(())
}

pub(super) fn validate_input(root: &Path, source: &Path) -> io::Result<()> {
    let mut ancestor = if source.is_absolute() {
        source.to_owned()
    } else {
        std::env::current_dir()?.join(source)
    };
    let mut suffix = Vec::new();
    let resolved = loop {
        match ancestor.canonicalize() {
            Ok(mut resolved) => {
                for component in suffix.into_iter().rev() {
                    if component == ".." {
                        resolved.pop();
                    } else if component != "." {
                        resolved.push(component);
                    }
                }
                break resolved;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let component = ancestor.components().next_back().ok_or(error)?;
                suffix.push(component.as_os_str().to_owned());
                ancestor.pop();
            }
            Err(error) => return Err(error),
        }
    };
    if resolved.starts_with(root) {
        return Err(io::Error::other(format!(
            "source input must be outside the managed work directory before cleanup: {}",
            source.display()
        )));
    }
    Ok(())
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

/// Under the held scan lock, start a clean confirmation without discarding
/// dependency downloads or another experiment's compiler artifacts.
pub(super) fn prepare_fresh_targets(
    root: &Path,
    experiment: &crate::ExperimentId,
) -> io::Result<()> {
    let targets = root.join("targets").join(experiment.as_str());
    crate::runner::safe_directory(root, &targets)?;
    fs::remove_dir_all(targets)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_boundaries_resolve_existing_ancestors_without_rejecting_missing_external_sources() {
        let area = WorkArea::open(None).unwrap();
        let outside = TempDir::new().unwrap();
        assert!(validate_input(area.path(), area.path()).is_err());
        assert!(validate_input(area.path(), &area.path().join("cache/absent")).is_err());
        assert!(validate_input(area.path(), &outside.path().join("absent/../consumer")).is_ok());
        #[cfg(unix)]
        {
            let alias = outside.path().join("work-alias");
            std::os::unix::fs::symlink(area.path(), &alias).unwrap();
            assert!(validate_input(area.path(), &alias.join("missing/../consumer")).is_err());
        }
    }

    #[test]
    fn clean_replay_discards_only_selected_targets_and_refuses_parent_symlinks() {
        let area = WorkArea::open(None).unwrap();
        let id = crate::ExperimentId::try_from("a".repeat(64)).unwrap();
        let selected = area.path().join("targets").join(id.as_str());
        let sibling = area.path().join("targets").join("b".repeat(64));
        let downloads = area
            .path()
            .join("workers")
            .join(id.as_str())
            .join("baseline/cache/cargo");
        for path in [&selected, &sibling, &downloads] {
            fs::create_dir_all(path).unwrap();
            fs::write(path.join("keep-or-clean"), "retained bytes").unwrap();
        }
        prepare_fresh_targets(area.path(), &id).unwrap();
        assert!(!selected.exists());
        assert!(sibling.join("keep-or-clean").is_file());
        assert!(downloads.join("keep-or-clean").is_file());
        #[cfg(unix)]
        {
            let outside = TempDir::new().unwrap();
            fs::write(outside.path().join("untouched"), "user data").unwrap();
            std::os::unix::fs::symlink(outside.path(), &selected).unwrap();
            assert!(prepare_fresh_targets(area.path(), &id).is_err());
            assert_eq!(
                fs::read_to_string(outside.path().join("untouched")).unwrap(),
                "user data"
            );
        }
    }

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
