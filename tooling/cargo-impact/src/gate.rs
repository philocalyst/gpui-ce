//! rustdoc executes in the chosen build environment; semver analysis is embedded.

use std::{
    cell::RefCell,
    ffi::OsString,
    io::{self, Write},
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant},
};

use cargo_metadata::{Package, TargetKind};
use cargo_semver_checks::{Check, GlobalConfig, ReleaseType, Rustdoc};

use crate::{
    cargo,
    model::{GateResult, SemverBump},
    process::{self, Capture},
    runner::{Builder, clean_command},
};

pub(crate) fn compare(
    library: &str,
    baseline: (&Path, &Package),
    candidate: (&Path, &Package),
    builder: &Builder<'_>,
    helper: Option<&Path>,
) -> Result<GateResult, String> {
    let baseline_doc = rustdoc(baseline.0, baseline.1, builder).map_err(|e| e.to_string())?;
    let candidate_doc = rustdoc(candidate.0, candidate.1, builder).map_err(|e| e.to_string())?;
    if let Some(helper) = helper {
        return compare_in_helper(library, &baseline_doc, &candidate_doc, builder, helper);
    }
    compare_docs(library, &baseline_doc, &candidate_doc)
}

/// Entry point for an application's hidden `__semver-helper` command. The host
/// should serialize the successful result to stdout and exit nonzero on errors.
/// Analysis remains embedded; the supervisor can terminate this separate process.
pub fn run_semver_helper(
    library: &str,
    baseline_doc: &Path,
    candidate_doc: &Path,
    memory_mib: u64,
) -> Result<GateResult, String> {
    if memory_mib < 128 {
        return Err("semver helper requires at least 128 MiB".into());
    }
    #[cfg(target_os = "linux")]
    {
        let bytes = memory_mib
            .checked_mul(1024 * 1024)
            .ok_or("memory limit overflow")?;
        let limit = libc::rlimit {
            rlim_cur: bytes as libc::rlim_t,
            rlim_max: bytes as libc::rlim_t,
        };
        // This applies only inside the disposable helper, never the scanner.
        if unsafe { libc::setrlimit(libc::RLIMIT_AS, &limit) } != 0 {
            return Err(format!(
                "setting semver memory limit: {}",
                io::Error::last_os_error()
            ));
        }
    }
    for path in [baseline_doc, candidate_doc] {
        if !path.is_file() || path.metadata().map_err(|e| e.to_string())?.len() > 128 * 1024 * 1024
        {
            return Err("semver rustdoc input must be a file of at most 128 MiB".into());
        }
    }
    compare_docs(library, baseline_doc, candidate_doc)
}

fn compare_in_helper(
    library: &str,
    baseline: &Path,
    candidate: &Path,
    builder: &Builder<'_>,
    helper: &Path,
) -> Result<GateResult, String> {
    let remaining = builder.deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err("semver gate deadline exceeded".into());
    }
    let mut command = clean_command(helper.as_os_str());
    command
        .current_dir(builder.root)
        .arg("__semver-helper")
        .arg("--library")
        .arg(library)
        .arg("--baseline-doc")
        .arg(baseline)
        .arg("--candidate-doc")
        .arg(candidate)
        .arg("--memory-mib")
        .arg(builder.execution.memory_mib.to_string());
    let memory_bytes = builder
        .execution
        .memory_mib
        .checked_mul(1024 * 1024)
        .ok_or("memory limit overflow")?;
    let mut last = None;
    let mut reason = "memory budget";
    let output = process::run_guarded_pid(&mut command, remaining, Capture::Bytes, |pid| {
        if last.is_some_and(|last: Instant| last.elapsed() < Duration::from_secs(1)) {
            return Ok(false);
        }
        last = Some(Instant::now());
        if let Some(budget) = builder.execution.max_work_bytes
            && crate::engine::work_bytes(builder.root)? > budget
        {
            reason = "work directory storage budget";
            return Ok(true);
        }
        Ok(helper_memory_bytes(pid)?.is_some_and(|bytes| bytes > memory_bytes))
    })
    .map_err(|e| e.to_string())?;
    if output.timed_out {
        return Err("embedded semver analysis exceeded the gate or scan deadline".into());
    }
    if output.resource_limited {
        return Err(format!("embedded semver analysis exceeded its {reason}"));
    }
    if !output.success {
        return Err(format!("embedded semver helper failed: {}", output.log));
    }
    if output.data_truncated {
        return Err("embedded semver helper output exceeded its evidence limit".into());
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("invalid embedded semver helper response: {e}"))
}

fn helper_memory_bytes(pid: u32) -> io::Result<Option<u64>> {
    #[cfg(target_os = "linux")]
    {
        let status = match std::fs::read_to_string(format!("/proc/{pid}/status")) {
            Ok(status) => status,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        Ok(status
            .lines()
            .find_map(|line| {
                line.strip_prefix("VmRSS:")?
                    .split_whitespace()
                    .next()?
                    .parse::<u64>()
                    .ok()
            })
            .map(|kib| kib.saturating_mul(1024)))
    }
    #[cfg(target_os = "macos")]
    {
        let output = process::run(
            clean_command("/bin/ps").args(["-o", "rss=", "-p", &pid.to_string()]),
            Duration::from_secs(1),
            Capture::Bytes,
        )?;
        Ok(String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse::<u64>()
            .ok()
            .map(|kib| kib.saturating_mul(1024)))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = pid;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "bounded semver memory supervision requires Linux or macOS",
        ))
    }
}

fn compare_docs(
    library: &str,
    baseline_doc: &Path,
    candidate_doc: &Path,
) -> Result<GateResult, String> {
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
    let source_fingerprint = (!builder.recipe.runner.is_isolated())
        .then(|| crate::source::fingerprint(root))
        .transpose()?;
    let mut args: Vec<OsString> = vec![
        "rustdoc".into(),
        "--offline".into(),
        "--locked".into(),
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
    if let Some(before) = source_fingerprint
        && crate::source::fingerprint(root)? != before
    {
        return Err(io::Error::other(
            "rustdoc execution changed its upstream source copy; the API gate is inconclusive. Generate files in OUT_DIR or use an isolated recipe.",
        ));
    }
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::{model::ExecutionOptions, runner::BuildRecipe};
    use std::{collections::BTreeMap, fs, os::unix::fs::PermissionsExt, sync::Mutex};

    fn supervised(
        script: &str,
        timeout: Duration,
        budget: Option<u64>,
    ) -> Result<GateResult, String> {
        let temporary = tempfile::TempDir::new().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let helper = root.join("helper");
        fs::write(&helper, format!("#!/bin/sh\n{script}\n")).unwrap();
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o755)).unwrap();
        let recipe = BuildRecipe::default();
        let execution = ExecutionOptions {
            max_work_bytes: budget,
            ..Default::default()
        };
        let images = Mutex::new(BTreeMap::new());
        let builder = Builder {
            recipe: &recipe,
            root: &root,
            target: root.join("target"),
            timeout,
            deadline: Instant::now() + timeout,
            scope: &root,
            execution: &execution,
            images: &images,
        };
        compare_in_helper(
            "example",
            &root.join("base.json"),
            &root.join("head.json"),
            &builder,
            &helper,
        )
    }

    #[test]
    fn embedded_helper_is_killable_and_keeps_a_structured_response() {
        let report = supervised(r#"printf '%s' '{"ran":true,"required_bump":"major","reason":"break","log":"evidence"}'"#, Duration::from_secs(5), None).unwrap();
        assert!(report.ran);
        assert_eq!(report.log, "evidence");
        let started = Instant::now();
        let error = supervised("sleep 30", Duration::from_millis(300), None).unwrap_err();
        assert!(error.contains("deadline"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(3));
        let error = supervised("printf garbage", Duration::from_secs(5), None).unwrap_err();
        assert!(
            error.contains("invalid embedded semver helper response"),
            "{error}"
        );
    }

    #[test]
    fn embedded_helper_cannot_outlive_a_storage_budget_failure() {
        let started = Instant::now();
        let error = supervised(
            "dd if=/dev/zero of=large bs=1024 count=1024 2>/dev/null\nsleep 30",
            Duration::from_secs(10),
            Some(512 * 1024),
        )
        .unwrap_err();
        assert!(error.contains("storage budget"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(4));
    }
}
