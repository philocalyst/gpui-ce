//! Local, reviewable issue bundles. Rendering cannot publish to an affected repository.

use std::fmt::Write;

use serde::Serialize;
use url::Url;

use crate::{
    config::{Config, DiscoveryConfig},
    forge::Repository,
    model::{Classification, DownstreamResult, DownstreamSource, DownstreamSpec, ImpactReport},
};

use super::presentation::{
    clip, errors, escape, fenced, inline, result_id, selected_build, source_link, source_spans,
};

#[derive(Clone, Debug, Serialize)]
pub struct IssueDraft {
    pub downstream: String,
    pub title: String,
    pub body: String,
    pub filename: String,
    pub reproduction_filename: String,
    pub reproduction_config: String,
    pub repository: Option<Repository>,
    pub composer_url: Option<Url>,
}

impl ImpactReport {
    pub fn issue_drafts(&self) -> Vec<IssueDraft> {
        self.downstreams
            .iter()
            .filter(|r| r.classification == Classification::Regression)
            .map(|result| draft(self, result))
            .collect()
    }
}

fn draft(report: &ImpactReport, result: &DownstreamResult) -> IssueDraft {
    let id = result_id(result);
    let title = format!(
        "{} compatibility regression in {}",
        clip(&report.library, 70),
        clip(&result.name, 70)
    );
    let repository = match &result.source {
        DownstreamSource::Git { url, forge, .. } => Repository::parse(url, *forge).ok(),
        DownstreamSource::Local { .. } => None,
    };
    let source = match (&result.source, &result.revision) {
        (DownstreamSource::Git { url, forge, .. }, Some(revision)) => DownstreamSource::Git {
            url: url.clone(),
            revision: revision.clone(),
            forge: *forge,
        },
        _ => result.source.clone(),
    };
    let mut config = Config {
        library: Some(report.library.clone()),
        recipe: result.recipe.clone(),
        execution: report.run.execution.clone(),
        discovery: DiscoveryConfig {
            crates_io: false,
            github: false,
            ..Default::default()
        },
        downstreams: vec![DownstreamSpec {
            name: result.name.clone(),
            source,
            manifest: result.manifest.clone(),
            recipe: None,
        }],
        ..Default::default()
    };
    config.execution.minimum_exercised = 1;
    let reproduction_config = toml::to_string_pretty(&config)
        .unwrap_or_else(|error| format!("# Could not export configuration: {error}\n"));
    let mut body = format!(
        "## Observed compatibility regression\n\n{} compiled successfully with the baseline {} source and failed with the candidate source. This is a compile-time comparison of the configuration below.\n\n",
        inline(&result.name),
        inline(&report.library)
    );
    if result.source_fingerprint.is_none() {
        body.push_str("> **Legacy or incomplete provenance:** this report did not record the consumer fingerprint, selected manifest, and compiler environment together. Verify the selected manifest and supply the original lockfile before using the starter reproduction configuration below.\n\n");
    }
    if let Some(revision) = &result.revision {
        let _ = writeln!(body, "Tested downstream revision: {}.\n", inline(revision));
    } else {
        body.push_str("The consumer was a local snapshot. Supply that same source snapshot when reproducing; no remote revision is inferred.\n\n");
    }
    if let Some(fingerprint) = &result.source_fingerprint {
        let _ = writeln!(
            body,
            "Consumer source fingerprint: {}.\n",
            inline(fingerprint)
        );
    }
    let _ = writeln!(
        body,
        "| Input | Source fingerprint |\n| --- | --- |\n| Baseline | {} |\n| Candidate | {} |\n",
        inline(
            report
                .baseline_fingerprint
                .as_deref()
                .unwrap_or("unavailable")
        ),
        inline(
            report
                .candidate_fingerprint
                .as_deref()
                .unwrap_or("unavailable")
        )
    );
    if let Some(upstream) = &report.run.upstream {
        body.push_str("Upstream revisions supplied by the caller (source fingerprints above identify the tested snapshots):\n\n");
        for (phase, sha) in [
            ("Baseline", &upstream.baseline_sha),
            ("Candidate", &upstream.candidate_sha),
        ] {
            if let Some(sha) = sha {
                let _ = writeln!(body, "- {phase}: {}", inline(sha));
            }
        }
        if let Some(repository) = &upstream.repository {
            let _ = writeln!(body, "- Repository: <{}>", repository.url());
        }
        body.push(char::from(10));
    }
    let build = selected_build(result);
    let diagnostics = errors(build);
    for diagnostic in diagnostics.iter().take(4) {
        let code = diagnostic
            .code
            .as_ref()
            .map(|c| c.code.as_str())
            .unwrap_or("compiler error");
        let _ = writeln!(
            body,
            "### {} · {}\n",
            inline(code),
            escape(&clip(&diagnostic.message, 600))
        );
        for (span, callsite) in source_spans(diagnostic).into_iter().take(2) {
            let label = format!(
                "{}:{}:{}",
                span.file_name, span.line_start, span.column_start
            );
            let prefix = if callsite { "Macro invocation: " } else { "" };
            if let Some(link) = source_link(result, diagnostic, span) {
                let _ = writeln!(body, "{prefix}[{}](<{link}>)\n", escape(&label));
            } else {
                let _ = writeln!(body, "{prefix}{}\n", inline(&label));
            }
            let snippet = span
                .text
                .iter()
                .take(12)
                .map(|line| clip(&line.text, 600))
                .collect::<Vec<_>>()
                .join("\n");
            if !snippet.is_empty() {
                fenced(&mut body, "rust", &snippet);
            }
        }
        if let Some(rendered) = &diagnostic.rendered {
            fenced(&mut body, "text", &clip(rendered, 4_000));
        }
    }
    if diagnostics.len() > 4 {
        let _ = writeln!(
            body,
            "{} additional unique errors are retained in the full report.\n",
            diagnostics.len() - 4
        );
    }
    body.push_str("## Reproduction\n\nSave the accompanying configuration, obtain the baseline and candidate library sources identified above, and run:\n\n");
    fenced(
        &mut body,
        "sh",
        &format!(
            "cargo impact check --library {} \\\n  --baseline PATH_TO_BASELINE --candidate PATH_TO_CANDIDATE \\\n  --config {id}.toml --force --no-discovery",
            shell_arg(&report.library)
        ),
    );
    if !result.recipe.runner.is_isolated() {
        body.push_str("The recorded recipe executes on the host; reproducing trusted local/Nix inputs requires `--allow-local`.\n\n");
    }
    if matches!(result.recipe.runner, crate::runner::Runner::Docker { .. }) {
        body.push_str("This starter configuration retains the image reference from the recipe. A floating tag can resolve differently on replay; compare the recorded image identity below, pin an available repository digest, and supply the original resolved Cargo.lock.\n\n");
    }
    let _ = writeln!(
        body,
        "Selected manifest: {}.\n",
        inline(&result.manifest.to_string_lossy())
    );
    fenced(&mut body, "toml", &reproduction_config);
    if let Some(rustc) = &build.provenance.rustc {
        body.push_str("Recorded compiler:\n\n");
        fenced(&mut body, "text", rustc);
    }
    if let Some(cargo) = &build.provenance.cargo {
        let _ = writeln!(body, "Recorded Cargo: {}.\n", inline(cargo));
    }
    if !build.provenance.runner_identity.is_empty() {
        let _ = writeln!(
            body,
            "Runner identity: {}.\n",
            inline(&build.provenance.runner_identity)
        );
    }
    if let Some(lock) = &build.provenance.lock_fingerprint {
        let _ = writeln!(body, "Resolved Cargo.lock fingerprint: {}.\n", inline(lock));
    }
    body.push_str("Attach this draft, its reproduction TOML, and the original report.json. Review whether the API change is intentional before submitting. No issue or notification was sent by cargo-impact.\n");
    let first = diagnostics.first();
    let compact = format!(
        "A cargo-impact comparison found a compile-time regression in {} when replacing {} with a candidate source. The baseline compiled successfully.\n\n{}\n\nTested downstream revision: {}.\n\nPlease attach the full issue Markdown, reproduction TOML, and report.json from the report bundle. Review this draft before submitting.",
        clip(&result.name, 160),
        clip(&report.library, 100),
        first
            .map(|d| format!(
                "{}: {}",
                d.code.as_ref().map(|c| c.code.as_str()).unwrap_or("error"),
                clip(&d.message, 400)
            ))
            .unwrap_or_default(),
        result.revision.as_deref().unwrap_or("local snapshot")
    );
    let composer_url = repository
        .as_ref()
        .and_then(|repo| repo.issue_composer(&title, &super::presentation::neutral(&compact)));
    IssueDraft {
        downstream: result.name.clone(),
        title,
        body,
        filename: format!("{id}.md"),
        reproduction_filename: format!("{id}.toml"),
        reproduction_config,
        repository,
        composer_url,
    }
}

fn shell_arg(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
