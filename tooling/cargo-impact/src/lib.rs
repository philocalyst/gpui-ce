//! Measure actual downstream source compatibility before treating API changes as breakage.

mod cargo;
mod engine;
mod gate;
mod model;
mod process;
pub mod report;
mod source;

pub mod bot;
pub mod config;
pub mod discovery;
pub mod doctor;
pub mod forge;
pub mod http;
pub mod replay;
pub mod runner;

pub use engine::{
    ImpactError, analyze, analyze_replay, analyze_replay_with_progress, analyze_with_discovery,
    analyze_with_progress,
};
pub use gate::run_semver_helper;
pub use model::{
    BuildPhase, BuildProvenance, BuildResult, Classification, CompilerDiagnostic, DependencyGraph,
    DependencyPackage, DiagnosticOrigin, DiagnosticPackage, DownstreamResult, DownstreamSource,
    DownstreamSpec, ExecutionOptions, ExperimentFailure, ExperimentId, ExperimentLifecycle,
    ExperimentStage, ExperimentStatus, GateResult, HarnessFailure, ImpactReport, ImpactRequest,
    LockfileEvidence, RunMetadata, RunStatus, SemverBump, UpstreamRevisions,
};
