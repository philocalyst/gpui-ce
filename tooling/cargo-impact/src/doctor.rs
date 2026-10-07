//! Diagnose the control-plane environment without checking out or building consumer code.

use std::{collections::BTreeSet, time::Duration};

use serde::Serialize;

use crate::{
    config::Config,
    process::{self, Capture},
    runner::{BuildRecipe, CargoDriver, Runner, clean_command},
};

#[derive(Clone, Debug, Serialize)]
pub struct DoctorReport {
    pub ready: bool,
    pub checks: Vec<SetupCheck>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SetupCheck {
    pub name: String,
    pub ready: bool,
    pub detail: String,
    pub remedy: Option<String>,
}

/// Probes are bounded and never inherit a forge token. Network access is unnecessary.
pub fn diagnose(config: &Config) -> DoctorReport {
    let mut recipes: Vec<&BuildRecipe> = vec![&config.recipe];
    recipes.extend(config.downstreams.iter().filter_map(|s| s.recipe.as_ref()));
    recipes.extend(config.overrides.values());
    let host_tools_required = recipes
        .iter()
        .any(|recipe| matches!(recipe.runner, Runner::Local));
    let mut checks = vec![probe(
        "Git",
        "git",
        &["--version"],
        "Install Git and ensure it is on PATH.",
    )];
    for (name, program, arguments) in [
        ("Cargo", "cargo", vec!["--version"]),
        ("rustc", "rustc", vec!["--version", "--verbose"]),
        ("rustdoc", "rustdoc", vec!["--version"]),
    ] {
        let mut check = probe(
            name,
            program,
            &arguments,
            "Install Rust with rustup for a local build recipe.",
        );
        if !host_tools_required {
            check.ready = true;
            check.detail = format!(
                "Host tool is advisory; the selected Docker/Nix environment supplies it. {}",
                check.detail
            );
            check.remedy = None;
        }
        checks.push(check);
    }
    let mut seen = BTreeSet::new();
    for recipe in recipes {
        match &recipe.runner {
            Runner::Docker {image} => {
                if seen.insert("docker".to_owned()) {
                    checks.push(probe("Docker daemon", "docker", &["info", "--format", "{{.ServerVersion}}"], "Start Docker Desktop or install Docker Engine. Linux GitHub-hosted runners include Docker."));
                }
                if seen.insert(format!("image:{image}")) {
                    let mut check = probe(&format!("Docker image {image}"), "docker", &["image", "inspect", "--format", "{{.Id}}", image], "The scan pulls a missing image automatically. Pre-pull it to avoid startup latency.");
                    // A missing image is advisory; a missing daemon is blocking.
                    if !check.ready {check.ready = true; check.detail = "Not available locally; the scan will attempt a pull.".into();}
                    checks.push(check);
                }
            }
            Runner::Local => checks.push(SetupCheck {name:"Local runner".into(), ready:true, detail:"Build scripts execute on this host; check requires --allow-local for trusted inputs.".into(), remedy:None}),
            Runner::Nix {file, ..} => {
                if seen.insert("nix".to_owned()) {checks.push(probe("Nix", "nix", &["--version"], "Install Nix with nix-command enabled."));}
                checks.push(SetupCheck {name:format!("Nix recipe {}", file.display()), ready:file.is_file(), detail:"The derivation supplies the build environment; check requires --allow-local.".into(), remedy:(!file.is_file()).then(|| "Provide the derivation path relative to impact.toml.".into())});
            }
        }
        if matches!(recipe.driver, CargoDriver::Boxington)
            && seen.insert(format!("mbx:{:?}", recipe.runner))
        {
            if matches!(recipe.runner, Runner::Local) {
                checks.push(probe(
                    "Boxington",
                    "mbx",
                    &["--version"],
                    "Install mr-boxington on the runner, or use driver = 'cargo'.",
                ));
            } else {
                checks.push(SetupCheck {name:"Boxington build environment".into(),ready:true,detail:"mbx must be supplied inside the selected Docker image or Nix derivation. This host probe cannot verify that environment.".into(),remedy:Some("Check the recipe image/derivation; missing mbx is a recoverable harness failure.".into())});
            }
        }
    }
    let authenticated = ["GITHUB_TOKEN", "GH_TOKEN"]
        .iter()
        .any(|key| std::env::var_os(key).is_some_and(|v| !v.is_empty()));
    checks.push(SetupCheck {name:"GitHub discovery".into(), ready:!config.discovery.github || authenticated, detail:if authenticated {"A token is present; it is kept out of build processes."} else {"No token detected. crates.io discovery works without one; GitHub code search requires authentication."}.into(), remedy:(config.discovery.github && !authenticated).then(|| "Set GITHUB_TOKEN with repository read access, or disable discovery.github.".into())});
    checks.push(SetupCheck {name:"Build budget".into(), ready:config.execution.validate().is_ok(), detail:format!("{} consumers at once × {} Cargo jobs; {} MiB and {} CPU(s) per Docker worker; storage limit {:?} bytes.",config.execution.jobs,config.execution.cargo_jobs,config.execution.memory_mib,config.execution.cpus,config.execution.max_work_bytes),remedy:config.execution.validate().err().map(|e| e.to_string())});
    DoctorReport {
        ready: checks.iter().all(|c| c.ready),
        checks,
    }
}

fn probe(name: &str, program: &str, arguments: &[&str], remedy: &str) -> SetupCheck {
    let output = process::run(
        clean_command(program).args(arguments),
        Duration::from_secs(5),
        Capture::Bytes,
    );
    let (ready, detail) = match output {
        Ok(output) if output.success => (
            true,
            String::from_utf8_lossy(&output.stdout)
                .trim()
                .chars()
                .take(400)
                .collect(),
        ),
        Ok(output) => (
            false,
            if output.timed_out {
                "Probe timed out after 5 seconds.".into()
            } else {
                output.log.chars().take(400).collect()
            },
        ),
        Err(error) => (false, error.to_string()),
    };
    SetupCheck {
        name: name.into(),
        ready,
        detail,
        remedy: (!ready).then(|| remedy.into()),
    }
}
