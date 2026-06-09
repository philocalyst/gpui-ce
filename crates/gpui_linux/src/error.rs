use thiserror::Error;

#[cfg(feature = "x11")]
use x11rb::errors::{ConnectionError, ReplyError, ReplyOrIdError};

#[derive(Error, Debug)]
pub enum LinuxError {
    #[cfg(feature = "x11")]
    #[error(transparent)]
    X11(#[from] X11Error),

    #[cfg(feature = "wayland")]
    #[error(transparent)]
    Wayland(#[from] WaylandError),

    #[error("missing xdg-desktop-portal implementation")]
    FilePickerPortalMissing,

    #[cfg(feature = "ashpd")]
    #[error("desktop portal error: {0}")]
    Portal(#[from] ashpd::Error),

    #[error("screen capture unsupported")]
    ScreenCaptureUnsupported,

    #[error("feature {0:?} not compiled")]
    FeatureNotCompiled(Feature),

    #[error("headless error: {0:?}")]
    HeadlessIncompatible(HeadlessError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("XKB error")]
    Xkb,

    #[error("not implemented: {0:?}")]
    NotImplemented(PlatformMethod),

    #[error("no display found")]
    NoDisplay,

    #[error("calloop error: {0}")]
    Calloop(#[from] calloop::Error),

    #[error("unsupported compositor: {0}")]
    UnsupportedCompositor(String),

    #[error("failed to initialize {0:?}")]
    InitializationFailed(PlatformComponent),

    #[error("oo7 error: {0}")]
    Oo7(#[from] oo7::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    ScreenCapture,
    Wayland,
    X11,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformMethod {
    RegisterUrlScheme,
    PathForAuxiliaryExecutable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformComponent {
    X11Client,
    WaylandClient,
    EventLoop,
    ResourceDatabase,
    CursorTheme,
    Clipboard,
    XkbExtension,
    XinputExtension,
    Keyring,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadlessError {
    NoDisplay,
    NeitherDisplayNorWaylandDisplaySet,
    HeadlessModeDoesNotSupportScreenCapture,
}

#[cfg(feature = "x11")]
#[derive(Error, Debug)]
pub enum X11Error {
    #[error("connection error: {0}")]
    Connection(#[from] ConnectionError),
    #[error("reply error: {0}")]
    Reply(#[from] ReplyError),
    #[error("reply or ID error: {0}")]
    ReplyOrId(#[from] ReplyOrIdError),
    #[error("parse error: {0}")]
    Parse(#[from] x11rb::errors::ParseError),
    
    #[error("X11 {0:?} failed: {1}")]
    Request(X11Request, #[source] ReplyError),

    #[error("event loop error: {0}")]
    EventLoop(String),
    #[error("window error (screen index {screen_index}): no screen found")]
    ScreenNotFound { screen_index: usize, total_screens: usize },
    #[error("atom error for name {name:?}: {source}")]
    Atom { name: String, #[source] source: ReplyError },
    #[error("clipboard error: {0}")]
    Clipboard(String),
    #[error("cursor icon not found: requested {requested:?}, fallback '{fallback}' also missing")]
    Cursor { requested: String, fallback: String },
    #[error("IME error: {0}")]
    Ime(String),
    #[error("drag-and-drop error: {0}")]
    Dnd(String),
    #[error("render error: {0}")]
    Render(String),
}

#[cfg(feature = "x11")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum X11Request {
    Generic,
    GetGeometry,
    CreateColormap,
    CreateWindow,
    ChangeProperty,
    QueryPointer,
    SetInputFocus,
    SendEvent,
    XiQueryVersion,
    XkbUseExtension,
    XkbSelectEvents,
    MapWindow,
    ConfigureWindow,
    InternAtom,
}

#[cfg(feature = "wayland")]
#[derive(Error, Debug)]
pub enum WaylandError {
    #[error("connection error: {0}")]
    Connection(String),
    #[error("window error: {0}")]
    Window(String),
    #[error("cursor theme loading failed: {0}")]
    Cursor(String),
    #[error("display name missing for output {id:?}")]
    Display { id: String },
    #[error("protocol error: {0}")]
    Protocol(String),
}

pub type Result<T> = std::result::Result<T, LinuxError>;
