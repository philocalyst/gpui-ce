use thiserror::Error;

pub type Result<T> = std::result::Result<T, WgpuError>;

#[derive(Debug, Error)]
pub enum WgpuError {
    #[error("adapter request failed")]
    RequestAdapter { details: String },
    #[error("no GPU adapters found")]
    NoAdapters,
    #[error("no suitable adapter found that can configure the display surface")]
    NoSuitableAdapter,
    #[error("failed to create wgpu device")]
    RequestDevice { details: String },
    #[error("failed to create surface")]
    SurfaceCreation { details: String },
    #[error("failed to get window handle")]
    WindowHandle { details: String },
    #[error("surface configuration failed")]
    SurfaceConfig { details: String },
    #[error("adapter is not compatible with surface")]
    IncompatibleAdapter { details: String },
    #[error("no compatible surface formats")]
    NoSurfaceFormats,
    #[error("no compatible alpha modes")]
    NoAlphaModes,
    #[error("surface reports no supported texture formats for adapter")]
    NoTextureFormats { adapter: String },
    #[error("surface reports no supported alpha modes for adapter")]
    NoSupportedAlphaMode { adapter: String },
    #[error("atlas allocation failed")]
    AtlasFull,
    #[error("color atlas texture format not supported")]
    ColorAtlasFormat { details: String },
    #[error("could not load font")]
    FontLoading { details: String },
    #[error("font face not found")]
    FontFaceNotFound { details: String },
    #[error("empty glyph bounds")]
    EmptyGlyphBounds,
    #[error("glyph rendering failed")]
    GlyphRendering { details: String },
    #[error("failed to match font")]
    FontMatching { details: String },
    #[error("incorrect font feature flag format")]
    FontFeatureFlag,
    #[error("failed to query font face")]
    FontFaceQuery { details: String },
    #[error("invalid PCI ID")]
    PciId { details: String },
    #[error(transparent)]
    TryFromInt(#[from] std::num::TryFromIntError),
}
