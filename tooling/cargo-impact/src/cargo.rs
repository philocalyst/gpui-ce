//! Cargo graph inspection, source injection, and build evidence.

use std::{
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
};

use cargo_metadata::{Metadata, Package};

use crate::{
    model::{BuildResult, DiagnosticOrigin, DiagnosticPackage, HarnessFailure},
    process::Capture,
    runner::Builder,
};

pub(crate) fn metadata(
    builder: &Builder<'_>,
    cwd: &Path,
    manifest: &Path,
    no_deps: bool,
) -> io::Result<Metadata> {
    let mut args = vec![
        "metadata".into(),
        "--format-version=1".into(),
        "--manifest-path".into(),
        manifest.as_os_str().to_owned(),
    ];
    if no_deps {
        args.push("--no-deps".into());
    } else {
        args.push("--offline".into());
        feature_args(builder, &mut args);
        if let Some(target) = &builder.recipe.target {
            args.extend(["--filter-platform".into(), target.into()]);
        }
    }
    let output = builder.run(cwd, &args, Capture::Bytes, false)?;
    if output.timed_out {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "Cargo metadata exhausted the consumer deadline",
        ));
    }
    if output.resource_limited {
        return Err(io::Error::new(
            io::ErrorKind::StorageFull,
            "work directory exceeded its storage budget",
        ));
    }
    if !output.success {
        return Err(io::Error::other(format!(
            "cargo metadata failed: {}",
            output.log
        )));
    }
    if output.data_truncated {
        return Err(io::Error::other("cargo metadata exceeded 16 MiB"));
    }
    serde_json::from_slice(&output.stdout).map_err(io::Error::other)
}

pub(crate) fn fetch(builder: &Builder<'_>, cwd: &Path, manifest: &Path) -> io::Result<()> {
    let mut args = vec![
        "fetch".into(),
        "--manifest-path".into(),
        manifest.as_os_str().to_owned(),
    ];
    if let Some(target) = &builder.recipe.target {
        args.extend(["--target".into(), target.into()]);
    }
    let output = builder.run(cwd, &args, Capture::Bytes, true)?;
    if output.timed_out {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "Cargo fetch exhausted the consumer deadline",
        ));
    }
    if output.resource_limited {
        return Err(io::Error::new(
            io::ErrorKind::StorageFull,
            "work directory exceeded its storage budget",
        ));
    }
    if !output.success {
        return Err(io::Error::other(format!(
            "cargo fetch failed: {}",
            output.log
        )));
    }
    Ok(())
}

pub(crate) fn library_package(metadata: &Metadata, library: &str) -> io::Result<Package> {
    metadata
        .packages
        .iter()
        .find(|p| p.name.as_str() == library && metadata.workspace_members.contains(&p.id))
        .cloned()
        .ok_or_else(|| io::Error::other(format!("workspace contains no package named {library}")))
}

pub(crate) fn selected_library(metadata: &Metadata, library: &str, path: &Path) -> Option<String> {
    metadata
        .packages
        .iter()
        .find(|p| p.name.as_str() == library && p.manifest_path.as_std_path() == path)
        .map(|p| p.id.to_string())
}

pub(crate) fn check(
    builder: &Builder<'_>,
    cwd: &Path,
    manifest: &Path,
    selected: String,
    metadata: &Metadata,
    original: &Path,
) -> io::Result<BuildResult> {
    let mut args = vec![
        "check".into(),
        "--offline".into(),
        "--all-targets".into(),
        "--message-format=json".into(),
        "--manifest-path".into(),
        manifest.as_os_str().to_owned(),
    ];
    feature_args(builder, &mut args);
    for package in &builder.recipe.packages {
        args.extend(["--package".into(), package.into()]);
    }
    if let Some(target) = &builder.recipe.target {
        args.extend(["--target".into(), target.into()]);
    }
    let provenance = builder.provenance(
        cwd,
        &metadata
            .workspace_root
            .join("Cargo.toml")
            .into_std_path_buf(),
    )?;
    let mut output = builder.run(cwd, &args, Capture::Cargo, false)?;
    for diagnostic in &mut output.diagnostics {
        let Some(package) = metadata
            .packages
            .iter()
            .find(|p| p.id.repr == diagnostic.package_id)
        else {
            continue;
        };
        let package_root = package.manifest_path.parent().unwrap().as_std_path();
        let origin = if package_root.starts_with(cwd) {
            DiagnosticOrigin::Downstream
        } else if package_root.starts_with(builder.root.join("upstream")) {
            DiagnosticOrigin::Library
        } else {
            DiagnosticOrigin::Dependency
        };
        diagnostic.package = Some(DiagnosticPackage {
            name: package.name.to_string(),
            manifest: package.manifest_path.clone().into_std_path_buf(),
            origin,
        });
        // Cargo invokes rustc from the workspace directory. Only link files that still
        // match the original checkout, not generated files or modified build-script output.
        map_diagnostic_sources(
            &diagnostic.diagnostic,
            metadata.workspace_root.as_std_path(),
            cwd,
            original,
            &mut diagnostic.source_files,
        );
    }
    let failure = if output.resource_limited {
        Some(HarnessFailure::StorageLimit)
    } else if output.timed_out {
        Some(HarnessFailure::Timeout)
    } else if output.data_truncated {
        Some(HarnessFailure::OutputLimit)
    } else if output
        .diagnostics
        .iter()
        .any(|d| matches!(d.level, cargo_metadata::diagnostic::DiagnosticLevel::Ice))
    {
        Some(HarnessFailure::InternalCompilerError)
    } else if output.diagnostics.iter().any(|d| {
        d.code
            .as_ref()
            .is_some_and(|code| matches!(code.code.as_str(), "E0460" | "E0463" | "E0514" | "E0786"))
    }) || (!output.success
        && !output.diagnostics.iter().any(|d| {
            matches!(d.level, cargo_metadata::diagnostic::DiagnosticLevel::Error)
                && (d.code.is_some() || !d.spans.is_empty())
        }))
    {
        Some(HarnessFailure::Environment)
    } else if output.diagnostics.iter().any(|d| {
        matches!(d.level, cargo_metadata::diagnostic::DiagnosticLevel::Error)
            && d.package
                .as_ref()
                .is_some_and(|p| p.origin == DiagnosticOrigin::Library)
    }) {
        Some(HarnessFailure::LibraryCompilation)
    } else {
        None
    };
    Ok(BuildResult {
        success: output.success,
        exit_code: output.code,
        timed_out: output.timed_out,
        diagnostics: output.diagnostics,
        log: output.log,
        log_truncated: output.log_truncated,
        diagnostics_truncated: output.data_truncated,
        compiled_packages: output.artifacts.into_iter().collect(),
        selected_library: Some(selected),
        failure,
        provenance,
    })
}

fn map_diagnostic_sources(
    diagnostic: &cargo_metadata::diagnostic::Diagnostic,
    rustc_cwd: &Path,
    root: &Path,
    original: &Path,
    files: &mut std::collections::BTreeMap<String, PathBuf>,
) {
    for span in &diagnostic.spans {
        map_span_source(span, rustc_cwd, root, original, files);
    }
    for child in &diagnostic.children {
        map_diagnostic_sources(child, rustc_cwd, root, original, files);
    }
}

fn map_span_source(
    span: &cargo_metadata::diagnostic::DiagnosticSpan,
    rustc_cwd: &Path,
    root: &Path,
    original: &Path,
    files: &mut std::collections::BTreeMap<String, PathBuf>,
) {
    if !files.contains_key(&span.file_name)
        && let Ok(path) = rustc_cwd.join(&span.file_name).canonicalize()
        && let Ok(relative) = path.strip_prefix(root)
        && let Ok(source) = original.join(relative).canonicalize()
        && source.starts_with(original)
        && path.is_file()
        && source.is_file()
        && fs::metadata(&path).is_ok_and(|m| m.len() <= 16 * 1024 * 1024)
        && fs::metadata(&source).is_ok_and(|m| m.len() <= 16 * 1024 * 1024)
        && let (Ok(current), Ok(initial)) = (fs::read(&path), fs::read(source))
        && current == initial
    {
        files.insert(span.file_name.clone(), relative.to_owned());
    }
    if let Some(expansion) = &span.expansion {
        map_span_source(&expansion.span, rustc_cwd, root, original, files);
        if let Some(definition) = &expansion.def_site_span {
            map_span_source(definition, rustc_cwd, root, original, files);
        }
    }
}

pub(crate) fn feature_args(builder: &Builder<'_>, args: &mut Vec<OsString>) {
    if builder.recipe.no_default_features {
        args.push("--no-default-features".into());
    }
    if !builder.recipe.features.is_empty() {
        args.extend([
            "--features".into(),
            builder.recipe.features.join(",").into(),
        ]);
    }
}

/// Rewrite direct sources while preserving feature, optionality, and version requirements.
/// A root patch also reaches transitive registry/git users. Verify actual resolution afterwards.
pub(crate) fn inject(
    root: &Path,
    workspace_manifest: &Path,
    manifests: &[PathBuf],
    library: &str,
    upstream_manifest: &Path,
) -> io::Result<()> {
    let upstream = upstream_manifest
        .parent()
        .ok_or_else(|| io::Error::other("upstream has no parent"))?;
    let mut git_sources = std::collections::BTreeSet::new();
    let mut pending = manifests.to_vec();
    let mut visited = std::collections::BTreeSet::new();
    while let Some(manifest) = pending.pop() {
        let manifest = manifest.canonicalize()?;
        if !manifest.starts_with(root) {
            return Err(io::Error::other(
                "a path dependency escapes the snapshot; configure the containing workspace as the source",
            ));
        }
        if !visited.insert(manifest.clone()) {
            continue;
        }
        let text = fs::read_to_string(&manifest)?;
        let mut value: toml::Value = toml::from_str(&text).map_err(io::Error::other)?;
        inject_tables(
            &mut value,
            library,
            upstream,
            &mut git_sources,
            manifest.parent().unwrap(),
            &mut pending,
        );
        fs::write(
            manifest,
            toml::to_string_pretty(&value).map_err(io::Error::other)?,
        )?;
    }
    let mut value: toml::Value =
        toml::from_str(&fs::read_to_string(workspace_manifest)?).map_err(io::Error::other)?;
    let root = value
        .as_table_mut()
        .ok_or_else(|| io::Error::other("invalid workspace manifest"))?;
    let patches = root
        .entry("patch")
        .or_insert_with(|| toml::Value::Table(Default::default()))
        .as_table_mut()
        .ok_or_else(|| io::Error::other("invalid patch table"))?;
    git_sources.insert("crates-io".into());
    for source in git_sources {
        let table = patches
            .entry(source)
            .or_insert_with(|| toml::Value::Table(Default::default()))
            .as_table_mut()
            .ok_or_else(|| io::Error::other("invalid patch source"))?;
        table.insert(
            library.into(),
            toml::Value::Table(toml::map::Map::from_iter([(
                "path".into(),
                toml::Value::String(upstream.display().to_string()),
            )])),
        );
    }
    fs::write(
        workspace_manifest,
        toml::to_string_pretty(&value).map_err(io::Error::other)?,
    )
}

fn inject_tables(
    value: &mut toml::Value,
    library: &str,
    upstream: &Path,
    git_sources: &mut std::collections::BTreeSet<String>,
    directory: &Path,
    pending: &mut Vec<PathBuf>,
) {
    let Some(table) = value.as_table_mut() else {
        return;
    };
    for key in ["dependencies", "dev-dependencies", "build-dependencies"] {
        let Some(dependencies) = table.get_mut(key).and_then(toml::Value::as_table_mut) else {
            continue;
        };
        for (name, spec) in dependencies {
            if spec.get("workspace").and_then(toml::Value::as_bool) == Some(true) {
                continue;
            }
            let package = spec
                .get("package")
                .and_then(toml::Value::as_str)
                .unwrap_or(name);
            if package != library {
                if let Some(path) = spec.get("path").and_then(toml::Value::as_str) {
                    pending.push(directory.join(path).join("Cargo.toml"));
                }
                continue;
            }
            let mut fields = match spec.clone() {
                toml::Value::Table(table) => table,
                toml::Value::String(version) => {
                    toml::map::Map::from_iter([("version".into(), toml::Value::String(version))])
                }
                _ => continue,
            };
            if let Some(git) = fields
                .remove("git")
                .and_then(|v| v.as_str().map(str::to_owned))
            {
                git_sources.insert(git);
            }
            for key in ["registry", "registry-index", "rev", "tag", "branch"] {
                fields.remove(key);
            }
            fields.insert(
                "path".into(),
                toml::Value::String(upstream.display().to_string()),
            );
            *spec = toml::Value::Table(fields);
        }
    }
    if let Some(workspace) = table.get_mut("workspace") {
        inject_tables(
            workspace,
            library,
            upstream,
            git_sources,
            directory,
            pending,
        );
    }
    if let Some(targets) = table.get_mut("target").and_then(toml::Value::as_table_mut) {
        for (_, target) in targets.iter_mut() {
            inject_tables(target, library, upstream, git_sources, directory, pending);
        }
    }
}

pub(crate) fn normalize_version(manifest: &Path, version: &str) -> io::Result<()> {
    let mut value: toml::Value =
        toml::from_str(&fs::read_to_string(manifest)?).map_err(io::Error::other)?;
    let package = value
        .get_mut("package")
        .and_then(toml::Value::as_table_mut)
        .ok_or_else(|| io::Error::other("invalid upstream package"))?;
    package.insert("version".into(), toml::Value::String(version.into()));
    fs::write(
        manifest,
        toml::to_string_pretty(&value).map_err(io::Error::other)?,
    )
}

pub(crate) fn patch_resolved_sources(
    metadata: &Metadata,
    manifest: &Path,
    library: &str,
    upstream: &Path,
) -> io::Result<bool> {
    let sources: std::collections::BTreeSet<_> = metadata
        .packages
        .iter()
        .filter(|p| p.name.as_str() == library)
        .filter_map(|p| p.source.as_ref())
        .filter_map(|s| {
            let source = s.to_string();
            if let Some(git) = source.strip_prefix("git+") {
                let mut url = url::Url::parse(git).ok()?;
                url.set_query(None);
                url.set_fragment(None);
                Some(url.to_string())
            } else {
                source.strip_prefix("registry+").map(str::to_owned)
            }
        })
        .collect();
    if sources.is_empty() {
        return Ok(false);
    }
    let mut value: toml::Value =
        toml::from_str(&fs::read_to_string(manifest)?).map_err(io::Error::other)?;
    let patches = value
        .as_table_mut()
        .ok_or_else(|| io::Error::other("invalid manifest"))?
        .entry("patch")
        .or_insert_with(|| toml::Value::Table(Default::default()))
        .as_table_mut()
        .ok_or_else(|| io::Error::other("invalid patch table"))?;
    for source in sources {
        let table = patches
            .entry(source)
            .or_insert_with(|| toml::Value::Table(Default::default()))
            .as_table_mut()
            .ok_or_else(|| io::Error::other("invalid patch source"))?;
        table.insert(
            library.into(),
            toml::Value::Table(toml::map::Map::from_iter([(
                "path".into(),
                toml::Value::String(
                    upstream
                        .parent()
                        .ok_or_else(|| io::Error::other("missing upstream directory"))?
                        .display()
                        .to_string(),
                ),
            )])),
        );
    }
    fs::write(
        manifest,
        toml::to_string_pretty(&value).map_err(io::Error::other)?,
    )?;
    Ok(true)
}
