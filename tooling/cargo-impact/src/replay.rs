//! Exact retained input identities for verified replay. Eligibility is evidence
//! validation, never permission to skip a new compilation or trust cached success.

use crate::{
    model::{
        BuildPhase, BuildResult, Classification, DiagnosticOrigin, DownstreamResult,
        DownstreamSpec, ExperimentId, ExperimentStatus, ImpactReport,
    },
    runner::Runner,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplayIdentity {
    pub version: u32,
    pub engine: String,
    pub experiment_id: ExperimentId,
    pub library: String,
    pub baseline_source: String,
    pub candidate_source: String,
    pub consumer_source: String,
    pub baseline: PhaseIdentity,
    pub candidate: PhaseIdentity,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PhaseIdentity {
    pub lockfile: String,
    pub dependency_graph: String,
    pub rustc: String,
    pub cargo: String,
    pub image: String,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ReplayIneligible {
    #[error("experiment has no completed typed lifecycle")]
    Unfinished,
    #[error("experiment did not establish an exercised controlled comparison")]
    UncontrolledOutcome,
    #[error(
        "replay requires the exact immutable Docker environment; local/Nix inputs are trusted host environments"
    )]
    UnsupportedRunner,
    #[error("missing or invalid replay evidence: {0}")]
    MissingEvidence(String),
    #[error("retained replay evidence does not agree: {0}")]
    InconsistentEvidence(String),
}

impl ReplayIdentity {
    pub fn from_report(
        report: &ImpactReport,
        result: &DownstreamResult,
    ) -> Result<Self, ReplayIneligible> {
        if result.lifecycle.status != ExperimentStatus::Complete {
            return Err(ReplayIneligible::Unfinished);
        }
        if result.lifecycle.failure.is_some()
            || !matches!(
                result.classification,
                Classification::Compatible | Classification::Regression
            )
        {
            return Err(ReplayIneligible::UncontrolledOutcome);
        }
        if crate::engine::classify(&result.baseline, &result.candidate) != result.classification {
            return Err(inconsistent("claimed comparison classification"));
        }
        if !matches!(result.recipe.runner, Runner::Docker { .. }) {
            return Err(ReplayIneligible::UnsupportedRunner);
        }
        let id = result
            .experiment_id
            .as_ref()
            .ok_or_else(|| missing("experiment ID"))?;
        let spec = DownstreamSpec {
            name: result.name.clone(),
            source: result.source.clone(),
            manifest: result.manifest.clone(),
            recipe: None,
        };
        if &ExperimentId::for_spec(&report.library, &spec, &result.recipe)
            .map_err(|_| missing("configured experiment identity"))?
            != id
        {
            return Err(inconsistent("configured experiment identity"));
        }
        let baseline = phase(BuildPhase::Baseline, &result.baseline, &report.library)?;
        let candidate = phase(BuildPhase::Candidate, &result.candidate, &report.library)?;
        if baseline.image != candidate.image
            || baseline.rustc != candidate.rustc
            || baseline.cargo != candidate.cargo
        {
            return Err(inconsistent(
                "baseline/candidate compiler and image environment",
            ));
        }
        Ok(Self {
            version: 1,
            engine: fingerprint(
                report.run.engine_fingerprint.as_deref(),
                "engine implementation",
            )?,
            experiment_id: id.clone(),
            library: report.library.clone(),
            baseline_source: fingerprint(
                report.baseline_fingerprint.as_deref(),
                "baseline source",
            )?,
            candidate_source: fingerprint(
                report.candidate_fingerprint.as_deref(),
                "candidate source",
            )?,
            consumer_source: fingerprint(result.source_fingerprint.as_deref(), "consumer source")?,
            baseline,
            candidate,
        })
    }

    pub fn digest(&self) -> Result<String, serde_json::Error> {
        serde_json::to_vec(self).map(|bytes| crate::source::key(&bytes))
    }
}

fn phase(
    phase: BuildPhase,
    build: &BuildResult,
    library: &str,
) -> Result<PhaseIdentity, ReplayIneligible> {
    let label = phase.as_str();
    if build.failure.is_some() || build.timed_out || build.diagnostics_truncated {
        return Err(ReplayIneligible::UncontrolledOutcome);
    }
    if phase == BuildPhase::Baseline && !build.success {
        return Err(ReplayIneligible::UncontrolledOutcome);
    }
    let selected = build
        .selected_library
        .as_ref()
        .ok_or_else(|| missing(&format!("{label} selected library")))?;
    if !build.compiled_packages.contains(selected) {
        return Err(ReplayIneligible::UncontrolledOutcome);
    }
    let lock = build
        .lockfile
        .as_ref()
        .ok_or_else(|| missing(&format!("{label} Cargo.lock")))?;
    if !lock.verify() || build.provenance.lock_fingerprint.as_deref() != Some(&lock.sha256) {
        return Err(inconsistent(&format!("{label} Cargo.lock digest")));
    }
    let graph = build
        .dependency_graph
        .as_ref()
        .ok_or_else(|| missing(&format!("{label} dependency graph")))?;
    if &graph.selected_library != selected {
        return Err(inconsistent(&format!("{label} selected graph library")));
    }
    if !graph.packages.iter().any(|package| {
        &package.id == selected
            && package.origin == DiagnosticOrigin::Library
            && package.name == library
    }) {
        return Err(inconsistent(&format!("{label} selected upstream package")));
    }
    if graph
        .packages
        .iter()
        .any(|package| package.origin == DiagnosticOrigin::Dependency && package.source.is_none())
    {
        return Err(missing(&format!(
            "{label} source proof for a path dependency outside frozen inputs"
        )));
    }
    let image = build
        .provenance
        .image_id
        .as_deref()
        .ok_or_else(|| missing(&format!("{label} immutable image")))?;
    fingerprint(
        image.strip_prefix("sha256:"),
        &format!("{label} immutable image"),
    )?;
    Ok(PhaseIdentity {
        lockfile: lock.sha256.clone(),
        dependency_graph: graph
            .fingerprint()
            .map_err(|_| inconsistent(&format!("{label} resolved dependency graph")))?,
        rustc: build
            .provenance
            .rustc
            .clone()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| missing(&format!("{label} rustc")))?,
        cargo: build
            .provenance
            .cargo
            .clone()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| missing(&format!("{label} cargo")))?,
        image: image.into(),
    })
}

fn fingerprint(value: Option<&str>, label: &str) -> Result<String, ReplayIneligible> {
    value
        .filter(|value| ExperimentId::try_from((*value).to_owned()).is_ok())
        .map(str::to_owned)
        .ok_or_else(|| missing(label))
}
fn missing(value: &str) -> ReplayIneligible {
    ReplayIneligible::MissingEvidence(value.into())
}
fn inconsistent(value: &str) -> ReplayIneligible {
    ReplayIneligible::InconsistentEvidence(value.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        BuildProvenance, DependencyGraph, DependencyPackage, DownstreamSource, ExperimentLifecycle,
        LockfileEvidence,
    };
    use crate::runner::BuildRecipe;
    use std::path::PathBuf;

    fn report() -> ImpactReport {
        let source = DownstreamSource::Local {
            path: "consumer".into(),
        };
        let recipe = BuildRecipe::default();
        let spec = DownstreamSpec {
            name: "example".into(),
            source: source.clone(),
            manifest: "Cargo.toml".into(),
            recipe: None,
        };
        let lock = LockfileEvidence {
            contents: "version = 4\n".into(),
            sha256: crate::source::key(b"version = 4\n"),
        };
        let package = |id: &str, name: &str, origin, dependencies: Vec<String>| {
            let source = None::<String>;
            let manifest = PathBuf::from("Cargo.toml");
            let version = "1.0.0";
            DependencyPackage {
                id: id.into(),
                identity: crate::source::key(
                    &serde_json::to_vec(&(origin, name, version, &source, &manifest)).unwrap(),
                ),
                name: name.into(),
                version: version.into(),
                source,
                manifest,
                origin,
                features: Vec::new(),
                dependencies,
            }
        };
        let build = BuildResult {
            success: true,
            exit_code: Some(0),
            compiled_packages: vec!["upstream".into(), "consumer".into()],
            selected_library: Some("upstream".into()),
            provenance: BuildProvenance {
                rustc: Some("rustc 1.99.0 (exact compiler)".into()),
                cargo: Some("cargo 1.99.0 (exact cargo)".into()),
                runner_identity: "rust:1.99.0 (sha256 digest)".into(),
                image_id: Some(format!("sha256:{}", "a".repeat(64))),
                lock_fingerprint: Some(lock.sha256.clone()),
            },
            lockfile: Some(lock),
            dependency_graph: Some(DependencyGraph {
                roots: vec!["consumer".into()],
                selected_library: "upstream".into(),
                packages: vec![
                    package(
                        "consumer",
                        "consumer",
                        DiagnosticOrigin::Downstream,
                        vec!["upstream".into()],
                    ),
                    package("upstream", "library", DiagnosticOrigin::Library, Vec::new()),
                ],
            }),
            ..Default::default()
        };
        let result = DownstreamResult {
            experiment_id: Some(ExperimentId::for_spec("library", &spec, &recipe).unwrap()),
            lifecycle: ExperimentLifecycle {
                status: ExperimentStatus::Complete,
                failure: None,
            },
            name: spec.name,
            source,
            manifest: spec.manifest,
            revision: None,
            source_fingerprint: Some("b".repeat(64)),
            recipe,
            classification: Classification::Compatible,
            baseline: build.clone(),
            candidate: build,
            message: None,
            elapsed_ms: 1,
        };
        let mut report = ImpactReport::failed("library", "fixture");
        report.error = None;
        report.baseline_fingerprint = Some("c".repeat(64));
        report.candidate_fingerprint = Some("d".repeat(64));
        report.run.engine_fingerprint = Some("e".repeat(64));
        report.downstreams.push(result);
        report
    }

    #[test]
    fn replay_identity_requires_complete_consistent_inputs_and_environment() {
        let valid = report();
        let identity = ReplayIdentity::from_report(&valid, &valid.downstreams[0]).unwrap();
        assert_eq!(identity.digest().unwrap().len(), 64);
        assert_eq!(identity.baseline, identity.candidate);

        let mut changed = valid.clone();
        changed.downstreams[0]
            .candidate
            .lockfile
            .as_mut()
            .unwrap()
            .contents
            .push_str("changed");
        assert!(matches!(
            ReplayIdentity::from_report(&changed, &changed.downstreams[0]),
            Err(ReplayIneligible::InconsistentEvidence(_))
        ));

        let mut changed = valid.clone();
        changed.downstreams[0].candidate.provenance.image_id =
            Some(format!("sha256:{}", "f".repeat(64)));
        assert!(matches!(
            ReplayIdentity::from_report(&changed, &changed.downstreams[0]),
            Err(ReplayIneligible::InconsistentEvidence(_))
        ));

        let mut changed = valid.clone();
        changed.downstreams[0]
            .recipe
            .features
            .push("different".into());
        assert!(matches!(
            ReplayIdentity::from_report(&changed, &changed.downstreams[0]),
            Err(ReplayIneligible::InconsistentEvidence(_))
        ));

        let mut changed = valid.clone();
        changed.baseline_fingerprint = None;
        assert!(matches!(
            ReplayIdentity::from_report(&changed, &changed.downstreams[0]),
            Err(ReplayIneligible::MissingEvidence(_))
        ));

        let mut changed = valid.clone();
        changed.downstreams[0].classification = Classification::Regression;
        assert!(matches!(
            ReplayIdentity::from_report(&changed, &changed.downstreams[0]),
            Err(ReplayIneligible::InconsistentEvidence(_))
        ));

        let mut changed = valid;
        changed.downstreams[0].lifecycle.status = ExperimentStatus::Candidate;
        assert_eq!(
            ReplayIdentity::from_report(&changed, &changed.downstreams[0]),
            Err(ReplayIneligible::Unfinished)
        );
    }

    #[test]
    fn graph_proof_rejects_missing_nodes_collisions_and_forged_upstream_roles() {
        let valid = report();
        let mut changed = valid.clone();
        changed.downstreams[0]
            .baseline
            .dependency_graph
            .as_mut()
            .unwrap()
            .packages[0]
            .dependencies
            .push("absent".into());
        assert!(matches!(
            ReplayIdentity::from_report(&changed, &changed.downstreams[0]),
            Err(ReplayIneligible::InconsistentEvidence(_))
        ));
        let mut changed = valid.clone();
        let graph = changed.downstreams[0]
            .baseline
            .dependency_graph
            .as_mut()
            .unwrap();
        graph.packages.push(graph.packages[1].clone());
        assert!(graph.fingerprint().is_err());
        let mut changed = valid;
        changed.downstreams[0]
            .baseline
            .dependency_graph
            .as_mut()
            .unwrap()
            .packages[1]
            .origin = DiagnosticOrigin::Downstream;
        assert!(matches!(
            ReplayIdentity::from_report(&changed, &changed.downstreams[0]),
            Err(ReplayIneligible::InconsistentEvidence(_))
        ));
    }

    #[test]
    fn legacy_reports_keep_unknown_lifecycle_and_cannot_invent_replay_evidence() {
        let mut value = serde_json::to_value(report()).unwrap();
        let result = value["downstreams"][0].as_object_mut().unwrap();
        result.remove("experiment_id");
        result.remove("lifecycle");
        for phase in ["baseline", "candidate"] {
            let build = result.get_mut(phase).unwrap().as_object_mut().unwrap();
            build.remove("lockfile");
            build.remove("dependency_graph");
        }
        let legacy: ImpactReport = serde_json::from_value(value).unwrap();
        let result = &legacy.downstreams[0];
        assert_eq!(result.lifecycle.status, ExperimentStatus::Unknown);
        assert_eq!(result.experiment_id, None);
        assert_eq!(
            ReplayIdentity::from_report(&legacy, result),
            Err(ReplayIneligible::Unfinished)
        );
    }
}
