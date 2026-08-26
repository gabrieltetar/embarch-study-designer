//! The two schema-version constants — design.md §3 decision 12 and its
//! 2026-08-25 amendment.
//!
//! **One constant became two, because it was guarding two hops with
//! different exposure.** Until Milestone 7 Phase B a single
//! `STUDY_DESIGNER_SCHEMA_VERSION` was compared at both the Core<->dev-bench
//! serial handshake and Core's `/status` HTTP response, with one trigger
//! list serving both — and that list was wrong at both ends.
//! `Study.validations` never reaches dev-bench at all (§3 decisions 17, 19),
//! so a change there can never drift a dev-bench decoder; yet `Study`
//! *including* `validations` crosses `embarch-api` -> `embarch-core` as JSON,
//! whose only drift check was that same constant. Dropping validation from
//! the list would have created a real undetected failure between two
//! processes this suite genuinely builds and deploys separately
//! (`embarch-dev-workflow.md`); keeping it charged a firmware handshake
//! number for something firmware cannot observe.
//!
//! So:
//!
//! - [`DEV_BENCH_WIRE_SCHEMA_VERSION`] — compared at `Hello`/`HelloAck`.
//!   Moves only for a change to something **dev-bench itself parses or
//!   emits**. This is the number whose movement costs a firmware reflash and
//!   a both-languages re-pinning pass (§3 decision 36's pairing).
//! - [`HOST_TYPE_SCHEMA_VERSION`] — served by Core's `GET /status`, compared
//!   by `embarch-api`. Moves for **any** change to a type crossing the
//!   api<->Core hop: a strict superset, covering every dev-bench wire change
//!   *and* host-side-only ones like `Study.validations` and
//!   `Study.requires`.
//!
//! Both stay mismatch *detectors*, not negotiators — there is no fallback to
//! an older wire format on a mismatch, matching the suite's existing
//! minimal-viable posture. What changed is that a host-side-only reshape no
//! longer moves a number dev-bench compares itself against, so it no longer
//! implies firmware work that cannot exist, while the api<->Core hop still
//! refuses to talk across the drift.
//!
//! **Neither constant is renumbered.** Both continue the single constant's
//! own sequence from v8, and the history below records which side each past
//! bump would have belonged to under this rule rather than rewriting the
//! numbers to match. Renumbering would make every version string ever
//! logged, pinned, or written into a doc ambiguous about which scheme it was
//! counted in — the same class of harm as
//! [`embarch-decision-reversals.md`][rev] row 18's stale bump number, in a
//! larger blast radius.
//!
//! [rev]: https://github.com/gabrieltetar/embarch-doc/blob/main/embarch-decision-reversals.md

/// Compared at the Core<->dev-bench serial `Hello`/`HelloAck` handshake
/// ([`crate::protocol::DevBenchMessage`]) — the hop most exposed to drift,
/// since dev-bench firmware is flashed once and can silently lag behind a
/// rebuilt Core. A mismatch fails the connection outright rather than
/// proceeding and failing mid-study on a decode error.
///
/// **Bump this only when dev-bench itself parses or emits the thing that
/// changed** — a change to [`crate::study::Step`],
/// [`crate::study::Action`], [`crate::result::StepResult`],
/// [`crate::sample`], [`crate::streams`]' wire records/[`crate::streams::StreamTap`],
/// or [`crate::protocol`]. A change confined to
/// [`crate::validation`], to `Study.requires`, or to any other host-side-only
/// type does **not** belong here; move [`HOST_TYPE_SCHEMA_VERSION`] alone.
///
/// # History
///
/// Every bump through v8 moved the single pre-split constant. The
/// attribution below says which of the two it would have moved under
/// today's rule; the numbers themselves are untouched (see this module's own
/// doc comment for why).
///
/// - **v2** (`embarch-dev-bench/design.md` §3 decisions 7/18) — added
///   `DevBenchMessage::LogLine` and `HelloAck.firmware_version`. **Wire.**
/// - **v3** (§3 decisions 24/25/27) — `Hello` lost `steps_crc` (moved to
///   `StudyStart`); added `DevBenchMessage::StudyStart`/`StepResult`/
///   `StudyDone`/`StreamChunkBatch`; `Sample` gained `unit`/`channel_id`.
///   **Wire.**
/// - **v4** (§3 decisions 31/32) — added `Action::GattDiscover`/
///   `GattMonitorAll` and the matching `StepResult.gatt_services`/
///   `gatt_activity` fields (§4.3a). One bump covering both new `Action`
///   variants and both new `StepResult` fields together, the same
///   one-bump-per-pass discipline v2's `LogLine`+`firmware_version` pairing
///   already established. **Wire.**
/// - **v5** (§3 decision 36) — added `Action::GattMonitorStart`/
///   `GattMonitorStop`, `DevBenchMessage::GattTranscriptRecord`, the
///   `GattTranscriptEntry`/`GattDirection`/`GattEventKind` types it carried
///   (§4.3b), and `DataChannel::GattTranscript`. **Wire** — except
///   `DataChannel::GattTranscript`, which was host-only even then and is
///   retired outright at v9.
/// - **v6** (§3 decision 42) — `Step` gained a trailing `delay_before_ms`,
///   the first `Step` field added since this constant existed. Appended
///   rather than inserted precisely because postcard carries no field names
///   and dev-bench hand-decodes `Step` in C, but appended is still a wire
///   change: a v5 decoder reads a v6 `Step` and then finds an unconsumed
///   trailing varint, and a v6 decoder runs off the end of a v5 `Step`. The
///   handshake rejecting the mismatch outright is the point. **Wire.**
/// - **v7** (§3 decision 43) — `Action::BleConnect` gained a trailing
///   `target_name`, so a study can name the DUT it means instead of taking
///   whichever peripheral advertises first. Same append-don't-insert
///   discipline as v6, and the same reason it's still a wire break.
///   **Wire.**
/// - **v8** (§3 decisions 39 **and** 40, one bump for the pair) — the
///   largest single wire change since this constant existed, and the first
///   that *removes* rather than appends. Decision 39's one generic inbound
///   stream pipeline: `Study` gained `streams` (§4.8) and `StudyResult`
///   gained `streams`; `DevBenchMessage`'s four stream variants collapsed
///   into the `StreamOpen`/`StreamChunkBatch`/`StreamClose` triple carrying
///   `{ rx_utc_ms, bytes }` records instead of `Sample`s; `StreamChannel`,
///   `DevBenchMessage::GattTranscriptRecord`,
///   `GattOperation::StreamCapture`, and `StepResult`'s
///   `power_samples_ref`/`waveform_ref` all retired; `StudyStart` gained
///   `streams`. The retired triple's discriminants (2, 3, 4) were reused
///   rather than left as holes, safe because this handshake refuses a
///   version mismatch outright and no dev-bench firmware carrying the old
///   shapes has ever been flashed. **Wire** — *and* host, since decision
///   40's `Study.requires`/`StudyResult.provenance` are host-side only and
///   rode the same bump. **This pairing is the precedent the split
///   contradicts**: under today's rule decision 40 alone would have moved
///   only [`HOST_TYPE_SCHEMA_VERSION`].
/// - **v9** (§3 decision 39's 2026-08-25 amendment, Milestone 7 Phase B —
///   one bump covering both of its halves, the standing one-bump-per-pass
///   discipline). `Study` gained `streams_crc` and `StudyStart` gained it
///   after `streams`, a sibling seal over the taps rather than a widening of
///   `steps_crc`; and `Step.power_sample`/`PowerSampleWindow` are retired
///   outright, a `StreamSource::PowerFrontEnd` tap being the only way to
///   author a power capture afterwards. **Wire, both halves** — dev-bench
///   checks the new seal and stops reading a field that is gone.
///
///   Note what did *not* move this number in the same pass: §3 decision 19's
///   amendment reshaped `ValidationSource`/`DataChannel`, and `validations`
///   never crosses this hop. That is the first change the split actually
///   spares dev-bench, and it is why the split was worth making.
///
/// - **v10 did not move this constant either**, and that is the second time
///   the split paid for itself. `Provenance` grew `overrides` (§3 decision
///   40, Milestone 7 Phase B item 2) — a `StudyResult` field, and
///   `StudyResult` is assembled *by the host* out of the `StepResult`
///   messages dev-bench sends. dev-bench has never encoded or decoded a
///   `StudyResult`, so no firmware decoder can drift on this and no
///   both-languages pin (§3 decision 36) applies. Under the pre-split single
///   constant this would have charged a firmware reflash for a type firmware
///   cannot observe.
///
/// Note what has never bumped either constant: `vendor.rs` (§3 decision 41)
/// is a table of compile-time constants with no wire representation of its
/// own — a vendor-defined characteristic resolves into an ordinary
/// `Action::DataExchange` carrying plain UUIDs before anything is encoded,
/// so dev-bench firmware never needs to know the table exists.
/// - **v10** (§3 decision 47, `embarch-core/design.md` §3 decision 35).
///   `HelloAck` gained `hardware_id` — dev-bench's own factory-unique chip
///   ID, hex-encoded — so Core can confirm the board answering on the serial
///   link is the same silicon its JTAG probe just verified. One field on the
///   frame that already carries the two things Core checks at this moment
///   (`schema_version`, `firmware_version`), which is why it costs a wire
///   bump and no new mechanism. **Wire**, and the host constant moves with
///   it as always.
/// - **v11** (`embarch-outpost/design.md` §3 decision 9, Milestone 7 Phase C).
///   `StreamEncoding::OutpostTrace` loses its `manifest_crc` payload and
///   becomes a unit variant. **Wire**, unavoidably: `StreamEncoding` rides
///   `StudyStart` inside every `StreamTap`, and dev-bench's decoder has to
///   walk past the variant even though it never interprets one — its tag 4
///   arm stops reading a varint.
///
///   Worth stating why a *removal* was affordable at all: the field encoded a
///   mechanism decision 9 had already reversed (a post-link manifest CRC the
///   firmware cannot know) and bound the wrong moment (study-authoring time,
///   the staleness pattern that decision exists to avoid). Phase C producing
///   the first real manifest is what made both visible. `embarch-dev-bench`'s
///   firmware still had not been flashed when this landed, which is the same
///   window decisions 29/39 were spent in and the reason this cost a reshape
///   rather than a migration.
pub const DEV_BENCH_WIRE_SCHEMA_VERSION: u32 = 11;

/// Served by `embarch-core`'s `GET /status` and compared by `embarch-api`
/// against its own compiled-in copy before submitting a `Study` (design.md
/// §5.1). `GET /status` is already that hop's connection-establishment
/// check, so it carries this rather than there being a separate handshake
/// call.
///
/// **A strict superset of [`DEV_BENCH_WIRE_SCHEMA_VERSION`]'s triggers.**
/// Bump this for any change to a type crossing the api<->Core hop — which
/// is `Study` and `StudyResult` **whole**, including the parts dev-bench
/// never sees: [`crate::validation`], `Study.requires`, `Study.gatt`.
/// Every dev-bench wire change is also one of these, so a pass that bumps
/// the wire constant bumps this one too; the reverse does not hold.
///
/// # History
///
/// v2-v8 as listed on [`DEV_BENCH_WIRE_SCHEMA_VERSION`] — every one of them
/// moved the single pre-split constant, and every one of them would have
/// moved this one too, since each either changed a type crossing this hop or
/// (v8's decision 40 half) was host-side-only to begin with.
///
/// - **v9** (Milestone 7 Phase B, §3 decision 39's amendment **and** §3
///   decision 19's amendment). Decision 39's amendment as listed opposite —
///   `Study.streams_crc`, `Step.power_sample` retired — plus the one change
///   in this pass that moves *only* this constant: `ValidationSource` splits
///   into a per-step target and a tap target, and `DataChannel` narrows to
///   `CapturedData`/`GattActivity`, retiring `PowerSamples`/`SensorWaveform`/
///   `GattTranscript`. `ValidationResult` carries the whole
///   `ValidationSource` rather than a flattened `step_index`/`channel` pair.
///   Both cross this hop inside `Study`/`StudyResult`; neither reaches
///   dev-bench.
/// - **v10** (§3 decision 40, Milestone 7 Phase B item 2) — the **first bump
///   that moves this constant alone**, which is the split working as
///   designed rather than a special case. `Provenance` gained `overrides:
///   Vec<VersionOverride, 2>`, so a run that was allowed to proceed past a
///   version requirement says so in its own result instead of being
///   indistinguishable from one that satisfied it. `VersionOverride` and
///   `VersionSubject` are new host-side types; nothing on the dev-bench wire
///   changed, and [`DEV_BENCH_WIRE_SCHEMA_VERSION`] stays at 9.
///
///   The two constants are **no longer equal**, which the split's own
///   amendment said would happen "on the first host-only pass" and which is
///   the only real evidence the mechanism works — equal numbers proved
///   nothing.
/// - **v11** — post-hoc validation is **removed outright**. `Study.validations`,
///   `StudyResult.validations`, `PostHocValidation`, `PostHocCheck`,
///   `ExpectedValue`, `SignalCheck`, `ValidationSource`, `DataChannel`,
///   `ContentValidity` and `ValidationResult` are all gone, along with the
///   `core-validation` feature and `signal.rs`'s evaluation logic. The second
///   bump to move this constant alone, and for the cleanest possible reason:
///   none of it ever crossed the dev-bench wire (§3 decision 17), so
///   [`DEV_BENCH_WIRE_SCHEMA_VERSION`] is untouched at 9 and dev-bench needs
///   no reflash. A removal is exactly as breaking as an addition on this hop —
///   an api built before this and a Core built after it disagree about
///   `Study`'s shape — which is what this constant exists to refuse.
/// - **v12** (§3 decision 47) — a wire bump, so this one follows by the rule
///   above rather than by a judgement call: `HelloAck.hardware_id`. Written
///   in that decision as 10 → 11 and **re-derived to 12 here**, because
///   decision 48's removal landed first and took this constant to 11; the
///   two decisions were designed in the same pass and only one of them could
///   be numbered before the other shipped. That re-derivation is
///   [embarch-decision-reversals.md](../embarch-decision-reversals.md) row
///   18's protocol working as intended, not a mistake being corrected.
/// - **v13** — a wire bump (`StreamEncoding::OutpostTrace` becomes a unit
///   variant), so this follows by the superset rule rather than by judgement.
pub const HOST_TYPE_SCHEMA_VERSION: u32 = 13;

/// [`HOST_TYPE_SCHEMA_VERSION`]'s triggers are a strict superset of
/// [`DEV_BENCH_WIRE_SCHEMA_VERSION`]'s (design.md §3 decision 12's
/// amendment), so every wire bump is also a host bump and the host number
/// can never trail the wire one.
///
/// Pinned at **compile time** rather than in a test, because the failure it
/// guards against is silent and a `#[cfg(test)]` check would not fire in the
/// builds that matter: a wire change that moved only the wire constant would
/// leave the api<->Core hop talking happily across a shape difference it
/// exists to refuse, in a release binary nobody ran the test suite against.
const _: () = assert!(HOST_TYPE_SCHEMA_VERSION >= DEV_BENCH_WIRE_SCHEMA_VERSION);
