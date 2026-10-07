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
pub mod runner;

pub use engine::{ImpactError, analyze, analyze_with_discovery, analyze_with_progress};
pub use gate::run_semver_helper;
pub use model::{
    BuildProvenance, BuildResult, Classification, CompilerDiagnostic, DiagnosticOrigin,
    DiagnosticPackage, DownstreamResult, DownstreamSource, DownstreamSpec, ExecutionOptions,
    GateResult, HarnessFailure, ImpactReport, ImpactRequest, RunMetadata, RunStatus, SemverBump,
    UpstreamRevisions,
};
