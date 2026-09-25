//! Trace export: the whole recording as Chrome trace JSON, for
//! ui.perfetto.dev or `chrome://tracing`.

use crate::analysis::{format, trace::chrome_trace_json};
use gpui::{Global, inspector::InspectorCapture};
use std::path::PathBuf;

/// Where traces can be opened.
pub(crate) const PERFETTO_URL: &str = "https://ui.perfetto.dev";

/// A directory "Export trace" saves into without asking where. Set it as a
/// global for headless runs and tests (which have no file dialogs), or to
/// always export to the same place.
#[derive(Clone, Debug)]
pub struct ExportDirectory(pub PathBuf);

impl Global for ExportDirectory {}

/// A recording, serialized for export.
pub(crate) struct Trace {
    /// Chrome trace JSON.
    pub json: String,
    /// Frames in it.
    pub frames: usize,
    /// A file name that says what it holds: `loupe-frames-12-251.json`.
    pub file_name: String,
}

impl Trace {
    /// Serializes every frame and input record in `capture`, naming the
    /// process after the window's `title`.
    pub fn of(capture: &InspectorCapture, title: &str) -> Self {
        let frames = capture.frames();
        let title = if title.trim().is_empty() {
            "gpui window"
        } else {
            title
        };
        Self {
            json: chrome_trace_json(frames, capture.input(), title),
            frames: frames.len(),
            file_name: file_name(
                frames.front().map(|frame| frame.id),
                frames.back().map(|frame| frame.id),
            ),
        }
    }

    /// `240 frames (1.2 MB)`.
    pub fn summary(&self) -> String {
        let frames = match self.frames {
            1 => "1 frame".to_string(),
            count => format!("{} frames", format::count(count as u64)),
        };
        format!("{frames} ({})", format::bytes(self.json.len() as u64))
    }
}

fn file_name(first: Option<u64>, last: Option<u64>) -> String {
    match (first, last) {
        (Some(first), Some(last)) => format!("loupe-frames-{first}-{last}.json"),
        _ => "loupe-trace.json".to_string(),
    }
}

/// Where the save dialog starts: the working directory, else the temp dir.
pub(crate) fn default_directory() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| std::env::temp_dir())
}

/// Writes `json` to `path`, returning the path written.
pub(crate) fn write(path: PathBuf, json: &str) -> std::io::Result<PathBuf> {
    std::fs::write(&path, json)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::steady_frames;

    #[test]
    fn traces_name_their_frames_and_parse() {
        let capture = steady_frames(3, 4.0);
        let trace = Trace::of(&capture, "  ");
        assert_eq!(trace.file_name, "loupe-frames-0-2.json");
        assert!(
            trace.summary().starts_with("3 frames ("),
            "{}",
            trace.summary()
        );
        let value: serde_json::Value = serde_json::from_str(&trace.json).unwrap();
        let process = &value["traceEvents"][0]["args"]["name"];
        assert_eq!(process, "gpui window");

        let empty = Trace::of(&InspectorCapture::new_for_test(), "Inbox");
        assert_eq!(empty.file_name, "loupe-trace.json");
        assert!(empty.summary().starts_with("0 frames"));
    }

    #[test]
    fn writing_returns_the_path() {
        let dir = std::env::temp_dir().join(format!("loupe-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = write(dir.join("t.json"), "{}").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
