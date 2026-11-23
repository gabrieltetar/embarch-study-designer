//! The PDF text extractor of `interfaces/hil-pdf.md`, for `verify`'s check 4.
//! Host-only, behind the `pdf-text` feature (decision 101).
//!
//! Pages come back in the page tree's order; each page's glyph runs in
//! content-stream order as `{text, x, y, size}`. Every glyph is mapped through
//! its font's ToUnicode map and one with no mapping is an error naming its page
//! (a numeral that cannot be read back is never skipped). Text is NFKC
//! normalised, U+2212 kept. No reading-order heuristic, no re-flow, no
//! de-hyphenation. Pictures and paths yield no text. Form XObjects are
//! followed; nothing else is.

use lopdf::{Dictionary, Document, Object, ObjectId};
use std::collections::BTreeMap;
use std::string::String;
use std::vec::Vec;
use unicode_normalization::UnicodeNormalization;

/// A line break is a change of `y` over this fraction of the font size (hil-pdf.md).
pub const LINE_BREAK_Y_SIZE: f64 = 0.5;
/// A word break is a horizontal gap between two glyphs over this fraction of the
/// font size (hil-pdf.md; ASSUMED, not measured until a fixture is rendered).
pub const WORD_GAP_SIZE: f64 = 0.3;
/// The advance, in em, of a glyph its font gives no width for (ASSUMED).
pub const DEFAULT_GLYPH_WIDTH_EM: f64 = 0.5;
const MAX_FORM_DEPTH: u32 = 8;

/// Why a PDF could not be read for check 4.
#[derive(Debug, Clone, PartialEq)]
pub enum ExtractError {
    /// The bytes are not a PDF this reader can open.
    Parse(String),
    /// A glyph with no ToUnicode mapping: "unextractable text on page N".
    Unmapped { page: u32, font: String, code: u32 },
    /// A text operator named a font the page's resources do not hold.
    MissingFont { page: u32, font: String },
    /// A content stream or font object could not be read.
    Malformed { page: u32, what: String },
}

impl core::fmt::Display for ExtractError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ExtractError::Parse(e) => write!(f, "not a readable PDF: {e}"),
            ExtractError::Unmapped { page, .. } => write!(f, "unextractable text on page {page}"),
            ExtractError::MissingFont { page, font } => {
                write!(f, "font {font} is not in the resources on page {page}")
            }
            ExtractError::Malformed { page, what } => write!(f, "page {page}: {what}"),
        }
    }
}

impl std::error::Error for ExtractError {}

/// One run of glyphs drawn with one positioning: a `Tj` string, or the part of
/// a `TJ` array between two adjustments that open a gap over [`WORD_GAP_SIZE`].
#[derive(Debug, Clone, PartialEq)]
pub struct GlyphRun {
    pub text: String,
    /// The run's origin in points from the page's bottom-left.
    pub x: f64,
    pub y: f64,
    /// The font size in points as drawn (the font size scaled by the matrices).
    pub size: f64,
    /// Where the run's last glyph's advance ends; what a gap is measured from.
    pub x_end: f64,
}

/// A page, 1-based by the page tree's order.
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub number: u32,
    pub runs: Vec<GlyphRun>,
}

/// A word: a maximal run of non-whitespace on one line.
#[derive(Debug, Clone, PartialEq)]
pub struct Word {
    pub text: String,
    /// Origin of the word's first glyph run.
    pub x: f64,
    pub y: f64,
    pub size: f64,
    pub x_end: f64,
    /// 0-based line index on the page, by [`LINE_BREAK_Y_SIZE`].
    pub line: usize,
}

type M = [f64; 6];
const IDENT: M = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// `a` then `b` (row-vector convention, as PDF writes it).
fn mul(a: &M, b: &M) -> M {
    [
        a[0] * b[0] + a[1] * b[2],
        a[0] * b[1] + a[1] * b[3],
        a[2] * b[0] + a[3] * b[2],
        a[2] * b[1] + a[3] * b[3],
        a[4] * b[0] + a[5] * b[2] + b[4],
        a[4] * b[1] + a[5] * b[3] + b[5],
    ]
}

// ---------------------------------------------------------------- fonts ----

struct Font {
    name: String,
    two_byte: bool,
    /// code -> text, from ToUnicode.
    map: BTreeMap<u32, String>,
    /// Byte lengths the ToUnicode codespace allows, if it states any.
    codespace_lens: Vec<usize>,
    widths: BTreeMap<u32, f64>,
    default_width: f64,
}

fn obj_num(o: &Object) -> Option<f64> {
    match o {
        Object::Integer(i) => Some(*i as f64),
        Object::Real(r) => Some(f64::from(*r)),
        _ => None,
    }
}

fn resolve<'a>(doc: &'a Document, o: &'a Object) -> &'a Object {
    match o {
        Object::Reference(id) => doc.get_object(*id).unwrap_or(o),
        _ => o,
    }
}

fn utf16_be_hex(hex: &str) -> String {
    let units: Vec<u16> = hex
        .as_bytes()
        .chunks(4)
        .filter_map(|c| u16::from_str_radix(core::str::from_utf8(c).ok()?, 16).ok())
        .collect();
    String::from_utf16_lossy(&units)
}

#[derive(Debug, PartialEq)]
enum Tok<'a> {
    Hex(&'a str),
    Open,
    Close,
    Word(&'a str),
}

fn cmap_tokens(src: &str) -> Vec<Tok<'_>> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'<' if b.get(i + 1) != Some(&b'<') => {
                let s = i + 1;
                while i < b.len() && b[i] != b'>' {
                    i += 1;
                }
                out.push(Tok::Hex(&src[s..i.min(b.len())]));
                i += 1;
            }
            b'[' => {
                out.push(Tok::Open);
                i += 1;
            }
            b']' => {
                out.push(Tok::Close);
                i += 1;
            }
            c if c.is_ascii_whitespace() => i += 1,
            b'%' => {
                while i < b.len() && b[i] != b'\n' && b[i] != b'\r' {
                    i += 1;
                }
            }
            _ => {
                let s = i;
                while i < b.len() && !b[i].is_ascii_whitespace() && !matches!(b[i], b'<' | b'[' | b']') {
                    i += 1;
                }
                out.push(Tok::Word(&src[s..i]));
            }
        }
    }
    out
}

fn hex_u32(h: &str) -> u32 {
    u32::from_str_radix(h.trim(), 16).unwrap_or(0)
}

fn parse_to_unicode(src: &[u8]) -> (BTreeMap<u32, String>, Vec<usize>) {
    let text = String::from_utf8_lossy(src);
    let toks = cmap_tokens(&text);
    let mut map = BTreeMap::new();
    let mut lens = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        match toks[i] {
            Tok::Word("begincodespacerange") => {
                i += 1;
                while let (Some(Tok::Hex(lo)), Some(Tok::Hex(_))) = (toks.get(i), toks.get(i + 1)) {
                    let l = lo.len() / 2;
                    if !lens.contains(&l) {
                        lens.push(l);
                    }
                    i += 2;
                }
            }
            Tok::Word("beginbfchar") => {
                i += 1;
                while let (Some(Tok::Hex(src)), Some(Tok::Hex(dst))) = (toks.get(i), toks.get(i + 1)) {
                    map.insert(hex_u32(src), utf16_be_hex(dst));
                    i += 2;
                }
            }
            Tok::Word("beginbfrange") => {
                i += 1;
                while let (Some(Tok::Hex(lo)), Some(Tok::Hex(hi))) = (toks.get(i), toks.get(i + 1)) {
                    let (lo, hi) = (hex_u32(lo), hex_u32(hi));
                    i += 2;
                    match toks.get(i) {
                        Some(Tok::Hex(dst)) => {
                            let base: Vec<u16> = utf16_be_hex(dst).encode_utf16().collect();
                            for code in lo..=hi.min(lo.saturating_add(0xFFFF)) {
                                let mut u = base.clone();
                                if let Some(last) = u.last_mut() {
                                    *last = last.wrapping_add((code - lo) as u16);
                                }
                                map.insert(code, String::from_utf16_lossy(&u));
                            }
                            i += 1;
                        }
                        Some(Tok::Open) => {
                            i += 1;
                            let mut code = lo;
                            while let Some(Tok::Hex(dst)) = toks.get(i) {
                                map.insert(code, utf16_be_hex(dst));
                                code += 1;
                                i += 1;
                            }
                            i += 1; // Close
                        }
                        _ => break,
                    }
                }
            }
            _ => i += 1,
        }
    }
    (map, lens)
}

fn load_font(doc: &Document, name: &[u8], dict: &Dictionary, page: u32) -> Result<Font, ExtractError> {
    let name = String::from_utf8_lossy(name).into_owned();
    let malformed = |what: &str| ExtractError::Malformed { page, what: format!("font {name}: {what}") };
    let subtype = dict.get(b"Subtype").ok().and_then(|o| resolve(doc, o).as_name().ok().map(<[u8]>::to_vec));
    let two_byte = subtype.as_deref() == Some(b"Type0");
    let (map, codespace_lens) = match dict.get(b"ToUnicode") {
        Ok(o) => match resolve(doc, o) {
            Object::Stream(s) => {
                let data = s.decompressed_content().map_err(|e| malformed(&format!("ToUnicode: {e}")))?;
                parse_to_unicode(&data)
            }
            _ => return Err(malformed("ToUnicode is not a stream")),
        },
        Err(_) => (BTreeMap::new(), Vec::new()),
    };
    let mut widths = BTreeMap::new();
    let mut default_width = DEFAULT_GLYPH_WIDTH_EM * 1000.0;
    if two_byte {
        let desc = dict
            .get(b"DescendantFonts")
            .ok()
            .and_then(|o| resolve(doc, o).as_array().ok())
            .and_then(|a| a.first())
            .and_then(|o| resolve(doc, o).as_dict().ok());
        if let Some(d) = desc {
            if let Some(dw) = d.get(b"DW").ok().and_then(|o| obj_num(resolve(doc, o))) {
                default_width = dw;
            }
            if let Some(w) = d.get(b"W").ok().and_then(|o| resolve(doc, o).as_array().ok()) {
                let mut i = 0;
                while i < w.len() {
                    let first = obj_num(resolve(doc, &w[i])).unwrap_or(0.0) as u32;
                    match w.get(i + 1).map(|o| resolve(doc, o)) {
                        Some(Object::Array(list)) => {
                            for (k, v) in list.iter().enumerate() {
                                if let Some(v) = obj_num(resolve(doc, v)) {
                                    widths.insert(first + k as u32, v);
                                }
                            }
                            i += 2;
                        }
                        Some(last) => {
                            let last = obj_num(last).unwrap_or(0.0) as u32;
                            let v = w.get(i + 2).and_then(|o| obj_num(resolve(doc, o))).unwrap_or(default_width);
                            for c in first..=last.min(first.saturating_add(0xFFFF)) {
                                widths.insert(c, v);
                            }
                            i += 3;
                        }
                        None => break,
                    }
                }
            }
        }
    } else {
        let first = dict.get(b"FirstChar").ok().and_then(|o| obj_num(resolve(doc, o))).unwrap_or(0.0) as u32;
        if let Some(w) = dict.get(b"Widths").ok().and_then(|o| resolve(doc, o).as_array().ok()) {
            for (k, v) in w.iter().enumerate() {
                if let Some(v) = obj_num(resolve(doc, v)) {
                    widths.insert(first + k as u32, v);
                }
            }
        }
    }
    Ok(Font { name, two_byte, map, codespace_lens, widths, default_width })
}

impl Font {
    /// Splits a string operand into character codes by the font's code length.
    fn codes(&self, s: &[u8]) -> Vec<u32> {
        let len = if self.codespace_lens.len() == 1 {
            self.codespace_lens[0]
        } else if self.two_byte {
            2
        } else {
            1
        }
        .max(1);
        s.chunks(len).map(|c| c.iter().fold(0u32, |a, b| (a << 8) | u32::from(*b))).collect()
    }
}

// ----------------------------------------------------------- interpreter ----

struct State<'d> {
    doc: &'d Document,
    page: u32,
    runs: Vec<GlyphRun>,
}

#[derive(Clone)]
struct Gfx {
    ctm: M,
    tm: M,
    tlm: M,
    font: Option<usize>,
    size: f64,
    tc: f64,
    tw: f64,
    th: f64,
    tl: f64,
    trise: f64,
}

/// A resource dictionary's fonts and the index of each by resource name.
type FontTable = (Vec<Font>, BTreeMap<Vec<u8>, usize>);

fn fonts_of(doc: &Document, res: Option<&Dictionary>, page: u32) -> Result<FontTable, ExtractError> {
    let mut fonts = Vec::new();
    let mut index = BTreeMap::new();
    if let Some(res) = res {
        if let Some(fd) = res.get(b"Font").ok().and_then(|o| resolve(doc, o).as_dict().ok()) {
            for (k, v) in fd.iter() {
                if let Ok(d) = resolve(doc, v).as_dict() {
                    index.insert(k.clone(), fonts.len());
                    fonts.push(load_font(doc, k, d, page)?);
                }
            }
        }
    }
    Ok((fonts, index))
}

fn interpret(
    st: &mut State<'_>,
    content: &[u8],
    res: Option<&Dictionary>,
    base_ctm: M,
    depth: u32,
) -> Result<(), ExtractError> {
    let page = st.page;
    let ops = lopdf::content::Content::decode(content)
        .map_err(|e| ExtractError::Malformed { page, what: format!("content stream: {e}") })?
        .operations;
    let (fonts, findex) = fonts_of(st.doc, res, page)?;
    let mut g = Gfx { ctm: base_ctm, tm: IDENT, tlm: IDENT, font: None, size: 0.0, tc: 0.0, tw: 0.0, th: 1.0, tl: 0.0, trise: 0.0 };
    let mut stack: Vec<Gfx> = Vec::new();
    let num = |ops: &[Object], i: usize| ops.get(i).and_then(obj_num).unwrap_or(0.0);

    for op in &ops {
        let a = &op.operands;
        match op.operator.as_str() {
            "q" => stack.push(g.clone()),
            "Q" => {
                if let Some(s) = stack.pop() {
                    g = s;
                }
            }
            "cm" => g.ctm = mul(&[num(a, 0), num(a, 1), num(a, 2), num(a, 3), num(a, 4), num(a, 5)], &g.ctm),
            "BT" => {
                g.tm = IDENT;
                g.tlm = IDENT;
            }
            "Tf" => {
                let name = a.first().and_then(|o| o.as_name().ok()).unwrap_or(b"");
                g.font = Some(*findex.get(name).ok_or_else(|| ExtractError::MissingFont {
                    page,
                    font: String::from_utf8_lossy(name).into_owned(),
                })?);
                g.size = num(a, 1);
            }
            "Tc" => g.tc = num(a, 0),
            "Tw" => g.tw = num(a, 0),
            "Tz" => g.th = num(a, 0) / 100.0,
            "TL" => g.tl = num(a, 0),
            "Ts" => g.trise = num(a, 0),
            "Td" | "TD" => {
                if op.operator == "TD" {
                    g.tl = -num(a, 1);
                }
                g.tlm = mul(&[1.0, 0.0, 0.0, 1.0, num(a, 0), num(a, 1)], &g.tlm);
                g.tm = g.tlm;
            }
            "Tm" => {
                g.tlm = [num(a, 0), num(a, 1), num(a, 2), num(a, 3), num(a, 4), num(a, 5)];
                g.tm = g.tlm;
            }
            "T*" => {
                g.tlm = mul(&[1.0, 0.0, 0.0, 1.0, 0.0, -g.tl], &g.tlm);
                g.tm = g.tlm;
            }
            "Tj" | "'" | "\"" | "TJ" => {
                if op.operator == "'" || op.operator == "\"" {
                    if op.operator == "\"" {
                        g.tw = num(a, 0);
                        g.tc = num(a, 1);
                    }
                    g.tlm = mul(&[1.0, 0.0, 0.0, 1.0, 0.0, -g.tl], &g.tlm);
                    g.tm = g.tlm;
                }
                let Some(fi) = g.font else {
                    return Err(ExtractError::Malformed { page, what: "text drawn before any Tf".into() });
                };
                let font = &fonts[fi];
                let items: Vec<&Object> = match op.operator.as_str() {
                    "TJ" => a.first().and_then(|o| o.as_array().ok()).map(|v| v.iter().collect()).unwrap_or_default(),
                    "Tj" | "'" => a.first().into_iter().collect(),
                    _ => a.get(2).into_iter().collect(),
                };
                show(st, font, &mut g, &items)?;
            }
            "Do" => {
                if depth >= MAX_FORM_DEPTH {
                    continue;
                }
                let name = a.first().and_then(|o| o.as_name().ok()).unwrap_or(b"");
                let xo = res
                    .and_then(|r| r.get(b"XObject").ok())
                    .and_then(|o| resolve(st.doc, o).as_dict().ok())
                    .and_then(|d| d.get(name).ok())
                    .map(|o| resolve(st.doc, o));
                if let Some(Object::Stream(s)) = xo {
                    let is_form = s.dict.get(b"Subtype").ok().and_then(|o| o.as_name().ok()) == Some(b"Form");
                    if is_form {
                        let data = s
                            .decompressed_content()
                            .map_err(|e| ExtractError::Malformed { page, what: format!("form XObject: {e}") })?;
                        let m = s
                            .dict
                            .get(b"Matrix")
                            .ok()
                            .and_then(|o| resolve(st.doc, o).as_array().ok())
                            .map(|v| {
                                let n = |i: usize| v.get(i).and_then(obj_num).unwrap_or(0.0);
                                [n(0), n(1), n(2), n(3), n(4), n(5)]
                            })
                            .unwrap_or(IDENT);
                        let own = s.dict.get(b"Resources").ok().and_then(|o| resolve(st.doc, o).as_dict().ok());
                        interpret(st, &data, own.or(res), mul(&m, &g.ctm), depth + 1)?;
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Draws the string operands (and `TJ` adjustments) of one text-showing operator.
fn show(st: &mut State<'_>, font: &Font, g: &mut Gfx, items: &[&Object]) -> Result<(), ExtractError> {
    struct Open {
        text: String,
        x: f64,
        y: f64,
        size: f64,
    }
    let page = st.page;
    let mut cur: Option<Open> = None;
    let mut last_end = (0.0, 0.0);
    let flush = |st: &mut State<'_>, cur: &mut Option<Open>, end_x: f64| {
        if let Some(o) = cur.take() {
            st.runs.push(GlyphRun { text: o.text.nfkc().collect(), x: o.x, y: o.y, size: o.size, x_end: end_x });
        }
    };
    for item in items {
        match item {
            Object::String(bytes, _) => {
                for code in font.codes(bytes) {
                    let text = font.map.get(&code).ok_or_else(|| ExtractError::Unmapped {
                        page,
                        font: font.name.clone(),
                        code,
                    })?;
                    let trm = mul(&[g.size * g.th, 0.0, 0.0, g.size, 0.0, g.trise], &mul(&g.tm, &g.ctm));
                    let (x, y) = (trm[4], trm[5]);
                    let size = (trm[2] * trm[2] + trm[3] * trm[3]).sqrt();
                    let w = font.widths.get(&code).copied().unwrap_or(font.default_width) / 1000.0;
                    let is_space = !font.two_byte && code == 32;
                    let tx = (w * g.size + g.tc + if is_space { g.tw } else { 0.0 }) * g.th;
                    g.tm = mul(&[1.0, 0.0, 0.0, 1.0, tx, 0.0], &g.tm);
                    let end_dev = mul(&g.tm, &g.ctm);
                    last_end = (end_dev[4], end_dev[5]);
                    match &mut cur {
                        Some(o) => o.text.push_str(text),
                        None => cur = Some(Open { text: text.clone(), x, y, size }),
                    }
                }
            }
            other => {
                // A TJ adjustment, in thousandths of the text-space unit, subtracted.
                if let Some(n) = obj_num(other) {
                    let shift = -n / 1000.0 * g.size * g.th;
                    let gap_over = shift > WORD_GAP_SIZE * g.size;
                    if gap_over {
                        flush(st, &mut cur, last_end.0);
                    }
                    g.tm = mul(&[1.0, 0.0, 0.0, 1.0, shift, 0.0], &g.tm);
                    let end_dev = mul(&g.tm, &g.ctm);
                    last_end = (end_dev[4], end_dev[5]);
                }
            }
        }
    }
    flush(st, &mut cur, last_end.0);
    Ok(())
}

/// Reads a PDF's pages for check 4.
pub fn extract_pages(bytes: &[u8]) -> Result<Vec<Page>, ExtractError> {
    let doc = Document::load_mem(bytes).map_err(|e| ExtractError::Parse(e.to_string()))?;
    let mut pages = Vec::new();
    // `get_pages` is keyed by 1-based page-tree position.
    for (number, id) in doc.get_pages() {
        pages.push(extract_page(&doc, number, id)?);
    }
    Ok(pages)
}

fn extract_page(doc: &Document, number: u32, id: ObjectId) -> Result<Page, ExtractError> {
    let content = doc.get_page_content(id);
    let (res_dict, res_ids) = doc
        .get_page_resources(id)
        .map_err(|e| ExtractError::Malformed { page: number, what: format!("resources: {e}") })?;
    let res: Option<&Dictionary> = res_dict.or_else(|| res_ids.first().and_then(|r| doc.get_dictionary(*r).ok()));
    let mut st = State { doc, page: number, runs: Vec::new() };
    interpret(&mut st, &content, res, IDENT, 0)?;
    Ok(Page { number, runs: st.runs })
}

/// A page's words, in content-stream order, with a line index each. A new line
/// starts where `y` changes by over [`LINE_BREAK_Y_SIZE`] of the size from the
/// previous run's; a word ends at whitespace or at a gap over [`WORD_GAP_SIZE`]
/// of the size between one run's end and the next one's start.
pub fn words(page: &Page) -> Vec<Word> {
    let mut out: Vec<Word> = Vec::new();
    let mut line = 0usize;
    let mut prev: Option<&GlyphRun> = None;
    let mut open = false; // the last word is still being extended
    for run in &page.runs {
        let mut joins = false;
        if let Some(p) = prev {
            if (run.y - p.y).abs() > LINE_BREAK_Y_SIZE * p.size.max(run.size) {
                line += 1;
            } else {
                joins = run.x - p.x_end <= WORD_GAP_SIZE * run.size;
            }
        }
        let mut first_piece = true;
        for piece in split_keeping(&run.text) {
            match piece {
                None => open = false,
                Some(text) => {
                    if first_piece && joins && open {
                        if let Some(w) = out.last_mut() {
                            w.text.push_str(&text);
                            w.x_end = run.x_end;
                        }
                    } else {
                        out.push(Word { text, x: run.x, y: run.y, size: run.size, x_end: run.x_end, line });
                    }
                    open = true;
                }
            }
            first_piece = false;
        }
        prev = Some(run);
    }
    out
}

/// Splits at whitespace: `Some(word)` for each non-empty piece, `None` for each
/// whitespace stretch (a break). A leading whitespace gives a leading `None`.
fn split_keeping(s: &str) -> Vec<Option<String>> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in s.chars() {
        if c.is_whitespace() {
            if !cur.is_empty() {
                out.push(Some(core::mem::take(&mut cur)));
            }
            if out.last() != Some(&None) {
                out.push(None);
            }
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        out.push(Some(cur));
    }
    out
}
