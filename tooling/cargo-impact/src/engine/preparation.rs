//! Freeze upstream inputs and make the API gate's mutable preparation copies.

use super::{ImpactError, ScanContext, storage::snapshot_in};
use crate::{cargo, gate, model::GateResult, runner::Builder, source};
use std::{io, path::PathBuf, time::Instant};

pub(super) struct FrozenUpstream {
    pub root: PathBuf,
    pub manifest: PathBuf,
    pub fingerprint: String,
}

pub(super) struct PreparedScan {
    pub baseline: FrozenUpstream,
    pub candidate: FrozenUpstream,
    pub gate: GateResult,
    baseline_version: String,
    candidate_version: String,
    tested_candidate_fingerprint: String,
}

impl PreparedScan {
    pub fn prepare(context: &ScanContext<'_>) -> Result<Self, ImpactError> {
        let ScanContext {
            request,
            root,
            deadline,
            images,
        } = context;
        let baseline = root.join("upstream/baseline");
        let candidate = root.join("upstream/candidate");
        let baseline_fingerprint =
            snapshot_in(request, root, &request.baseline, &baseline, *deadline)?;
        let candidate_fingerprint =
            snapshot_in(request, root, &request.candidate, &candidate, *deadline)?;
        let gate_baseline = root.join("gate-sources/baseline");
        let gate_candidate = root.join("gate-sources/candidate");
        snapshot_in(request, root, &baseline, &gate_baseline, *deadline)?;
        snapshot_in(request, root, &candidate, &gate_candidate, *deadline)?;
        let scope = root.join("gate-worker");
        let builder = Builder {
            recipe: &request.recipe,
            root,
            target: root.join("gate-target"),
            scope: &scope,
            timeout: request.timeout,
            deadline: Instant::now()
                .checked_add(request.timeout)
                .ok_or_else(|| io::Error::other("timeout exceeds the host clock range"))?
                .min(*deadline),
            execution: &request.execution,
            images,
        };
        let package = |source: &std::path::Path| {
            cargo::library_package(
                &cargo::metadata(&builder, source, &source.join("Cargo.toml"), true)?,
                &request.library,
            )
        };
        let baseline_package = package(&gate_baseline)?;
        let candidate_package = package(&gate_candidate)?;
        let gate = if request.force {
            GateResult {
                ran: true,
                required_bump: None,
                reason: "Downstream checks explicitly forced; the API gate was bypassed.".into(),
                log: String::new(),
            }
        } else {
            gate::compare(
                &request.library,
                (&gate_baseline, &baseline_package),
                (&gate_candidate, &candidate_package),
                &builder,
                request.semver_helper.as_deref(),
            )
            .map_err(ImpactError::Semver)?
        };
        if !request.recipe.runner.is_isolated()
            && (source::fingerprint(&baseline)? != baseline_fingerprint
                || source::fingerprint(&candidate)? != candidate_fingerprint)
        {
            return Err(io::Error::other(
                "gate execution mutated canonical upstream inputs; this experiment is inconclusive",
            )
            .into());
        }
        if Instant::now() >= *deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "scan deadline exceeded during API preparation or analysis",
            )
            .into());
        }
        let baseline_manifest = baseline.join(
            baseline_package
                .manifest_path
                .as_std_path()
                .strip_prefix(&gate_baseline)
                .map_err(io::Error::other)?,
        );
        let candidate_manifest = candidate.join(
            candidate_package
                .manifest_path
                .as_std_path()
                .strip_prefix(&gate_candidate)
                .map_err(io::Error::other)?,
        );
        Ok(Self {
            baseline: FrozenUpstream {
                root: baseline,
                manifest: baseline_manifest,
                fingerprint: baseline_fingerprint,
            },
            candidate: FrozenUpstream {
                root: candidate,
                manifest: candidate_manifest,
                fingerprint: candidate_fingerprint.clone(),
            },
            gate,
            baseline_version: baseline_package.version.to_string(),
            candidate_version: candidate_package.version.to_string(),
            tested_candidate_fingerprint: candidate_fingerprint,
        })
    }

    pub fn prepare_injection(&mut self, notes: &mut Vec<String>) -> io::Result<()> {
        if self.baseline_version != self.candidate_version {
            cargo::normalize_version(&self.candidate.manifest, &self.baseline_version)?;
            self.tested_candidate_fingerprint = source::fingerprint(&self.candidate.root)?;
            notes.push(format!("Candidate version {} was injected as baseline version {} to test source compatibility.", self.candidate_version, self.baseline_version));
        }
        Ok(())
    }

    pub fn inputs_changed(&self) -> io::Result<bool> {
        Ok(
            source::fingerprint(&self.baseline.root)? != self.baseline.fingerprint
                || source::fingerprint(&self.candidate.root)? != self.tested_candidate_fingerprint,
        )
    }
}
