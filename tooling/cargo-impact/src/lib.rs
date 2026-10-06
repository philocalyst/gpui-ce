mod engine;
mod model;
mod process;

pub use engine::{ImpactError, analyze};
pub use model::{
    BuildResult, Classification, CompilerDiagnostic, DiagnosticLine, DiagnosticSpan,
    DownstreamResult, DownstreamSource, DownstreamSpec, GateResult, ImpactReport, ImpactRequest,
};

pub mod bot;
pub mod discovery;
pub mod forge;
pub mod http;
mod report;
