//! Shared interpretation of current and legacy evidence for every report view.

use serde::Serialize;

use crate::model::{Classification, DownstreamResult, ExperimentStatus, ImpactReport};

/// Older reports lack lifecycle evidence; their recorded classifications remain readable.
pub(super) fn has_outcome(result: &DownstreamResult) -> bool {
    result.lifecycle.status == ExperimentStatus::Unknown || result.lifecycle.is_finished()
}

pub(super) fn outcome(result: &DownstreamResult) -> Option<Classification> {
    has_outcome(result).then(|| {
        if result.lifecycle.failure.is_some() {
            Classification::HarnessFailure
        } else {
            result.classification
        }
    })
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct ReportSummary {
    pub regressions: usize,
    pub compatible: usize,
    pub baseline_failures: usize,
    pub harness_failures: usize,
    pub not_exercised: usize,
    pub pending: usize,
}

impl ReportSummary {
    pub fn from_report(report: &ImpactReport) -> Self {
        let mut summary = Self::default();
        for result in &report.downstreams {
            let Some(outcome) = outcome(result) else {
                summary.pending += 1;
                continue;
            };
            match outcome {
                Classification::Regression => summary.regressions += 1,
                Classification::Compatible => summary.compatible += 1,
                Classification::PreExistingFailure => summary.baseline_failures += 1,
                Classification::HarnessFailure => summary.harness_failures += 1,
                Classification::NotExercised => summary.not_exercised += 1,
            }
        }
        summary
    }

    pub(super) fn count(self, outcome: Classification) -> usize {
        match outcome {
            Classification::Regression => self.regressions,
            Classification::Compatible => self.compatible,
            Classification::PreExistingFailure => self.baseline_failures,
            Classification::HarnessFailure => self.harness_failures,
            Classification::NotExercised => self.not_exercised,
        }
    }
}
