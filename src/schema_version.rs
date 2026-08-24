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
///
/// v2 (embarch-dev-bench/design.md §3 decisions 7/18): added
/// `DevBenchMessage::LogLine` and `HelloAck.firmware_version`.
///
/// v3 (decisions 24/25/27): `Hello` loses `steps_crc` (moved to
/// `StudyStart`); added `DevBenchMessage::StudyStart`/`StepResult`/
/// `StudyDone`/`StreamChunkBatch`; `Sample` gained `unit`/`channel_id`.
///
/// v4 (decisions 31/32): added `Action::GattDiscover`/`GattMonitorAll` and
/// the matching `StepResult.gatt_services`/`gatt_activity` fields (§4.3a).
/// One bump covering both new `Action` variants and both new `StepResult`
/// fields together, same one-bump-per-pass discipline v2's
/// `LogLine`+`firmware_version` pairing already established.
pub const STUDY_DESIGNER_SCHEMA_VERSION: u32 = 4;
