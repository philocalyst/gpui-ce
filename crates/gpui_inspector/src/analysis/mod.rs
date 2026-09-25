//! Pure analysis over inspector captures.
//!
//! Everything here is plain, deterministic Rust over the model types in
//! [`gpui::inspector`]: no views, no window access. Lenses call these
//! functions (memoized per capture generation, never per render) to turn raw
//! records into answers: statistics, "why this size", flame layouts,
//! bottom-up tables, insights, audit findings, trace exports, style
//! patches and source links.

pub mod audit;
pub mod bottom_up;
pub mod contrast;
pub mod flame;
pub mod format;
pub mod insights;
pub mod rust_patch;
pub mod source;
pub mod stats;
pub mod style_grid;
pub mod trace;
pub mod why_size;

#[cfg(test)]
mod fixtures;

/// How much a finding matters, least severe first (so `max` is the worst).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// Worth knowing; not necessarily a problem.
    Info,
    /// Likely a problem users notice.
    Warning,
    /// A problem users certainly notice.
    Critical,
}
