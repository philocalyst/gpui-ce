//! Report I/O is bounded, rejects internal symlinks, and publishes by rename.

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

use fs2::FileExt;
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::{ArtifactDigest, BundleError};

pub(super) fn relative_path(path: &Path) -> Result<(), BundleError> {
    if path.as_os_str().is_empty()
        || path
            .to_str()
            .is_none_or(|value| value.contains(['\\', ':', '\0']))
        || !path
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
    {
        return Err(BundleError::UnsafePath(path.into()));
    }
    Ok(())
}

pub(super) fn owned_file(root: &Path, relative: &Path) -> Result<PathBuf, BundleError> {
    relative_path(relative)?;
    let mut path = root.to_owned();
    for component in relative.components() {
        path.push(component);
        if fs::symlink_metadata(&path)?.is_symlink() {
            return Err(BundleError::UnsafePath(relative.into()));
        }
    }
    if !fs::metadata(&path)?.is_file() {
        return Err(BundleError::UnsafePath(relative.into()));
    }
    Ok(path)
}

pub(super) fn directory(root: &Path, relative: &Path) -> Result<PathBuf, BundleError> {
    relative_path(relative)?;
    let mut path = root.to_owned();
    for component in relative.components() {
        path.push(component);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if !metadata.is_dir() || metadata.is_symlink() => {
                return Err(BundleError::UnsafePath(relative.into()));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => fs::create_dir(&path)?,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(path)
}

pub(super) fn bounded_read(path: &Path, limit: u64) -> Result<Vec<u8>, BundleError> {
    let file = File::open(path)?;
    if file.metadata()?.len() > limit {
        return Err(BundleError::TooLarge);
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(BundleError::TooLarge);
    }
    Ok(bytes)
}

pub(super) fn digest(path: &Path, limit: u64) -> Result<ArtifactDigest, BundleError> {
    let mut file = File::open(path)?;
    if file.metadata()?.len() > limit {
        return Err(BundleError::TooLarge);
    }
    let mut hash = Sha256::new();
    let mut bytes = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        bytes = bytes
            .checked_add(read as u64)
            .ok_or(BundleError::TooLarge)?;
        if bytes > limit {
            return Err(BundleError::TooLarge);
        }
        hash.update(&buffer[..read]);
    }
    Ok(ArtifactDigest {
        bytes,
        sha256: format!("{:x}", hash.finalize()),
    })
}

pub(super) fn atomic_bytes(path: &Path, contents: &[u8]) -> Result<(), BundleError> {
    let parent = path
        .parent()
        .ok_or_else(|| BundleError::UnsafePath(path.into()))?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    staged.write_all(contents)?;
    staged.as_file().sync_all()?;
    staged.persist(path).map_err(|error| error.error)?;
    sync_directory(parent)?;
    Ok(())
}

pub(super) fn atomic_json<T: Serialize>(path: &Path, value: &T) -> Result<(), BundleError> {
    atomic_json_bounded(path, value, super::MAX_MANIFEST_BYTES).map(|_| ())
}

pub(super) fn atomic_json_bounded<T: Serialize>(
    path: &Path,
    value: &T,
    limit: u64,
) -> Result<ArtifactDigest, BundleError> {
    let parent = path
        .parent()
        .ok_or_else(|| BundleError::UnsafePath(path.into()))?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    let digest = {
        let mut writer = DigestWriter {
            file: staged.as_file_mut(),
            hash: Sha256::new(),
            bytes: 0,
            limit,
        };
        if let Err(error) = serde_json::to_writer_pretty(&mut writer, value) {
            return Err(
                if error.io_error_kind() == Some(std::io::ErrorKind::FileTooLarge) {
                    BundleError::TooLarge
                } else {
                    error.into()
                },
            );
        }
        ArtifactDigest {
            bytes: writer.bytes,
            sha256: format!("{:x}", writer.hash.finalize()),
        }
    };
    staged.as_file().sync_all()?;
    staged.persist(path).map_err(|error| error.error)?;
    sync_directory(parent)?;
    Ok(digest)
}

struct DigestWriter<'a> {
    file: &'a mut File,
    hash: Sha256,
    bytes: u64,
    limit: u64,
}

impl Write for DigestWriter<'_> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        if buffer.len() as u64 > self.limit.saturating_sub(self.bytes) {
            return Err(std::io::ErrorKind::FileTooLarge.into());
        }
        let written = self.file.write(buffer)?;
        self.bytes += written as u64;
        self.hash.update(&buffer[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

pub(super) fn sync_directory(path: &Path) -> Result<(), BundleError> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    Ok(())
}

pub(super) struct WriterLock(File);

impl WriterLock {
    pub fn acquire(root: &Path) -> Result<Self, BundleError> {
        let path = root.join(".cargo-impact-report.lock");
        if fs::symlink_metadata(&path)
            .is_ok_and(|metadata| metadata.is_symlink() || !metadata.is_file())
        {
            return Err(BundleError::UnsafePath(path));
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let file = options.open(path)?;
        file.try_lock_exclusive().map_err(|error| {
            if error.kind() == std::io::ErrorKind::WouldBlock {
                BundleError::Busy
            } else {
                error.into()
            }
        })?;
        Ok(Self(file))
    }
}

impl Drop for WriterLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}
