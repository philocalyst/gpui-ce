//! Shared evidence selection; renderers never guess a repository from a rustc path.

use std::{collections::BTreeSet, path::Path};

use cargo_metadata::diagnostic::{DiagnosticLevel, DiagnosticSpan, DiagnosticSpanLine};
use sha2::{Digest, Sha256};
use url::Url;

use crate::{
    forge::Repository,
    model::{
        BuildPhase, BuildResult, Classification, CompilerDiagnostic, DependencyPackage,
        DiagnosticOrigin, DownstreamResult, DownstreamSource, ExperimentStatus, HarnessFailure,
    },
};

pub(crate) fn lock_path(result: &DownstreamResult, phase: BuildPhase) -> String {
    format!("locks/{}/{}.lock", result_id(result), phase.as_str())
}

pub(crate) fn replay_command(id: &crate::ExperimentId) -> String {
    format!(
        "cargo impact replay --report-dir PATH_TO_REPORT \\\n  --experiment {} \\\n  --baseline PATH_TO_BASELINE --candidate PATH_TO_CANDIDATE \\\n  --work-dir .cargo-impact-replay --output-dir impact-replay",
        id
    )
}

pub(crate) fn result_status(
    result: &DownstreamResult,
) -> (&'static str, &'static str, &'static str) {
    if let Some(outcome) = super::view::outcome(result) {
        return classification(outcome);
    }
    let label = match result.lifecycle.status {
        ExperimentStatus::Queued => "Queued",
        ExperimentStatus::Preparing => "Preparing source",
        ExperimentStatus::Baseline => "Building baseline",
        ExperimentStatus::Candidate => "Building candidate",
        _ => "Pending",
    };
    ("pending", "⏳", label)
}

pub(crate) fn dependency_paths<'a>(
    build: &'a BuildResult,
    target: &str,
) -> Vec<Vec<&'a DependencyPackage>> {
    let Some(graph) = &build.dependency_graph else {
        return Vec::new();
    };
    let packages: std::collections::BTreeMap<_, _> = graph
        .packages
        .iter()
        .map(|package| (package.id.as_str(), package))
        .collect();
    graph
        .roots
        .iter()
        .take(6)
        .filter_map(|root| {
            graph
                .dependency_path(root, target)?
                .iter()
                .map(|id| packages.get(id.as_str()).copied())
                .collect()
        })
        .collect()
}

pub(crate) fn path_label(path: &[&DependencyPackage]) -> String {
    let mut label = path
        .iter()
        .take(12)
        .map(|package| format!("{} {}", package.name, package.version))
        .collect::<Vec<_>>()
        .join(" → ");
    if path.len() > 12 {
        label.push_str(" → … (full graph in JSON)");
    }
    label
}

pub(crate) fn result_id(result: &DownstreamResult) -> String {
    if let Some(id) = &result.experiment_id {
        return format!("consumer-{id}");
    }
    let mut hash = Sha256::new();
    if let Ok(identity) = serde_json::to_vec(&(
        &result.name,
        &result.manifest,
        &result.source,
        &result.recipe,
    )) {
        hash.update(identity);
    }
    format!("consumer-{:x}", hash.finalize())[..25].into()
}

pub(crate) fn classification(value: Classification) -> (&'static str, &'static str, &'static str) {
    match value {
        Classification::Regression => ("regression", "🔴", "Regression"),
        Classification::Compatible => ("compatible", "✅", "Compatible"),
        Classification::PreExistingFailure => ("pre-existing", "🟡", "Baseline already failed"),
        Classification::HarnessFailure => ("harness", "⚙️", "Harness failure"),
        Classification::NotExercised => ("not-exercised", "⚪", "Not exercised"),
    }
}

pub(crate) fn selected_build(result: &DownstreamResult) -> &BuildResult {
    if result.classification == Classification::PreExistingFailure
        || result.candidate.diagnostics.is_empty()
    {
        &result.baseline
    } else {
        &result.candidate
    }
}

pub(crate) fn errors(build: &BuildResult) -> Vec<&CompilerDiagnostic> {
    let mut seen = BTreeSet::new();
    build
        .diagnostics
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .filter(|d| seen.insert(diagnostic_key(d)))
        .collect()
}

/// A symptom identity for repeated Cargo targets and dependency fallout, not proof of causality.
pub(crate) fn diagnostic_key(diagnostic: &CompilerDiagnostic) -> String {
    let primary = diagnostic.spans.iter().find(|span| span.is_primary);
    let origin = diagnostic.package.as_ref().map(|p| p.origin);
    let file = primary.map(|span| {
        diagnostic
            .source_files
            .get(&span.file_name)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| {
                Path::new(&span.file_name)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            })
    });
    let value = (
        diagnostic.package.as_ref().map(|p| p.name.as_str()),
        origin,
        diagnostic.code.as_ref().map(|c| c.code.as_str()),
        &diagnostic.message,
        file,
        primary.map(|s| (s.line_start, s.column_start)),
        primary.map(|s| &s.text),
    );
    let bytes = serde_json::to_vec(&value).unwrap_or_default();
    format!("{:x}", Sha256::digest(bytes))
}

pub(crate) fn source_link(
    result: &DownstreamResult,
    diagnostic: &CompilerDiagnostic,
    span: &DiagnosticSpan,
) -> Option<Url> {
    if !source_unchanged(result) {
        return None;
    }
    let (DownstreamSource::Git { url, forge, .. }, Some(revision)) =
        (&result.source, &result.revision)
    else {
        return None;
    };
    let file = diagnostic.source_files.get(&span.file_name)?;
    Repository::parse(url, *forge)
        .ok()?
        .source_link(revision, file.to_str()?, span.line_start)
}

pub(crate) fn source_unchanged(result: &DownstreamResult) -> bool {
    result
        .lifecycle
        .failure
        .as_ref()
        .is_none_or(|failure| failure.cause != HarnessFailure::InputMutation)
        && ![result.baseline.failure, result.candidate.failure]
            .contains(&Some(HarnessFailure::InputMutation))
}

pub(crate) fn source_spans(diagnostic: &CompilerDiagnostic) -> Vec<(&DiagnosticSpan, bool)> {
    let mut spans = Vec::new();
    for span in diagnostic.spans.iter().filter(|s| s.is_primary).take(4) {
        spans.push((span, false));
        if !diagnostic.source_files.contains_key(&span.file_name)
            && let Some(callsite) =
                std::iter::successors(span.expansion.as_deref(), |e| e.span.expansion.as_deref())
                    .map(|e| &e.span)
                    .find(|s| diagnostic.source_files.contains_key(&s.file_name))
        {
            spans.push((callsite, true));
        }
    }
    spans
}

pub(crate) fn origin(value: DiagnosticOrigin) -> &'static str {
    match value {
        DiagnosticOrigin::Downstream => "downstream",
        DiagnosticOrigin::Library => "injected library",
        DiagnosticOrigin::Dependency => "external dependency",
    }
}

/// rustc's snippet offsets are one-based character columns, with an exclusive end.
/// Slice characters rather than bytes; malformed or synthetic spans remain safe to render.
pub(crate) fn highlighted_text(line: &DiagnosticSpanLine) -> String {
    let start = line.highlight_start.saturating_sub(1);
    let end = line.highlight_end.saturating_sub(1).max(start);
    line.text
        .chars()
        .skip(start)
        .take(end.saturating_sub(start).min(180))
        .collect()
}

pub(crate) fn clip(value: &str, limit: usize) -> String {
    let mut chars = value.chars();
    let mut result: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        result.push('…');
    }
    result
}

pub(crate) fn neutral(value: &str) -> String {
    value.replace('@', "@\u{200b}")
}

pub(crate) fn escape(value: &str) -> String {
    let value = neutral(value)
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace(['\n', '\r'], " ");
    let mut result = String::new();
    for c in value.chars() {
        if "\\`*[]_|".contains(c) {
            result.push('\\');
        }
        result.push(c);
    }
    result
}

pub(crate) fn html(value: &str) -> String {
    neutral(value)
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

pub(crate) fn html_code(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

pub(crate) fn inline(value: &str) -> String {
    let value = neutral(value).replace(['\n', '\r'], " ");
    let longest = value.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest + 1);
    format!("{fence} {value} {fence}")
}

pub(crate) fn fenced(output: &mut String, language: &str, value: &str) {
    use std::fmt::Write;
    let longest = value.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    let value = if matches!(language, "rust" | "sh" | "toml") {
        value.to_owned()
    } else {
        neutral(value)
    };
    let _ = writeln!(output, "{fence}{language}\n{value}\n{fence}\n");
}

#[cfg(test)]
mod replay_command_tests {
    #[test]
    fn copied_replay_command_passes_only_the_intended_arguments_through_a_shell() {
        let id = crate::ExperimentId::try_from("a".repeat(64)).unwrap();
        let command = super::replay_command(&id);
        // Execute the real continuation syntax with a recording cargo function.
        let script = format!("cargo() {{ printf '%s\\n' \"$@\"; }}\n{command}");
        let output = std::process::Command::new("sh")
            .args(["-c", &script])
            .output()
            .unwrap();
        assert!(output.status.success());
        let arguments = String::from_utf8(output.stdout).unwrap();
        assert_eq!(
            arguments.lines().collect::<Vec<_>>(),
            vec![
                "impact",
                "replay",
                "--report-dir",
                "PATH_TO_REPORT",
                "--experiment",
                id.as_str(),
                "--baseline",
                "PATH_TO_BASELINE",
                "--candidate",
                "PATH_TO_CANDIDATE",
                "--work-dir",
                ".cargo-impact-replay",
                "--output-dir",
                "impact-replay"
            ]
        );
    }
}
