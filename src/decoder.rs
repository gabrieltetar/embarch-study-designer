//! Engineer-declared struct decoding for a stream tap's payloads — decision
//! 52 (interfaces/decoders.md).
//!
//! **This is [`crate::streams::StreamEncoding`]'s "the only place a byte
//! payload acquires a meaning" rule, made expressive enough to be useful.**
//! `StreamEncoding::Samples` could say "these bytes are packed `i16`s"; it
//! could not say "two header fields, then a repeating triple", which is what
//! a real sensor notification actually looks like. A `GattNotify` tap whose
//! payload is a small header plus a packed sample array had no honest
//! rendering before this existed — only `Raw`, which produces a `.bin` and
//! no CSV at all.
//!
//! **Nothing here is ever inferred.** A [`StructLayout`] is authored by an
//! engineer in the firmware repo's own `embarch/study-structs.toml`
//! ([`crate::registry::StructRegistry`]), named there, and resolved into the
//! submitted `Study` at build time — the same shape
//! [`crate::study_builder::RowAction::Registered`] already uses to resolve a
//! chosen label to literal bytes. decision 35 is unchanged and
//! this is an instance of it, not an exception: the engineer states the
//! layout, this module only applies it.
//!
//! # Where the layout travels, and where it deliberately does not
//!
//! A resolved [`StructLayout`] rides in `Study.decoders` — a **host-only**
//! field, like `Study.requires`, that is never transmitted to dev-bench.
//! dev-bench captures bytes and stamps their arrival; what a payload *means*
//! is exactly the knowledge decision 39 took away from it. Only the one-byte
//! index into that list rides on the tap itself
//! ([`crate::streams::StreamEncoding::Struct`]), because a tap's encoding
//! does cross the wire inside `StudyStart` and dev-bench has to be able to
//! walk past it.

use heapless::{String, Vec};
use serde::{Deserialize, Serialize};

use crate::limits::{
    MAX_DECODER_NAME_LEN, MAX_STRUCT_CSV_ROW_LEN, MAX_STRUCT_FIELDS, MAX_STRUCT_FIELD_NAME_LEN,
};

/// One scalar field's width, signedness and byte order — and nothing else.
///
/// **No scale, no offset, no unit**, for the same reason
/// [`crate::streams::SampleLayout`] carries none: those are a claim about
/// what a particular DUT's numbers mean, which is the engineer's knowledge.
/// A raw ADC count renders as a raw ADC count, under the name the engineer
/// gave it.
///
/// Append-only, like every other enum in this crate that reaches a wire or a
/// persisted file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScalarType {
    U8,
    I8,
    U16Le,
    U16Be,
    I16Le,
    I16Be,
    U32Le,
    U32Be,
    I32Le,
    I32Be,
    U64Le,
    U64Be,
    I64Le,
    I64Be,
    F32Le,
    F32Be,
    F64Le,
    F64Be,
}

impl ScalarType {
    /// Bytes this field occupies in a payload.
    pub const fn width(self) -> usize {
        match self {
            ScalarType::U8 | ScalarType::I8 => 1,
            ScalarType::U16Le | ScalarType::U16Be | ScalarType::I16Le | ScalarType::I16Be => 2,
            ScalarType::U32Le
            | ScalarType::U32Be
            | ScalarType::I32Le
            | ScalarType::I32Be
            | ScalarType::F32Le
            | ScalarType::F32Be => 4,
            ScalarType::U64Le
            | ScalarType::U64Be
            | ScalarType::I64Le
            | ScalarType::I64Be
            | ScalarType::F64Le
            | ScalarType::F64Be => 8,
        }
    }

    /// The TOML/JSON spelling, so no caller writes the string itself.
    pub const fn as_str(self) -> &'static str {
        match self {
            ScalarType::U8 => "u8",
            ScalarType::I8 => "i8",
            ScalarType::U16Le => "u16le",
            ScalarType::U16Be => "u16be",
            ScalarType::I16Le => "i16le",
            ScalarType::I16Be => "i16be",
            ScalarType::U32Le => "u32le",
            ScalarType::U32Be => "u32be",
            ScalarType::I32Le => "i32le",
            ScalarType::I32Be => "i32be",
            ScalarType::U64Le => "u64le",
            ScalarType::U64Be => "u64be",
            ScalarType::I64Le => "i64le",
            ScalarType::I64Be => "i64be",
            ScalarType::F32Le => "f32le",
            ScalarType::F32Be => "f32be",
            ScalarType::F64Le => "f64le",
            ScalarType::F64Be => "f64be",
        }
    }

    /// Every scalar type, in the order a picker should offer them: the two
    /// one-byte widths, then each larger width little-endian before
    /// big-endian, integers before floats.
    ///
    /// **Public because a host serves this list rather than restating it.**
    /// A layout editor needs the eighteen spellings, and an eighteen-entry
    /// array retyped in JavaScript is the same drift `embarch-ui` decision 17
    /// argues about limits and `suite/017` closed for action labels — one
    /// copy, served, is the answer this suite keeps arriving at. A host that
    /// is served an empty list renders an empty picker, which is a refusal;
    /// it must not fall back to a guessed eighteen.
    ///
    /// [`parse`](Self::parse) iterates this rather than holding its own copy,
    /// so a nineteenth variant becomes parseable and offerable in one edit.
    pub const ALL: [ScalarType; 18] = [
        ScalarType::U8,
        ScalarType::I8,
        ScalarType::U16Le,
        ScalarType::U16Be,
        ScalarType::I16Le,
        ScalarType::I16Be,
        ScalarType::U32Le,
        ScalarType::U32Be,
        ScalarType::I32Le,
        ScalarType::I32Be,
        ScalarType::U64Le,
        ScalarType::U64Be,
        ScalarType::I64Le,
        ScalarType::I64Be,
        ScalarType::F32Le,
        ScalarType::F32Be,
        ScalarType::F64Le,
        ScalarType::F64Be,
    ];

    /// Parses the spelling [`as_str`](Self::as_str) produces. `None` for
    /// anything else — a hand-edited `study-structs.toml` typo is named,
    /// never silently defaulted to a plausible width.
    pub fn parse(text: &str) -> Option<ScalarType> {
        Self::ALL.into_iter().find(|t| t.as_str() == text)
    }

    /// Reads this field out of `bytes`, which must be exactly
    /// [`width`](Self::width) long, rendering it into `out` the way the CSV
    /// column carries it: integers as integers, floats as floats. **Never as
    /// an `f32` first** — a `u64` sample counter round-tripped through `f32`
    /// loses its low bits, which is precisely the kind of plausible-but-wrong
    /// number this crate keeps refusing to produce.
    /// Read this field as a signed 64-bit integer, or `None` if it is one
    /// of the two float widths.
    ///
    /// Added for `.eap` guard evaluation (decisions 59/60),
    /// which is integer-only: [`crate::eap::Operand::Literal`] is an `i64`,
    /// so a float field has nothing it could be compared against. Returning
    /// `None` rather than lossily converting is the same refusal decision 52
    /// already makes about `SampleLayout` — "integers render as integers,
    /// because a `u64` round-tripped through an `f32` is a plausible, wrong
    /// number."
    ///
    /// `u64` values above `i64::MAX` saturate rather than wrapping negative.
    /// A DUT counter that large is not a real value this suite will compare,
    /// and a silently negative one would be the plausible-wrong-number
    /// failure again.
    /// Read this field as an `f64` — never `None`, unlike
    /// [`read_i64`](Self::read_i64): every width this crate declares fits
    /// losslessly or acceptably-lossily into an `f64` (a `u64`/`i64` past
    /// 2^53 loses precision, the same way any other language's "just use a
    /// double" does), so there is no width this has to refuse.
    ///
    /// Decoded straight from the native type and cast, not routed through
    /// [`read_i64`]'s `.min(i64::MAX as u64)` clamp — that clamp exists only
    /// so a caller needing a signed `i64` (`.eap` guard comparisons) never
    /// sees a `u64` wrap negative. `f64` has no such caller-shaped need: a
    /// `u64` cast straight to `f64` is already the nearest representable
    /// value, clamping it first would just move *where* the precision loss
    /// happens.
    ///
    /// Added for [`StructLayout::chart_value`] — a live chart wants a plain
    /// number for every declared width, floats included, which `read_i64`
    /// structurally cannot give it.
    ///
    /// `bytes` must be at least [`width`](Self::width) long — the same
    /// precondition this type's own `render` already carries, since a caller
    /// (`chart_value`) is expected to bounds-check once, the way `row` does,
    /// rather than have every per-field decode repeat that check.
    pub fn read_f64(self, bytes: &[u8]) -> f64 {
        macro_rules! le_be {
            ($ty:ty, $n:expr, $be:expr) => {{
                let mut buf = [0u8; $n];
                buf.copy_from_slice(&bytes[..$n]);
                let v = if $be { <$ty>::from_be_bytes(buf) } else { <$ty>::from_le_bytes(buf) };
                v as f64
            }};
        }
        match self {
            ScalarType::U8 => bytes[0] as f64,
            ScalarType::I8 => bytes[0] as i8 as f64,
            ScalarType::U16Le => le_be!(u16, 2, false),
            ScalarType::U16Be => le_be!(u16, 2, true),
            ScalarType::I16Le => le_be!(i16, 2, false),
            ScalarType::I16Be => le_be!(i16, 2, true),
            ScalarType::U32Le => le_be!(u32, 4, false),
            ScalarType::U32Be => le_be!(u32, 4, true),
            ScalarType::I32Le => le_be!(i32, 4, false),
            ScalarType::I32Be => le_be!(i32, 4, true),
            ScalarType::U64Le => le_be!(u64, 8, false),
            ScalarType::U64Be => le_be!(u64, 8, true),
            ScalarType::I64Le => le_be!(i64, 8, false),
            ScalarType::I64Be => le_be!(i64, 8, true),
            ScalarType::F32Le => le_be!(f32, 4, false),
            ScalarType::F32Be => le_be!(f32, 4, true),
            ScalarType::F64Le => le_be!(f64, 8, false),
            ScalarType::F64Be => le_be!(f64, 8, true),
        }
    }

    pub fn read_i64(self, bytes: &[u8]) -> Option<i64> {
        if bytes.len() < self.width() {
            return None;
        }
        macro_rules! int {
            ($ty:ty, $n:expr, $be:expr) => {{
                let mut buf = [0u8; $n];
                buf.copy_from_slice(&bytes[..$n]);
                let v = if $be { <$ty>::from_be_bytes(buf) } else { <$ty>::from_le_bytes(buf) };
                v as i64
            }};
        }
        Some(match self {
            ScalarType::U8 => bytes[0] as i64,
            ScalarType::I8 => bytes[0] as i8 as i64,
            ScalarType::U16Le => int!(u16, 2, false),
            ScalarType::U16Be => int!(u16, 2, true),
            ScalarType::I16Le => int!(i16, 2, false),
            ScalarType::I16Be => int!(i16, 2, true),
            ScalarType::U32Le => int!(u32, 4, false),
            ScalarType::U32Be => int!(u32, 4, true),
            ScalarType::I32Le => int!(i32, 4, false),
            ScalarType::I32Be => int!(i32, 4, true),
            ScalarType::U64Le => {
                let mut b = [0u8; 8];
                b.copy_from_slice(&bytes[..8]);
                u64::from_le_bytes(b).min(i64::MAX as u64) as i64
            }
            ScalarType::U64Be => {
                let mut b = [0u8; 8];
                b.copy_from_slice(&bytes[..8]);
                u64::from_be_bytes(b).min(i64::MAX as u64) as i64
            }
            ScalarType::I64Le => int!(i64, 8, false),
            ScalarType::I64Be => int!(i64, 8, true),
            ScalarType::F32Le | ScalarType::F32Be | ScalarType::F64Le | ScalarType::F64Be => {
                return None
            }
        })
    }

    /// Whether this width carries an integer — the `.eap` grammar's own
    /// admission test for a `select_if`-reachable field.
    pub const fn is_integer(self) -> bool {
        !matches!(
            self,
            ScalarType::F32Le | ScalarType::F32Be | ScalarType::F64Le | ScalarType::F64Be
        )
    }

    /// Write an integer into `out` in this field's width and byte order,
    /// truncating to the width. Used to assemble a `write` payload
    /// (decision 61) from the same vocabulary decode reads.
    ///
    /// Returns `None` for a float width, matching [`read_i64`](Self::read_i64).
    pub fn write_i64(self, value: i64, out: &mut [u8]) -> Option<usize> {
        let w = self.width();
        if out.len() < w || !self.is_integer() {
            return None;
        }
        let raw = (value as u64).to_le_bytes();
        // Little-endian source, reversed for the big-endian widths. A
        // truncating write is deliberate and matches what a C firmware
        // assembling the same packet would do with a cast.
        let be = matches!(
            self,
            ScalarType::U16Be | ScalarType::I16Be | ScalarType::U32Be
                | ScalarType::I32Be | ScalarType::U64Be | ScalarType::I64Be
        );
        for i in 0..w {
            out[i] = if be { raw[w - 1 - i] } else { raw[i] };
        }
        Some(w)
    }

    fn render(self, bytes: &[u8], out: &mut String<MAX_STRUCT_CSV_ROW_LEN>) -> core::fmt::Result {
        use core::fmt::Write;
        macro_rules! le_be {
            ($ty:ty, $n:expr, $be:expr) => {{
                let mut buf = [0u8; $n];
                buf.copy_from_slice(bytes);
                let v = if $be { <$ty>::from_be_bytes(buf) } else { <$ty>::from_le_bytes(buf) };
                write!(out, "{v}")
            }};
        }
        match self {
            ScalarType::U8 => write!(out, "{}", bytes[0]),
            ScalarType::I8 => write!(out, "{}", bytes[0] as i8),
            ScalarType::U16Le => le_be!(u16, 2, false),
            ScalarType::U16Be => le_be!(u16, 2, true),
            ScalarType::I16Le => le_be!(i16, 2, false),
            ScalarType::I16Be => le_be!(i16, 2, true),
            ScalarType::U32Le => le_be!(u32, 4, false),
            ScalarType::U32Be => le_be!(u32, 4, true),
            ScalarType::I32Le => le_be!(i32, 4, false),
            ScalarType::I32Be => le_be!(i32, 4, true),
            ScalarType::U64Le => le_be!(u64, 8, false),
            ScalarType::U64Be => le_be!(u64, 8, true),
            ScalarType::I64Le => le_be!(i64, 8, false),
            ScalarType::I64Be => le_be!(i64, 8, true),
            ScalarType::F32Le => le_be!(f32, 4, false),
            ScalarType::F32Be => le_be!(f32, 4, true),
            ScalarType::F64Le => le_be!(f64, 8, false),
            ScalarType::F64Be => le_be!(f64, 8, true),
        }
    }
}

/// One named scalar in a [`StructLayout`]. Fields are packed in declaration
/// order with no padding — a layout that needs padding declares a field for
/// it, rather than this module guessing at an alignment rule the DUT's
/// compiler may or may not have applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructField {
    /// Becomes a CSV column header, so it is bounded like every other name
    /// this crate renders.
    pub name: String<MAX_STRUCT_FIELD_NAME_LEN>,
    #[serde(rename = "type")]
    pub ty: ScalarType,
}

/// A named payload layout — decision 52.
///
/// `header` is read once at offset 0. `repeat`, when non-empty, is then read
/// as many times as fits in what remains, producing **one CSV row per
/// repetition** with the header's values denormalized onto each. That is the
/// whole reason this type exists rather than a flat field list: a
/// notification carrying a sequence number and twenty packed samples is one
/// record and twenty rows, and rendering it as one row with twenty columns
/// makes it unanalyzable by every tool that reads a CSV.
///
/// A payload with an empty `repeat` produces exactly one row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructLayout {
    pub name: String<MAX_DECODER_NAME_LEN>,
    #[serde(default)]
    pub header: Vec<StructField, MAX_STRUCT_FIELDS>,
    /// Read repeatedly until fewer than [`repeat_width`](Self::repeat_width)
    /// bytes remain. Empty means "no repeating part", not "repeat nothing".
    #[serde(default)]
    pub repeat: Vec<StructField, MAX_STRUCT_FIELDS>,
    /// The one field, of `header` or `repeat`, whose value Core pushes live
    /// (`StudyEvent::StructChartValue`) the instant it decodes a row —
    /// separate from the rendered CSV, which every field still reaches
    /// regardless of this. Named by [`StructField::name`].
    ///
    /// `None` — the ordinary starting state most layouts leave this in — is
    /// "no live chart for this layout", not an oversight: charting is opt-in
    /// per layout, and exactly one field, because a live chart is one line
    /// on one axis, not every numeric column charted at once.
    #[serde(default)]
    pub chart_field: Option<String<MAX_STRUCT_FIELD_NAME_LEN>>,
}

/// Why a [`StructLayout`] can't be used, or can't decode a payload.
///
/// A [`DecodeError`] never discards the record: the raw bytes are already on
/// disk before any decode is attempted, and the rendered row still gets
/// written with its decoded columns empty and this reason in `decode_note`
/// (decision 52). A failed decode costs a rendering, not a
/// capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// The payload is shorter than the header alone.
    ShortHeader { need: usize, have: usize },
    /// The header fit, but what followed is neither empty nor a whole number
    /// of repetitions — so the layout does not describe these bytes, and
    /// decoding the whole repetitions anyway would present a partial packet
    /// as a complete one.
    TrailingBytes { extra: usize, repeat_width: usize },
    /// A layout with no `header` and no `repeat` describes nothing.
    Empty,
}

impl core::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            // **No commas in any of these.** The rendered text lands in a
            // CSV column, and this crate's stance on a value that would
            // break the column shape is to not produce one rather than to
            // quote it (`gatt::csv_escape_ok`'s own rule).
            DecodeError::ShortHeader { need, have } => {
                write!(f, "{have} bytes but layout header needs {need}")
            }
            DecodeError::TrailingBytes { extra, repeat_width } => write!(
                f,
                "{extra} trailing byte(s) after the last whole {repeat_width}-byte repetition"
            ),
            DecodeError::Empty => write!(f, "layout declares no fields"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for DecodeError {}

impl StructLayout {
    /// Bytes the header occupies.
    pub fn header_width(&self) -> usize {
        self.header.iter().map(|f| f.ty.width()).sum()
    }

    /// Bytes one repetition occupies; 0 when there is no repeating part.
    pub fn repeat_width(&self) -> usize {
        self.repeat.iter().map(|f| f.ty.width()).sum()
    }

    /// The decoded column names this layout contributes, in row order:
    /// `rep_index`, then every header field, then every repeat field.
    ///
    /// Core prepends its own fixed columns and appends `payload_hex`,
    /// `decode_note` and `core_rx_utc_ms` around this — column *knowledge*
    /// stays here, exactly as it does for `Sample::csv_header`.
    pub fn column_header(&self) -> Result<String<MAX_STRUCT_CSV_ROW_LEN>, DecodeError> {
        if self.header.is_empty() && self.repeat.is_empty() {
            return Err(DecodeError::Empty);
        }
        let mut out: String<MAX_STRUCT_CSV_ROW_LEN> = String::new();
        // `rep_index` is emitted even for a layout with no repeating part,
        // so every Struct-encoded CSV has the same column skeleton and a
        // reader doesn't have to know which kind of layout produced it.
        let _ = out.push_str("rep_index");
        for field in self.header.iter().chain(self.repeat.iter()) {
            let _ = out.push(',');
            let _ = out.push_str(&field.name);
        }
        Ok(out)
    }

    /// How many rows `payload` produces, or why it produces none.
    ///
    /// A layout with a repeating part and a payload holding exactly the
    /// header produces **zero** rows and no error: an empty repetition list
    /// is a real thing for a DUT to send, and inventing a row for it would
    /// be inventing data.
    pub fn row_count(&self, payload: &[u8]) -> Result<usize, DecodeError> {
        if self.header.is_empty() && self.repeat.is_empty() {
            return Err(DecodeError::Empty);
        }
        let header_width = self.header_width();
        if payload.len() < header_width {
            return Err(DecodeError::ShortHeader { need: header_width, have: payload.len() });
        }
        let rest = payload.len() - header_width;
        let repeat_width = self.repeat_width();
        if repeat_width == 0 {
            // No repeating part: the payload must be exactly the header.
            // Extra bytes mean the layout doesn't describe this packet.
            if rest != 0 {
                return Err(DecodeError::TrailingBytes { extra: rest, repeat_width: 0 });
            }
            return Ok(1);
        }
        if !rest.is_multiple_of(repeat_width) {
            return Err(DecodeError::TrailingBytes {
                extra: rest % repeat_width,
                repeat_width,
            });
        }
        Ok(rest / repeat_width)
    }

    /// Renders row `index` of `payload` as the comma-separated decoded
    /// columns [`column_header`](Self::column_header) names — no leading or
    /// trailing comma, so a caller composes it into its own row.
    ///
    /// `index` must be below [`row_count`](Self::row_count); a caller
    /// iterating that count cannot exceed it.
    pub fn row(
        &self,
        payload: &[u8],
        index: usize,
    ) -> Result<String<MAX_STRUCT_CSV_ROW_LEN>, DecodeError> {
        let count = self.row_count(payload)?;
        debug_assert!(index < count, "row index past row_count");
        let _ = count;
        let mut out: String<MAX_STRUCT_CSV_ROW_LEN> = String::new();
        // Rendering into a bounded String can only fail by overflowing it;
        // that is a truncated row rather than a wrong one, and the raw .bin
        // is authoritative either way.
        let _ = render_usize(&mut out, index);
        let mut at = 0usize;
        for field in &self.header {
            let width = field.ty.width();
            let _ = out.push(',');
            let _ = field.ty.render(&payload[at..at + width], &mut out);
            at += width;
        }
        at = self.header_width() + index * self.repeat_width();
        for field in &self.repeat {
            let width = field.ty.width();
            let _ = out.push(',');
            let _ = field.ty.render(&payload[at..at + width], &mut out);
            at += width;
        }
        Ok(out)
    }

    /// The `chart_field`'s value for row `index` of `payload`, or `None`
    /// when no `chart_field` is declared, it names no field in this layout,
    /// or `payload` is too short for row `index` to exist at all (mirrors
    /// `row()`'s own bounds — never panics, never guesses).
    ///
    /// Purely additive: called *alongside* [`row`](Self::row), never in
    /// place of it. A `None` here costs a live chart one missing point,
    /// nothing more — the raw bytes on disk and the rendered CSV row are
    /// unaffected either way.
    pub fn chart_value(&self, payload: &[u8], index: usize) -> Option<f64> {
        let field_name = self.chart_field.as_ref()?;
        let count = self.row_count(payload).ok()?;
        if index >= count {
            return None;
        }
        // Header fields are read once at a fixed offset, mirroring `row()`.
        let mut at = 0usize;
        for field in &self.header {
            let width = field.ty.width();
            if &field.name == field_name {
                return payload.get(at..at + width).map(|bytes| field.ty.read_f64(bytes));
            }
            at += width;
        }
        // Repeat fields are read at header_width() + index * repeat_width(),
        // plus the field's own offset within one repetition — the same
        // arithmetic `row()` uses.
        at = self.header_width() + index * self.repeat_width();
        for field in &self.repeat {
            let width = field.ty.width();
            if &field.name == field_name {
                return payload.get(at..at + width).map(|bytes| field.ty.read_f64(bytes));
            }
            at += width;
        }
        // `chart_field` names nothing in this layout. Reachable only when a
        // hand-edited or otherwise-unvalidated layout skipped
        // `StructDef::to_layout`'s own check (`registry.rs`), which refuses
        // exactly this at authoring time.
        None
    }

    /// The empty decoded columns a row carries when the payload didn't match
    /// this layout — one empty field per column, so a failed row still lines
    /// up with the header instead of shifting every later column left.
    pub fn empty_columns(&self) -> String<MAX_STRUCT_CSV_ROW_LEN> {
        let mut out: String<MAX_STRUCT_CSV_ROW_LEN> = String::new();
        // One separator per column boundary: `rep_index` plus every field
        // means `header.len() + repeat.len()` commas and no trailing one.
        for _ in 0..(self.header.len() + self.repeat.len()) {
            let _ = out.push(',');
        }
        out
    }
}

fn render_usize(out: &mut String<MAX_STRUCT_CSV_ROW_LEN>, value: usize) -> core::fmt::Result {
    use core::fmt::Write;
    write!(out, "{value}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(name: &str, ty: ScalarType) -> StructField {
        StructField { name: String::try_from(name).unwrap(), ty }
    }

    fn layout(name: &str, header: &[StructField], repeat: &[StructField]) -> StructLayout {
        StructLayout {
            name: String::try_from(name).unwrap(),
            header: Vec::from_slice(header).unwrap(),
            repeat: Vec::from_slice(repeat).unwrap(),
            chart_field: None,
        }
    }

    fn layout_with_chart(
        name: &str,
        header: &[StructField],
        repeat: &[StructField],
        chart_field: &str,
    ) -> StructLayout {
        let mut l = layout(name, header, repeat);
        l.chart_field = Some(String::try_from(chart_field).unwrap());
        l
    }

    #[test]
    fn a_header_only_layout_produces_exactly_one_row() {
        let l = layout("batt", &[field("percent", ScalarType::U8)], &[]);
        assert_eq!(l.column_header().unwrap().as_str(), "rep_index,percent");
        assert_eq!(l.row_count(&[97]).unwrap(), 1);
        assert_eq!(l.row(&[97], 0).unwrap().as_str(), "0,97");
    }

    #[test]
    fn a_repeating_group_produces_one_row_per_repetition() {
        // The whole reason this type exists: a sequence number plus packed
        // samples is one record and N rows, not one row with N columns.
        let l = layout(
            "ppg",
            &[field("seq", ScalarType::U16Le)],
            &[field("green", ScalarType::I16Le), field("red", ScalarType::I16Le)],
        );
        assert_eq!(l.column_header().unwrap().as_str(), "rep_index,seq,green,red");
        let payload = [
            0x29, 0x00, // seq = 41
            0x01, 0x00, 0x02, 0x00, // green 1, red 2
            0xff, 0xff, 0xfe, 0xff, // green -1, red -2
        ];
        assert_eq!(l.row_count(&payload).unwrap(), 2);
        assert_eq!(l.row(&payload, 0).unwrap().as_str(), "0,41,1,2");
        assert_eq!(l.row(&payload, 1).unwrap().as_str(), "1,41,-1,-2");
    }

    #[test]
    fn an_empty_repetition_list_is_zero_rows_and_not_an_error() {
        // A DUT sending a header with nothing after it is a real thing; a
        // row invented for it would be invented data.
        let l = layout("ppg", &[field("seq", ScalarType::U16Le)], &[field("g", ScalarType::I16Le)]);
        assert_eq!(l.row_count(&[0x01, 0x00]).unwrap(), 0);
    }

    #[test]
    fn a_payload_that_does_not_fit_the_layout_is_named_not_forced() {
        let l = layout("ppg", &[field("seq", ScalarType::U16Le)], &[field("g", ScalarType::I16Le)]);
        assert_eq!(
            l.row_count(&[0x01]),
            Err(DecodeError::ShortHeader { need: 2, have: 1 })
        );
        // Three bytes past the header is one whole repetition plus one
        // stray byte — decoding the whole one anyway would present a
        // partial packet as complete.
        assert_eq!(
            l.row_count(&[0x01, 0x00, 0x02, 0x00, 0x03]),
            Err(DecodeError::TrailingBytes { extra: 1, repeat_width: 2 })
        );
        let empty = layout("nothing", &[], &[]);
        assert_eq!(empty.row_count(&[]), Err(DecodeError::Empty));
        assert_eq!(empty.column_header(), Err(DecodeError::Empty));
    }

    #[test]
    fn no_decode_error_renders_a_comma_or_a_quote() {
        // The reason text lands in a CSV column. This crate refuses to
        // produce a value that would break the column shape rather than
        // quoting it -- the same rule `gatt::csv_escape_ok` applies to a
        // step name.
        for e in [
            DecodeError::ShortHeader { need: 6, have: 3 },
            DecodeError::TrailingBytes { extra: 1, repeat_width: 4 },
            DecodeError::Empty,
        ] {
            let mut text: String<MAX_STRUCT_CSV_ROW_LEN> = String::new();
            use core::fmt::Write as _;
            write!(text, "{e}").unwrap();
            assert!(!text.contains(','), "{}", text.as_str());
            assert!(!text.contains('"'), "{}", text.as_str());
        }
    }

    #[test]
    fn a_header_only_layout_refuses_a_longer_payload() {
        // Without this, a 20-byte packet decoded against a 2-byte layout
        // would render its first two bytes and silently drop the other 18.
        let l = layout("batt", &[field("percent", ScalarType::U8)], &[]);
        assert_eq!(
            l.row_count(&[97, 98]),
            Err(DecodeError::TrailingBytes { extra: 1, repeat_width: 0 })
        );
    }

    #[test]
    fn an_empty_row_lines_up_with_the_header_it_could_not_fill() {
        let l = layout(
            "ppg",
            &[field("seq", ScalarType::U16Le)],
            &[field("green", ScalarType::I16Le), field("red", ScalarType::I16Le)],
        );
        let header = l.column_header().unwrap();
        let empty = l.empty_columns();
        assert_eq!(
            header.matches(',').count(),
            empty.matches(',').count(),
            "a failed row must not shift every later column left"
        );
    }

    #[test]
    fn integers_are_never_routed_through_a_float() {
        // A u64 counter round-tripped through f32 loses its low bits, which
        // is exactly the plausible-but-wrong number this crate refuses to
        // produce. SampleLayout can only give f32; this is why.
        let l = layout("t", &[field("counter", ScalarType::U64Le)], &[]);
        let payload = 0x0020_0000_0000_0001u64.to_le_bytes();
        assert_eq!(l.row(&payload, 0).unwrap().as_str(), "0,9007199254740993");
    }

    #[test]
    fn every_width_and_byte_order_is_what_it_says() {
        assert_eq!(ScalarType::U8.width(), 1);
        assert_eq!(ScalarType::I16Be.width(), 2);
        assert_eq!(ScalarType::F32Le.width(), 4);
        assert_eq!(ScalarType::F64Be.width(), 8);
        let be = layout("t", &[field("v", ScalarType::U16Be)], &[]);
        assert_eq!(be.row(&[0x01, 0x02], 0).unwrap().as_str(), "0,258");
        let le = layout("t", &[field("v", ScalarType::U16Le)], &[]);
        assert_eq!(le.row(&[0x01, 0x02], 0).unwrap().as_str(), "0,513");
        let f = layout("t", &[field("v", ScalarType::F32Be)], &[]);
        assert_eq!(f.row(&[0x3f, 0x80, 0x00, 0x00], 0).unwrap().as_str(), "0,1");
    }

    #[test]
    fn every_scalar_type_round_trips_through_its_own_spelling() {
        // The TOML file is hand-editable, so a typo must be named rather
        // than silently defaulted to a plausible width.
        for ty in [
            ScalarType::U8, ScalarType::I8, ScalarType::U16Le, ScalarType::U16Be,
            ScalarType::I16Le, ScalarType::I16Be, ScalarType::U32Le, ScalarType::U32Be,
            ScalarType::I32Le, ScalarType::I32Be, ScalarType::U64Le, ScalarType::U64Be,
            ScalarType::I64Le, ScalarType::I64Be, ScalarType::F32Le, ScalarType::F32Be,
            ScalarType::F64Le, ScalarType::F64Be,
        ] {
            assert_eq!(ScalarType::parse(ty.as_str()), Some(ty), "{}", ty.as_str());
        }
        assert_eq!(ScalarType::parse("u24le"), None);
        assert_eq!(ScalarType::parse(""), None);
    }

    #[test]
    fn chart_value_reads_a_header_field() {
        let l = layout_with_chart(
            "batt",
            &[field("percent", ScalarType::U8), field("mv", ScalarType::U16Le)],
            &[],
            "mv",
        );
        // percent=97, mv=3700 (0x0e74 little-endian)
        assert_eq!(l.chart_value(&[97, 0x74, 0x0e], 0), Some(3700.0));
    }

    #[test]
    fn chart_value_reads_a_repeat_field_and_differs_per_row() {
        let l = layout_with_chart(
            "ppg",
            &[field("seq", ScalarType::U16Le)],
            &[field("green", ScalarType::I16Le), field("red", ScalarType::I16Le)],
            "green",
        );
        let payload = [
            0x29, 0x00, // seq = 41
            0x01, 0x00, 0x02, 0x00, // green 1, red 2
            0xff, 0xff, 0xfe, 0xff, // green -1, red -2
        ];
        assert_eq!(l.chart_value(&payload, 0), Some(1.0));
        assert_eq!(l.chart_value(&payload, 1), Some(-1.0));
    }

    #[test]
    fn chart_value_is_none_when_no_chart_field_is_declared() {
        let l = layout("batt", &[field("percent", ScalarType::U8)], &[]);
        assert_eq!(l.chart_value(&[97], 0), None);
    }

    #[test]
    fn chart_value_is_none_when_chart_field_names_nothing_in_the_layout() {
        // Reachable only from a layout that skipped `StructDef::to_layout`'s
        // own check -- `chart_value` still refuses to guess rather than
        // panicking or picking the nearest name.
        let mut l = layout("batt", &[field("percent", ScalarType::U8)], &[]);
        l.chart_field = Some(String::try_from("does_not_exist").unwrap());
        assert_eq!(l.chart_value(&[97], 0), None);
    }

    #[test]
    fn chart_value_is_none_for_a_row_index_the_payload_cannot_hold() {
        let l = layout_with_chart(
            "ppg",
            &[field("seq", ScalarType::U16Le)],
            &[field("green", ScalarType::I16Le)],
            "green",
        );
        let payload = [0x29, 0x00, 0x01, 0x00]; // seq + exactly one repetition
        assert_eq!(l.chart_value(&payload, 0), Some(1.0));
        // Row 1 does not exist -- must not panic or invent a value.
        assert_eq!(l.chart_value(&payload, 1), None);
    }

    #[test]
    fn chart_value_is_none_when_the_payload_is_too_short_to_decode_at_all() {
        let l = layout_with_chart("batt", &[field("percent", ScalarType::U8)], &[], "percent");
        assert_eq!(l.chart_value(&[], 0), None);
    }

    #[test]
    fn read_f64_never_returns_none_and_matches_read_i64_for_integers() {
        // Unlike `read_i64`, every width -- floats included -- produces a
        // value. For the integer widths the two must agree once cast.
        let i64be_bytes = 0x0020_0000_0000_0001i64.to_be_bytes();
        let cases: [(ScalarType, &[u8]); 4] = [
            (ScalarType::U8, &[200][..]),
            (ScalarType::I8, &[0xff][..]),
            (ScalarType::U16Le, &[0x01, 0x02][..]),
            (ScalarType::I64Be, &i64be_bytes[..]),
        ];
        for (ty, bytes) in cases {
            assert_eq!(ty.read_f64(bytes), ty.read_i64(bytes).unwrap() as f64);
        }
        // Floats: `read_i64` refuses these, `read_f64` decodes them.
        assert_eq!(ScalarType::F32Be.read_f64(&[0x3f, 0x80, 0x00, 0x00]), 1.0);
        assert_eq!(ScalarType::F64Le.read_f64(&1.5f64.to_le_bytes()), 1.5);
    }

    #[test]
    fn read_f64_does_not_clamp_a_large_u64_the_way_read_i64_does() {
        // `read_i64` saturates a u64 above i64::MAX; `read_f64` has no such
        // caller-shaped need and casts straight through, losing only the
        // ordinary f64 precision a value this large already implies.
        let bytes = u64::MAX.to_le_bytes();
        assert_eq!(ScalarType::U64Le.read_f64(&bytes), u64::MAX as f64);
        assert_eq!(ScalarType::U64Le.read_i64(&bytes), Some(i64::MAX));
    }
}

#[cfg(test)]
mod scalar_vocabulary_tests {
    use super::ScalarType;

    /// `ALL` is the served picker list and `parse` iterates it, so a variant
    /// missing from `ALL` is a spelling nothing can author *and* nothing can
    /// load. Distinct spellings, all of them parsing back, and the count.
    #[test]
    fn all_is_complete_and_its_spellings_are_distinct() {
        assert_eq!(ScalarType::ALL.len(), 18);
        let mut seen: heapless::Vec<&str, 18> = heapless::Vec::new();
        for t in ScalarType::ALL {
            assert_eq!(ScalarType::parse(t.as_str()), Some(t));
            assert!(!seen.contains(&t.as_str()), "duplicate spelling {}", t.as_str());
            seen.push(t.as_str()).unwrap();
        }
    }

    /// Anything that is not one of the eighteen is named, never defaulted to
    /// a plausible width.
    #[test]
    fn an_unknown_spelling_is_refused() {
        assert_eq!(ScalarType::parse("u24le"), None);
        assert_eq!(ScalarType::parse("U8"), None);
        assert_eq!(ScalarType::parse(""), None);
    }
}

