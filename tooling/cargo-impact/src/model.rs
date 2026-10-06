use std::{path::PathBuf, time::Duration};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug)]
pub struct ImpactRequest {
    pub library: String,
    pub baseline: PathBuf,
    pub candidate: PathBuf,
    pub downstreams: Vec<DownstreamSpec>,
    pub work_dir: Option<PathBuf>,
    pub timeout: Duration,
}

#[derive(Clone, Debug)]
pub struct DownstreamSpec {
    pub name: String,
    pub source: DownstreamSource,
}

#[derive(Clone, Debug)]
pub enum DownstreamSource {
    Local(PathBuf),
    Git { url: String, revision: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImpactReport {
    pub library: String,
    pub gate: GateResult,
    pub downstreams: Vec<DownstreamResult>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GateResult {
    pub ran: bool,
    pub required_bump: Option<String>,
    pub detected_bump: Option<String>,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DownstreamResult {
    pub name: String,
    pub classification: Classification,
    pub baseline: BuildResult,
    pub candidate: BuildResult,
    pub message: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Classification {
    Compatible,
    Regression,
    PreExistingFailure,
    HarnessFailure,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BuildResult {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub diagnostics: Vec<CompilerDiagnostic>,
    pub log: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompilerDiagnostic {
    pub level: String,
    pub code: Option<String>,
    pub message: String,
    pub rendered: Option<String>,
    pub spans: Vec<DiagnosticSpan>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiagnosticSpan {
    pub file_name: String,
    pub line_start: u32,
    pub column_start: u32,
    pub line_end: u32,
    pub column_end: u32,
    pub is_primary: bool,
    pub text: Vec<DiagnosticLine>,
    pub label: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiagnosticLine {
    pub text: String,
    pub highlight_start: u32,
    pub highlight_end: u32,
}
