//! Human reports are a concise view over the complete structured evidence.

use std::fmt::Write;

use crate::{
    discovery::Discovery,
    forge::Repository,
    model::{
        BuildResult, Classification, DownstreamResult, DownstreamSource, GateResult, ImpactReport,
    },
};

impl ImpactReport {
    pub fn failed(library: &str, error: &str) -> Self {
        Self {
            schema_version: 1,
            library: library.into(),
            baseline_fingerprint: None,
            candidate_fingerprint: None,
            error: Some(error.into()),
            gate: GateResult {
                ran: false,
                required_bump: None,
                reason: "The experiment did not complete; its result is inconclusive.".into(),
                log: error.into(),
            },
            discovery: Discovery::default(),
            downstreams: Vec::new(),
        }
    }
    pub fn has_regressions(&self) -> bool {
        self.downstreams
            .iter()
            .any(|r| r.classification == Classification::Regression)
    }
    pub fn has_harness_failures(&self) -> bool {
        self.error.is_some()
            || self
                .downstreams
                .iter()
                .any(|r| r.classification == Classification::HarnessFailure)
    }
    /// Large reports fall back to counts instead of cutting a diagnostic fence in half.
    pub(crate) fn comment_markdown(&self) -> String {
        let markdown = self.markdown();
        if markdown.chars().count() <= 30_000 {
            return markdown;
        }
        let count = |classification| {
            self.downstreams
                .iter()
                .filter(|r| r.classification == classification)
                .count()
        };
        format!(
            "# Downstream impact: {}\n\n{}\n\n{} regressions, {} compatible, {} baseline failures, {} harness failures, {} not exercised.\n\nThe detailed report exceeds the comment limit; inspect the full artifact for diagnostics and coverage notes.",
            escape(&self.library.chars().take(100).collect::<String>()),
            escape(&self.gate.reason.chars().take(1000).collect::<String>()),
            count(Classification::Regression),
            count(Classification::Compatible),
            count(Classification::PreExistingFailure),
            count(Classification::HarnessFailure),
            count(Classification::NotExercised),
        )
    }
    pub fn markdown(&self) -> String {
        let mut output = format!(
            "# Downstream impact: {}\n\n{}\n\n",
            escape(&self.library),
            escape(&self.gate.reason)
        );
        if let Some(error) = &self.error {
            fenced(&mut output, error);
            output.push_str("Adjust the library build recipe (runner, features, target, or system dependencies) and retry.\n\n");
        }
        if !self.discovery.notes.is_empty() {
            output.push_str("## Coverage and configuration\n\n");
            for note in &self.discovery.notes {
                let _ = writeln!(output, "- {}", escape(note));
            }
            output.push('\n');
        }
        if self.downstreams.is_empty() {
            output.push_str("No downstream builds were recorded. This does not establish ecosystem compatibility.\n");
            return output;
        }
        output.push_str("| Downstream | Result |\n| --- | --- |\n");
        for result in &self.downstreams {
            let label = match result.classification {
                Classification::Compatible => "Both versions compiled",
                Classification::Regression => "Regression",
                Classification::PreExistingFailure => "Baseline already failed; inconclusive",
                Classification::HarnessFailure => "Harness failure; retry with a recipe override",
                Classification::NotExercised => "Library was not exercised",
            };
            let _ = writeln!(output, "| {} | {label} |", escape(&result.name));
        }
        for result in &self.downstreams {
            if result.classification == Classification::Compatible {
                continue;
            }
            let _ = writeln!(output, "\n## {}\n", escape(&result.name));
            if let Some(message) = &result.message {
                let _ = writeln!(output, "{}\n", escape(message));
            }
            if let Some(revision) = &result.revision {
                let _ = writeln!(output, "Downstream revision: {}\n", escape(revision));
            }
            let build = if result.classification == Classification::PreExistingFailure {
                &result.baseline
            } else {
                &result.candidate
            };
            render_diagnostics(&mut output, result, build);
            if result.classification == Classification::HarnessFailure {
                for (label, build) in [
                    ("Baseline", &result.baseline),
                    ("Candidate", &result.candidate),
                ] {
                    if !build.log.trim().is_empty() {
                        let _ = writeln!(output, "{label} harness output:\n");
                        // The concise Markdown view is bounded independently of retained JSON logs.
                        fenced(
                            &mut output,
                            &build.log.chars().take(4000).collect::<String>(),
                        );
                    }
                }
            }
            output.push_str("Complete diagnostics, suggestions, macro expansions, byte ranges, snippets, and bounded logs are in report.json.\n");
        }
        output
    }
}

fn render_diagnostics(output: &mut String, result: &DownstreamResult, build: &BuildResult) {
    let mut seen = std::collections::BTreeSet::new();
    for diagnostic in build
        .diagnostics
        .iter()
        .filter(|d| matches!(d.level, cargo_metadata::diagnostic::DiagnosticLevel::Error))
        // --all-targets can emit the same error for both the library and its test target.
        // Preserve both in JSON, but show the source problem once in the concise report.
        .filter(|d| {
            seen.insert((
                d.package_id.as_str(),
                d.code.as_ref().map(|c| c.code.as_str()),
                d.message.as_str(),
                d.spans
                    .iter()
                    .find(|s| s.is_primary)
                    .map(|s| (s.file_name.as_str(), s.line_start, s.column_start)),
            ))
        })
        .take(8)
    {
        let code = diagnostic
            .code
            .as_ref()
            .map(|c| c.code.as_str())
            .unwrap_or("error");
        let _ = writeln!(
            output,
            "{}: {}\n",
            escape(code),
            escape(&diagnostic.message)
        );
        if let Some(span) = diagnostic.spans.iter().find(|s| s.is_primary) {
            let label = format!(
                "{}:{}:{}",
                escape(&span.file_name),
                span.line_start,
                span.column_start
            );
            let link = match (&result.source, &result.revision) {
                (DownstreamSource::Git { url, forge, .. }, Some(revision)) => {
                    Repository::parse(url, *forge).ok().and_then(|repo| {
                        repo.source_link(revision, &span.file_name, span.line_start)
                    })
                }
                _ => None,
            };
            if let Some(link) = link {
                let _ = writeln!(output, "[{label}](<{link}>)\n");
            } else {
                let _ = writeln!(output, "{label}\n");
            }
        }
        if let Some(rendered) = &diagnostic.rendered {
            fenced(output, &rendered.chars().take(8000).collect::<String>());
        }
    }
}

// Neutralize mentions, HTML, and Markdown injection from arbitrary source/forge data.
fn escape(value: &str) -> String {
    let value = value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('@', "@\u{200b}")
        .replace('\n', " ");
    let mut output = String::new();
    for character in value.chars() {
        if "\\`*[]_|".contains(character) {
            output.push('\\');
        }
        output.push(character);
    }
    output
}

fn fenced(output: &mut String, value: &str) {
    let longest = value.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    let _ = writeln!(
        output,
        "{fence}text\n{}\n{fence}\n",
        value.replace('@', "@\u{200b}")
    );
}
