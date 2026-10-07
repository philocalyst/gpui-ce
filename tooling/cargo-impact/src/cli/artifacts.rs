//! Offline artifact commands and the common scan exit/report contract.

use super::Result;
use cargo_impact::ImpactReport;
use std::path::Path;

pub(super) fn failed_check(report_dir: &Path, library: &str, error: impl ToString) -> Result<i32> {
    let report = ImpactReport::failed(library, &error.to_string());
    emit_report(report_dir, &report)?;
    Ok(2)
}

pub(super) fn emit_report(report_dir: &Path, report: &ImpactReport) -> Result<()> {
    report.write_artifacts(report_dir)?;
    let summary = cargo_impact::report::ReportSummary::from_report(report);
    println!(
        "{}: {} regressions, {} compatible, {} harness failures. {}",
        report.library,
        summary.regressions,
        summary.compatible,
        summary.harness_failures,
        report.gate.reason
    );
    println!("Inspect: {}", report_dir.join("index.html").display());
    Ok(())
}

pub(super) fn open_report(path: &Path) -> Result<()> {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    if !std::process::Command::new(program)
        .arg(path)
        .status()?
        .success()
    {
        return Err("could not open the report; open index.html manually".into());
    }
    Ok(())
}
