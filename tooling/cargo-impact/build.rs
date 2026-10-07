//! Keep `init` self-contained as modules and templates evolve.

use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

fn main() -> io::Result<()> {
    let root =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("Cargo sets manifest directory"));
    let mut files: Vec<PathBuf> = [
        "Cargo.toml",
        "Cargo.lock",
        "build.rs",
        "action.yml",
        "clippy.toml",
        "LICENSE.md",
        "README.md",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect();
    for directory in ["src", "examples", "docs"] {
        collect(&root, Path::new(directory), &mut files)?;
        println!("cargo:rerun-if-changed={directory}");
    }
    files.sort();
    let mut engine = Sha256::new();
    engine.update(b"cargo-impact-engine-v1\0");
    let mut source = String::from("const SCANNER: &[Asset] = &[\n");
    for file in files {
        let path = file
            .to_str()
            .ok_or_else(|| io::Error::other("scanner assets must be UTF-8"))?;
        if !fs::symlink_metadata(root.join(&file))?.is_file() {
            return Err(io::Error::other("scanner assets must be regular files"));
        }
        println!("cargo:rerun-if-changed={path}");
        if engine_input(path) {
            let bytes = fs::read(root.join(&file))?;
            engine.update((path.len() as u64).to_le_bytes());
            engine.update(path.as_bytes());
            engine.update((bytes.len() as u64).to_le_bytes());
            engine.update(bytes);
        }
        source.push_str(&format!("({path:?}, include_bytes!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/\", {path:?})) as &[u8]),\n"));
    }
    source.push_str("];\n");
    println!(
        "cargo:rustc-env=CARGO_IMPACT_ENGINE_SHA256={:x}",
        engine.finalize()
    );
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo sets build output directory"));
    fs::write(output.join("scanner-assets.rs"), source)
}

fn engine_input(path: &str) -> bool {
    path.starts_with("src/engine/")
        || matches!(
            path,
            "Cargo.toml"
                | "Cargo.lock"
                | "build.rs"
                | "src/cargo.rs"
                | "src/config.rs"
                | "src/gate.rs"
                | "src/model.rs"
                | "src/process.rs"
                | "src/runner.rs"
                | "src/source.rs"
                | "src/replay.rs"
        )
}

fn collect(root: &Path, relative: &Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(root.join(relative))? {
        let entry = entry?;
        let path = relative.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            collect(root, &path, files)?;
        } else if kind.is_file() {
            files.push(path);
        } else {
            return Err(io::Error::other(
                "scanner asset tree must not contain symlinks or special files",
            ));
        }
    }
    Ok(())
}
