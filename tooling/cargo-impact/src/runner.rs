//! Build environments form one explicit boundary. HTTP tokens never cross it.

use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use serde::{Deserialize, Serialize};

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
        let cached = self.root.join("cache");
        fs::create_dir_all(cached.join("cargo"))?;
        fs::create_dir_all(cached.join("mbx"))?;
        fs::create_dir_all(cached.join("mbx-shims"))?;
        fs::create_dir_all(self.root.join("home"))?;
        fs::create_dir_all(&self.target)?;
        let driver = if matches!(self.recipe.driver, CargoDriver::Boxington)
            && args.first().is_some_and(|v| v == "check")
        {
            "mbx"
        } else {
            "cargo"
        };
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
                    "--memory=4g",
                    "--cpus=2",
                    "--tmpfs=/tmp:rw,nosuid,size=1g",
                ])
                .args(["--network", if network { "bridge" } else { "none" }])
                .arg("--mount")
                .arg(format!(
                    "type=bind,source={},target={}",
                    self.root.display(),
                    self.root.display()
                ))
                .arg("--workdir")
                .arg(cwd);
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
                cmd.arg(image).arg(driver);
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
        let result = process::run(&mut command, self.timeout, capture);
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
            ("CARGO_BUILD_JOBS", "2".into()),
            ("MBX_CACHE_DIR", cached.join("mbx").into_os_string()),
            ("MBX_SHIMS_DIR", cached.join("mbx-shims").into_os_string()),
            ("MBX_DISPLAY", "plain".into()),
            ("MBX_TARGET_VIEWS", "0".into()),
            ("HOME", self.root.join("home").into_os_string()),
        ]
    }
}

/// Preserve only tools and rustup's public installation location, never the caller's token environment.
pub(crate) fn clean_command(program: &str) -> Command {
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
