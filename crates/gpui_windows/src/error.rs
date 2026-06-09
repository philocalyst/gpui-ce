use thiserror::Error;

#[derive(Error, Debug)]
pub enum WindowsError {
    #[error("Windows API error: {0}")]
    Api(#[from] windows::core::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("COM initialization failed: {0}")]
    ComInit(#[source] windows::core::Error),

    #[error("DPI awareness setup failed: {0}")]
    DpiAwareness(#[source] windows::core::Error),

    #[error("window class registration failed: {0}")]
    RegisterClass(#[source] windows::core::Error),

    #[error("window creation failed: {0}")]
    CreateWindowHandle(#[source] windows::core::Error),

    #[error("raw window handle error: {0}")]
    RawWindowHandle(#[source] raw_window_handle::HandleError),

    #[error("failed to create Direct3D 11 device: {0}")]
    CreateDevice(#[source] windows::core::Error),

    #[error("failed to create DXGI factory: {0}")]
    CreateDxgiFactory(#[source] windows::core::Error),

    #[error("failed to create swap chain (width={width}, height={height}): {source}")]
    CreateSwapChain { width: u32, height: u32, #[source] source: windows::core::Error },

    #[error("failed to create composition swap chain (width={width}, height={height}): {source}")]
    CreateCompositionSwapChain { width: u32, height: u32, #[source] source: windows::core::Error },

    #[error("no suitable DXGI adapter found")]
    NoAdapter,

    #[error("render target initialization failed: {0}")]
    RenderTargetInit(#[source] windows::core::Error),

    #[error("missing render target view")]
    MissingRenderTargetView,

    #[error("device lost handler failed: {0}")]
    DeviceLost(#[source] windows::core::Error),

    #[error("swap chain present failed: {0}")]
    PresentFailed(#[source] windows::core::Error),

    #[error("scene batch too large: {path_count} paths, {shadow_count} shadows, {quad_count} quads")]
    SceneTooLarge { path_count: usize, shadow_count: usize, quad_count: usize },

    #[error("shader compilation error (module={module:?}, target={target:?}): {message}")]
    ShaderCompilation { module: String, target: String, message: String, raw_error: Option<String> },

    #[error("DirectComposition initialization failed: {0}")]
    DirectCompositionInit(#[source] windows::core::Error),

    #[error("failed to set swap chain for DirectComposition: {0}")]
    DirectCompositionSwapChain(#[source] windows::core::Error),

    #[error("DXGI device creation failed: {0}")]
    DxgiDevice(#[source] windows::core::Error),

    #[error("failed to get system font collection: {0}")]
    SystemFontCollection(#[source] windows::core::Error),

    #[error("DirectWrite GPU state recreation failed: {0}")]
    DirectWriteGpuState(#[source] windows::core::Error),

    #[error("glyph bounds are empty")]
    GlyphBoundsEmpty,

    #[error("locale name not found for locale '{locale}': {source}")]
    LocaleNotFound { locale: String, #[source] source: windows::core::Error },

    #[error("NVIDIA driver version query failed: {0}")]
    NvidiaDriverVersion(String),

    #[error("AMD driver version query failed: {0}")]
    AmdDriverVersion(String),

    #[error("DXGI driver version query failed: {0}")]
    DxgiDriverVersion(#[source] windows::core::Error),

    #[error("credential operation failed: {0}")]
    Credential(#[source] windows::core::Error),

    #[error("registry error: {0}")]
    Registry(#[from] windows_registry::Error),

    #[error("clipboard operation failed: {0}")]
    Clipboard(#[source] windows::core::Error),

    #[error("display not found for index {0}")]
    DisplayNotFound(usize),

    #[error("URL scheme registration not supported")]
    UrlSchemeRegistration,

    #[error("dock menu item only supports `MenuItem::Action`, got {item_type:?}")]
    DockMenuItemNotSupported { item_type: &'static str },

    #[error("atlas allocation failed for texture kind {kind:?} with size {width}x{height}")]
    AtlasAllocation { kind: &'static str, width: u32, height: u32 },

    #[error("resize swap chain failed (new size: {width}x{height}): {source}")]
    ResizeSwapChain { width: u32, height: u32, #[source] source: windows::core::Error },

    #[error("failed to select font: {font_name:?}: {source}")]
    FontSelection { font_name: String, #[source] source: windows::core::Error },

    #[error("glyph advance query failed for font {font_id:?}: {source}")]
    FontAdvance { font_id: String, #[source] source: windows::core::Error },

    #[error("glyph raster bounds query failed: {source}")]
    GlyphRasterBounds(#[source] windows::core::Error),

    #[error("glyph rasterization failed: {source}")]
    GlyphRasterization(#[source] windows::core::Error),

    #[error("path tessellation error: {0}")]
    PathTessellation(String),

    #[error("report live objects failed: {0}")]
    ReportLiveObjects(#[source] windows::core::Error),

    #[error("file dialog operation failed: {0}")]
    FileDialog(#[source] windows::core::Error),

    #[error("Direct3D debug layer error: {0}")]
    DebugLayer(#[source] windows::core::Error),

    #[error("jump list operation failed: {0}")]
    JumpList(#[source] windows::core::Error),
}

pub type Result<T> = std::result::Result<T, WindowsError>;
