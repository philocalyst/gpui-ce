use annotate_snippets::{AnnotationKind, Group, Level, Snippet};
use regex::Regex;
use serde_yaml::Value;
use std::{
    collections::HashMap,
    fs,
    ops::Range,
    path::{Path, PathBuf},
    sync::LazyLock,
};

use crate::error::XtaskError;

static GITHUB_INPUT_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\$\{\{[[:blank:]]*([[:alnum:]]|[[:punct:]])+?[[:blank:]]*\}\}"#)
        .expect("Should compile")
});

pub struct WorkflowFile {
    raw_content: String,
    pub parsed_content: Value,
}

impl WorkflowFile {
    pub fn load(workflow_file_path: &Path) -> std::result::Result<Self, XtaskError> {
        let raw_content = fs::read_to_string(workflow_file_path).map_err(|_| {
            XtaskError::Message { details: format!(
                "Could not read workflow file at {}",
                workflow_file_path.display()
            ) }
        })?;
        let parsed_content = serde_yaml::from_str(&raw_content)
            .map_err(|e| XtaskError::Message { details: format!("Failed to parse workflow file: {e:?}") })?;
        Ok(Self {
            raw_content,
            parsed_content,
        })
    }
}

pub struct WorkflowValidationError {
    file_path: PathBuf,
    contents: WorkflowFile,
    errors: Vec<RunValidationError>,
}

impl WorkflowValidationError {
    pub fn new(
        errors: Vec<RunValidationError>,
        contents: WorkflowFile,
        file_path: PathBuf,
    ) -> Self {
        Self {
            file_path,
            contents,
            errors,
        }
    }

    pub fn annotation_group<'a>(&'a self) -> Group<'a> {
        let raw_content = &self.contents.raw_content;
        let mut identical_lines = HashMap::new();

        let ranges = self
            .errors
            .iter()
            .flat_map(|error| error.found_injection_patterns.iter())
            .map(|(line, pattern_range)| {
                let initial_offset = identical_lines
                    .get(&(line.as_str(), pattern_range.start))
                    .copied()
                    .unwrap_or_default();

                let line_start = raw_content[initial_offset..]
                    .find(line.as_str())
                    .map(|offset| offset + initial_offset)
                    .unwrap_or_default();

                let pattern_start = line_start + pattern_range.start;
                let pattern_end = pattern_start + pattern_range.len();

                identical_lines.insert((line.as_str(), pattern_range.start), pattern_end);

                pattern_start..pattern_end
            });

        Level::ERROR
            .primary_title("Found GitHub input injection in run command")
            .element(
                Snippet::source(&self.contents.raw_content)
                    .path(self.file_path.display().to_string())
                    .annotations(ranges.map(|range| {
                        AnnotationKind::Primary
                            .span(range)
                            .label("This should be passed via an environment variable")
                    })),
            )
    }
}

pub struct RunValidationError {
    found_injection_patterns: Vec<(String, Range<usize>)>,
}

pub fn validate_run_command(command: &str) -> std::result::Result<(), RunValidationError> {
    let patterns: Vec<_> = command
        .lines()
        .flat_map(move |line| {
            GITHUB_INPUT_PATTERN
                .find_iter(line)
                .map(|m| (line.to_owned(), m.range()))
        })
        .collect();

    if patterns.is_empty() {
        Ok(())
    } else {
        Err(RunValidationError {
            found_injection_patterns: patterns,
        })
    }
}
