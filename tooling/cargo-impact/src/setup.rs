//! Installation is self-contained even before the scanner is published.

use std::{fs, io, path::Path};

type Asset = (&'static str, &'static [u8]);

macro_rules! assets {
    ($($path:literal),* $(,)?) => { &[$(($path, include_bytes!(concat!("../", $path)) as &[u8])),*] };
}

const SCANNER: &[Asset] = assets![
    "Cargo.toml",
    "Cargo.lock",
    "clippy.toml",
    "LICENSE.md",
    "src/lib.rs",
    "src/main.rs",
    "src/setup.rs",
    "src/bot.rs",
    "src/cargo.rs",
    "src/config.rs",
    "src/discovery.rs",
    "src/engine.rs",
    "src/forge.rs",
    "src/gate.rs",
    "src/http.rs",
    "src/model.rs",
    "src/process.rs",
    "src/report.rs",
    "src/runner.rs",
    "src/source.rs",
    "examples/impact.yml",
    "examples/impact-comment.yml",
];

pub(crate) fn initialize(directory: &Path, library: &str) -> io::Result<()> {
    if library.is_empty()
        || !library
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(io::Error::other("invalid library name"));
    }
    let mut files = vec![
        (
            "impact.toml".to_owned(),
            format!(
                "library = {library:?}\n\n[discovery]\ncrates_io = true\nmax_repositories = 20\n"
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
        if directory.join(path).exists() {
            return Err(io::Error::other(format!(
                "{} already exists",
                directory.join(path).display()
            )));
        }
    }
    for (path, bytes) in files {
        let path = directory.join(path);
        fs::create_dir_all(
            path.parent()
                .ok_or_else(|| io::Error::other("missing output parent"))?,
        )?;
        fs::write(path, bytes)?;
    }
    Ok(())
}
