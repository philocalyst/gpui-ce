use thiserror::Error;

#[derive(Error, Debug)]
pub enum MacError {
    #[error("feature not compiled: {feature}")]
    FeatureNotCompiled { feature: &'static str },
    #[error("atlas allocation failed")]
    AtlasAllocation,
    #[error("app not running from a bundle")]
    BundleNotRunning,
    #[error("core video error (code={code}) during {operation}")]
    CoreVideo { code: i32, operation: &'static str },
    #[error("invalid size for operation {operation}: {width}x{height}")]
    InvalidSize { operation: &'static str, width: f64, height: f64 },
    #[error("{message}")]
    Message { message: String },
    #[error("macOS framework error: {description} (code={code})")]
    NSError { code: i64, domain: String, description: String },
    #[error("null display UUID for display ID {0}")]
    NullDisplayUUID(u32),
    #[error("render target too large: {width}x{height} (max {max_width}x{max_height})")]
    RenderTooLarge { width: u64, height: u64, max_width: u64, max_height: u64 },
    #[error("resource not found: {resource}")]
    ResourceNotFound { resource: String },
    #[error("security error (code={status}) during {operation}")]
    Security { status: i32, operation: String },
    #[error("thread policy configuration failed: {0}")]
    ThreadPolicy(String),
    #[error("URL scheme registration failed: {scheme} requires macOS 12+: {message}")]
    UrlScheme { scheme: String, message: String },
    #[error("screen capture error: {0}")]
    ScreenCapture(String),
    #[error("layer-backed renderer required but not available")]
    RenderRequiresLayer,
    #[error("no Metal drawable available")]
    NoDrawable,
    #[error("invalid image data: {0}")]
    InvalidImageData(String),
    #[error("instance buffer too large: {size} instances")]
    InstanceBufferTooLarge { size: usize },
    #[error("invalid render size: {width}x{height}")]
    InvalidRenderSize { width: f64, height: f64 },
    #[error("scene too large: {path_count} paths, {sprite_count} sprites, {quad_count} quads")]
    SceneTooLarge { path_count: usize, sprite_count: usize, quad_count: usize },
    #[error("glyph bounds are empty")]
    GlyphBoundsEmpty,
    #[error("failed to load embedded font: {0}")]
    EmbeddedFontLoad(String),
}

impl From<MacError> for gpui::PlatformError {
    fn from(error: MacError) -> Self {
        gpui::PlatformError::Other(std::sync::Arc::new(Box::new(error)))
    }
}

pub type Result<T> = std::result::Result<T, MacError>;
