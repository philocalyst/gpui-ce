//! Human reports are a small view over the lossless structured report.

use std::fmt::Write;

use crate::model::{Classification, ImpactReport};

impl ImpactReport {
    pub fn has_regressions(&self) -> bool {
        self.downstreams
            .iter()
            .any(|r| r.classification == Classification::Regression)
    }

    pub fn has_harness_failures(&self) -> bool {
        self.downstreams
            .iter()
            .any(|r| r.classification == Classification::HarnessFailure)
    }

    pub fn markdown(&self) -> String {
        let mut output = format!(
            "# Downstream impact: {}\n\n{}\n\n",
            escape(&self.library),
            escape(&self.gate.reason)
        );
        if self.downstreams.is_empty() {
            output.push_str("No downstream builds were recorded. This does not establish ecosystem compatibility.\n");
            return output;
        }
        output.push_str("| Downstream | Result |\n| --- | --- |\n");
        for result in &self.downstreams {
            let label = match result.classification {
                Classification::Compatible => "Compiled with candidate",
                Classification::Regression => "Regression",
                Classification::PreExistingFailure => "Baseline already failed",
                Classification::HarnessFailure => "Harness failure; inconclusive",
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
            for diagnostic in result
                .candidate
                .diagnostics
                .iter()
                .filter(|d| matches!(d.level, cargo_metadata::diagnostic::DiagnosticLevel::Error))
                .take(8)
            {
                let _ = writeln!(
                    output,
                    "{}: {}\n",
                    escape(
                        diagnostic
                            .code
                            .as_ref()
                            .map(|c| c.code.as_str())
                            .unwrap_or("error")
                    ),
                    escape(&diagnostic.message)
                );
                if let Some(span) = diagnostic.spans.iter().find(|s| s.is_primary) {
                    let _ = writeln!(
                        output,
                        "{}:{}:{}\n",
                        escape(&span.file_name),
                        span.line_start,
                        span.column_start
                    );
                }
                if let Some(rendered) = &diagnostic.rendered {
                    fenced(&mut output, rendered);
                }
            }
            output.push_str(
                "Full diagnostics, child notes, spans, and bounded logs are in report.json.\n",
            );
        }
        output
    }
}

// Reports can contain arbitrary source code, compiler output, and repository names.
// Neutralize mentions and HTML; choose a fence longer than any content backtick run.
fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('@', "@\u{200b}")
        .replace('|', "\\|")
        .replace('\n', " ")
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
