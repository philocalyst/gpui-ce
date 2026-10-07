//! Installation is self-contained even before the scanner is published.

use std::{
    fs,
    io::{self, Write},
    path::Path,
};

type Asset = (&'static str, &'static [u8]);

include!(concat!(env!("OUT_DIR"), "/scanner-assets.rs"));

pub(crate) fn initialize(directory: &Path, library: &str) -> io::Result<()> {
    if library.is_empty()
        || !library
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(io::Error::other("invalid library name"));
    }
    let ignore_path = directory.join(".gitignore");
    if fs::symlink_metadata(&ignore_path)
        .is_ok_and(|metadata| !metadata.is_file() || metadata.file_type().is_symlink())
    {
        return Err(io::Error::other(".gitignore must be a regular file"));
    }
    let mut ignores = match fs::read_to_string(&ignore_path) {
        Ok(content) => content,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };
    for pattern in [".cargo-impact/", "impact-report/"] {
        if !ignores.lines().any(|line| line == pattern) {
            if !ignores.is_empty() && !ignores.ends_with('\n') {
                ignores.push('\n');
            }
            ignores.push_str(pattern);
            ignores.push('\n');
        }
    }
    let mut files = vec![
        (".gitignore".into(),ignores.into_bytes()),
        (
            "impact.toml".to_owned(),
            format!(
                "library = {library:?}\n\n[execution]\njobs = 2\ncargo_jobs = 1\nmemory_mib = 2048\nmax_work_bytes = 5368709120\n\n[discovery]\ncrates_io = true\nmax_repositories = 20\n"
            )
            .into_bytes(),
        ),
        (
            ".github/workflows/impact.yml".into(),
            include_bytes!("../examples/impact.yml").to_vec(),
        ),
        (
            ".github/workflows/impact-comment.yml".into(),
            include_bytes!("../examples/impact-comment.yml").to_vec(),
        ),
    ];
    files.extend(
        SCANNER
            .iter()
            .map(|(path, bytes)| (format!(".github/cargo-impact/{path}"), bytes.to_vec())),
    );
    // Check every destination before writing anything; a rerun cannot partly overwrite a setup.
    for (path, _) in &files {
        let destination = directory.join(path);
        // Existing symlinks, including dangling ones, are never setup destinations.
        if path != ".gitignore" && fs::symlink_metadata(&destination).is_ok() {
            return Err(io::Error::other(format!(
                "{} already exists",
                directory.join(path).display()
            )));
        }
        for ancestor in destination.ancestors().skip(1) {
            // The caller chooses the root; normal OS aliases such as /var are valid.
            if ancestor == directory {
                break;
            }
            if fs::symlink_metadata(ancestor)
                .is_ok_and(|metadata| !metadata.is_dir() || metadata.file_type().is_symlink())
            {
                return Err(io::Error::other(format!(
                    "setup path ancestor must be a directory without symlinks: {}",
                    ancestor.display()
                )));
            }
        }
    }
    for (path, bytes) in files {
        let path = directory.join(path);
        fs::create_dir_all(
            path.parent()
                .ok_or_else(|| io::Error::other("missing output parent"))?,
        )?;
        if path.file_name().is_some_and(|name| name == ".gitignore") {
            let mut temporary = tempfile::NamedTempFile::new_in(
                path.parent()
                    .ok_or_else(|| io::Error::other("missing output parent"))?,
            )?;
            temporary.write_all(&bytes)?;
            if let Ok(metadata) = fs::metadata(&path) {
                temporary
                    .as_file()
                    .set_permissions(metadata.permissions())?;
            }
            temporary.as_file().sync_all()?;
            temporary.persist(path).map_err(|error| error.error)?;
        } else {
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)?
                .write_all(&bytes)?;
        }
    }
    Ok(())
}
