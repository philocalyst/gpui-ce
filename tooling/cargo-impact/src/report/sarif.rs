//! SARIF 2.1 output for review tools. Foreign consumer locations stay commit-pinned.

use std::{collections::BTreeMap, path::Component};

use serde_json::{Value, json};

use crate::model::{Classification, ImpactReport, RunStatus};

use super::presentation::{
    clip, diagnostic_key, errors, selected_build, source_link, source_spans,
};

pub(super) fn render(report: &ImpactReport) -> Value {
    let mut rules = BTreeMap::new();
    let mut results = Vec::new();
    for consumer in report
        .downstreams
        .iter()
        .filter(|r| r.classification != Classification::Compatible)
    {
        for diagnostic in errors(selected_build(consumer)) {
            let code = diagnostic
                .code
                .as_ref()
                .map(|c| c.code.as_str())
                .unwrap_or("compiler-error");
            let rule_id = format!("rustc/{code}");
            let rule = rules.entry(rule_id.clone()).or_insert_with(|| {
                json!({
                    "id": rule_id,
                    "name": code,
                    "shortDescription": {"text": clip(&diagnostic.message, 200)},
                    "defaultConfiguration": {"level": "error"}
                })
            });
            if code.len() == 5
                && code.starts_with('E')
                && code[1..].chars().all(|c| c.is_ascii_digit())
            {
                rule["helpUri"] =
                    json!(format!("https://doc.rust-lang.org/error_codes/{code}.html"));
            }
            let mut locations = Vec::new();
            for (span, _) in source_spans(diagnostic) {
                let Some(file) = diagnostic.source_files.get(&span.file_name) else {
                    continue;
                };
                if file.is_absolute()
                    || !file.components().all(|c| matches!(c, Component::Normal(_)))
                {
                    continue;
                }
                let uri = source_link(consumer, diagnostic, span)
                    .map(|url| url.to_string())
                    .unwrap_or_else(|| {
                        let mut url =
                            url::Url::parse("https://relative.invalid/").expect("constant URL");
                        url.path_segments_mut()
                            .expect("hierarchical URL")
                            .extend(file.components().map(|c| c.as_os_str().to_string_lossy()));
                        url.path().trim_start_matches('/').to_owned()
                    });
                let mut region = json!({"startLine": span.line_start.max(1), "startColumn":span.column_start.max(1)});
                if span.line_end >= span.line_start
                    && (span.line_end > span.line_start || span.column_end > span.column_start)
                {
                    region["endLine"] = json!(span.line_end);
                    region["endColumn"] = json!(span.column_end.max(1));
                }
                locations.push(
                    json!({"physicalLocation":{"artifactLocation":{"uri":uri},"region":region}}),
                );
            }
            results.push(json!({
                "ruleId":rule_id,
                "level":if consumer.classification == Classification::Regression {"error"} else {"warning"},
                "message":{"text":format!("{}: {}", consumer.name, diagnostic.message)},
                "locations":locations,
                "partialFingerprints":{"cargo-impact/compiler-symptom":diagnostic_key(diagnostic)},
                "properties":{"consumer":consumer.name,"classification":consumer.classification,"revision":consumer.revision,"package":diagnostic.package,"sourceFingerprint":consumer.source_fingerprint}
            }));
        }
    }
    let mut notifications = Vec::new();
    if let Some(error) = &report.error {
        notifications.push(json!({"level":"error","message":{"text":clip(error,8_000)}}));
    }
    for consumer in report
        .downstreams
        .iter()
        .filter(|r| r.classification == Classification::HarnessFailure)
    {
        notifications.push(json!({"level":"error","message":{"text":format!("{}: {}",consumer.name,consumer.message.as_deref().unwrap_or("build environment failed; inspect report.json"))}}));
    }
    if report.run.status == RunStatus::Running {
        notifications.push(json!({"level":"warning","message":{"text":"Partial experiment: missing consumers are inconclusive."}}));
    }
    if report.run.coverage_sufficient == Some(false) {
        notifications.push(json!({"level":"warning","message":{"text":format!("Insufficient exercised coverage: {} consumers; required {}.",report.run.exercised_downstreams,report.run.execution.minimum_exercised)}}));
    }
    json!({
        "$schema":"https://json.schemastore.org/sarif-2.1.0.json",
        "version":"2.1.0",
        "runs":[{
            "tool":{"driver":{"name":"cargo-impact","version":env!("CARGO_PKG_VERSION"),"rules":rules.into_values().collect::<Vec<_>>() }},
            "results":results,
            "invocations":[{"executionSuccessful": !report.has_harness_failures() && report.run.status == RunStatus::Complete && report.run.coverage_sufficient != Some(false),"toolExecutionNotifications":notifications}],
            "properties":{"library":report.library,"runStatus":report.run.status,"baselineFingerprint":report.baseline_fingerprint,"candidateFingerprint":report.candidate_fingerprint}
        }]
    })
}
