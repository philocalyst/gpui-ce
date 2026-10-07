//! Cargo graph inspection, source injection, and build evidence.

use std::{
    ffi::OsString,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

use cargo_metadata::{Metadata, Package};

use crate::{
    model::{
        BuildResult, DependencyGraph, DependencyPackage, DiagnosticOrigin, DiagnosticPackage,
        HarnessFailure, LockfileEvidence,
    },
    process::Capture,
    runner::Builder,
    source,
};

pub(crate) fn metadata(
    builder: &Builder<'_>,
    cwd: &Path,
    manifest: &Path,
    no_deps: bool,
) -> io::Result<Metadata> {
    metadata_with_lock(builder, cwd, manifest, no_deps, false)
}

pub(crate) fn metadata_with_lock(
    builder: &Builder<'_>,
    cwd: &Path,
    manifest: &Path,
    no_deps: bool,
    locked: bool,
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
    if locked {
        args.push("--locked".into());
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
    fetch_with_lock(builder, cwd, manifest, false)
}

pub(crate) fn fetch_with_lock(
    builder: &Builder<'_>,
    cwd: &Path,
    manifest: &Path,
    locked: bool,
) -> io::Result<()> {
    let mut args = vec![
        "fetch".into(),
        "--manifest-path".into(),
        manifest.as_os_str().to_owned(),
    ];
    if locked {
        args.push("--locked".into());
    }
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

pub(crate) struct ResolvedBuild {
    pub selected: String,
    pub metadata: Metadata,
    pub injection_sources: Vec<String>,
}

pub(crate) fn check(
    builder: &Builder<'_>,
    cwd: &Path,
    manifest: &Path,
    resolved: &ResolvedBuild,
    original: &Path,
    replay: Option<(crate::BuildPhase, &crate::replay::PhaseIdentity, &str)>,
) -> io::Result<BuildResult> {
    let metadata = &resolved.metadata;
    let selected = &resolved.selected;
    let mut args = vec![
        "check".into(),
        "--offline".into(),
        "--locked".into(),
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
    let lockfile = resolved_lockfile(metadata.workspace_root.as_std_path())?;
    let mut provenance = builder.provenance(cwd)?;
    provenance.lock_fingerprint = lockfile.as_ref().map(|lock| lock.sha256.clone());
    let dependency_graph = dependency_graph(metadata, cwd, builder.root, selected);
    let mut prepared = PreparedCheck {
        selected: selected.clone(),
        evidence: BuildResult {
            selected_library: Some(selected.clone()),
            provenance,
            lockfile,
            dependency_graph,
            injection_sources: resolved.injection_sources.clone(),
            ..Default::default()
        },
    };
    if let Some((phase, expected, library)) = replay
        && let Err(error) = expected.verify_prepared(phase, &prepared.evidence, library)
    {
        prepared.evidence.failure = Some(HarnessFailure::EvidenceMismatch);
        prepared.evidence.log = format!("Replay refused before compilation: {error}");
        return Ok(prepared.evidence);
    }
    check_prepared(builder, cwd, &args, metadata, original, prepared)
}

struct PreparedCheck {
    selected: String,
    evidence: BuildResult,
}

fn check_prepared(
    builder: &Builder<'_>,
    cwd: &Path,
    args: &[OsString],
    metadata: &Metadata,
    original: &Path,
    prepared: PreparedCheck,
) -> io::Result<BuildResult> {
    let PreparedCheck { selected, evidence } = prepared;
    let source_fingerprint = (!builder.recipe.runner.is_isolated())
        .then(|| source::fingerprint(cwd))
        .transpose()?;
    let mut output = builder.run(cwd, args, Capture::Cargo, false)?;
    let source_changed = source_fingerprint
        .is_some_and(|before| source::fingerprint(cwd).map_or(true, |after| before != after));
    if source_changed {
        output.log.push_str("\nThe build changed its consumer source copy; this comparison is inconclusive. Generate files in OUT_DIR or use an isolated recipe.\n");
    }
    let packages: std::collections::BTreeMap<_, _> = metadata
        .packages
        .iter()
        .map(|package| (package.id.repr.as_str(), package))
        .collect();
    let mut source_map = VerifiedSources::new(metadata.workspace_root.as_std_path(), cwd, original);
    for diagnostic in &mut output.diagnostics {
        let Some(package) = packages.get(diagnostic.package_id.as_str()) else {
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
        source_map.map_diagnostic(&diagnostic.diagnostic, &mut diagnostic.source_files);
    }
    let failure = if source_changed {
        Some(HarnessFailure::InputMutation)
    } else if output.resource_limited {
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
        success: output.success && failure.is_none(),
        exit_code: output.code,
        timed_out: output.timed_out,
        diagnostics: output.diagnostics,
        log: output.log,
        log_truncated: output.log_truncated,
        diagnostics_truncated: output.data_truncated,
        compiled_packages: output.artifacts.into_iter().collect(),
        selected_library: Some(selected),
        failure,
        provenance: evidence.provenance,
        lockfile: evidence.lockfile,
        dependency_graph: evidence.dependency_graph,
        injection_sources: evidence.injection_sources,
    })
}

fn resolved_lockfile(workspace: &Path) -> io::Result<Option<LockfileEvidence>> {
    let path = workspace.join("Cargo.lock");
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !metadata.is_file() || metadata.is_symlink() {
        return Err(io::Error::other(
            "resolved Cargo.lock must be a regular file without symlinks",
        ));
    }
    let mut contents = String::new();
    fs::File::open(path)?
        .take(16 * 1024 * 1024 + 1)
        .read_to_string(&mut contents)?;
    if contents.len() > 16 * 1024 * 1024 {
        return Err(io::Error::other("resolved Cargo.lock exceeds 16 MiB"));
    }
    Ok(Some(LockfileEvidence {
        sha256: source::key(contents.as_bytes()),
        contents,
    }))
}

fn dependency_graph(
    metadata: &Metadata,
    cwd: &Path,
    work: &Path,
    selected: &str,
) -> Option<DependencyGraph> {
    let resolved = metadata.resolve.as_ref()?;
    let indexed: std::collections::BTreeMap<_, _> = metadata
        .packages
        .iter()
        .map(|package| (package.id.repr.as_str(), package))
        .collect();
    let mut packages: Vec<_> = resolved
        .nodes
        .iter()
        .filter_map(|node| {
            let package = indexed.get(node.id.repr.as_str())?;
            let mut features: Vec<_> = node.features.iter().map(ToString::to_string).collect();
            features.sort();
            let mut dependencies: Vec<_> = node
                .deps
                .iter()
                .map(|dependency| dependency.pkg.to_string())
                .collect();
            dependencies.sort();
            dependencies.dedup();
            let manifest = package.manifest_path.as_std_path();
            let upstream = work.join("upstream");
            let (origin, relative) = if let Ok(relative) = manifest.strip_prefix(cwd) {
                (DiagnosticOrigin::Downstream, relative)
            } else if let Ok(relative) = manifest.strip_prefix(&upstream) {
                (DiagnosticOrigin::Library, relative)
            } else {
                (DiagnosticOrigin::Dependency, Path::new("Cargo.toml"))
            };
            let identity = source::key(
                &serde_json::to_vec(&(
                    origin,
                    &package.name,
                    &package.version,
                    &package.source,
                    relative,
                ))
                .ok()?,
            );
            Some(DependencyPackage {
                id: node.id.to_string(),
                identity,
                name: package.name.to_string(),
                version: package.version.to_string(),
                source: package.source.as_ref().map(ToString::to_string),
                manifest: relative.to_path_buf(),
                origin,
                features,
                dependencies,
            })
        })
        .collect();
    packages.sort_by(|a, b| a.id.cmp(&b.id));
    let mut roots: Vec<_> = metadata
        .workspace_members
        .iter()
        .map(ToString::to_string)
        .collect();
    roots.sort();
    Some(DependencyGraph {
        roots,
        selected_library: selected.into(),
        packages,
    })
}

/// Repeated errors, macro expansions, and path aliases share one bounded file
/// comparison. Negative mappings are cached too; generated/external sources
/// cannot turn thousands of diagnostics into repeated large file reads.
struct VerifiedSources<'a> {
    rustc_cwd: &'a Path,
    root: &'a Path,
    original: &'a Path,
    names: std::collections::BTreeMap<String, Option<PathBuf>>,
    files: std::collections::BTreeMap<PathBuf, Option<PathBuf>>,
}

impl<'a> VerifiedSources<'a> {
    fn new(rustc_cwd: &'a Path, root: &'a Path, original: &'a Path) -> Self {
        Self {
            rustc_cwd,
            root,
            original,
            names: Default::default(),
            files: Default::default(),
        }
    }

    fn map_diagnostic(
        &mut self,
        diagnostic: &cargo_metadata::diagnostic::Diagnostic,
        files: &mut std::collections::BTreeMap<String, PathBuf>,
    ) {
        for span in &diagnostic.spans {
            self.map_span(span, files);
        }
        for child in &diagnostic.children {
            self.map_diagnostic(child, files);
        }
    }

    fn map_span(
        &mut self,
        span: &cargo_metadata::diagnostic::DiagnosticSpan,
        files: &mut std::collections::BTreeMap<String, PathBuf>,
    ) {
        if let Some(path) = self.lookup(&span.file_name) {
            files.insert(span.file_name.clone(), path);
        }
        if let Some(expansion) = &span.expansion {
            self.map_span(&expansion.span, files);
            if let Some(definition) = &expansion.def_site_span {
                self.map_span(definition, files);
            }
        }
    }

    fn lookup(&mut self, name: &str) -> Option<PathBuf> {
        if let Some(mapped) = self.names.get(name) {
            return mapped.clone();
        }
        let mapped = self
            .rustc_cwd
            .join(name)
            .canonicalize()
            .ok()
            .and_then(|path| {
                if let Some(mapped) = self.files.get(&path) {
                    return mapped.clone();
                }
                let mapped = self.verify(&path);
                self.files.insert(path, mapped.clone());
                mapped
            });
        self.names.insert(name.into(), mapped.clone());
        mapped
    }

    fn verify(&self, path: &Path) -> Option<PathBuf> {
        let relative = path.strip_prefix(self.root).ok()?;
        let source = self.original.join(relative).canonicalize().ok()?;
        (source.starts_with(self.original) && equal_source_files(path, &source).unwrap_or(false))
            .then(|| relative.to_owned())
    }
}

fn equal_source_files(current: &Path, initial: &Path) -> io::Result<bool> {
    const LIMIT: u64 = 16 * 1024 * 1024;
    let current_metadata = fs::metadata(current)?;
    let initial_metadata = fs::metadata(initial)?;
    if !current_metadata.is_file()
        || !initial_metadata.is_file()
        || current_metadata.len() != initial_metadata.len()
        || current_metadata.len() > LIMIT
    {
        return Ok(false);
    }
    let mut current = fs::File::open(current)?.take(LIMIT + 1);
    let mut initial = fs::File::open(initial)?.take(LIMIT + 1);
    let mut current_bytes = [0u8; 64 * 1024];
    let mut initial_bytes = [0u8; 64 * 1024];
    let mut total = 0;
    loop {
        let count = current.read(&mut current_bytes)?;
        total += count as u64;
        if total > LIMIT {
            return Ok(false);
        }
        if count == 0 {
            return Ok(initial.read(&mut initial_bytes[..1])? == 0);
        }
        match initial.read_exact(&mut initial_bytes[..count]) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(false),
            Err(error) => return Err(error),
        }
        if current_bytes[..count] != initial_bytes[..count] {
            return Ok(false);
        }
    }
}

fn read_manifest(path: &Path) -> io::Result<String> {
    const LIMIT: u64 = 8 * 1024 * 1024;
    let mut contents = String::new();
    let file = fs::File::open(path)?;
    if file.metadata()?.len() > LIMIT {
        return Err(io::Error::other(
            "Cargo manifest exceeds 8 MiB; narrow or split the selected workspace",
        ));
    }
    file.take(LIMIT + 1).read_to_string(&mut contents)?;
    if contents.len() as u64 > LIMIT {
        return Err(io::Error::other(
            "Cargo manifest exceeds 8 MiB; narrow or split the selected workspace",
        ));
    }
    Ok(contents)
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
        let text = read_manifest(&manifest)?;
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
        toml::from_str(&read_manifest(workspace_manifest)?).map_err(io::Error::other)?;
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
        toml::from_str(&read_manifest(manifest)?).map_err(io::Error::other)?;
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
) -> io::Result<Vec<String>> {
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
        return Ok(Vec::new());
    }
    let sources: Vec<_> = sources.into_iter().collect();
    apply_source_patches(manifest, &sources, library, upstream)?;
    Ok(sources)
}

pub(crate) fn apply_source_patches(
    manifest: &Path,
    sources: &[String],
    library: &str,
    upstream: &Path,
) -> io::Result<()> {
    if sources.is_empty() {
        return Ok(());
    }
    let mut value: toml::Value =
        toml::from_str(&read_manifest(manifest)?).map_err(io::Error::other)?;
    let patches = value
        .as_table_mut()
        .ok_or_else(|| io::Error::other("invalid manifest"))?
        .entry("patch")
        .or_insert_with(|| toml::Value::Table(Default::default()))
        .as_table_mut()
        .ok_or_else(|| io::Error::other("invalid patch table"))?;
    for source in sources {
        let table = patches
            .entry(source.clone())
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
    Ok(())
}
