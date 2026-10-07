//! Validate writable destinations before saving evidence or pruning build state.

use std::{
    io,
    path::{Path, PathBuf},
};

pub(super) fn report_layout<'a>(
    report: &Path,
    work: &Path,
    sources: impl IntoIterator<Item = &'a Path>,
) -> io::Result<()> {
    disjoint(
        report,
        work,
        "report output and managed work must use separate directory trees",
    )?;
    for source in sources {
        disjoint(
            report,
            source,
            "report output must be outside source trees; choose a sibling directory or a temporary CI directory",
        )?;
    }
    Ok(())
}

pub(super) fn disjoint(first: &Path, second: &Path, message: &str) -> io::Result<()> {
    let first = destination(first)?;
    let second = destination(second)?;
    if first.starts_with(&second) || second.starts_with(&first) {
        return Err(io::Error::other(message));
    }
    Ok(())
}

/// Resolve existing aliases before checking a destination that may not yet exist.
fn destination(path: &Path) -> io::Result<PathBuf> {
    let mut existing = std::path::absolute(path)?;
    let mut tail = Vec::new();
    loop {
        match existing.canonicalize() {
            Ok(mut root) => {
                for component in tail.into_iter().rev() {
                    root.push(component);
                }
                return Ok(root);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let name = existing.file_name().ok_or_else(|| {
                    io::Error::other(
                        "create destinations containing unresolved '..' before running",
                    )
                })?;
                tail.push(name.to_owned());
                if !existing.pop() {
                    return Err(error);
                }
            }
            Err(error) => return Err(error),
        }
    }
}
