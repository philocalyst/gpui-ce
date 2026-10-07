//! Build environments form one explicit boundary. HTTP tokens never cross it.

use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
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
        self.run_program(cwd, driver, args, capture, network)
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
        for path in [&cached, &self.scope.join("home"), &self.target] {
            safe_directory(self.root, path)?;
        }
        fs::create_dir_all(cached.join("cargo"))?;
        fs::create_dir_all(cached.join("mbx"))?;
        fs::create_dir_all(cached.join("mbx-shims"))?;
        fs::create_dir_all(self.scope.join("home"))?;
        fs::create_dir_all(&self.target)?;
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

    pub(crate) fn provenance(
        &self,
        cwd: &Path,
        manifest: &Path,
    ) -> std::io::Result<BuildProvenance> {
        let version = |program: &str, arguments: &[&str]| -> std::io::Result<Option<String>> {
            let args: Vec<_> = arguments.iter().map(OsString::from).collect();
            let output = self.run_program(cwd, program, &args, Capture::Bytes, false)?;
            Ok(output
                .success
                .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned()))
        };
        let runner_identity = match &self.recipe.runner {
            Runner::Docker { image } => {
                format!(
                    "{image} ({})",
                    pin_image(image, self.images, self.deadline)?
                )
            }
            Runner::Local => "local".into(),
            Runner::Nix { file, attribute } => format!("nix:{}#{attribute}", file.display()),
        };
        Ok(BuildProvenance {
            rustc: version("rustc", &["-Vv"])?,
            cargo: version("cargo", &["--version"])?,
            runner_identity,
            lock_fingerprint: fs::read(manifest.with_file_name("Cargo.lock"))
                .ok()
                .map(|bytes| crate::source::key(&bytes)),
        })
    }
}

fn pin_image(
    image: &str,
    images: &Mutex<BTreeMap<String, String>>,
    deadline: Instant,
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
    if !output.success {
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

fn safe_directory(root: &Path, path: &Path) -> std::io::Result<()> {
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
    fn persisted_worker_symlinks_cannot_become_host_bind_mounts() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let path = root.join("cache");
        std::os::unix::fs::symlink("/", &path).unwrap();
        assert!(safe_directory(&root, &path).is_err());
        assert!(mount(&mut Command::new("docker"), &root, &path, false).is_err());
    }
}
