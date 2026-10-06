//! Measure actual downstream source compatibility before treating API changes as breakage.

mod cargo;
mod engine;
mod gate;
mod model;
mod process;
mod report;
mod source;

pub mod bot;
pub mod config;
pub mod discovery;
pub mod forge;
pub mod http;
pub mod runner;

pub use engine::{ImpactError, analyze, analyze_with_discovery};
pub use model::{
    BuildResult, Classification, CompilerDiagnostic, DownstreamResult, DownstreamSource,
    DownstreamSpec, GateResult, HarnessFailure, ImpactReport, ImpactRequest, SemverBump,
};
