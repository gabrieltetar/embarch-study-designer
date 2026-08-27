//! Engineer-declared struct decoding for a stream tap's payloads — design.md
//! §3 decision 52, §4.8a.
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
//! chosen label to literal bytes. design.md §3 decision 35 is unchanged and
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

    /// Parses the spelling [`as_str`](Self::as_str) produces. `None` for
    /// anything else — a hand-edited `study-structs.toml` typo is named,
    /// never silently defaulted to a plausible width.
    pub fn parse(text: &str) -> Option<ScalarType> {
        const ALL: [ScalarType; 18] = [
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
        ALL.into_iter().find(|t| t.as_str() == text)
    }

    /// Reads this field out of `bytes`, which must be exactly
    /// [`width`](Self::width) long, rendering it into `out` the way the CSV
    /// column carries it: integers as integers, floats as floats. **Never as
    /// an `f32` first** — a `u64` sample counter round-tripped through `f32`
    /// loses its low bits, which is precisely the kind of plausible-but-wrong
    /// number this crate keeps refusing to produce.
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

/// A named payload layout — design.md §3 decision 52.
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
}

/// Why a [`StructLayout`] can't be used, or can't decode a payload.
///
/// A [`DecodeError`] never discards the record: the raw bytes are already on
/// disk before any decode is attempted, and the rendered row still gets
/// written with its decoded columns empty and this reason in `decode_note`
/// (design.md §3 decision 52). A failed decode costs a rendering, not a
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
        }
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
}
