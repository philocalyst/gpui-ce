use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use cargo_semver_checks::{Check, GlobalConfig, ReleaseType, Rustdoc};
use serde_json::Value;
use tempfile::TempDir;
use thiserror::Error;

use crate::{
    model::{
        BuildResult, Classification, CompilerDiagnostic, DiagnosticLine, DiagnosticSpan,
        DownstreamResult, DownstreamSource, DownstreamSpec, GateResult, ImpactReport,
        ImpactRequest,
    },
    process,
};

#[derive(Debug, Error)]
pub enum ImpactError {
    #[error("failed to prepare impact run: {0}")]
    Io(#[from] std::io::Error),
    #[error("cargo-semver-checks failed: {0}")]
    Semver(String),
    #[error("invalid downstream repository {repository}: {message}")]
    InvalidRepository { repository: String, message: String },
}

pub fn analyze(request: &ImpactRequest) -> Result<ImpactReport, ImpactError> {
    let workspace = match &request.work_dir {
        Some(path) => {
            fs::create_dir_all(path)?;
            WorkArea::Persistent(path.clone())
        }
        None => WorkArea::Temporary(TempDir::new()?),
    };
    let root = workspace.path();
    let baseline = root.join("upstream/baseline");
    let candidate = root.join("upstream/candidate");
    copy_tree(&request.baseline, &baseline)?;
    copy_tree(&request.candidate, &candidate)?;

    let gate = semver_gate(&request.library, &baseline, &candidate)?;
    let downstreams = if gate.ran {
        request
            .downstreams
            .iter()
            .map(|downstream| {
                analyze_downstream(
                    downstream,
                    &request.library,
                    &baseline,
                    &candidate,
                    root,
                    request.timeout,
                )
            })
            .collect::<Result<_, _>>()?
    } else {
        Vec::new()
    };
    Ok(ImpactReport {
        library: request.library.clone(),
        gate,
        downstreams,
    })
}

enum WorkArea {
    Temporary(TempDir),
    Persistent(PathBuf),
}

impl WorkArea {
    fn path(&self) -> &Path {
        match self {
            Self::Temporary(dir) => dir.path(),
            Self::Persistent(path) => path,
        }
    }
}

fn semver_gate(library: &str, baseline: &Path, candidate: &Path) -> Result<GateResult, ImpactError> {
    let mut check = Check::new(Rustdoc::from_root(candidate));
    check
        .set_baseline(Rustdoc::from_root(baseline))
        .set_packages(vec![library.to_owned()])
        // Pin the assumed release type so an upstream version bump cannot hide a break.
        .set_release_type(ReleaseType::Patch);
    let report = check
        .check_release(&mut GlobalConfig::new())
        .map_err(|error| ImpactError::Semver(format!("{error:#}")))?;
    let crate_report = report
        .crate_reports()
        .get(library)
        .ok_or_else(|| ImpactError::Semver(format!("no report returned for crate `{library}`")))?;
    let required_bump = crate_report.required_bump().map(|bump| format!("{bump:?}"));
    let detected_bump = Some(format!("{:?}", crate_report.detected_bump()));
    let ran = required_bump.as_deref() == Some("Major");
    Ok(GateResult {
        ran,
        required_bump,
        detected_bump,
        reason: if ran {
            "cargo-semver-checks found a breaking API change requiring a major version bump".into()
        } else {
            "cargo-semver-checks found no breaking API change requiring downstream checks".into()
        },
    })
}

fn analyze_downstream(
    downstream: &DownstreamSpec,
    library: &str,
    baseline: &Path,
    candidate: &Path,
    work_root: &Path,
    timeout: Duration,
) -> Result<DownstreamResult, ImpactError> {
    let repo_root = checkout(downstream, work_root)?;
    let working_repo = work_root.join("downstreams").join(&downstream.name);
    copy_tree(&repo_root, &working_repo)?;
    let manifest = working_repo.join("Cargo.toml");
    let replacements = match patch_dependencies(&manifest, library, baseline) {
        Ok(replacements) => replacements,
        Err(error) => return Ok(harness_result(downstream, &error.to_string())),
    };
    if replacements == 0 {
        return Ok(harness_result(
            downstream,
            "could not find a dependency declaration for the selected library",
        ));
    }

    // Isolate package fingerprints between repositories. Reuse this target
    // directory for the baseline and candidate builds of the same repository.
    let target_dir = work_root.join("target").join(&downstream.name);
    if let Some(problem) = selected_library(&working_repo, library, baseline, &target_dir, timeout)? {
        return Ok(harness_result(downstream, &problem));
    }
    let baseline_result = cargo_check(&working_repo, &target_dir, timeout)?;
    let replacements = match patch_dependencies(&manifest, library, candidate) {
        Ok(replacements) => replacements,
        Err(error) => return Ok(harness_result(downstream, &error.to_string())),
    };
    if replacements == 0 {
        return Ok(harness_result(
            downstream,
            "dependency declaration disappeared while switching to candidate",
        ));
    }
    if let Some(problem) = selected_library(&working_repo, library, candidate, &target_dir, timeout)? {
        return Ok(harness_result(downstream, &problem));
    }
    let candidate_result = cargo_check(&working_repo, &target_dir, timeout)?;
    let classification = match (baseline_result.success, candidate_result.success) {
        (true, true) => Classification::Compatible,
        (true, false) => Classification::Regression,
        (false, false) if introduces_diagnostic(&baseline_result, &candidate_result) => {
            Classification::Regression
        }
        (false, false) => Classification::PreExistingFailure,
        (false, true) => Classification::Compatible,
    };
    let message = match classification {
        Classification::Regression => Some("baseline compiled and candidate failed".into()),
        Classification::PreExistingFailure => Some("downstream failed with both upstream versions".into()),
        _ => None,
    };
    Ok(DownstreamResult {
        name: downstream.name.clone(),
        classification,
        baseline: baseline_result,
        candidate: candidate_result,
        message,
    })
}

fn checkout(spec: &DownstreamSpec, root: &Path) -> Result<PathBuf, ImpactError> {
    match &spec.source {
        DownstreamSource::Local(path) => Ok(path.canonicalize()?),
        DownstreamSource::Git { url, revision } => {
            let destination = root.join("checkouts").join(&spec.name);
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            let target_dir = root.join("target");
            let clone_args = [
                std::ffi::OsStr::new("clone"),
                std::ffi::OsStr::new("--quiet"),
                std::ffi::OsStr::new("--no-checkout"),
                std::ffi::OsStr::new(url),
                destination.as_os_str(),
            ];
            let clone = process::run("git", &clone_args, root, &target_dir, Duration::from_secs(900))?;
            if !clone.success {
                return Err(ImpactError::InvalidRepository {
                    repository: spec.name.clone(),
                    message: format!("git clone failed: {}", process::write_log(&clone.stdout, &clone.stderr)),
                });
            }
            let checkout_args = [
                std::ffi::OsStr::new("checkout"),
                std::ffi::OsStr::new("--quiet"),
                std::ffi::OsStr::new(revision),
            ];
            let checkout = process::run("git", &checkout_args, &destination, &target_dir, Duration::from_secs(900))?;
            if !checkout.success {
                return Err(ImpactError::InvalidRepository {
                    repository: spec.name.clone(),
                    message: format!("git checkout `{revision}` failed: {}", process::write_log(&checkout.stdout, &checkout.stderr)),
                });
            }
            Ok(destination)
        }
    }
}

fn selected_library(
    repository: &Path,
    library: &str,
    expected_path: &Path,
    target_dir: &Path,
    timeout: Duration,
) -> Result<Option<String>, ImpactError> {
    let manifest = repository.join("Cargo.toml");
    let args = [
        "metadata".as_ref(),
        "--format-version".as_ref(),
        "1".as_ref(),
        "--manifest-path".as_ref(),
        manifest.as_os_str(),
    ];
    let output = process::run("cargo", &args, repository, target_dir, timeout)?;
    if !output.success {
        return Ok(Some(format!("cargo metadata failed: {}", process::write_log(&output.stdout, &output.stderr))));
    }
    let metadata: Value = serde_json::from_slice(&output.stdout).map_err(|error| {
        ImpactError::InvalidRepository {
            repository: repository.display().to_string(),
            message: format!("cargo metadata returned invalid JSON: {error}"),
        }
    })?;
    let expected_path = expected_path.join("Cargo.toml").canonicalize()?;
    let selected = metadata
        .get("packages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|package| {
            package.get("name").and_then(Value::as_str) == Some(library)
                && package
                    .get("manifest_path")
                    .and_then(Value::as_str)
                    .and_then(|path| Path::new(path).canonicalize().ok())
                    .as_ref()
                    == Some(&expected_path)
        });
    Ok((!selected).then(|| {
        format!("Cargo resolved `{library}` from a different package source than the requested upstream tree")
    }))
}

fn cargo_check(repository: &Path, target_dir: &Path, timeout: Duration) -> Result<BuildResult, ImpactError> {
    let manifest = repository.join("Cargo.toml");
    let args = [
        "check".as_ref(),
        "--all-targets".as_ref(),
        "--message-format=json".as_ref(),
        "--manifest-path".as_ref(),
        manifest.as_os_str(),
    ];
    let output = process::run("cargo", &args, repository, target_dir, timeout)?;
    let diagnostics = parse_diagnostics(&output.stdout);
    Ok(BuildResult {
        success: output.success,
        exit_code: output.code,
        timed_out: output.timed_out,
        diagnostics,
        log: process::write_log(&output.stdout, &output.stderr),
    })
}

fn parse_diagnostics(output: &[u8]) -> Vec<CompilerDiagnostic> {
    let parsed: Vec<_> = String::from_utf8_lossy(output)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|message| message.get("reason").and_then(Value::as_str) == Some("compiler-message"))
        .filter_map(|message| message.get("message").cloned())
        .map(|message| CompilerDiagnostic {
            level: message.get("level").and_then(Value::as_str).unwrap_or("unknown").to_owned(),
            code: message
                .get("code")
                .and_then(|code| code.get("code"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            message: message.get("message").and_then(Value::as_str).unwrap_or("").to_owned(),
            rendered: message.get("rendered").and_then(Value::as_str).map(str::to_owned),
            spans: message
                .get("spans")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|span| DiagnosticSpan {
                    file_name: span.get("file_name").and_then(Value::as_str).unwrap_or("").to_owned(),
                    line_start: number(span, "line_start"),
                    column_start: number(span, "column_start"),
                    line_end: number(span, "line_end"),
                    column_end: number(span, "column_end"),
                    is_primary: span.get("is_primary").and_then(Value::as_bool).unwrap_or(false),
                    text: span
                        .get("text")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .map(|line| DiagnosticLine {
                            text: line.get("text").and_then(Value::as_str).unwrap_or("").to_owned(),
                            highlight_start: number(line, "highlight_start"),
                            highlight_end: number(line, "highlight_end"),
                        })
                        .collect(),
                    label: span.get("label").and_then(Value::as_str).map(str::to_owned),
                })
                .collect(),
        })
        .collect();
    let mut seen = HashSet::new();
    parsed
        .into_iter()
        .filter(|diagnostic| {
            let key = format!("{}:{}:{:?}", diagnostic.code.as_deref().unwrap_or(""), diagnostic.message, diagnostic.spans);
            seen.insert(key)
        })
        .collect()
}

fn introduces_diagnostic(baseline: &BuildResult, candidate: &BuildResult) -> bool {
    let baseline: HashSet<_> = baseline.diagnostics.iter().map(diagnostic_key).collect();
    candidate.diagnostics.iter().map(diagnostic_key).any(|key| !baseline.contains(&key))
}

fn diagnostic_key(diagnostic: &CompilerDiagnostic) -> String {
    let primary = diagnostic.spans.iter().find(|span| span.is_primary);
    format!(
        "{}:{}:{}:{}:{}",
        diagnostic.code.as_deref().unwrap_or(""),
        diagnostic.message,
        primary.map(|span| span.file_name.as_str()).unwrap_or(""),
        primary.map(|span| span.line_start).unwrap_or_default(),
        primary.map(|span| span.label.as_deref().unwrap_or("")).unwrap_or("")
    )
}

fn number(value: &Value, field: &str) -> u32 {
    value.get(field).and_then(Value::as_u64).unwrap_or_default() as u32
}

fn harness_result(spec: &DownstreamSpec, message: &str) -> DownstreamResult {
    DownstreamResult {
        name: spec.name.clone(),
        classification: Classification::HarnessFailure,
        baseline: BuildResult::default(),
        candidate: BuildResult::default(),
        message: Some(message.to_owned()),
    }
}

fn patch_dependencies(manifest: &Path, library: &str, upstream: &Path) -> Result<usize, ImpactError> {
    let source = fs::read_to_string(manifest)?;
    let mut value: toml::Value = toml::from_str(&source).map_err(|error| ImpactError::InvalidRepository {
        repository: manifest.display().to_string(),
        message: format!("invalid Cargo.toml: {error}"),
    })?;
    let count = replace_dependency_tables(&mut value, library, upstream);
    if count > 0 {
        fs::write(manifest, toml::to_string_pretty(&value).map_err(|error| ImpactError::InvalidRepository {
            repository: manifest.display().to_string(),
            message: format!("could not write patched Cargo.toml: {error}"),
        })?)?;
    }
    Ok(count)
}

fn replace_dependency_tables(value: &mut toml::Value, library: &str, upstream: &Path) -> usize {
    let mut count = 0;
    let Some(table) = value.as_table_mut() else { return 0 };
    for key in ["dependencies", "dev-dependencies", "build-dependencies"] {
        if let Some(value) = table.get_mut(key) {
            if let Some(dependencies) = value.as_table_mut() {
                for (name, spec) in dependencies.iter_mut() {
                    let is_target = name == library
                        || spec.as_table().and_then(|fields| fields.get("package")).and_then(toml::Value::as_str) == Some(library);
                    if is_target {
                        let mut fields = match spec.clone() {
                            toml::Value::Table(fields) => fields,
                            _ => toml::map::Map::new(),
                        };
                        fields.remove("git");
                        fields.remove("registry");
                        fields.remove("branch");
                        fields.remove("tag");
                        fields.remove("rev");
                        fields.remove("version");
                        fields.insert("path".into(), toml::Value::String(upstream.display().to_string()));
                        *spec = toml::Value::Table(fields);
                        count += 1;
                    }
                }
            }
        }
    }
    for (_, child) in table.iter_mut() {
        count += replace_dependency_tables(child, library, upstream);
    }
    count
}

fn copy_tree(source: &Path, destination: &Path) -> Result<(), std::io::Error> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == ".git" || name == "target" || name == ".cargo-impact" {
            continue;
        }
        let from = entry.path();
        let to = destination.join(name);
        let metadata = fs::symlink_metadata(&from)?;
        if metadata.file_type().is_symlink() {
            continue;
        } else if metadata.is_dir() {
            copy_tree(&from, &to)?;
        } else if metadata.is_file() {
            if let Some(parent) = to.parent() { fs::create_dir_all(parent)?; }
            fs::copy(from, to)?;
        }
    }
    Ok(())
}
