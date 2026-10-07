//! rustdoc executes in the chosen build environment; semver analysis is embedded.

use std::{
    cell::RefCell,
    ffi::OsString,
    io::{self, Write},
    path::{Path, PathBuf},
    rc::Rc,
};

use cargo_metadata::{Package, TargetKind};
use cargo_semver_checks::{Check, GlobalConfig, ReleaseType, Rustdoc};

use crate::{
    cargo,
    model::{GateResult, SemverBump},
    process::Capture,
    runner::Builder,
};

pub(crate) fn compare(
    library: &str,
    baseline: (&Path, &Package),
    candidate: (&Path, &Package),
    builder: &Builder<'_>,
) -> Result<GateResult, String> {
    let baseline_doc = rustdoc(baseline.0, baseline.1, builder).map_err(|e| e.to_string())?;
    let candidate_doc = rustdoc(candidate.0, candidate.1, builder).map_err(|e| e.to_string())?;
    let mut check = Check::new(Rustdoc::from_path(candidate_doc));
    check
        .set_baseline(Rustdoc::from_path(baseline_doc))
        .set_packages(vec![library.into()])
        .set_release_type(ReleaseType::Patch);
    let output = Rc::new(RefCell::new(Vec::new()));
    let mut config = GlobalConfig::new();
    config
        .set_stdout(Box::new(BoundedWriter(output.clone())))
        .set_stderr(Box::new(BoundedWriter(output.clone())))
        .set_out_color_choice(false)
        .set_err_color_choice(false);
    let report = check
        .check_release(&mut config)
        .map_err(|e| format!("{e:#}"))?;
    let report = report
        .crate_reports()
        .get(library)
        .ok_or_else(|| "embedded checker returned no crate report".to_owned())?;
    let bump = match report.required_bump() {
        Some(ReleaseType::Major) => Some(SemverBump::Major),
        Some(ReleaseType::Minor) => Some(SemverBump::Minor),
        Some(ReleaseType::Patch) => Some(SemverBump::Patch),
        _ => None,
    };
    let ran = bump == Some(SemverBump::Major);
    Ok(GateResult {
        ran,
        required_bump: bump,
        reason: if ran {
            "A breaking API change warrants downstream compilation."
        } else {
            "No breaking API change was found for the selected features and target."
        }
        .into(),
        log: String::from_utf8_lossy(&output.borrow()).into_owned(),
    })
}

fn rustdoc(root: &Path, package: &Package, builder: &Builder<'_>) -> io::Result<PathBuf> {
    let target = package
        .targets
        .iter()
        .find(|t| {
            t.kind
                .iter()
                .any(|k| matches!(k, TargetKind::Lib | TargetKind::ProcMacro))
        })
        .ok_or_else(|| io::Error::other("selected package has no Rust library target"))?;
    // Separate output locations keep baseline JSON intact when compiling candidate.
    let phase = if root.file_name().is_some_and(|v| v == "baseline") {
        "baseline"
    } else {
        "candidate"
    };
    let scope = builder.scope.join(phase);
    let builder = Builder {
        target: builder.target.join(phase),
        scope: &scope,
        ..*builder
    };
    let manifest = root.join("Cargo.toml");
    cargo::fetch(&builder, root, &manifest)?;
    let mut args: Vec<OsString> = vec![
        "rustdoc".into(),
        "--offline".into(),
        "--lib".into(),
        "--package".into(),
        package.name.as_str().into(),
        "--manifest-path".into(),
        manifest.into_os_string(),
    ];
    cargo::feature_args(&builder, &mut args);
    if let Some(target) = &builder.recipe.target {
        args.extend(["--target".into(), target.into()]);
    }
    args.extend([
        "--".into(),
        "-Zunstable-options".into(),
        "--output-format=json".into(),
        "--document-private-items".into(),
        "--document-hidden-items".into(),
    ]);
    let output = builder.run(root, &args, Capture::Cargo, false)?;
    if !output.success {
        return Err(io::Error::other(format!(
            "rustdoc generation failed: {}",
            output.log
        )));
    }
    let directory = builder.recipe.target.as_ref().map_or_else(
        || builder.target.clone(),
        |target| builder.target.join(target),
    );
    let file = directory
        .join("doc")
        .join(format!("{}.json", target.name.replace('-', "_")));
    if !file.is_file() {
        return Err(io::Error::other("rustdoc succeeded without producing JSON"));
    }
    if !file
        .canonicalize()?
        .starts_with(builder.target.canonicalize()?)
    {
        return Err(io::Error::other(
            "rustdoc JSON escaped its managed target directory",
        ));
    }
    if file.metadata()?.len() > 128 * 1024 * 1024 {
        return Err(io::Error::other(
            "rustdoc JSON exceeds the 128 MiB evidence limit; narrow the selected features or force controlled downstream checks",
        ));
    }
    Ok(file)
}

struct BoundedWriter(Rc<RefCell<Vec<u8>>>);
impl Write for BoundedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut buffer = self.0.borrow_mut();
        let remaining = (256 * 1024usize).saturating_sub(buffer.len());
        buffer.extend_from_slice(&bytes[..bytes.len().min(remaining)]);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
