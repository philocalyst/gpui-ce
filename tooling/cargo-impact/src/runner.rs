//! Build environments form one explicit boundary. HTTP tokens never cross it.

use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

use crate::model::{BuildProvenance, ExecutionOptions};
use crate::process::{self, Capture, ProcessOutput};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BuildRecipe {
    pub runner: Runner,
    pub driver: CargoDriver,
    pub features: Vec<String>,
    pub no_default_features: bool,
    pub target: Option<String>,
    pub packages: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CargoDriver {
    #[default]
    Cargo,
    Boxington,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Runner {
    Docker {
        image: String,
    },
    Local,
    /// Supplies a derivation's build environment; local execution requires explicit opt-in.
    Nix {
        file: PathBuf,
        #[serde(default)]
        attribute: String,
    },
}

impl Default for Runner {
    fn default() -> Self {
        Self::Docker {
            image: "rust:1.99.0-bookworm".into(),
        }
    }
}

impl Runner {
    pub fn is_isolated(&self) -> bool {
        matches!(self, Self::Docker { .. })
    }
}

pub(crate) struct Builder<'a> {
    pub recipe: &'a BuildRecipe,
    pub root: &'a Path,
    pub target: PathBuf,
    pub timeout: Duration,
    pub deadline: Instant,
    pub scope: &'a Path,
    pub execution: &'a ExecutionOptions,
    pub images: &'a Mutex<BTreeMap<String, String>>,
}

static CONTAINER_SEQUENCE: AtomicU64 = AtomicU64::new(0);

impl Builder<'_> {
    pub fn run(
        &self,
        cwd: &Path,
        args: &[OsString],
        capture: Capture,
        network: bool,
    ) -> std::io::Result<ProcessOutput> {
        let driver = if matches!(self.recipe.driver, CargoDriver::Boxington)
            && args.first().is_some_and(|v| v == "check")
        {
            "mbx"
        } else {
            "cargo"
        };
        let mut controlled: Vec<_> = args.iter().take(1).cloned().collect();
        // Command-line configuration wins over project Cargo configuration,
        // including [env] force=true. Only the explicit recipe driver may wrap
        // the trusted compiler; source-controlled replacements cannot choose it.
        for setting in [
            "build.rustc=\"rustc\"",
            "build.rustdoc=\"rustdoc\"",
            "build.rustc-wrapper=\"\"",
            "build.rustc-workspace-wrapper=\"\"",
        ] {
            controlled.extend(["--config".into(), setting.into()]);
        }
        // Scalar [env] entries already defer to the worker environment. A
        // dotted override would turn those valid strings into conflicting
        // tables, so only override force on existing forced table entries.
        for key in forced_environment(cwd, self.recipe.runner.is_isolated())? {
            controlled.extend(["--config".into(), format!("env.{key}.force=false").into()]);
        }
        // Cargo rustdoc contains `--` followed by rustdoc flags. Global Cargo
        // policy must precede that separator rather than becoming rustdoc args.
        controlled.extend(args.iter().skip(1).cloned());
        self.run_program(cwd, driver, &controlled, capture, network)
    }

    fn remaining(&self) -> std::io::Result<Duration> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "consumer experiment deadline exceeded",
            ));
        }
        Ok(remaining.min(self.timeout))
    }

    fn run_program(
        &self,
        cwd: &Path,
        driver: &str,
        args: &[OsString],
        capture: Capture,
        network: bool,
    ) -> std::io::Result<ProcessOutput> {
        self.remaining()?;
        let cached = self.scope.join("cache");
        for path in [
            &cached,
            &cached.join("cargo"),
            &cached.join("mbx"),
            &cached.join("mbx-shims"),
            &self.scope.join("home"),
            &self.target,
        ] {
            safe_directory(self.root, path)?;
        }
        // Builds may write their own Cargo home, but cannot install a config for the
        // next invocation. Registry/Git source trees are mounted read-only offline.
        for filename in ["config", "config.toml", "credentials", "credentials.toml"] {
            let path = cached.join("cargo").join(filename);
            if fs::symlink_metadata(&path).is_ok() {
                fs::remove_file(path)?;
            }
        }
        let name = format!(
            "cargo-impact-{}-{}",
            std::process::id(),
            CONTAINER_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        );
        let mut command = match &self.recipe.runner {
            Runner::Local => clean_command(driver),
            Runner::Nix { file, attribute } => {
                let mut cmd = clean_command("nix");
                if let Some(search_path) = std::env::var_os("NIX_PATH") {
                    cmd.env("NIX_PATH", search_path);
                }
                cmd.args(["develop", "--file"])
                    .arg(file)
                    .arg(attribute)
                    .args(["--command", driver]);
                cmd
            }
            Runner::Docker { image } => {
                let image_id = pin_image(image, self.images, self.deadline)?;
                let mut cmd = clean_command("docker");
                cmd.args([
                    "run",
                    "--rm",
                    "--name",
                    &name,
                    "--read-only",
                    "--cap-drop=ALL",
                    "--security-opt=no-new-privileges",
                    "--pids-limit=512",
                    "--tmpfs=/tmp:rw,nosuid,size=1g",
                ])
                .arg(format!("--memory={}m", self.execution.memory_mib))
                .arg(format!("--cpus={}", self.execution.cpus))
                .args(["--network", if network { "bridge" } else { "none" }])
                .arg("--workdir")
                .arg(cwd);
                self.append_mounts(&mut cmd, cwd, &cached, network)?;
                #[cfg(unix)]
                unsafe {
                    cmd.arg("--user")
                        .arg(format!("{}:{}", libc::getuid(), libc::getgid()));
                }
                for (key, value) in self.environment(&cached) {
                    cmd.arg("--env")
                        .arg(format!("{key}={}", value.to_string_lossy()));
                }
                if args.first().is_some_and(|v| v == "rustdoc") {
                    cmd.args(["--env", "RUSTC_BOOTSTRAP=1"]);
                }
                cmd.arg(image_id).arg(driver);
                cmd
            }
        };
        if !self.recipe.runner.is_isolated() {
            for (key, value) in self.environment(&cached) {
                command.env(key, value);
            }
        }
        if args.first().is_some_and(|v| v == "rustdoc") && !self.recipe.runner.is_isolated() {
            // Stable rustdoc exposes JSON through this flag; do not relax downstream checks.
            command.env("RUSTC_BOOTSTRAP", "1");
        }
        command.args(args).current_dir(cwd);
        let timeout = self.remaining()?;
        let mut checked = None;
        let result = process::run_guarded(&mut command, timeout, capture, || {
            let Some(budget) = self.execution.max_work_bytes else {
                return Ok(false);
            };
            if checked.is_some_and(|last: Instant| last.elapsed() < Duration::from_secs(2)) {
                return Ok(false);
            }
            checked = Some(Instant::now());
            Ok(crate::engine::work_bytes(self.root)? > budget)
        });
        if self.recipe.runner.is_isolated() {
            // Killing the Docker CLI does not kill its container. Always reap the container too.
            let mut cleanup = clean_command("docker");
            cleanup.args(["rm", "--force", &name]);
            let _ = process::run(&mut cleanup, Duration::from_secs(20), Capture::Bytes);
        }
        result
    }

    fn environment(&self, cached: &Path) -> Vec<(&'static str, OsString)> {
        vec![
            ("RUSTC", "rustc".into()),
            ("RUSTDOC", "rustdoc".into()),
            ("RUSTC_WRAPPER", "".into()),
            ("RUSTC_WORKSPACE_WRAPPER", "".into()),
            ("CARGO_HOME", cached.join("cargo").into_os_string()),
            ("CARGO_TARGET_DIR", self.target.clone().into_os_string()),
            ("CARGO_TERM_COLOR", "never".into()),
            ("CARGO_INCREMENTAL", "1".into()),
            ("CARGO_PROFILE_DEV_DEBUG", "0".into()),
            (
                "CARGO_BUILD_JOBS",
                self.execution.cargo_jobs.to_string().into(),
            ),
            ("MBX_CACHE_DIR", cached.join("mbx").into_os_string()),
            ("MBX_SHIMS_DIR", cached.join("mbx-shims").into_os_string()),
            ("MBX_DISPLAY", "plain".into()),
            ("MBX_TARGET_VIEWS", "0".into()),
            ("HOME", self.scope.join("home").into_os_string()),
        ]
    }

    fn append_mounts(
        &self,
        command: &mut Command,
        cwd: &Path,
        cached: &Path,
        network: bool,
    ) -> std::io::Result<()> {
        // Never expose the scan root or another worker. Upstream sources are
        // immutable inputs; this worker owns its scratch and phase caches.
        // Fetch can create the lockfile, but build/rustdoc phases never get
        // writable source. A build script must use OUT_DIR for generated files.
        mount(command, self.root, cwd, !network)?;
        for path in [
            self.target.as_path(),
            cached,
            self.scope.join("home").as_path(),
        ] {
            mount(command, self.root, path, false)?;
        }
        let upstream = self.root.join("upstream");
        if upstream.is_dir() && !cwd.starts_with(&upstream) {
            mount(command, self.root, &upstream, true)?;
        }
        if !network {
            for directory in ["registry", "git"] {
                let sources = cached.join("cargo").join(directory);
                if sources.is_dir() {
                    mount(command, self.root, &sources, true)?;
                }
            }
        }
        Ok(())
    }

    pub(crate) fn provenance(&self, cwd: &Path) -> std::io::Result<BuildProvenance> {
        let version = |program: &str, arguments: &[&str]| -> std::io::Result<Option<String>> {
            let args: Vec<_> = arguments.iter().map(OsString::from).collect();
            let output = self.run_program(cwd, program, &args, Capture::Bytes, false)?;
            Ok(output
                .success
                .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned()))
        };
        let (runner_identity, image_id, image_reference) = match &self.recipe.runner {
            Runner::Docker { image } => {
                let id = pin_image(image, self.images, self.deadline)?;
                let reference = image_reference(&id, self.deadline)?;
                (format!("{image} ({id})"), Some(id), reference)
            }
            Runner::Local => ("local".into(), None, None),
            Runner::Nix { file, attribute } => {
                (format!("nix:{}#{attribute}", file.display()), None, None)
            }
        };
        Ok(BuildProvenance {
            rustc: version("rustc", &["-Vv"])?,
            cargo: version("cargo", &["--version"])?,
            runner_identity,
            lock_fingerprint: None,
            image_id,
            image_reference,
        })
    }
}

const CONTROLLED_ENV: &[&str] = &[
    "RUSTC",
    "RUSTDOC",
    "RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER",
    "PATH",
    "HOME",
];

/// Follow Cargo's config/include precedence without logging configuration
/// values, which may contain credentials. Managed Cargo homes are cleared
/// before each invocation; Docker only exposes the source directory.
fn forced_environment(cwd: &Path, isolated: bool) -> std::io::Result<Vec<String>> {
    let mut environment = BTreeMap::new();
    let mut ancestors: Vec<_> = cwd
        .ancestors()
        .take(if isolated { 1 } else { usize::MAX })
        .collect();
    ancestors.reverse();
    let mut remaining_files = 64usize;
    for ancestor in ancestors {
        let config = ancestor.join(".cargo/config");
        let config = if config.exists() {
            config
        } else {
            ancestor.join(".cargo/config.toml")
        };
        load_environment(&config, true, &mut remaining_files, &mut environment)?;
    }
    Ok(environment
        .into_iter()
        .filter_map(|(key, value): (String, toml::Value)| {
            (value.get("force").and_then(toml::Value::as_bool) == Some(true)).then_some(key)
        })
        .collect())
}

fn load_environment(
    config: &Path,
    optional: bool,
    remaining_files: &mut usize,
    environment: &mut BTreeMap<String, toml::Value>,
) -> std::io::Result<()> {
    let file = match fs::File::open(config) {
        Ok(file) => file,
        Err(error) if optional && error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    *remaining_files = remaining_files.checked_sub(1).ok_or_else(|| {
        std::io::Error::other("Cargo config/include exceeds 64 files or contains a cycle")
    })?;
    let mut contents = String::new();
    file.take(1024 * 1024 + 1).read_to_string(&mut contents)?;
    if contents.len() > 1024 * 1024 {
        return Err(std::io::Error::other("Cargo configuration exceeds 1 MiB"));
    }
    let value: toml::Value = contents.parse().map_err(|_| {
        std::io::Error::other(format!(
            "invalid Cargo configuration in {}",
            config.display()
        ))
    })?;
    if let Some(includes) = value.get("include") {
        for include in includes
            .as_array()
            .ok_or_else(|| std::io::Error::other("Cargo config include must be an array"))?
        {
            let path = include
                .as_str()
                .or_else(|| include.get("path").and_then(toml::Value::as_str))
                .ok_or_else(|| std::io::Error::other("Cargo config include requires a path"))?;
            let optional = include
                .get("optional")
                .and_then(toml::Value::as_bool)
                .unwrap_or(false);
            load_environment(
                &config.parent().unwrap().join(path),
                optional,
                remaining_files,
                environment,
            )?;
        }
    }
    if let Some(values) = value.get("env").and_then(toml::Value::as_table) {
        for key in CONTROLLED_ENV {
            if let Some(value) = values.get(*key) {
                match (environment.get_mut(*key), value) {
                    (Some(toml::Value::Table(previous)), toml::Value::Table(next)) => {
                        previous.extend(next.clone())
                    }
                    _ => {
                        environment.insert((*key).into(), value.clone());
                    }
                }
            }
        }
    }
    Ok(())
}

fn pin_image(
    image: &str,
    images: &Mutex<BTreeMap<String, String>>,
    deadline: Instant,
) -> std::io::Result<String> {
    pin_image_with_pull(image, images, deadline, true)
}

fn pin_image_with_pull(
    image: &str,
    images: &Mutex<BTreeMap<String, String>>,
    deadline: Instant,
    allow_pull: bool,
) -> std::io::Result<String> {
    let mut images = images
        .lock()
        .map_err(|_| std::io::Error::other("worker image cache poisoned"))?;
    if deadline <= Instant::now() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "worker image preparation exhausted the consumer deadline",
        ));
    }
    if let Some(image_id) = images.get(image) {
        return Ok(image_id.clone());
    }
    let inspect = || -> std::io::Result<ProcessOutput> {
        let mut command = clean_command("docker");
        command.args(["image", "inspect", "--format", "{{.Id}}", image]);
        process::run(
            &mut command,
            deadline.saturating_duration_since(Instant::now()),
            Capture::Bytes,
        )
    };
    let mut output = inspect()?;
    if output.timed_out {
        return Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "worker image inspection exhausted the consumer deadline",
        ));
    }
    if !output.success {
        if !allow_pull {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "immutable local image is unavailable; image config IDs cannot be pulled from a registry",
            ));
        }
        let mut pull = clean_command("docker");
        pull.args(["pull", image]);
        let pulled = process::run(
            &mut pull,
            deadline.saturating_duration_since(Instant::now()),
            Capture::Bytes,
        )?;
        if pulled.timed_out {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "worker image preparation exhausted the consumer deadline",
            ));
        }
        if !pulled.success {
            return Err(std::io::Error::other(format!(
                "could not prepare worker image {image}: {}",
                pulled.log
            )));
        }
        output = inspect()?;
    }
    let image_id = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !output.success
        || image_id.strip_prefix("sha256:").is_none_or(|digest| {
            digest.len() != 64 || !digest.chars().all(|c| c.is_ascii_hexdigit())
        })
    {
        return Err(std::io::Error::other(
            "Docker returned no immutable worker image ID",
        ));
    }
    images.insert(image.to_owned(), image_id.clone());
    Ok(image_id)
}

pub(crate) fn immutable_image_reference(reference: &str) -> bool {
    let Some((repository, digest)) = reference.rsplit_once("@sha256:") else {
        return false;
    };
    !repository.is_empty()
        && repository.len() <= 255
        && !repository.starts_with('-')
        && repository
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"/.:_-".contains(&byte))
        && digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn image_reference(image: &str, deadline: Instant) -> std::io::Result<Option<String>> {
    let mut command = clean_command("docker");
    command.args([
        "image",
        "inspect",
        "--format",
        "{{json .RepoDigests}}",
        image,
    ]);
    let output = process::run(
        &mut command,
        deadline.saturating_duration_since(Instant::now()),
        Capture::Bytes,
    )?;
    if output.timed_out {
        return Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "Docker image provenance exhausted the consumer deadline",
        ));
    }
    if !output.success || output.data_truncated {
        return Err(std::io::Error::other(
            "Docker could not retain image repository digests",
        ));
    }
    let mut digests: Vec<String> = serde_json::from_slice::<Option<Vec<String>>>(&output.stdout)
        .map_err(|_| std::io::Error::other("Docker returned invalid image repository digests"))?
        .unwrap_or_default();
    if digests
        .iter()
        .any(|reference| !immutable_image_reference(reference))
    {
        return Err(std::io::Error::other(
            "Docker returned a non-immutable image repository reference",
        ));
    }
    digests.sort();
    Ok(digests.into_iter().next())
}

pub(crate) fn prepare_replay_image(
    configured: &str,
    expected: &crate::replay::PhaseIdentity,
    images: &Mutex<BTreeMap<String, String>>,
    deadline: Instant,
) -> std::io::Result<String> {
    let reference = expected
        .image_reference
        .as_deref()
        .unwrap_or(&expected.image);
    let actual = pin_image_with_pull(reference, images, deadline, expected.image_reference.is_some())
        .map_err(|error| match &expected.image_reference {
            Some(_) => error,
            None => std::io::Error::new(
                error.kind(),
                format!(
                    "Replay requires the locally built image {} to be present on this daemon; the original report has no registry-pullable digest. {error}",
                    expected.image
                ),
            ),
        })?;
    if actual == expected.image {
        images
            .lock()
            .map_err(|_| std::io::Error::other("worker image cache poisoned"))?
            .insert(configured.into(), actual.clone());
    }
    Ok(actual)
}

fn mount(command: &mut Command, root: &Path, path: &Path, readonly: bool) -> std::io::Result<()> {
    if !path.starts_with(root) || path.canonicalize()? != path {
        return Err(std::io::Error::other(
            "Docker mount must be an owned directory without symlink components",
        ));
    }
    let path = path
        .to_str()
        .ok_or_else(|| std::io::Error::other("Docker mount paths must be UTF-8"))?;
    if path.contains(',') {
        return Err(std::io::Error::other(
            "Docker mount paths must not contain commas",
        ));
    }
    command.arg("--mount").arg(format!(
        "type=bind,source={path},target={path}{}",
        if readonly { ",readonly" } else { "" }
    ));
    Ok(())
}

pub(crate) fn safe_directory(root: &Path, path: &Path) -> std::io::Result<()> {
    let relative = path.strip_prefix(root).map_err(std::io::Error::other)?;
    let mut current = root.to_owned();
    for component in relative.components() {
        if !matches!(component, std::path::Component::Normal(_)) {
            return Err(std::io::Error::other("unsafe worker path"));
        }
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if !metadata.is_dir() || metadata.is_symlink() => {
                return Err(std::io::Error::other(
                    "worker directories must not contain symlinks or files",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match fs::create_dir(&current) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        let metadata = fs::symlink_metadata(&current)?;
                        if !metadata.is_dir() || metadata.is_symlink() {
                            return Err(std::io::Error::other(
                                "worker directory was replaced by a symlink or file",
                            ));
                        }
                    }
                    Err(error) => return Err(error),
                }
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Preserve only tools and rustup's public installation location, never the caller's token environment.
pub(crate) fn clean_command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(program);
    command.env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    if let Some(home) = std::env::var_os("HOME") {
        let rustup = std::env::var_os("RUSTUP_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(home).join(".rustup"));
        command.env("RUSTUP_HOME", rustup);
    }
    if let Some(toolchain) = std::env::var_os("RUSTUP_TOOLCHAIN") {
        command.env("RUSTUP_TOOLCHAIN", toolchain);
    }
    command.env("LANG", "C.UTF-8");
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_image_requires_a_pullable_digest_or_explicit_local_availability() {
        let reference = format!("ghcr.io/example/worker@sha256:{}", "a".repeat(64));
        assert!(immutable_image_reference(&reference));
        for invalid in [
            "rust:latest",
            "--platform=other@sha256:abc",
            "https://token@host/image@sha256:abc",
        ] {
            assert!(!immutable_image_reference(invalid));
        }
        let expected = crate::replay::PhaseIdentity {
            image: format!("sha256:{}", "b".repeat(64)),
            image_reference: Some(reference.clone()),
            lockfile: String::new(),
            dependency_graph: String::new(),
            rustc: String::new(),
            cargo: String::new(),
            injection_sources: Vec::new(),
        };
        // The repository digest is the acquisition key; .Id remains the proof.
        let images = Mutex::new(BTreeMap::from([(reference, expected.image.clone())]));
        assert_eq!(
            prepare_replay_image(
                "mutable-tag",
                &expected,
                &images,
                Instant::now() + Duration::from_secs(1)
            )
            .unwrap(),
            expected.image
        );
        assert_eq!(
            images.lock().unwrap().get("mutable-tag"),
            Some(&expected.image)
        );
        let mut local = expected;
        local.image_reference = None;
        let images = Mutex::new(BTreeMap::from([(local.image.clone(), local.image.clone())]));
        assert_eq!(
            prepare_replay_image(
                "local-image",
                &local,
                &images,
                Instant::now() + Duration::from_secs(1)
            )
            .unwrap(),
            local.image
        );
    }

    #[test]
    fn compiler_policy_follows_includes_legacy_precedence_and_scalar_types() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let cargo = root.join(".cargo");
        fs::create_dir(&cargo).unwrap();
        fs::write(
            cargo.join("included.toml"),
            "[env]\nRUSTC={value='replacement', force=true}\nRUSTDOC={value='replacement', force=true}\n",
        ).unwrap();
        fs::write(
            cargo.join("config"),
            "include=['included.toml', {path='absent.toml', optional=true}]\n[env]\nRUSTC={value='other'}\nRUSTC_WRAPPER='replacement'\nPATH={value='/missing', force=true}\n",
        ).unwrap();
        fs::write(
            cargo.join("config.toml"),
            "[env]\nHOME={value='/missing', force=true}\n",
        )
        .unwrap();
        assert_eq!(
            forced_environment(&root, true).unwrap(),
            ["PATH", "RUSTC", "RUSTDOC"]
        );
    }

    #[test]
    fn compiler_policy_bounds_include_cycles_and_omits_sensitive_parse_values() {
        let temp = tempfile::TempDir::new().unwrap();
        let cargo = temp.path().join(".cargo");
        fs::create_dir(&cargo).unwrap();
        let config = cargo.join("config.toml");
        fs::write(&config, "include=['config.toml']\n").unwrap();
        let error = forced_environment(temp.path(), true)
            .unwrap_err()
            .to_string();
        assert!(error.contains("64 files"), "{error}");
        fs::write(config, "token = private-credential-value\n").unwrap();
        let error = forced_environment(temp.path(), true)
            .unwrap_err()
            .to_string();
        assert!(error.contains("invalid Cargo configuration"), "{error}");
        assert!(!error.contains("private-credential-value"), "{error}");
    }

    #[test]
    fn docker_mounts_do_not_expose_scan_root_siblings_or_writable_upstream() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let scope = root.join("workers/consumer/baseline");
        let cwd = root.join("downstreams/consumer");
        let cached = scope.join("cache");
        let target = root.join("targets/consumer/baseline");
        for path in [
            &cwd,
            &target,
            &scope.join("home"),
            &root.join("upstream"),
            &cached.join("cargo/registry"),
            &cached.join("cargo/git"),
        ] {
            fs::create_dir_all(path).unwrap();
        }
        let recipe = BuildRecipe::default();
        let execution = ExecutionOptions::default();
        let images = Mutex::new(BTreeMap::new());
        let builder = Builder {
            recipe: &recipe,
            root: &root,
            scope: &scope,
            target,
            timeout: Duration::from_secs(10),
            deadline: Instant::now() + Duration::from_secs(10),
            execution: &execution,
            images: &images,
        };
        let mut command = Command::new("docker");
        builder
            .append_mounts(&mut command, &cwd, &cached, false)
            .unwrap();
        let mounts: Vec<_> = command
            .get_args()
            .filter_map(|argument| argument.to_str())
            .filter(|argument| argument.starts_with("type=bind"))
            .collect();
        assert_eq!(mounts.len(), 7);
        assert!(
            mounts.iter().any(|mount| mount
                == &format!("type=bind,source={0},target={0},readonly", cwd.display()))
        );
        assert!(mounts.iter().any(|mount| mount
            == &format!(
                "type=bind,source={0},target={0},readonly",
                root.join("upstream").display()
            )));
        assert!(
            !mounts
                .iter()
                .any(|mount| mount.contains(&format!("source={},", root.display())))
        );
        assert!(
            !mounts
                .iter()
                .any(|mount| mount.contains("candidate") || mount.contains("checkouts"))
        );
        assert!(
            mounts
                .iter()
                .filter(|mount| mount.contains("cargo/registry") || mount.contains("cargo/git"))
                .all(|mount| mount.ends_with(",readonly"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn nested_cache_symlinks_fail_before_native_preparation_touches_other_files() {
        for directory in ["cargo", "mbx", "mbx-shims"] {
            let temp = tempfile::TempDir::new().unwrap();
            let outside = tempfile::TempDir::new().unwrap();
            let root = temp.path().canonicalize().unwrap();
            let scope = root.join("workers/consumer/baseline");
            fs::create_dir_all(scope.join("cache")).unwrap();
            fs::write(outside.path().join("config.toml"), "retained").unwrap();
            std::os::unix::fs::symlink(outside.path(), scope.join("cache").join(directory))
                .unwrap();
            let recipe = BuildRecipe::default();
            let execution = ExecutionOptions::default();
            let images = Mutex::new(BTreeMap::new());
            let builder = Builder {
                recipe: &recipe,
                root: &root,
                scope: &scope,
                target: root.join("targets/consumer/baseline"),
                timeout: Duration::from_secs(5),
                deadline: Instant::now() + Duration::from_secs(5),
                execution: &execution,
                images: &images,
            };
            let error = builder
                .run(&root, &["metadata".into()], Capture::Bytes, false)
                .err()
                .expect("nested cache symlink must fail before invoking Cargo");
            assert!(
                error.to_string().contains("symlinks"),
                "{directory}: {error}"
            );
            assert_eq!(
                fs::read_to_string(outside.path().join("config.toml")).unwrap(),
                "retained"
            );
            assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 1);
        }
    }

    #[cfg(unix)]
    #[test]
    fn persisted_worker_symlinks_cannot_become_host_bind_mounts() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let path = root.join("cache");
        std::os::unix::fs::symlink("/", &path).unwrap();
        assert!(safe_directory(&root, &path).is_err());
        assert!(mount(&mut Command::new("docker"), &root, &path, false).is_err());
    }
}
