//! Source links: opening a construction site in the user's editor, and
//! telling the app's own code apart from library code.
//!
//! `Location::file()` is relative to the workspace root for crates built
//! from a workspace (`crates/app/src/list.rs`) and absolute for registry and
//! git dependencies, so a path is resolved against the working directory (or
//! the nearest ancestor that contains it) before it becomes an editor URL.

use gpui::inspector::ElementKind;
use std::path::{Path, PathBuf};

/// How Loupe opens a source location in an editor: a URL template with
/// `{path}`, `{line}` and `{col}` placeholders.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EditorUrl {
    /// `zed://file/{path}:{line}:{col}`.
    #[default]
    Zed,
    /// `vscode://file/{path}:{line}:{col}`.
    VsCode,
    /// `cursor://file/{path}:{line}:{col}`.
    Cursor,
    /// JetBrains IDEs: `idea://open?file={path}&line={line}&column={col}`.
    Idea,
    /// Any other editor's scheme, e.g. `subl://open?url=file://{path}&line={line}`.
    Custom(&'static str),
}

impl EditorUrl {
    /// Every built-in editor, for pickers.
    pub const PRESETS: [EditorUrl; 4] = [
        EditorUrl::Zed,
        EditorUrl::VsCode,
        EditorUrl::Cursor,
        EditorUrl::Idea,
    ];

    /// The URL template.
    pub fn template(self) -> &'static str {
        match self {
            EditorUrl::Zed => "zed://file/{path}:{line}:{col}",
            EditorUrl::VsCode => "vscode://file/{path}:{line}:{col}",
            EditorUrl::Cursor => "cursor://file/{path}:{line}:{col}",
            EditorUrl::Idea => "idea://open?file={path}&line={line}&column={col}",
            EditorUrl::Custom(template) => template,
        }
    }

    /// The editor's name, for tooltips: "Open in Zed".
    pub fn label(self) -> &'static str {
        match self {
            EditorUrl::Zed => "Zed",
            EditorUrl::VsCode => "VS Code",
            EditorUrl::Cursor => "Cursor",
            EditorUrl::Idea => "IntelliJ",
            EditorUrl::Custom(_) => "editor",
        }
    }

    /// The URL that opens `path` (absolute) at `line`:`column`.
    pub fn url(self, path: &str, line: u32, column: u32) -> String {
        editor_url(self.template(), path, line, column)
    }
}

/// Fills an editor URL template. `{path}` is percent-encoded where URLs
/// need it, and an absolute path directly after a path `/` in the template
/// does not double the slash (`zed://file/{path}` + `/src/a.rs` is
/// `zed://file/src/a.rs`, as editors expect).
pub fn editor_url(template: &str, path: &str, line: u32, column: u32) -> String {
    let path = path.replace('\\', "/");
    let mut url = String::with_capacity(template.len() + path.len());
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        let Some(len) = rest[start..].find('}') else {
            break;
        };
        let (before, placeholder) = (&rest[..start], &rest[start + 1..start + len]);
        url.push_str(before);
        match placeholder {
            "path" => {
                // `file/` + `/src` must not become `file//src`, but a
                // `file://` authority keeps the path's own slash.
                let path = if url.ends_with('/') && !url.ends_with("//") {
                    path.trim_start_matches('/')
                } else {
                    &path
                };
                url.push_str(&percent_encode(path));
            }
            "line" => url.push_str(&line.to_string()),
            "col" | "column" => url.push_str(&column.to_string()),
            other => {
                url.push('{');
                url.push_str(other);
                url.push('}');
            }
        }
        rest = &rest[start + len + 1..];
    }
    url.push_str(rest);
    url
}

/// Escapes the characters that would end or corrupt a URL path or query.
fn percent_encode(path: &str) -> String {
    let mut encoded = String::with_capacity(path.len());
    for c in path.chars() {
        match c {
            ' ' => encoded.push_str("%20"),
            '%' => encoded.push_str("%25"),
            '#' => encoded.push_str("%23"),
            '?' => encoded.push_str("%3F"),
            '&' => encoded.push_str("%26"),
            c => encoded.push(c),
        }
    }
    encoded
}

/// An absolute path for a `Location::file()`: absolute paths are kept;
/// relative ones are joined to the nearest of `cwd` and its ancestors in
/// which the file `exists` (workspace crates record paths relative to the
/// workspace root, while the process may run from a crate directory), or
/// to `cwd` itself if none has it.
pub fn resolve_source_path(file: &str, cwd: &Path, exists: impl Fn(&Path) -> bool) -> PathBuf {
    let path = Path::new(file);
    if path.is_absolute() {
        return path.to_path_buf();
    }
    cwd.ancestors()
        .map(|dir| dir.join(path))
        .find(|candidate| exists(candidate))
        .unwrap_or_else(|| cwd.join(path))
}

/// Crates that make up gpui itself (published and workspace names). They
/// count as library code even when built from a local checkout.
const GPUI_CRATES: &[&str] = &[
    "gpui",
    "gpui_ce",
    "gpui_elements",
    "gpui_ce_elements",
    "gpui_macros",
    "gpui_ce_macros",
    "gpui_platform",
    "gpui_wgpu",
    "gpui_util",
    "gpui_tokio",
];

/// The Rust standard library's crates.
const STD_CRATES: &[&str] = &["std", "core", "alloc"];

/// Whether a source path is library code rather than the application's
/// own: gpui (wherever it is built from), any registry or git dependency,
/// or the standard library. `crates/gpui/src/elements/div.rs`,
/// `…/registry/src/…/ui_kit-0.3.0/src/button.rs` and
/// `/rustc/<hash>/library/core/src/ops/function.rs` are; `src/list.rs` and
/// `crates/inbox/src/main.rs` are not.
pub fn is_library_path(path: &str) -> bool {
    let path = path.replace('\\', "/");
    path.starts_with("/rustc/")
        || path.contains("/registry/src/")
        || path.contains("/git/checkouts/")
        || workspace_crate(&path).is_some_and(|krate| GPUI_CRATES.contains(&krate))
}

/// Whether `type_name` is one of gpui's (or the standard library's) own
/// types, e.g. `gpui::window::Root`, as opposed to the application's.
pub fn is_library_type(type_name: &str) -> bool {
    let krate = type_name.split("::").next().unwrap_or_default();
    GPUI_CRATES.contains(&krate) || STD_CRATES.contains(&krate)
}

/// Whether an element comes from library code: views and components by
/// their type (a view drawn from inside gpui is still the app's view),
/// other elements by where they were constructed. `None` for elements
/// without a source location.
pub fn is_library_element(kind: &ElementKind, source: Option<&str>) -> Option<bool> {
    match kind {
        ElementKind::View { type_name, .. } | ElementKind::Component { type_name } => {
            Some(is_library_type(type_name))
        }
        ElementKind::Element { .. } => source.map(is_library_path),
    }
}

/// The workspace crate a path is in: the directory after the last `crates/`.
fn workspace_crate(path: &str) -> Option<&str> {
    let rest = if let Some(rest) = path.strip_prefix("crates/") {
        rest
    } else {
        let ix = path.rfind("/crates/")?;
        &path[ix + "/crates/".len()..]
    };
    rest.split('/').next()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::EntityId;

    #[test]
    fn editor_urls_fill_every_placeholder() {
        let path = "/home/me/app/src/list.rs";
        assert_eq!(
            EditorUrl::Zed.url(path, 52, 9),
            "zed://file/home/me/app/src/list.rs:52:9"
        );
        assert_eq!(
            EditorUrl::VsCode.url(path, 52, 9),
            "vscode://file/home/me/app/src/list.rs:52:9"
        );
        assert_eq!(
            EditorUrl::Idea.url(path, 52, 9),
            "idea://open?file=/home/me/app/src/list.rs&line=52&column=9"
        );
        assert_eq!(
            EditorUrl::Custom("subl://open?url=file://{path}&line={line}").url(path, 3, 1),
            "subl://open?url=file:///home/me/app/src/list.rs&line=3"
        );
    }

    #[test]
    fn editor_urls_escape_paths_and_keep_unknown_placeholders() {
        assert_eq!(
            editor_url("zed://file/{path}:{line}", "/tmp/my app/#1?.rs", 1, 1),
            "zed://file/tmp/my%20app/%231%3F.rs:1"
        );
        assert_eq!(
            editor_url("x://{path}{nope}{col", "C:\\src\\a.rs", 1, 2),
            "x://C:/src/a.rs{nope}{col"
        );
    }

    #[test]
    fn relative_sources_resolve_against_the_nearest_ancestor_that_has_them() {
        let cwd = Path::new("/work/repo/crates/app");
        let exists = |path: &Path| path == Path::new("/work/repo/crates/app/src/main.rs");
        assert_eq!(
            resolve_source_path("crates/app/src/main.rs", cwd, |path| {
                path == Path::new("/work/repo/crates/app/src/main.rs")
            }),
            PathBuf::from("/work/repo/crates/app/src/main.rs")
        );
        assert_eq!(
            resolve_source_path("src/main.rs", cwd, exists),
            PathBuf::from("/work/repo/crates/app/src/main.rs")
        );
        assert_eq!(
            resolve_source_path("src/gone.rs", cwd, exists),
            PathBuf::from("/work/repo/crates/app/src/gone.rs"),
            "falls back to the working directory"
        );
        assert_eq!(
            resolve_source_path("/abs/lib.rs", cwd, |_| false),
            PathBuf::from("/abs/lib.rs")
        );
    }

    #[test]
    fn library_paths_are_gpui_dependencies_and_std() {
        for path in [
            "crates/gpui/src/elements/div.rs",
            "/work/gpui-ce/crates/gpui_elements/src/editable_text.rs",
            "/home/me/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui_ce-0.3.0/src/app.rs",
            "/home/me/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/ui_kit-1.0.2/src/button.rs",
            "/home/me/.cargo/git/checkouts/zed-a1b2c3d4/5e6f7a8/crates/ui/src/button.rs",
            "/rustc/90b35a6239c3d8bdabc530a6a0816f7ff89a0aaf/library/core/src/ops/function.rs",
            "C:\\work\\crates\\gpui\\src\\view.rs",
        ] {
            assert!(is_library_path(path), "{path}");
        }
        for path in [
            "src/issue_list.rs",
            "crates/inbox/src/main.rs",
            "/work/app/crates/ui/src/button.rs",
            "crates/gpui_inspector/src/fixtures.rs",
            "examples/gpui_demo.rs",
            "",
        ] {
            assert!(!is_library_path(path), "{path}");
        }
    }

    #[test]
    fn views_are_classified_by_type_and_elements_by_source() {
        let view = ElementKind::View {
            entity: EntityId::from(1),
            type_name: "inbox::IssueList",
        };
        let root = ElementKind::View {
            entity: EntityId::from(2),
            type_name: "gpui::window::Root",
        };
        let div = ElementKind::Element {
            type_name: "gpui::elements::div::Div",
        };
        let gpui_path = Some("crates/gpui/src/element.rs");
        assert_eq!(is_library_element(&view, gpui_path), Some(false));
        assert_eq!(is_library_element(&root, None), Some(true));
        assert_eq!(is_library_element(&div, gpui_path), Some(true));
        assert_eq!(is_library_element(&div, Some("src/main.rs")), Some(false));
        assert_eq!(is_library_element(&div, None), None);
    }
}
