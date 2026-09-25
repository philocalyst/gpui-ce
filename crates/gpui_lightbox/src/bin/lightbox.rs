//! `lightbox`: builds the Lightbox report from what tests wrote.
//!
//! ```text
//! cargo run -p gpui_ce_lightbox -- report   # contact sheets + target/lightbox/index.html
//! cargo run -p gpui_ce_lightbox -- clean    # forget old suites (benchmark history stays)
//! ```

use anyhow::{Context as _, Result, bail};
use gpui_lightbox::output::root;
use std::fs;

const USAGE: &str = "\
lightbox — visual testing reports for gpui

USAGE:
    lightbox [report]   Build contact sheets and target/lightbox/index.html
    lightbox clean      Remove every suite's output, keeping benchmark history

Output lives in $LIGHTBOX_DIR, or target/lightbox in the workspace.";

fn main() -> Result<()> {
    let command = std::env::args().nth(1);
    match command.as_deref() {
        None | Some("report") => report(),
        Some("clean") => clean(),
        Some("help" | "-h" | "--help") => {
            println!("{USAGE}");
            Ok(())
        }
        Some(other) => bail!("unknown command {other:?}\n\n{USAGE}"),
    }
}

fn report() -> Result<()> {
    let root = root();
    let (index, summary) = gpui_lightbox::report::build(&root)?;
    println!("{summary}");
    for suite in gpui_lightbox::Suite::all(&root)? {
        println!("  sheet  {}", suite.path("sheet.png").display());
    }
    println!("report   file://{}", index.display());
    Ok(())
}

fn clean() -> Result<()> {
    let root = root();
    if !root.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(&root)? {
        let path = entry?.path();
        if path.file_name().is_some_and(|name| name == "bench") {
            continue;
        }
        if path.is_dir() {
            fs::remove_dir_all(&path).with_context(|| format!("removing {}", path.display()))?;
        } else {
            fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
        }
    }
    println!("cleaned {} (benchmark history kept)", root.display());
    Ok(())
}
