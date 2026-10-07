//! A self-contained report: no network assets, no source-controlled scripts or HTML.

use std::fmt::Write;

use crate::model::{
    BuildResult, Classification, CompilerDiagnostic, DownstreamResult, ImpactReport, RunStatus,
};
use cargo_metadata::diagnostic::DiagnosticSpanLine;

use super::{issues::IssueDraft, presentation::*};

pub(super) fn render(report: &ImpactReport) -> String {
    let mut output = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>cargo-impact · {}</title><style>{}</style></head><body>",
        html(&report.library),
        include_str!("report.css")
    );
    output
        .push_str("<header><div class=\"brand\">cargo-impact <span>COMPATIBILITY LAB</span></div>");
    let _ = write!(
        output,
        "<h1>Downstream impact<br><span>{}</span></h1><p>{}</p>",
        html(&report.library),
        html(&report.gate.reason)
    );
    if report.run.status == RunStatus::Running {
        let _ = write!(
            output,
            "<aside class=\"notice warning\" role=\"status\"><strong>Partial report · {}/{} consumers completed</strong><br>The scan is running or was interrupted. Missing results are inconclusive.</aside>",
            report.run.completed_downstreams, report.run.planned_downstreams
        );
    }
    if let Some(error) = &report.error {
        let _ = write!(
            output,
            "<aside class=\"notice warning\"><strong>Experiment could not complete</strong><pre>{}</pre><p>Run <code>cargo impact doctor</code>, inspect the failure, and retry with a corrected recipe.</p></aside>",
            html(&clip(error, 8_000))
        );
    }
    if report.run.coverage_sufficient == Some(false) {
        let _ = write!(
            output,
            "<aside class=\"notice warning\"><strong>Insufficient exercised coverage</strong><p>{} consumers exercised the injected library; the configured minimum is {}. This scan is inconclusive.</p></aside>",
            report.run.exercised_downstreams, report.run.execution.minimum_exercised
        );
    }
    if !report.gate.log.is_empty() {
        let _ = write!(
            output,
            "<details><summary>API comparison evidence</summary><pre>{}</pre></details>",
            html(&clip(&report.gate.log, 12_000))
        );
    }
    output.push_str("<nav class=\"downloads\" aria-label=\"Report downloads\"><a href=\"report.json\" download>Full evidence ↗</a><a href=\"report.md\" download>Markdown ↗</a><a href=\"report.sarif\" download>SARIF ↗</a><a href=\"issues/index.md\">Issue bundles ↗</a></nav></header><main>");
    let count = |kind| {
        report
            .downstreams
            .iter()
            .filter(|r| r.classification == kind)
            .count()
    };
    output.push_str("<section class=\"scorecards\" aria-label=\"Result filters\">");
    for kind in [
        Classification::Regression,
        Classification::Compatible,
        Classification::PreExistingFailure,
        Classification::HarnessFailure,
        Classification::NotExercised,
    ] {
        let (slug, icon, label) = classification(kind);
        let _ = write!(
            output,
            "<button type=\"button\" class=\"scorecard {slug}\" data-filter=\"{slug}\" aria-pressed=\"false\"><span>{icon} {label}</span><strong>{}</strong></button>",
            count(kind)
        );
    }
    output.push_str("</section>");
    if report.run.elapsed_ms > 0 {
        let _ = write!(
            output,
            "<p class=\"run-stats\">{:.1}s elapsed · {} workers × {} Cargo jobs · {:.1} MiB managed storage</p>",
            report.run.elapsed_ms as f64 / 1_000.0,
            report.run.execution.jobs,
            report.run.execution.cargo_jobs,
            report.run.storage_bytes as f64 / (1024.0 * 1024.0)
        );
    } else {
        output.push_str(
            "<p class=\"run-stats\">Runtime and storage measurements were not recorded.</p>",
        );
    }
    if !report.discovery.notes.is_empty() {
        output.push_str(
            "<details class=\"coverage\"><summary>Coverage and configuration notes</summary><ul>",
        );
        for note in &report.discovery.notes {
            let _ = write!(output, "<li>{}</li>", html(&clip(note, 4_000)));
        }
        output.push_str("</ul><p>These results cover the recorded experiments. Discovery indexes and successful configurations do not enumerate every user or feature.</p></details>");
    }
    output.push_str("<section class=\"toolbar\" aria-label=\"Find consumers\"><label for=\"search\">Find a consumer, error, or package</label><input id=\"search\" type=\"search\" placeholder=\"Name, E0624, dependency…\"><button type=\"button\" data-filter=\"all\" aria-pressed=\"true\">Show all</button><span id=\"visible-count\" role=\"status\" aria-live=\"polite\"></span></section>");
    if report.downstreams.is_empty() {
        output.push_str("<aside class=\"notice\"><strong>No downstream builds recorded</strong><p>This does not establish ecosystem compatibility. The API gate may have skipped builds; use <code>--force</code> to investigate macros or behavior outside its model.</p></aside>");
    }
    let drafts = report.issue_drafts();
    output.push_str("<div class=\"table-scroll\"><table id=\"results\"><thead><tr><th>Consumer</th><th>Baseline</th><th>Candidate</th><th>Comparison</th><th>Time</th></tr></thead><tbody>");
    for result in &report.downstreams {
        let id = result_id(result);
        let (slug, icon, label) = classification(result.classification);
        let _ = write!(
            output,
            "<tr data-status=\"{slug}\" data-consumer=\"{id}\"><th scope=\"row\"><a href=\"#{id}\">{}</a></th><td>{}</td><td>{}</td><td><span class=\"badge {slug}\">{icon} {label}</span></td><td>{}</td></tr>",
            html(&clip(&result.name, 200)),
            build_status(&result.baseline),
            build_status(&result.candidate),
            if result.elapsed_ms == 0 {
                "not recorded".into()
            } else {
                format!("{:.1}s", result.elapsed_ms as f64 / 1_000.0)
            }
        );
    }
    output.push_str("</tbody></table></div><div id=\"consumer-details\">");
    for result in &report.downstreams {
        consumer(
            &mut output,
            result,
            drafts.iter().find(|d| d.downstream == result.name),
        );
    }
    output.push_str("</div><footer><strong>Retained compiler evidence.</strong> JSON contains complete retained diagnostic trees and all Cargo targets. This view deduplicates compiler symptoms and bounds excerpts. Generated/external paths stay unlinked; macro callsites retain their own provenance. Issue forms require human submission.</footer></main><div id=\"copy-status\" role=\"status\" aria-live=\"polite\"></div><script>");
    output.push_str(include_str!("report.js"));
    output.push_str("</script></body></html>");
    output
}

fn build_status(build: &BuildResult) -> String {
    if let Some(failure) = build.failure {
        return format!(
            "<span class=\"muted\">{}</span>",
            html(&format!("{failure:?}"))
        );
    }
    if build.success {
        return "<span class=\"pass\">✓ Compiled</span>".into();
    }
    let codes = errors(build)
        .iter()
        .filter_map(|d| d.code.as_ref().map(|c| c.code.as_str()))
        .collect::<std::collections::BTreeSet<_>>();
    if codes.is_empty() {
        "<span class=\"muted\">Not completed</span>".into()
    } else {
        format!(
            "<span class=\"fail\">{}</span>",
            html(&codes.into_iter().collect::<Vec<_>>().join(", "))
        )
    }
}

fn consumer(output: &mut String, result: &DownstreamResult, draft: Option<&IssueDraft>) {
    let id = result_id(result);
    let (slug, icon, label) = classification(result.classification);
    let _ = write!(
        output,
        "<article id=\"{id}\" data-status=\"{slug}\" class=\"consumer\"><div class=\"consumer-heading\"><h2>{}</h2><span class=\"badge {slug}\">{icon} {label}</span></div>",
        html(&result.name)
    );
    if let Some(message) = &result.message {
        let _ = write!(output, "<p>{}</p>", html(&clip(message, 2_000)));
    }
    let _ = write!(
        output,
        "<dl class=\"metadata\"><div><dt>Manifest</dt><dd><code>{}</code></dd></div><div><dt>Revision</dt><dd><code>{}</code></dd></div><div><dt>Source fingerprint</dt><dd><code>{}</code></dd></div></dl>",
        html(&result.manifest.to_string_lossy()),
        html(result.revision.as_deref().unwrap_or("local snapshot")),
        html(
            result
                .source_fingerprint
                .as_deref()
                .unwrap_or("not recorded")
        )
    );
    if let Some(draft) = draft {
        let _ = write!(
            output,
            "<nav class=\"issue-actions\" aria-label=\"Issue draft actions\"><button type=\"button\" data-copy=\"draft-{id}\">Copy issue draft</button><a href=\"issues/{}\" download>Download draft</a><a href=\"issues/{}\" download>Reproduction TOML</a>",
            draft.filename, draft.reproduction_filename
        );
        if let Some(url) = &draft.composer_url {
            let _ = write!(
                output,
                "<a href=\"{}\" target=\"_blank\" rel=\"noopener noreferrer\">Open issue form ↗</a>",
                html(url.as_str())
            );
        }
        let _ = write!(
            output,
            "</nav><textarea id=\"draft-{id}\" class=\"draft-text\" aria-hidden=\"true\" tabindex=\"-1\">{}</textarea><p class=\"muted\">Review the draft and intentional API changes before submitting. This report sends no issue or notification.</p>",
            html_code(&draft.body)
        );
    }
    let build = selected_build(result);
    let diagnostics = errors(build);
    for diagnostic in diagnostics.iter().take(8) {
        render_diagnostic(output, result, diagnostic);
    }
    if diagnostics.len() > 8 {
        let _ = write!(
            output,
            "<p class=\"notice\">{} additional unique errors are retained in report.json.</p>",
            diagnostics.len() - 8
        );
    }
    if result.classification == Classification::HarnessFailure {
        output.push_str("<details open><summary>Environment failure and retry recipe</summary>");
        for (phase, build) in [
            ("Baseline", &result.baseline),
            ("Candidate", &result.candidate),
        ] {
            let mut context = build.log.clone();
            for diagnostic in &build.diagnostics {
                if let Some(rendered) = &diagnostic.rendered {
                    context = context.replace(rendered, "");
                }
            }
            if !context.trim().is_empty() {
                let _ = write!(
                    output,
                    "<h3>{phase} harness log</h3><pre>{}</pre>",
                    html(&clip(&context, 8_000))
                );
            }
        }
        let _ = write!(
            output,
            "<p>Add a complete override for <code>{}</code>; both sides will use it on the next attempt. Check system dependencies, feature selection, target installation, package selection and runner availability.</p>",
            html(&result.name)
        );
        if let Ok(recipe) = toml::to_string_pretty(&result.recipe) {
            let _ = write!(output, "<pre>{}</pre>", html_code(&recipe));
        }
        output.push_str("</details>");
    }
    output.push_str("<details><summary>Both build phases and reproduction evidence</summary>");
    for (phase, build) in [
        ("Baseline", &result.baseline),
        ("Candidate", &result.candidate),
    ] {
        let _ = write!(
            output,
            "<h3>{phase}</h3><dl class=\"metadata\"><div><dt>Runner</dt><dd><code>{}</code></dd></div><div><dt>Resolved lockfile</dt><dd><code>{}</code></dd></div><div><dt>Library selected</dt><dd><code>{}</code></dd></div></dl>",
            html_code(&build.provenance.runner_identity),
            html(
                build
                    .provenance
                    .lock_fingerprint
                    .as_deref()
                    .unwrap_or("not recorded")
            ),
            html(build.selected_library.as_deref().unwrap_or("not selected"))
        );
        if let Some(rustc) = &build.provenance.rustc {
            let _ = write!(output, "<pre>{}</pre>", html_code(rustc));
        }
        if let Some(cargo) = &build.provenance.cargo {
            let _ = write!(output, "<p><code>{}</code></p>", html_code(cargo));
        }
        if build.log_truncated || build.diagnostics_truncated || build.timed_out {
            let _ = write!(
                output,
                "<p class=\"notice warning\">Evidence limit: log truncated = {}, diagnostics truncated = {}, timed out = {}. Inspect report.json.</p>",
                build.log_truncated, build.diagnostics_truncated, build.timed_out
            );
        }
    }
    output.push_str("</details></article>");
}

fn render_diagnostic(
    output: &mut String,
    result: &DownstreamResult,
    diagnostic: &CompilerDiagnostic,
) {
    let code = diagnostic
        .code
        .as_ref()
        .map(|c| c.code.as_str())
        .unwrap_or("error");
    let _ = write!(
        output,
        "<section class=\"diagnostic\"><h3><code>{}</code> {}</h3>",
        html(code),
        html(&clip(&diagnostic.message, 1_000))
    );
    if let Some(package) = &diagnostic.package {
        let _ = write!(
            output,
            "<p class=\"muted\">Package <code>{}</code> · {}</p>",
            html(&package.name),
            origin(package.origin)
        );
    }
    for (span, callsite) in source_spans(diagnostic) {
        let label = format!(
            "{}:{}:{}",
            span.file_name, span.line_start, span.column_start
        );
        let prefix = if callsite {
            "Macro invocation"
        } else {
            "Primary span"
        };
        let _ = write!(
            output,
            "<p class=\"source-location\"><strong>{prefix}</strong> · "
        );
        if let Some(url) = source_link(result, diagnostic, span) {
            let _ = write!(
                output,
                "<a href=\"{}\" target=\"_blank\" rel=\"noopener noreferrer\">{}</a>",
                html(url.as_str()),
                html(&label)
            );
        } else {
            let _ = write!(output, "<code>{}</code>", html(&label));
        }
        let _ = write!(
            output,
            " · lines {}–{}, columns {}–{}</p>",
            span.line_start, span.line_end, span.column_start, span.column_end
        );
        if !span.text.is_empty() {
            output.push_str("<pre class=\"source-code\" aria-label=\"Source excerpt with highlighted compiler span\"><code>");
            for (index, line) in span.text.iter().take(12).enumerate() {
                let _ = write!(
                    output,
                    "<span class=\"source-row\"><span class=\"line-number\">{}</span>{}</span>",
                    span.line_start.saturating_add(index),
                    highlighted_line(line)
                );
            }
            output.push_str("</code></pre>");
            if span.text.len() > 12 {
                output.push_str("<p class=\"muted\">Excerpt shortened; full source snippets remain in JSON.</p>");
            }
        }
    }
    if !diagnostic.children.is_empty() {
        output.push_str("<ul class=\"compiler-help\">");
        for child in diagnostic.children.iter().take(8) {
            let _ = write!(
                output,
                "<li><strong>{:?}</strong> {}</li>",
                child.level,
                html(&clip(&child.message, 1_000))
            );
        }
        output.push_str("</ul>");
    }
    if let Some(rendered) = &diagnostic.rendered {
        let _ = write!(
            output,
            "<details><summary>Exact rustc diagnostic</summary><pre>{}</pre></details>",
            html(&clip(rendered, 8_000))
        );
    }
    output.push_str("</section>");
}

fn highlighted_line(line: &DiagnosticSpanLine) -> String {
    let chars: Vec<_> = line.text.chars().collect();
    let start = line.highlight_start.saturating_sub(1).min(chars.len());
    let end = line
        .highlight_end
        .saturating_sub(1)
        .max(start)
        .min(chars.len());
    let window_start = start.saturating_sub(100);
    let window_end = (end.max(start).saturating_add(160))
        .min(chars.len())
        .min(window_start.saturating_add(600));
    let before: String = chars[window_start..start.min(window_end)].iter().collect();
    let marked: String = chars[start.min(window_end)..end.min(window_end)]
        .iter()
        .collect();
    let after: String = chars[end.min(window_end)..window_end].iter().collect();
    format!(
        "{}{}<mark>{}</mark>{}{}",
        if window_start > 0 { "…" } else { "" },
        html_code(&before),
        html_code(&marked),
        html_code(&after),
        if window_end < chars.len() { "…" } else { "" }
    )
}
