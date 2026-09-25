//! Pure analysis over inspector captures.
//!
//! Everything here is plain, deterministic Rust over the model types in
//! [`gpui::inspector`]: no views, no window access. Lenses call these
//! functions (memoized per capture generation, never per render) to turn raw
//! records into answers: statistics, "why this size", flame layouts,
//! bottom-up tables, insights, audit findings, trace exports and style
//! patches.

pub mod format;
pub mod stats;
pub mod why_size;

#[cfg(test)]
mod fixtures;
