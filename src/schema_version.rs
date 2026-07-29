//! `STUDY_DESIGNER_SCHEMA_VERSION` — design.md §3 decision 12.
//!
//! Bumped by hand only when a wire-relevant type in this crate changes,
//! independent of `Cargo.toml`'s own semver. Checked at both connection
//! points: the Core<->dev-bench serial `Hello`/`HelloAck` handshake
//! ([`crate::protocol::DevBenchMessage`]), and Core's `/status` HTTP
//! response (a field `embarch-core` adds itself, outside this crate).

/// Bump this alongside any change to a type in [`crate::study`],
/// [`crate::result`], [`crate::validation`], [`crate::sample`], or
/// [`crate::protocol`] that changes its wire representation.
pub const STUDY_DESIGNER_SCHEMA_VERSION: u32 = 1;
