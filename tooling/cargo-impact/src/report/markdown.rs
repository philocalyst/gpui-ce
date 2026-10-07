use std::{collections::BTreeMap, fmt::Write};

use crate::model::{
    BuildPhase, BuildResult, Classification, CompilerDiagnostic, DownstreamResult, ImpactReport,
    RunStatus,
};

use super::{issues::IssueDraft, presentation::*};

pub(super) fn render(report: &ImpactReport, concise: bool) -> String {
    render_with_links(report, concise, "")
}

pub(super) fn render_with_links(report: &ImpactReport, concise: bool, prefix: &str) -> String {
    let mut output = format!("# Downstream impact · {}\n\n", inline(&report.library));
    if report.run.status == RunStatus::Running {
        let _ = writeln!(
            output,
            "> [!WARNING]\n> **Partial report:** {}/{} consumers completed. This scan is still running or was interrupted; missing results are inconclusive.\n",
            report.run.completed_downstreams, report.run.planned_downstreams
        );
    }
    if let Some(error) = &report.error {
        output.push_str("> [!WARNING]\n> **The experiment did not complete.** This report cannot establish compatibility.\n\n");
        fenced(&mut output, "text", &clip(error, 4_000));
        output.push_str("Adjust the library build recipe after inspecting the failure with `cargo impact doctor`, then retry.\n\n");
    }
    let _ = writeln!(output, "{}\n", escape(&report.gate.reason));
    if let Some(identity) = &report.run.replay_of {
        let _ = writeln!(output, "**Replay attempt:** {}\n", inline(identity));
        if let Some(expected) = report.run.replay_expected {
            let (_, _, label) = classification(expected);
            let _ = writeln!(
                output,
                "Original comparison: **{label}**. The fresh assessment is recorded below.\n"
            );
        }
    }
    summary(&mut output, report);
    if report.run.coverage_sufficient == Some(false) {
        let _ = writeln!(
            output,
            "> [!WARNING]\n> **Insufficient exercised coverage:** {} consumers exercised the injected library; the configured minimum is {}. This scan is inconclusive.\n",
            report.run.exercised_downstreams, report.run.execution.minimum_exercised
        );
    }
    if !report.gate.log.is_empty() {
        output.push_str("<details>\n<summary>API comparison evidence</summary>\n\n");
        fenced(
            &mut output,
            "text",
            &clip(&report.gate.log, if concise { 2_000 } else { 12_000 }),
        );
        output.push_str("</details>\n\n");
    }
    if concise {
        output.push_str("Download the report artifact bundle and open **index.html** for highlighted spans, filtering, issue drafts and reproduction recipes.\n\n");
    } else {
        let _ = writeln!(
            output,
            "[Interactive report]({prefix}index.html) · [Full compiler evidence]({prefix}report.json) · [SARIF]({prefix}report.sarif) · [Issue drafts and reproduction recipes]({prefix}issues/index.md)\n"
        );
    }
    if !report.discovery.notes.is_empty() {
        output.push_str("<details>\n<summary>Coverage and configuration</summary>\n\n");
        for note in &report.discovery.notes {
            let _ = writeln!(output, "- {}", escape(&clip(note, 2_000)));
        }
        output.push_str("\n</details>\n\n");
    }
    if report.downstreams.is_empty() {
        output.push_str("> [!NOTE]\n> No downstream builds were recorded. This does not establish ecosystem compatibility.\n");
        return output;
    }
    let mut ordered = report.downstreams.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|r| match super::view::outcome(r) {
        Some(Classification::Regression) => 0,
        Some(Classification::HarnessFailure) => 1,
        Some(Classification::PreExistingFailure) => 2,
        Some(Classification::NotExercised) => 3,
        Some(Classification::Compatible) => 4,
        None => 5,
    });
    let detailed: Vec<_> = ordered
        .iter()
        .copied()
        .filter(|r| super::view::outcome(r) != Some(Classification::Compatible))
        .take(if concise { 3 } else { usize::MAX })
        .collect();
    output.push_str("| Consumer | Baseline | Candidate | Comparison | Inspect |\n| --- | --- | --- | --- | --- |\n");
    for result in ordered.iter().take(if concise { 40 } else { usize::MAX }) {
        let (_, icon, label) = result_status(result);
        let inspect = if detailed.iter().any(|r| std::ptr::eq(*r, *result)) {
            format!("[Details](#{})", result_id(result))
        } else if concise {
            "Artifact bundle".into()
        } else {
            "Recorded in JSON".into()
        };
        let _ = writeln!(
            output,
            "| {} | {} | {} | {icon} **{label}** | {inspect} |",
            escape(&clip(&result.name, 180)),
            build_label(&result.baseline),
            build_label(&result.candidate)
        );
    }
    if concise && report.downstreams.len() > 40 {
        let _ = writeln!(
            output,
            "\n{} additional consumers are in the artifact bundle.\n",
            report.downstreams.len() - 40
        );
    }
    if !concise {
        shared_failures(&mut output, report);
    }
    let drafts = report.issue_drafts();
    let maximum = if concise { 3 } else { usize::MAX };
    let interesting = report
        .downstreams
        .iter()
        .filter(|r| super::view::outcome(r) != Some(Classification::Compatible))
        .collect::<Vec<_>>();
    for result in detailed {
        render_consumer(
            &mut output,
            result,
            drafts.iter().find(|d| d.id == result_id(result)),
            concise,
            prefix,
        );
    }
    if interesting.len() > maximum {
        let _ = writeln!(
            output,
            "\n**{} more consumers need inspection.** Download the full report bundle to see every result.\n",
            interesting.len() - maximum
        );
    }
    output.push_str("\nThe JSON retains all Cargo targets, diagnostic trees, suggestions, macro expansions, byte ranges, and bounded logs. Displayed excerpts are concise; source links are included only for verified files in the tested consumer checkout.\n");
    output
}

fn summary(output: &mut String, report: &ImpactReport) {
    let summary = super::ReportSummary::from_report(report);
    let _ = writeln!(
        output,
        "**{} regressions** · {} compatible · {} baseline failures · {} harness failures · {} not exercised · {} pending\n",
        summary.regressions,
        summary.compatible,
        summary.baseline_failures,
        summary.harness_failures,
        summary.not_exercised,
        summary.pending,
    );
    if report.run.elapsed_ms > 0 {
        let _ = writeln!(
            output,
            "Elapsed: {:.1}s · Workers: {} · Managed storage: {:.1} MiB\n",
            report.run.elapsed_ms as f64 / 1_000.0,
            report.run.execution.jobs,
            report.run.storage_bytes as f64 / (1024.0 * 1024.0)
        );
    }
}

fn build_label(build: &BuildResult) -> String {
    if let Some(failure) = build.failure {
        return format!("⚙️ {}", escape(&format!("{failure:?}")));
    }
    if build.success {
        return "✅ Compiled".into();
    }
    let codes = errors(build)
        .iter()
        .filter_map(|d| d.code.as_ref().map(|c| c.code.as_str()))
        .collect::<std::collections::BTreeSet<_>>();
    if codes.is_empty() {
        "⚪ No successful build".into()
    } else {
        format!(
            "❌ {}",
            codes.into_iter().map(inline).collect::<Vec<_>>().join(", ")
        )
    }
}

fn shared_failures(output: &mut String, report: &ImpactReport) {
    let mut groups: BTreeMap<String, (&CompilerDiagnostic, Vec<&DownstreamResult>)> =
        BTreeMap::new();
    for result in report
        .downstreams
        .iter()
        .filter(|r| super::view::outcome(r) == Some(Classification::Regression))
    {
        for diagnostic in errors(selected_build(result)) {
            groups
                .entry(diagnostic_key(diagnostic))
                .or_insert((diagnostic, Vec::new()))
                .1
                .push(result);
        }
    }
    let shared: Vec<_> = groups
        .values()
        .filter(|(_, results)| results.len() > 1)
        .collect();
    if shared.is_empty() {
        return;
    }
    output.push_str("\n<details>\n<summary>Repeated compiler failures across consumers</summary>\n\nThese groups identify matching compiler symptoms, not proven root causes. Inspect shared dependencies before filing duplicate issues.\n\n");
    for (diagnostic, consumers) in shared {
        let _ = writeln!(
            output,
            "- **{}** — {}: {}",
            escape(
                diagnostic
                    .code
                    .as_ref()
                    .map(|c| c.code.as_str())
                    .unwrap_or("error")
            ),
            escape(&clip(&diagnostic.message, 300)),
            consumers
                .iter()
                .map(|r| format!("[{}](#{})", escape(&r.name), result_id(r)))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    output.push_str("\n</details>\n");
}

fn render_consumer(
    output: &mut String,
    result: &DownstreamResult,
    draft: Option<&IssueDraft>,
    concise: bool,
    prefix: &str,
) {
    let id = result_id(result);
    let (_, icon, label) = result_status(result);
    let _ = writeln!(
        output,
        "\n<a name=\"{id}\"></a>\n\n## {icon} {} · {label}\n",
        escape(&result.name)
    );
    if let Some(message) = &result.message {
        let _ = writeln!(output, "{}\n", escape(&clip(message, 2_000)));
    }
    if let Some(failure) = &result.lifecycle.failure {
        let _ = writeln!(
            output,
            "**Failure stage:** {:?} · {:?}\n\n{}\n",
            failure.stage,
            failure.cause,
            escape(&clip(&failure.message, 2_000))
        );
    }
    if let Some(draft) = draft {
        if !concise {
            let _ = write!(
                output,
                "[Download issue draft]({prefix}issues/{}) · [Reproduction recipe]({prefix}issues/{})",
                draft.filename, draft.reproduction_filename
            );
        }
        if let Some(composer) = &draft.composer_url {
            let _ = write!(
                output,
                "{}[Open issue form](<{composer}>)",
                if concise { "" } else { " · " }
            );
        }
        output.push_str("\n\nThe issue form is a draft for human review; nothing is submitted automatically.\n\n");
    }
    if let Some(revision) = &result.revision {
        let _ = writeln!(
            output,
            "Tested revision: {} · Manifest: {}\n",
            inline(revision),
            inline(&result.manifest.to_string_lossy())
        );
    }
    let build = selected_build(result);
    if !concise && let Some(selected) = &build.selected_library {
        for path in dependency_paths(build, selected) {
            let _ = writeln!(
                output,
                "Resolved dependency path: {}\n",
                inline(&path_label(&path))
            );
        }
    }
    let diagnostics = errors(build);
    let maximum = if concise { 2 } else { 8 };
    for diagnostic in diagnostics.iter().take(maximum) {
        let code = diagnostic
            .code
            .as_ref()
            .map(|c| c.code.as_str())
            .unwrap_or("error");
        let _ = writeln!(
            output,
            "### {}: {}\n",
            escape(code),
            escape(&clip(&diagnostic.message, 1_000))
        );
        if let Some(package) = &diagnostic.package {
            let _ = writeln!(
                output,
                "Package: {} ({})\n",
                inline(&package.name),
                origin(package.origin)
            );
        }
        for (span, callsite) in source_spans(diagnostic) {
            let label = format!(
                "{}:{}:{}",
                span.file_name, span.line_start, span.column_start
            );
            let prefix = if callsite {
                "**Macro invocation:** "
            } else {
                "**Source:** "
            };
            if let Some(link) = source_link(result, diagnostic, span) {
                let _ = writeln!(output, "{prefix}[{}](<{link}>)\n", escape(&label));
            } else {
                let _ = writeln!(output, "{prefix}{}\n", inline(&label));
            }
            let highlighted: Vec<_> = span
                .text
                .iter()
                .map(highlighted_text)
                .filter(|s| !s.is_empty())
                .take(3)
                .collect();
            if !highlighted.is_empty() {
                let _ = writeln!(
                    output,
                    "**Highlighted expression:** {}\n",
                    highlighted
                        .iter()
                        .map(|s| format!("**{}**", inline(s)))
                        .collect::<Vec<_>>()
                        .join(" · ")
                );
            }
            let snippet = span
                .text
                .iter()
                .take(12)
                .map(|line| clip(&line.text, 600))
                .collect::<Vec<_>>()
                .join("\n");
            if !snippet.is_empty() {
                fenced(output, "rust", &snippet);
            }
        }
        if let Some(rendered) = &diagnostic.rendered {
            output.push_str("<details>\n<summary>Exact compiler diagnostic</summary>\n\n");
            fenced(
                output,
                "text",
                &clip(rendered, if concise { 3_000 } else { 8_000 }),
            );
            output.push_str("</details>\n\n");
        }
    }
    if diagnostics.len() > maximum {
        let _ = writeln!(
            output,
            "**{} additional unique errors** are available in report.json.\n",
            diagnostics.len() - maximum
        );
    }
    if super::view::outcome(result) == Some(Classification::HarnessFailure) {
        output.push_str("<details>\n<summary>Environment failure and retry recipe</summary>\n\n");
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
                let _ = writeln!(output, "**{phase}**\n");
                fenced(output, "text", &clip(&context, 4_000));
            }
        }
        let _ = writeln!(
            output,
            "Retry this consumer with a complete recipe override for {}. Correct the runner, features, package selection, target, or system dependencies, then rerun with the same work directory.\n",
            inline(&result.name)
        );
        if let Ok(recipe) = toml::to_string_pretty(&result.recipe) {
            fenced(output, "toml", &recipe);
        }
        output.push_str("</details>\n\n");
    }
    if !build.provenance.runner_identity.is_empty() {
        output.push_str("<details>\n<summary>Compiler and build provenance</summary>\n\n");
        let _ = writeln!(
            output,
            "Runner: {}\n",
            inline(&build.provenance.runner_identity)
        );
        if let Some(rustc) = &build.provenance.rustc {
            fenced(output, "text", rustc);
        }
        if let Some(cargo) = &build.provenance.cargo {
            let _ = writeln!(output, "Cargo: {}\n", inline(cargo));
        }
        if let Some(lock) = &build.provenance.lock_fingerprint {
            let _ = writeln!(output, "Resolved lockfile: {}\n", inline(lock));
        }
        if !concise {
            for (phase, build) in [
                (BuildPhase::Baseline, &result.baseline),
                (BuildPhase::Candidate, &result.candidate),
            ] {
                if build.lockfile.as_ref().is_some_and(|lock| lock.verify()) {
                    let _ = writeln!(
                        output,
                        "[Download {} Cargo.lock]({prefix}{})\n",
                        phase.as_str(),
                        lock_path(result, phase)
                    );
                }
            }
        }
        output.push_str("</details>\n\n");
    }
}
