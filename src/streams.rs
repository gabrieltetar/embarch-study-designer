//! Stream taps — design.md §3 decision 39, §4.8.
//!
//! One generic inbound capture pipeline, replacing the three near-identical
//! bespoke ones this crate had grown (power, sensor waveform, GATT
//! transcript) before an outpost trace would have made it four.
//!
//! A tap declares exactly four things and nothing else: **where the bytes
//! come from** ([`StreamSource`]), **how long the tap lives**
//! ([`StreamScope`]), **how to render what arrives** ([`StreamEncoding`]),
//! and **what to call the output** (`StreamTap::name`). Everything that used
//! to be a bespoke channel is now a declared source; everything that used to
//! be a bespoke row shape is now a declared encoding — the CSV column shapes
//! themselves are unchanged, they just live behind an encoding rather than
//! behind their own message class.
//!
//! **[`StreamEncoding`] is the only place in this crate where a byte payload
//! acquires a meaning, it is always engineer-declared, and no component ever
//! guesses one** — design.md §3 decision 35's no-inference rule, applied to
//! the read direction.

use heapless::{String, Vec};
use serde::{Deserialize, Serialize};

use crate::ids::Uuid;
use crate::limits::{
    MAX_SIGNAL_NAME_LEN, MAX_STREAMS_PER_STUDY, MAX_STREAM_CHUNK_BYTES, MAX_STREAM_NAME_LEN,
};
use crate::sample::{Sample, Unit};

/// The one stream name a submitted `Study` may not use: it belongs to the
/// reserved [`StreamSource::DevBenchLog`] tap that carries dev-bench's own
/// `LogLine` output (design.md §4.8), built by [`dev_bench_log_tap`].
/// Rejected by `POST /study`'s pre-flight validation (design.md §3 decision
/// 18) via [`validate_taps`].
pub const RESERVED_DEV_BENCH_STREAM_NAME: &str = "dev-bench";

/// The reserved `dev-bench` tap, synthesized rather than declared (design.md
/// §4.8) — the one tap a submitted `Study` may not author, since
/// [`validate_taps`] rejects its name.
///
/// **Its `id` is `declared.len()`**, which is free by construction: a
/// declared tap's `id` is its own index in `Study.streams`, so the first
/// index past the end can never collide with one. That single rule is what
/// lets both ends agree on the handle without either sending it — which is
/// why it lives here rather than being computed twice.
///
/// Encoding is [`StreamEncoding::Text`] because that is what a firmware log
/// line is, and scope is [`StreamScope::WholeStudy`] because dev-bench can
/// have something to say before the first step and after the last.
///
/// **Where the bytes come from, in the shipped implementation:** Core
/// renders them out of the `DevBenchMessage::LogLine` frames it already
/// receives, rather than dev-bench opening a second channel for its own log
/// and sending each line twice. §4.8's original sketch had dev-bench
/// emitting `StreamOpen`/`StreamChunkBatch`/`StreamClose` for this tap
/// itself; that was strictly more firmware, more link traffic, and more
/// SRAM on a board already at 98% of `sram0_0_seg`, to move bytes that were
/// crossing the link anyway. The asymmetry the tap exists to close — a
/// firmware log reaching only Core's rolling log and never the study's own
/// results — is closed either way.
pub fn dev_bench_log_tap(declared: &Vec<StreamTap, MAX_STREAMS_PER_STUDY>) -> StreamTap {
    StreamTap {
        // `declared.len()` is at most MAX_STREAMS_PER_STUDY, far inside u8.
        id: declared.len() as u8,
        name: String::try_from(RESERVED_DEV_BENCH_STREAM_NAME)
            .expect("RESERVED_DEV_BENCH_STREAM_NAME fits MAX_STREAM_NAME_LEN"),
        source: StreamSource::DevBenchLog,
        encoding: StreamEncoding::Text,
        scope: StreamScope::WholeStudy,
    }
}

/// One declared capture channel for the duration of a `Study` (design.md
/// §4.8).
///
/// `id` is the wire handle — its own index in `Study.streams` — and is what
/// [`crate::protocol::DevBenchMessage::StreamOpen`]/`StreamChunkBatch`/
/// `StreamClose` carry instead of a channel enum. `name` names the output
/// file under a study's `streams/` directory and the post-hoc validation
/// source, and nothing else.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamTap {
    /// This tap's own index in `Study.streams`. Enforced by
    /// [`validate_taps`] rather than derived, so the wire handle a
    /// `StreamOpen` carries is a value the author can read straight out of
    /// the submitted `Study` instead of having to count.
    pub id: u8,
    pub name: String<MAX_STREAM_NAME_LEN>,
    pub source: StreamSource,
    pub encoding: StreamEncoding,
    pub scope: StreamScope,
}

/// Where a tap's bytes come from (design.md §4.8).
///
/// The first four are **dev-bench-mediated** — dev-bench receives the bytes
/// and forwards them, stamping arrival and interpreting nothing.
/// [`StreamSource::Signal`] is the exception and the one genuinely new idea
/// decision 39 adds: Core reads it itself, with the carrier resolved live by
/// `embarch-topology` (`embarch-topology/design.md` §3 decision 18) — a
/// local serial port for a `Route::Direct` signal, relayed bytes for a
/// `Route::ViaDevBench` one. The tap is identical either way, which is what
/// lets the identical saved study (design.md §3 decision 38) run unchanged
/// across a rewiring of the bench.
///
/// Append-only, same wire-compatibility rule as
/// [`crate::protocol::DevBenchMessage`] (design.md §3 decision 10).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StreamSource {
    /// Notifications/indications from one DUT characteristic, subscribed by
    /// dev-bench for the tap's scope. Replaces the retired
    /// `GattOperation::StreamCapture` -> `StreamChannel::SensorWaveform`
    /// pair (design.md §3 decision 21).
    GattNotify {
        service_uuid: Uuid,
        characteristic_uuid: Uuid,
    },
    /// dev-bench's own power-sampling front end. Replaces the retired
    /// `StreamChannel::Power` (design.md §3 decision 20).
    PowerFrontEnd { sample_hz: u32 },
    /// dev-bench's exhaustive GATT transcript (design.md §3 decision 36,
    /// §4.3b) — the record type, its both-directions coverage, and its
    /// `gatt.csv` columns all survive; only its dedicated
    /// `DevBenchMessage::GattTranscriptRecord` variant is retired.
    GattTranscript,
    /// dev-bench's own log output. Reserved: this tap is synthesized under
    /// [`RESERVED_DEV_BENCH_STREAM_NAME`] rather than declared (see
    /// [`dev_bench_log_tap`]), closing a real asymmetry —
    /// `DevBenchMessage::LogLine` reached only Core's rolling log and never
    /// a study's own results at all.
    DevBenchLog,
    /// A named signal Core reads itself, resolved to a carrier live by
    /// `embarch-topology`'s `SignalLink` (`embarch-topology/design.md` §3
    /// decision 18). The outpost's tap
    /// (`embarch-outpost/design.md` §3 decisions 11, 12).
    ///
    /// **The tap names the signal, never the carrier.** A source variant
    /// naming a concrete port or dev-bench pin would re-author every saved
    /// study the day the bench was rewired.
    Signal {
        name: String<MAX_SIGNAL_NAME_LEN>,
    },
}

impl StreamSource {
    /// Whether dev-bench is the node that produces this tap's bytes. Core
    /// opens its own carrier for the one source that isn't
    /// ([`StreamSource::Signal`]), taking neither `hw_lock` nor
    /// `study_lock` (`embarch-core/design.md` §3 decision 30).
    pub const fn is_dev_bench_mediated(&self) -> bool {
        !matches!(self, StreamSource::Signal { .. })
    }
}

/// How to render a tap's bytes (design.md §4.8).
///
/// **The only place a byte payload acquires a meaning in this crate.** Every
/// variant is engineer-declared in the submitted `Study`; nothing anywhere
/// in the suite infers one from the bytes themselves (design.md §3 decision
/// 35). Append-only.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum StreamEncoding {
    /// Written verbatim, decoded by nothing. The honest default for a
    /// payload whose meaning nobody has declared.
    Raw,
    /// Bytes are text; the host may render them as lines. No structure is
    /// assumed beyond that.
    Text,
    /// Bytes are packed scalar samples — the `data.csv`/`waveform.csv` row
    /// shape (design.md §4.7, §5.2), unchanged, now reached through a
    /// declared encoding rather than through a `Sample`-carrying wire
    /// message. [`samples_in`] does the decode, so column knowledge still
    /// lives only in this crate.
    Samples {
        layout: SampleLayout,
        unit: Unit,
        channel_id: u8,
    },
    /// Each record's bytes are one postcard-encoded
    /// [`crate::gatt::GattTranscriptEntry`] (design.md §4.3b), rendered
    /// through that type's own `to_csv_row` into `gatt.csv`'s unchanged
    /// columns.
    ///
    /// The entry carries its own `rx_utc_ms` as well as the record's — the
    /// same value from the same clock, kept rather than removed so the
    /// transcript row shape decision 36 pinned stays byte-for-byte what it
    /// was. The `step_index` column comes from whichever step the host has
    /// open when the record arrives, which is exactly what decision 36
    /// defined that field to mean.
    GattTranscript,
    /// Decoded against a build-time manifest from the DUT's own firmware
    /// build (`embarch-outpost/design.md` §3 decision 9), via
    /// [`crate::outpost`]. A firmware whose header frame reports a different
    /// build **refuses to decode rather than decoding wrong** — the raw bytes
    /// are still written either way.
    ///
    /// **A unit variant, corrected 2026-08-25 when Phase C produced the first
    /// real manifest.** This shipped as `OutpostTrace { manifest_crc: u32 }`,
    /// meaning "the manifest this study was authored against", and both halves
    /// of that were wrong:
    ///
    /// * **The firmware cannot report a manifest CRC.** The manifest is
    ///   generated *from the linked image* — it holds thread and ISR tables
    ///   read out of the ELF — so there is no CRC the firmware could have been
    ///   built knowing. The field encoded the post-link CRC patch that
    ///   `embarch-outpost/design.md` §3 decision 9's own rework had already
    ///   replaced with a compile-time build ID; the type layer kept the
    ///   mechanism the decision dropped.
    /// * **Author-time is the wrong moment to bind it.** A CRC chosen when the
    ///   study was written is a persisted record of resolved state consulted at
    ///   a later, unrelated moment — the write-ahead staleness pattern
    ///   `embarch-topology/design.md` §3 decision 3 exists to eliminate, and
    ///   the one decision 9 spent three paragraphs distinguishing itself from.
    ///   A saved study would go stale on the next rebuild.
    ///
    /// Which manifest is a Core-side runtime question, answered by the flash
    /// that bound it and verified by the build ID in the stream's own header.
    /// The tap declares only what the bytes *are*.
    OutpostTrace,
    /// Each record's bytes are one instance of an engineer-declared payload
    /// layout — design.md §3 decision 52, [`crate::decoder`]. `decoder`
    /// indexes into `Study.decoders`.
    ///
    /// **An index, not the layout itself, and that is the whole design.** A
    /// tap's encoding crosses the wire inside `StudyStart`, where dev-bench
    /// has to walk past it to reach `scope`; a variable-length struct
    /// definition there would cost a nested walker in hand-written C for a
    /// value dev-bench must never act on. The layouts live in
    /// `Study.decoders` instead — a **host-only** field, like
    /// `Study.requires`, that never crosses that hop at all. So this variant
    /// costs the C decoder one `u8` read, and what a payload *means* stays
    /// exactly as far away from dev-bench as decision 39 put it.
    ///
    /// [`crate::streams::StreamEncoding::Samples`] is not replaced. It says
    /// "packed scalars of one type, all one column"; this says "these named
    /// fields, then this repeating group" and renders named columns. A power
    /// front end wants the first; a notification packet wants the second.
    Struct { decoder: u8 },
}

/// How to read scalar elements out of a [`StreamEncoding::Samples`] payload
/// (design.md §4.8).
///
/// **Element width, type, and byte order only — no scaling, no offset, no
/// unit conversion.** Those would be a claim about what a particular DUT's
/// bytes *mean*, which is the engineer's knowledge and not this crate's
/// (design.md §3 decision 35). `unit` names the quantity; nothing here
/// transforms the number.
///
/// Append-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SampleLayout {
    /// Packed IEEE-754 binary32, little-endian.
    F32Le,
    /// Packed IEEE-754 binary32, big-endian.
    F32Be,
    /// Packed signed 16-bit two's-complement, little-endian.
    I16Le,
    /// Packed signed 16-bit two's-complement, big-endian.
    I16Be,
    /// Packed unsigned 16-bit, little-endian.
    U16Le,
    /// Packed unsigned 16-bit, big-endian.
    U16Be,
}

impl SampleLayout {
    /// Bytes per element. A trailing partial element is dropped by
    /// [`samples_in`] rather than zero-padded into a plausible, wrong value.
    pub const fn element_len(self) -> usize {
        match self {
            SampleLayout::F32Le | SampleLayout::F32Be => 4,
            SampleLayout::I16Le
            | SampleLayout::I16Be
            | SampleLayout::U16Le
            | SampleLayout::U16Be => 2,
        }
    }

    /// Reads one element out of `bytes`, which must be exactly
    /// [`element_len`](Self::element_len) long.
    fn read(self, bytes: &[u8]) -> f32 {
        match self {
            SampleLayout::F32Le => f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            SampleLayout::F32Be => f32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            SampleLayout::I16Le => i16::from_le_bytes([bytes[0], bytes[1]]) as f32,
            SampleLayout::I16Be => i16::from_be_bytes([bytes[0], bytes[1]]) as f32,
            SampleLayout::U16Le => u16::from_le_bytes([bytes[0], bytes[1]]) as f32,
            SampleLayout::U16Be => u16::from_be_bytes([bytes[0], bytes[1]]) as f32,
        }
    }
}

/// How long a tap lives (design.md §4.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StreamScope {
    /// Open for the whole study — what an outpost trace uses
    /// (`embarch-outpost/design.md` §3 decision 10).
    WholeStudy,
    /// Open across a step range, `from` and `to` both **inclusive** indices
    /// into `Study.steps` — what a power window uses. A single-step window
    /// is `from == to`.
    Steps { from: u32, to: u32 },
}

impl StreamScope {
    /// Whether this scope has the tap open while `step_index` runs.
    pub const fn covers(&self, step_index: u32) -> bool {
        match *self {
            StreamScope::WholeStudy => true,
            StreamScope::Steps { from, to } => step_index >= from && step_index <= to,
        }
    }
}

/// One arrival-stamped run of bytes (design.md §4.8) — **never a decoded
/// value.** `rx_utc_ms` is stamped by whichever node received the bytes
/// (dev-bench for a dev-bench-mediated source, Core for a
/// [`StreamSource::Signal`]), on the same clock convention
/// `Sample::rx_utc_ms` already uses (design.md §4.7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamRecord {
    pub rx_utc_ms: u64,
    pub bytes: Vec<u8, MAX_STREAM_CHUNK_BYTES>,
}

/// What a tap produced, per `StudyResult` (design.md §4.8). Replaces the
/// retired `StepResult::power_samples_ref`/`waveform_ref` — a stream belongs
/// to the study, not to one step, which is what those two fields could never
/// express for a tap whose scope outlives a single step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamRef {
    pub name: String<MAX_STREAM_NAME_LEN>,
    /// Bytes actually written to this stream's file.
    pub bytes_written: u64,
    /// Whether the capture is short of what the source produced — a
    /// retention cap hit, or a `StreamClose` reporting a non-zero `dropped`.
    /// **A stream that lost data says so** rather than presenting a shorter,
    /// plausible capture as complete.
    pub truncated: bool,
}

/// Why a submitted `Study`'s `streams` aren't usable — `POST /study`'s
/// pre-flight validation failure (design.md §3 decision 18), computed here
/// so Core holds no independent knowledge of the rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamTapError {
    /// `id` must equal the tap's own index in `Study.streams`, since `id` is
    /// the wire handle every `StreamOpen`/`StreamChunkBatch`/`StreamClose`
    /// carries.
    IdIsNotItsIndex { index: usize, id: u8 },
    /// An unnamed tap has no output file to write to.
    EmptyName { index: usize },
    /// [`RESERVED_DEV_BENCH_STREAM_NAME`].
    ReservedName { index: usize },
    /// Two taps naming the same output file would interleave into one.
    DuplicateName { index: usize },
    /// A `Steps` scope whose `to` precedes its `from` opens nothing, which
    /// is a silently-empty capture — the failure mode decisions 34 and 36
    /// were both opened by.
    InvertedStepRange { index: usize },
    /// A `Steps` scope referencing a step the study doesn't have.
    StepRangeOutOfBounds { index: usize, step_count: u32 },
    /// A `StreamEncoding::Struct` naming a `Study.decoders` entry that isn't
    /// there. Caught here rather than at render time, where it would surface
    /// as a study that ran, captured, and produced no CSV.
    UnknownDecoder { index: usize, decoder: u8, decoder_count: usize },
}

impl core::fmt::Display for StreamTapError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            StreamTapError::IdIsNotItsIndex { index, id } => write!(
                f,
                "streams[{index}] declares id {id}, but a tap's id is its own index in \
                 Study.streams — it is the handle StreamOpen/StreamChunkBatch/StreamClose carry"
            ),
            StreamTapError::EmptyName { index } => {
                write!(f, "streams[{index}] has an empty name; a tap names its own output file")
            }
            StreamTapError::ReservedName { index } => write!(
                f,
                "streams[{index}] is named '{RESERVED_DEV_BENCH_STREAM_NAME}', which is reserved \
                 for dev-bench's own log tap"
            ),
            StreamTapError::DuplicateName { index } => write!(
                f,
                "streams[{index}] repeats a name an earlier tap already uses; two taps would \
                 write one file"
            ),
            StreamTapError::InvertedStepRange { index } => write!(
                f,
                "streams[{index}] declares a step range whose end precedes its start, so it would \
                 never open"
            ),
            StreamTapError::StepRangeOutOfBounds { index, step_count } => write!(
                f,
                "streams[{index}] declares a step range outside this study's {step_count} step(s)"
            ),
            StreamTapError::UnknownDecoder { index, decoder, decoder_count } => write!(
                f,
                "streams[{index}] decodes with Study.decoders[{decoder}], but this study declares \
                 {decoder_count} decoder(s)"
            ),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for StreamTapError {}

/// Pre-flight validation for a submitted `Study`'s taps (design.md §3
/// decision 18, §4.8). `step_count` is `study.steps.len()`.
///
/// Deliberately in this crate rather than in Core: the rules are properties
/// of the type model, and a second copy in Core is exactly the drift §1
/// exists to prevent.
pub fn validate_taps(
    taps: &Vec<StreamTap, MAX_STREAMS_PER_STUDY>,
    step_count: u32,
    decoder_count: usize,
) -> Result<(), StreamTapError> {
    for (index, tap) in taps.iter().enumerate() {
        if usize::from(tap.id) != index {
            return Err(StreamTapError::IdIsNotItsIndex { index, id: tap.id });
        }
        if tap.name.trim().is_empty() {
            return Err(StreamTapError::EmptyName { index });
        }
        if tap.name.as_str() == RESERVED_DEV_BENCH_STREAM_NAME {
            return Err(StreamTapError::ReservedName { index });
        }
        if taps[..index].iter().any(|earlier| earlier.name == tap.name) {
            return Err(StreamTapError::DuplicateName { index });
        }
        if let StreamScope::Steps { from, to } = tap.scope {
            if to < from {
                return Err(StreamTapError::InvertedStepRange { index });
            }
            if to >= step_count {
                return Err(StreamTapError::StepRangeOutOfBounds { index, step_count });
            }
        }
        if let StreamEncoding::Struct { decoder } = tap.encoding {
            if usize::from(decoder) >= decoder_count {
                return Err(StreamTapError::UnknownDecoder {
                    index,
                    decoder,
                    decoder_count,
                });
            }
        }
    }
    Ok(())
}

/// Decodes one [`StreamRecord`] into the `Sample`s a
/// [`StreamEncoding::Samples`] tap declared it holds — the crate-side half
/// of writing `data.csv`/`waveform.csv`, so Core's job stays "call
/// `Sample::to_csv_row` on each of these and append its own
/// `core_rx_utc_ms`" (design.md §4.7, §5.2) with no column or layout
/// knowledge of its own.
///
/// `sample_hz`, when given (a [`StreamSource::PowerFrontEnd`] tap declares
/// it), spreads the record's samples across the interval the declared rate
/// implies: element *i* is stamped `rx_utc_ms + i * 1000 / sample_hz`. With
/// no declared rate — a [`StreamSource::GattNotify`] waveform, say, whose
/// rate nobody has stated — **every sample in the record carries the
/// record's own arrival stamp**, unchanged, rather than an interpolation
/// nobody declared the basis for.
///
/// A trailing partial element is dropped, not zero-padded.
pub fn samples_in<'a>(
    record: &'a StreamRecord,
    layout: SampleLayout,
    unit: Unit,
    channel_id: u8,
    sample_hz: Option<u32>,
) -> SampleIter<'a> {
    SampleIter {
        bytes: &record.bytes,
        rx_utc_ms: record.rx_utc_ms,
        layout,
        unit,
        channel_id,
        // A declared rate of 0 Hz is not a rate; treat it as undeclared
        // rather than dividing by it.
        sample_hz: sample_hz.filter(|hz| *hz > 0),
        index: 0,
    }
}

/// [`samples_in`]'s iterator. Borrows the record; allocates nothing.
#[derive(Debug, Clone)]
pub struct SampleIter<'a> {
    bytes: &'a [u8],
    rx_utc_ms: u64,
    layout: SampleLayout,
    unit: Unit,
    channel_id: u8,
    sample_hz: Option<u32>,
    index: usize,
}

impl Iterator for SampleIter<'_> {
    type Item = Sample;

    fn next(&mut self) -> Option<Sample> {
        let width = self.layout.element_len();
        let start = self.index * width;
        let end = start.checked_add(width)?;
        if end > self.bytes.len() {
            return None;
        }
        let value = self.layout.read(&self.bytes[start..end]);
        let rx_utc_ms = match self.sample_hz {
            Some(hz) => self.rx_utc_ms + (self.index as u64 * 1000) / u64::from(hz),
            None => self.rx_utc_ms,
        };
        self.index += 1;
        Some(Sample { rx_utc_ms, value, unit: self.unit, channel_id: self.channel_id })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tap(id: u8, name: &str, scope: StreamScope) -> StreamTap {
        StreamTap {
            id,
            name: String::try_from(name).unwrap(),
            source: StreamSource::GattTranscript,
            encoding: StreamEncoding::GattTranscript,
            scope,
        }
    }

    fn taps(list: &[StreamTap]) -> Vec<StreamTap, MAX_STREAMS_PER_STUDY> {
        let mut v: Vec<StreamTap, MAX_STREAMS_PER_STUDY> = Vec::new();
        for t in list {
            v.push(t.clone()).unwrap();
        }
        v
    }

    #[test]
    fn a_well_formed_tap_list_validates() {
        let list = taps(&[
            tap(0, "gatt", StreamScope::WholeStudy),
            tap(1, "outpost", StreamScope::Steps { from: 0, to: 3 }),
        ]);
        assert_eq!(validate_taps(&list, 4, 0), Ok(()));
    }

    #[test]
    fn an_id_that_is_not_its_own_index_is_rejected() {
        // `id` is the wire handle every StreamOpen/StreamChunkBatch/
        // StreamClose carries, so an id that doesn't index back into
        // Study.streams would route a capture into the wrong file.
        let list = taps(&[tap(1, "gatt", StreamScope::WholeStudy)]);
        assert_eq!(
            validate_taps(&list, 1, 0),
            Err(StreamTapError::IdIsNotItsIndex { index: 0, id: 1 })
        );
    }

    #[test]
    fn the_reserved_dev_bench_name_is_rejected() {
        let list = taps(&[tap(0, RESERVED_DEV_BENCH_STREAM_NAME, StreamScope::WholeStudy)]);
        assert_eq!(validate_taps(&list, 1, 0), Err(StreamTapError::ReservedName { index: 0 }));
    }

    #[test]
    fn a_blank_or_repeated_name_is_rejected() {
        assert_eq!(
            validate_taps(&taps(&[tap(0, "   ", StreamScope::WholeStudy)]), 1, 0),
            Err(StreamTapError::EmptyName { index: 0 })
        );
        let dup = taps(&[
            tap(0, "trace", StreamScope::WholeStudy),
            tap(1, "trace", StreamScope::WholeStudy),
        ]);
        assert_eq!(validate_taps(&dup, 1, 0), Err(StreamTapError::DuplicateName { index: 1 }));
    }

    #[test]
    fn a_step_range_that_could_never_open_is_rejected() {
        // Both of these produce a capture that is silently empty, which is
        // the exact failure decisions 34 and 36 were each opened by.
        assert_eq!(
            validate_taps(&taps(&[tap(0, "power", StreamScope::Steps { from: 3, to: 1 })]), 8, 0),
            Err(StreamTapError::InvertedStepRange { index: 0 })
        );
        assert_eq!(
            validate_taps(&taps(&[tap(0, "power", StreamScope::Steps { from: 0, to: 9 })]), 4, 0),
            Err(StreamTapError::StepRangeOutOfBounds { index: 0, step_count: 4 })
        );
    }

    #[test]
    fn a_struct_encoding_naming_a_decoder_the_study_lacks_is_rejected() {
        // Caught at submit, where the author can fix it. Left to render
        // time it is a study that ran, captured, and produced no CSV —
        // "nothing captured, no error" again, one layer up.
        let mut list = taps(&[tap(0, "ppg", StreamScope::WholeStudy)]);
        list[0].encoding = StreamEncoding::Struct { decoder: 2 };
        assert_eq!(
            validate_taps(&list, 1, 2),
            Err(StreamTapError::UnknownDecoder { index: 0, decoder: 2, decoder_count: 2 })
        );
        assert_eq!(validate_taps(&list, 1, 3), Ok(()));
    }

    #[test]
    fn scope_covers_the_inclusive_step_range_it_declares() {
        assert!(StreamScope::WholeStudy.covers(0));
        assert!(StreamScope::WholeStudy.covers(u32::MAX));
        let window = StreamScope::Steps { from: 1, to: 2 };
        assert!(!window.covers(0));
        assert!(window.covers(1));
        assert!(window.covers(2), "`to` is inclusive");
        assert!(!window.covers(3));
    }

    #[test]
    fn the_reserved_log_taps_id_is_the_first_index_past_the_declared_ones() {
        // The whole point of the rule: neither end sends this handle, so
        // both have to derive the same one, and it must never collide with
        // a declared tap's id (which is that tap's own index).
        let none: Vec<StreamTap, MAX_STREAMS_PER_STUDY> = Vec::new();
        let reserved = dev_bench_log_tap(&none);
        assert_eq!(reserved.id, 0);
        assert_eq!(reserved.name.as_str(), RESERVED_DEV_BENCH_STREAM_NAME);
        assert_eq!(reserved.source, StreamSource::DevBenchLog);
        assert_eq!(reserved.encoding, StreamEncoding::Text);
        assert_eq!(reserved.scope, StreamScope::WholeStudy);

        let two = taps(&[
            tap(0, "gatt", StreamScope::WholeStudy),
            tap(1, "power", StreamScope::WholeStudy),
        ]);
        assert_eq!(dev_bench_log_tap(&two).id, 2);
    }

    #[test]
    fn the_reserved_tap_stays_addressable_even_with_a_full_declared_list() {
        // A full Study still leaves the reserved handle free, because it is
        // one past the last index rather than a slot carved out of the
        // capacity. Nothing here needs MAX_STREAMS_PER_STUDY to grow.
        let mut full: Vec<StreamTap, MAX_STREAMS_PER_STUDY> = Vec::new();
        for i in 0..MAX_STREAMS_PER_STUDY {
            let mut t = tap(i as u8, "x", StreamScope::WholeStudy);
            t.name = String::try_from(
                ["a", "b", "c", "d", "e", "f", "g", "h"][i],
            )
            .unwrap();
            full.push(t).unwrap();
        }
        assert_eq!(validate_taps(&full, 1, 0), Ok(()));
        let reserved = dev_bench_log_tap(&full);
        assert_eq!(usize::from(reserved.id), MAX_STREAMS_PER_STUDY);
        assert!(
            full.iter().all(|t| t.id != reserved.id),
            "the reserved id must not collide with any declared tap's"
        );
    }

    #[test]
    fn a_study_may_not_declare_the_reserved_tap_itself() {
        // The synthesized tap and validate_taps' rejection are two halves of
        // one rule: exactly one producer for that name.
        let list = taps(&[tap(0, RESERVED_DEV_BENCH_STREAM_NAME, StreamScope::WholeStudy)]);
        assert_eq!(validate_taps(&list, 1, 0), Err(StreamTapError::ReservedName { index: 0 }));
        let none: Vec<StreamTap, MAX_STREAMS_PER_STUDY> = Vec::new();
        assert_eq!(dev_bench_log_tap(&none).name.as_str(), RESERVED_DEV_BENCH_STREAM_NAME);
    }

    #[test]
    fn only_a_signal_tap_is_read_by_core_itself() {
        assert!(!StreamSource::Signal { name: String::try_from("outpost").unwrap() }
            .is_dev_bench_mediated());
        for source in [
            StreamSource::PowerFrontEnd { sample_hz: 1_000 },
            StreamSource::GattTranscript,
            StreamSource::DevBenchLog,
        ] {
            assert!(source.is_dev_bench_mediated(), "{source:?}");
        }
    }

    #[test]
    fn samples_decode_out_of_a_record_in_the_declared_layout() {
        let record = StreamRecord {
            rx_utc_ms: 1_000,
            bytes: Vec::from_slice(&[
                0x00, 0x00, 0x80, 0x3f, // 1.0f32, little-endian
                0x00, 0x00, 0x00, 0x40, // 2.0f32
            ])
            .unwrap(),
        };
        let decoded: Vec<Sample, 8> =
            samples_in(&record, SampleLayout::F32Le, Unit::Milliamps, 3, None)
                .collect();
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].value, 1.0);
        assert_eq!(decoded[1].value, 2.0);
        assert_eq!(decoded[0].unit, Unit::Milliamps);
        assert_eq!(decoded[0].channel_id, 3);
        // With no declared rate, nothing is interpolated: every sample
        // carries the record's own arrival stamp, unchanged.
        assert_eq!(decoded[0].rx_utc_ms, 1_000);
        assert_eq!(decoded[1].rx_utc_ms, 1_000);
    }

    #[test]
    fn a_declared_sample_rate_spreads_a_records_samples_across_it() {
        let record = StreamRecord {
            rx_utc_ms: 1_000,
            bytes: Vec::from_slice(&[0x01, 0x00, 0x02, 0x00, 0xff, 0xff]).unwrap(),
        };
        let decoded: Vec<Sample, 8> =
            samples_in(&record, SampleLayout::I16Le, Unit::Raw, 0, Some(1_000)).collect();
        assert_eq!(decoded.len(), 3);
        assert_eq!(decoded[2].value, -1.0, "I16Le is two's-complement");
        assert_eq!(decoded[0].rx_utc_ms, 1_000);
        assert_eq!(decoded[1].rx_utc_ms, 1_001);
        assert_eq!(decoded[2].rx_utc_ms, 1_002);
    }

    #[test]
    fn a_trailing_partial_element_is_dropped_not_zero_padded() {
        // Zero-padding would produce a plausible, wrong value — the same
        // "decodes into plausible-looking garbage" failure the cross-language
        // wire pinning exists to catch.
        let record = StreamRecord {
            rx_utc_ms: 0,
            bytes: Vec::from_slice(&[0x00, 0x00, 0x80, 0x3f, 0x11]).unwrap(),
        };
        let decoded: Vec<Sample, 8> =
            samples_in(&record, SampleLayout::F32Le, Unit::Volts, 0, None).collect();
        assert_eq!(decoded.len(), 1);
    }

    #[test]
    fn a_declared_rate_of_zero_is_treated_as_no_rate_rather_than_divided_by() {
        let record = StreamRecord {
            rx_utc_ms: 5,
            bytes: Vec::from_slice(&[0x01, 0x00, 0x02, 0x00]).unwrap(),
        };
        let decoded: Vec<Sample, 8> =
            samples_in(&record, SampleLayout::U16Le, Unit::Raw, 0, Some(0)).collect();
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[1].rx_utc_ms, 5);
    }

    #[test]
    fn every_layouts_byte_order_is_what_it_says() {
        let be = StreamRecord { rx_utc_ms: 0, bytes: Vec::from_slice(&[0x01, 0x02]).unwrap() };
        let u16_be: Vec<Sample, 4> =
            samples_in(&be, SampleLayout::U16Be, Unit::Raw, 0, None).collect();
        assert_eq!(u16_be[0].value, 258.0);
        let u16_le: Vec<Sample, 4> =
            samples_in(&be, SampleLayout::U16Le, Unit::Raw, 0, None).collect();
        assert_eq!(u16_le[0].value, 513.0);

        let f32_be = StreamRecord {
            rx_utc_ms: 0,
            bytes: Vec::from_slice(&[0x3f, 0x80, 0x00, 0x00]).unwrap(),
        };
        let f: Vec<Sample, 4> =
            samples_in(&f32_be, SampleLayout::F32Be, Unit::Raw, 0, None).collect();
        assert_eq!(f[0].value, 1.0);
        assert_eq!(SampleLayout::I16Be.element_len(), 2);
        assert_eq!(SampleLayout::F32Be.element_len(), 4);
    }
}
