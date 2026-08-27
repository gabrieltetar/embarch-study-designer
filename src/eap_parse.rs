//! The `.eap` text grammar: lexer, parser, and the lowering that splits one
//! manifest into the half dev-bench executes and the half the host renders —
//! design.md §3 decisions 58/59, §4.9.
//!
//! `std`-only authoring-time tooling, the same posture as
//! [`crate::gatt_extract`] (§3 decision 33): an `.eap` file is read off the
//! firmware repo's disk and resolved into a `Study` at build time. Nothing
//! here runs on the DUT or on the dev-bench MCU — dev-bench carries the
//! resolved [`crate::eap::ProtocolDef`] and no parser at all.
//!
//! # The grammar, in full
//!
//! ```text
//! file          = { protocol } ;
//! protocol      = "protocol" ident "{" { source | frame | struct | session | state } "}" ;
//!
//! source        = "source" ident "=" "characteristic"
//!                 "(" "service" ":" string "," "char" ":" string ")" ;
//!
//! frame         = "frame" ident "on" ident [ select_if ] "{" { field } "}" ;
//! select_if     = "select_if" "{" "offset" ":" int "," "len" ":" int "," "eq" ":" match "}" ;
//! match         = "magic" "(" string ")" | int | "[" int { "," int } "]" ;
//!
//! struct        = "struct" ident "{" { field } "}" ;
//!
//! field         = scalar | span | repeat | bitpack | crc ;
//! scalar        = scalar_ty ident [ "@" int ] [ "signed" ] [ "fixed" "(" number "," ident ")" ] ;
//! span          = "bytes" ident [ "@" int ] [ ".." ( int | "len" ) ] ;
//! repeat        = "repeat" ident "[" ( int | "count_from" ":" ident ) "]" ":" ident ;
//! bitpack       = "bitpack" ident "[" "count_from" ":" ident "]"
//!                 "width_from" ":" ident [ "delta" ] [ "zigzag" ] [ "seed" ":" path ] ;
//! crc           = ( "crc32" | "crc16" ) "ieee" "policy" ":" ( "skip" | "error" | "retry" ) ;
//!
//! session       = "session" "{" { "var" ident ":" int_ty "=" int } "}" ;
//!
//! state         = "state" ident ( "outcome" ":" ( "pass" | "fail" ) | "{" { clause } "}" ) ;
//! clause        = on_enter | on_event | on_timeout ;
//! on_enter      = "on_enter" ":" write ;
//! write         = "write" ident "{" [ wfield { "," wfield } ] "}" [ "with_response" ] ;
//! wfield        = scalar_ty ":" operand ;
//! on_event      = "on_event" ident ":" { remember } { when } [ otherwise ] ;
//! remember      = "remember" ident "=" expr ;
//! when          = "when" cond ":" "goto" ident ;
//! otherwise     = "otherwise" ":" "goto" ident ;
//! on_timeout    = "on_timeout" int "ms" [ "retry" int ] ":" "goto" ident ;
//!
//! expr          = operand [ "+" operand ] ;
//! cond          = operand cmp operand ;
//! cmp           = "==" | "!=" | "<" | "<=" | ">" | ">=" ;
//! operand       = int
//!               | "session" "." ident
//!               | ident "." ident
//!               | "len" "(" ident "." ident ")" ;
//!
//! scalar_ty     = "u8" | "i8" | "u16le" | "u16be" | "i16le" | "i16be"
//!               | "u32le" | "u32be" | "i32le" | "i32be"
//!               | "u64le" | "u64be" | "i64le" | "i64be"
//!               | "f32le" | "f32be" | "f64le" | "f64be" ;
//! int_ty        = "u32" | "u64" | "i32" | "i64" ;
//! ident         = ( letter | "_" ) { letter | digit | "_" } ;
//! int           = [ "-" ] ( digit { digit } | "0x" hexdigit { hexdigit } ) ;
//! string        = '"' { any - '"' } '"' ;
//! comment       = "#" { any - newline } ;
//! ```
//!
//! Whitespace and `#` comments separate tokens and are otherwise
//! insignificant; newlines are not. `,` is accepted and ignored between the
//! fields of a `write` payload, so both the one-per-line and the
//! comma-separated styles in §4.9's worked examples parse.
//!
//! # Two things the grammar deliberately does not have
//!
//! **No author-declared CRC seed.** The design draft wrote
//! `crc32 ieee seed: 0xFFFFFFFF policy: skip`. The seed is gone as a
//! parameter, because CRC-32/ISO-HDLC — init `0xFFFFFFFF`, reflected in and
//! out, final XOR `0xFFFFFFFF` — already *is* what that spelling names, and
//! is bit for bit Zephyr's own `crc32_ieee`. Keeping it configurable would
//! mean constructing a custom algorithm per frame, i.e. a second CRC
//! implementation living beside [`crate::crc`]'s, which is the one thing the
//! design explicitly asked not to happen. `policy` stays, because that
//! genuinely varies per frame.
//!
//! **No `crc16`.** It is in the grammar above and is **rejected by the
//! parser** with a named error. Zephyr ships several mutually incompatible
//! CRC-16s (ANSI, CCITT, ITU, each with its own seed and reflection), the
//! design named none of them, and neither worked protocol uses one. Guessing
//! which would be exactly the inference this suite refuses everywhere else;
//! shipping all four would be four primitives with no caller, the shape
//! `embarch-core` §3 decision 30's settlement 2 already records as a
//! mistake. It is one line to add the day a real frame needs a named
//! variant.

use std::collections::HashMap;
use std::fmt;

use heapless::{String as HString, Vec as HVec};

use crate::decoder::{ScalarType, StructField, StructLayout};
use crate::eap::{
    ActiveState, CompareOp, Condition, EventArm, Expr, FrameDef, FrameMatch, GuardedGoto, Operand,
    ProtocolDef, ProtocolSource, Remember, ScalarRead, SessionVarDef, SpanRead, StateDef,
    StateKind, TerminalOutcome, TimeoutArm, WriteAction, WriteField,
};
use crate::ids::Uuid;

// --- Errors -------------------------------------------------------------

/// Every way an `.eap` file can fail to become a [`ProtocolDef`].
///
/// Each carries the source line, because the whole point of a purpose-built
/// text grammar over TOML is legibility, and an error that cannot say where
/// it happened gives that back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EapError {
    pub line: u32,
    pub kind: EapErrorKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EapErrorKind {
    /// A character that begins no token.
    BadChar(char),
    /// A `"` that never closes.
    UnterminatedString,
    /// An integer literal that does not fit an `i64`, or a malformed `0x`.
    BadInteger(String),
    /// Expected one thing, found another.
    Expected { want: String, got: String },
    /// A name used where nothing declares it.
    Unknown { what: &'static str, name: String },
    /// The same name declared twice in one scope.
    Duplicate { what: &'static str, name: String },
    /// A UUID string that is neither 16-bit/32-bit shorthand nor a full
    /// 128-bit form.
    BadUuid(String),
    /// A capacity in [`crate::limits`] was exceeded — a real, disclosed
    /// limit, never a silent truncation.
    TooMany { what: &'static str, limit: usize },
    /// A name longer than its bounded `heapless::String`.
    NameTooLong { what: &'static str, limit: usize },
    /// A float width used where the expression set needs an integer.
    NonIntegerField(String),
    /// `crc16` — see the module docs.
    Crc16Unsupported,
    /// A `select_if` whose declared `len` disagrees with the literal it
    /// carries. Refused rather than trusting one over the other.
    MatchLenMismatch { declared: usize, actual: usize },
    /// A structural rule the resolved protocol breaks.
    Invalid(crate::eap::ProtocolError),
}

impl fmt::Display for EapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: ", self.line)?;
        match &self.kind {
            EapErrorKind::BadChar(c) => write!(f, "unexpected character {c:?}"),
            EapErrorKind::UnterminatedString => write!(f, "unterminated string"),
            EapErrorKind::BadInteger(s) => write!(f, "bad integer literal {s:?}"),
            EapErrorKind::Expected { want, got } => write!(f, "expected {want}, found {got}"),
            EapErrorKind::Unknown { what, name } => write!(f, "unknown {what} {name:?}"),
            EapErrorKind::Duplicate { what, name } => write!(f, "duplicate {what} {name:?}"),
            EapErrorKind::BadUuid(s) => write!(f, "not a UUID: {s:?}"),
            EapErrorKind::TooMany { what, limit } => {
                write!(f, "too many {what} (limit {limit})")
            }
            EapErrorKind::NameTooLong { what, limit } => {
                write!(f, "{what} name longer than {limit} bytes")
            }
            EapErrorKind::NonIntegerField(n) => {
                write!(f, "field {n:?} is a float width and no guard can compare it")
            }
            EapErrorKind::Crc16Unsupported => write!(
                f,
                "crc16 names no specific algorithm and none is implemented; use crc32"
            ),
            EapErrorKind::MatchLenMismatch { declared, actual } => write!(
                f,
                "select_if declares len {declared} but its literal is {actual} bytes"
            ),
            EapErrorKind::Invalid(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for EapError {}

type R<T> = Result<T, EapError>;

// --- Lexer --------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Int(i64),
    /// Only ever a `fixed(scale, …)` argument. Lexed as one token rather
    /// than as `Int . Int`, which would read `0.005` as `0.5` — the leading
    /// zeros of the fraction are significant and a three-token spelling
    /// throws them away.
    Float(f64),
    Str(String),
    /// One of `{ } ( ) [ ] : , . @ = + < > ! - ..` and the two-char
    /// comparisons, kept as text so the parser reads like the grammar.
    Sym(&'static str),
}

impl fmt::Display for Tok {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Tok::Ident(s) => write!(f, "`{s}`"),
            Tok::Int(v) => write!(f, "`{v}`"),
            Tok::Float(v) => write!(f, "`{v}`"),
            Tok::Str(s) => write!(f, "{s:?}"),
            Tok::Sym(s) => write!(f, "`{s}`"),
        }
    }
}

fn lex(src: &str) -> R<Vec<(Tok, u32)>> {
    let mut out = Vec::new();
    let mut line = 1u32;
    let b: Vec<char> = src.chars().collect();
    let mut i = 0usize;
    while i < b.len() {
        let c = b[i];
        if c == '\n' {
            line += 1;
            i += 1;
            continue;
        }
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '#' {
            while i < b.len() && b[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '"' {
            let mut s = String::new();
            i += 1;
            loop {
                if i >= b.len() || b[i] == '\n' {
                    return Err(EapError { line, kind: EapErrorKind::UnterminatedString });
                }
                if b[i] == '"' {
                    i += 1;
                    break;
                }
                s.push(b[i]);
                i += 1;
            }
            out.push((Tok::Str(s), line));
            continue;
        }
        if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == '_') {
                i += 1;
            }
            out.push((Tok::Ident(b[start..i].iter().collect()), line));
            continue;
        }
        if c.is_ascii_digit() {
            let start = i;
            if c == '0' && i + 1 < b.len() && (b[i + 1] == 'x' || b[i + 1] == 'X') {
                i += 2;
                while i < b.len() && (b[i].is_ascii_hexdigit() || b[i] == '_') {
                    i += 1;
                }
                let text: String = b[start..i].iter().filter(|c| **c != '_').collect();
                let v = i64::from_str_radix(&text[2..], 16)
                    .map_err(|_| EapError { line, kind: EapErrorKind::BadInteger(text.clone()) })?;
                out.push((Tok::Int(v), line));
                continue;
            }
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == '_') {
                i += 1;
            }
            // A single `.` followed by a digit continues the number; `..`
            // does not, so `payload @ 0..len` still lexes as a range.
            let is_float = i + 1 < b.len() && b[i] == '.' && b[i + 1].is_ascii_digit();
            if is_float {
                i += 1;
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
                let text: String = b[start..i].iter().filter(|c| **c != '_').collect();
                let v = text.parse::<f64>().map_err(|_| EapError {
                    line,
                    kind: EapErrorKind::BadInteger(text.clone()),
                })?;
                out.push((Tok::Float(v), line));
                continue;
            }
            let text: String = b[start..i].iter().filter(|c| **c != '_').collect();
            let v = text
                .parse::<i64>()
                .map_err(|_| EapError { line, kind: EapErrorKind::BadInteger(text.clone()) })?;
            out.push((Tok::Int(v), line));
            continue;
        }
        // Two-character symbols first, so `>=` never lexes as `>` then `=`.
        let two: String = b[i..(i + 2).min(b.len())].iter().collect();
        let sym2 = match two.as_str() {
            "==" => Some("=="),
            "!=" => Some("!="),
            "<=" => Some("<="),
            ">=" => Some(">="),
            ".." => Some(".."),
            _ => None,
        };
        if let Some(s) = sym2 {
            out.push((Tok::Sym(s), line));
            i += 2;
            continue;
        }
        let sym1 = match c {
            '{' => "{",
            '}' => "}",
            '(' => "(",
            ')' => ")",
            '[' => "[",
            ']' => "]",
            ':' => ":",
            ',' => ",",
            '.' => ".",
            '@' => "@",
            '=' => "=",
            '+' => "+",
            '<' => "<",
            '>' => ">",
            '-' => "-",
            _ => return Err(EapError { line, kind: EapErrorKind::BadChar(c) }),
        };
        out.push((Tok::Sym(sym1), line));
        i += 1;
    }
    Ok(out)
}

// --- AST ----------------------------------------------------------------
//
// The *whole* grammar, both halves. Lowering (below) is where it splits into
// the part dev-bench executes and the part the host renders.

/// A parsed `.eap` file: one or more protocol blocks.
#[derive(Debug, Clone, PartialEq)]
pub struct EapFile {
    pub protocols: Vec<AstProtocol>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AstProtocol {
    pub name: String,
    pub sources: Vec<(String, Uuid, Uuid)>,
    pub frames: Vec<AstFrame>,
    pub structs: Vec<AstStruct>,
    pub session: Vec<(String, i64)>,
    pub states: Vec<AstState>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AstFrame {
    pub name: String,
    pub source: String,
    pub select_if: Option<(u16, Vec<u8>)>,
    pub fields: Vec<AstField>,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AstStruct {
    pub name: String,
    pub fields: Vec<AstField>,
}

/// One field of a frame or a struct — every primitive in the grammar,
/// including the four that never reach dev-bench.
#[derive(Debug, Clone, PartialEq)]
pub enum AstField {
    Scalar {
        name: String,
        ty: ScalarType,
        at: Option<u16>,
        /// `fixed(scale, unit)` — a **render-only** modifier (§3 decision
        /// 59). Parsed and carried so a rendering can apply it; never
        /// consulted by a guard, whose operands are integers.
        fixed: Option<(f64, String)>,
        line: u32,
    },
    Span {
        name: String,
        at: Option<u16>,
        /// `None` = rest of the payload.
        len: Option<u16>,
        line: u32,
    },
    Repeat {
        name: String,
        count: AstCount,
        elem: String,
        line: u32,
    },
    Bitpack {
        name: String,
        count_from: String,
        width_from: String,
        delta: bool,
        zigzag: bool,
        seed: Option<String>,
        line: u32,
    },
    Crc32 {
        policy: CrcPolicy,
        line: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AstCount {
    /// `repeat channel[30]` — a literal count. The worked GWF1 record uses
    /// one, because its channel table is always the firmware's full
    /// compile-time capacity: the flash erase happens before the geometry is
    /// known.
    Literal(u16),
    /// `repeat chunk[count_from: n_chunks]`.
    From(String),
}

/// What a frame does when its trailing checksum does not match.
///
/// Per-frame and author-declared, never a property of the interpreter —
/// `skip` is what this suite's reference DUT actually does (a bad record is
/// dropped and the blob walk continues), but that is its firmware's choice
/// to state, not a default to bake in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrcPolicy {
    Skip,
    Error,
    Retry,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AstState {
    pub name: String,
    pub terminal: Option<TerminalOutcome>,
    pub on_enter: Option<AstWrite>,
    pub on_event: Vec<AstEventArm>,
    pub on_timeout: Option<(u32, u8, String)>,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AstWrite {
    pub source: String,
    pub fields: Vec<(ScalarType, AstOperand)>,
    pub with_response: bool,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AstEventArm {
    pub frame: String,
    pub remember: Vec<(String, AstExpr)>,
    pub when: Vec<(AstCond, String)>,
    pub otherwise: Option<String>,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AstOperand {
    Literal(i64),
    Session(String),
    Field { frame: String, field: String },
    SpanLen { frame: String, field: String },
}

#[derive(Debug, Clone, PartialEq)]
pub enum AstExpr {
    Term(AstOperand),
    Add(AstOperand, AstOperand),
}

#[derive(Debug, Clone, PartialEq)]
pub struct AstCond {
    pub lhs: AstOperand,
    pub op: CompareOp,
    pub rhs: AstOperand,
}

// --- Parser -------------------------------------------------------------

struct P {
    t: Vec<(Tok, u32)>,
    i: usize,
}

impl P {
    fn line(&self) -> u32 {
        self.t.get(self.i).map(|(_, l)| *l).unwrap_or_else(|| {
            self.t.last().map(|(_, l)| *l).unwrap_or(1)
        })
    }
    fn err<T>(&self, want: &str) -> R<T> {
        let got = match self.t.get(self.i) {
            Some((tok, _)) => tok.to_string(),
            None => "end of file".to_string(),
        };
        Err(EapError {
            line: self.line(),
            kind: EapErrorKind::Expected { want: want.to_string(), got },
        })
    }
    fn peek(&self) -> Option<&Tok> {
        self.t.get(self.i).map(|(t, _)| t)
    }
    fn eat_sym(&mut self, s: &str) -> bool {
        if matches!(self.peek(), Some(Tok::Sym(x)) if *x == s) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn want_sym(&mut self, s: &str) -> R<()> {
        if self.eat_sym(s) {
            Ok(())
        } else {
            self.err(&format!("`{s}`"))
        }
    }
    fn eat_kw(&mut self, k: &str) -> bool {
        if matches!(self.peek(), Some(Tok::Ident(x)) if x == k) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn want_kw(&mut self, k: &str) -> R<()> {
        if self.eat_kw(k) {
            Ok(())
        } else {
            self.err(&format!("`{k}`"))
        }
    }
    fn is_kw(&self, k: &str) -> bool {
        matches!(self.peek(), Some(Tok::Ident(x)) if x == k)
    }
    fn ident(&mut self) -> R<String> {
        match self.peek().cloned() {
            Some(Tok::Ident(s)) => {
                self.i += 1;
                Ok(s)
            }
            _ => self.err("an identifier"),
        }
    }
    fn int(&mut self) -> R<i64> {
        let neg = self.eat_sym("-");
        match self.peek().cloned() {
            Some(Tok::Int(v)) => {
                self.i += 1;
                Ok(if neg { -v } else { v })
            }
            _ => self.err("an integer"),
        }
    }
    fn u16v(&mut self) -> R<u16> {
        let line = self.line();
        let v = self.int()?;
        u16::try_from(v).map_err(|_| EapError {
            line,
            kind: EapErrorKind::BadInteger(v.to_string()),
        })
    }
    fn string(&mut self) -> R<String> {
        match self.peek().cloned() {
            Some(Tok::Str(s)) => {
                self.i += 1;
                Ok(s)
            }
            _ => self.err("a quoted string"),
        }
    }
}

/// Parse an `.eap` file into its AST. See [`resolve`] to lower one protocol
/// into the [`ProtocolDef`] dev-bench executes.
pub fn parse(src: &str) -> R<EapFile> {
    let mut p = P { t: lex(src)?, i: 0 };
    let mut protocols = Vec::new();
    while p.peek().is_some() {
        p.want_kw("protocol")?;
        protocols.push(parse_protocol(&mut p)?);
    }
    Ok(EapFile { protocols })
}

fn parse_protocol(p: &mut P) -> R<AstProtocol> {
    let name = p.ident()?;
    p.want_sym("{")?;
    let mut out = AstProtocol {
        name,
        sources: Vec::new(),
        frames: Vec::new(),
        structs: Vec::new(),
        session: Vec::new(),
        states: Vec::new(),
    };
    while !p.eat_sym("}") {
        if p.peek().is_none() {
            return p.err("`}`");
        }
        if p.eat_kw("source") {
            let alias = p.ident()?;
            p.want_sym("=")?;
            p.want_kw("characteristic")?;
            p.want_sym("(")?;
            p.want_kw("service")?;
            p.want_sym(":")?;
            let line = p.line();
            let svc = parse_uuid(&p.string()?, line)?;
            p.eat_sym(",");
            p.want_kw("char")?;
            p.want_sym(":")?;
            let line = p.line();
            let chr = parse_uuid(&p.string()?, line)?;
            p.want_sym(")")?;
            out.sources.push((alias, svc, chr));
        } else if p.eat_kw("frame") {
            let mut structs = Vec::new();
            let f = parse_frame(p, &mut structs)?;
            out.structs.append(&mut structs);
            out.frames.push(f);
        } else if p.eat_kw("struct") {
            let sname = p.ident()?;
            p.want_sym("{")?;
            let fields = parse_body(p, &mut out.structs)?;
            out.structs.push(AstStruct { name: sname, fields });
        } else if p.eat_kw("session") {
            p.want_sym("{")?;
            while !p.eat_sym("}") {
                p.want_kw("var")?;
                let vname = p.ident()?;
                p.want_sym(":")?;
                // The declared integer width is accepted and not stored: a
                // session variable is an `i64` in the evaluator, and a
                // narrower declaration would only be a promise the
                // evaluator does not keep.
                let _ = p.ident()?;
                p.want_sym("=")?;
                let init = p.int()?;
                out.session.push((vname, init));
            }
        } else if p.eat_kw("state") {
            out.states.push(parse_state(p)?);
        } else {
            return p.err("`source`, `frame`, `struct`, `session`, `state` or `}`");
        }
    }
    Ok(out)
}

fn parse_frame(p: &mut P, structs: &mut Vec<AstStruct>) -> R<AstFrame> {
    let line = p.line();
    let name = p.ident()?;
    p.want_kw("on")?;
    let source = p.ident()?;
    let mut select_if = None;
    if p.eat_kw("select_if") {
        p.want_sym("{")?;
        p.want_kw("offset")?;
        p.want_sym(":")?;
        let offset = p.u16v()?;
        p.eat_sym(",");
        p.want_kw("len")?;
        p.want_sym(":")?;
        let declared = p.int()? as usize;
        p.eat_sym(",");
        p.want_kw("eq")?;
        p.want_sym(":")?;
        let mline = p.line();
        let bytes = parse_match(p)?;
        if bytes.len() != declared {
            return Err(EapError {
                line: mline,
                kind: EapErrorKind::MatchLenMismatch { declared, actual: bytes.len() },
            });
        }
        p.want_sym("}")?;
        select_if = Some((offset, bytes));
    }
    p.want_sym("{")?;
    let fields = parse_body(p, structs)?;
    Ok(AstFrame { name, source, select_if, fields, line })
}

/// `magic("GWF1")`, a bare integer, or `[0x01, 0x02]`.
///
/// A `magic(…)` string's bytes are its UTF-8 bytes, and `\xNN` escapes are
/// accepted so a format tag with a non-printable byte (`BSS\x03`) can be
/// written the way its firmware's own header writes it.
fn parse_match(p: &mut P) -> R<Vec<u8>> {
    if p.eat_kw("magic") {
        p.want_sym("(")?;
        let line = p.line();
        let s = p.string()?;
        p.want_sym(")")?;
        return unescape(&s, line);
    }
    if p.eat_sym("[") {
        let mut v = Vec::new();
        while !p.eat_sym("]") {
            let line = p.line();
            let b = p.int()?;
            v.push(u8::try_from(b).map_err(|_| EapError {
                line,
                kind: EapErrorKind::BadInteger(b.to_string()),
            })?);
            p.eat_sym(",");
        }
        return Ok(v);
    }
    let line = p.line();
    let b = p.int()?;
    Ok(vec![u8::try_from(b).map_err(|_| EapError {
        line,
        kind: EapErrorKind::BadInteger(b.to_string()),
    })?])
}

fn unescape(s: &str, line: u32) -> R<Vec<u8>> {
    let mut out = Vec::new();
    let cs: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < cs.len() {
        if cs[i] == '\\' && i + 3 < cs.len() && cs[i + 1] == 'x' {
            let hex: String = cs[i + 2..i + 4].iter().collect();
            let b = u8::from_str_radix(&hex, 16)
                .map_err(|_| EapError { line, kind: EapErrorKind::BadInteger(hex.clone()) })?;
            out.push(b);
            i += 4;
        } else {
            let mut buf = [0u8; 4];
            out.extend_from_slice(cs[i].encode_utf8(&mut buf).as_bytes());
            i += 1;
        }
    }
    Ok(out)
}

/// A frame or struct body. `struct` declarations are accepted **inside** a
/// frame block as well as beside it — §4.9's worked GWF1 record writes them
/// that way, next to the `repeat` that uses them, which reads better than
/// making an author scroll — and are hoisted into `structs` either way. A
/// struct's scope is the whole protocol regardless of where it was written.
fn parse_body(p: &mut P, structs: &mut Vec<AstStruct>) -> R<Vec<AstField>> {
    let mut fields = Vec::new();
    while !p.eat_sym("}") {
        if p.peek().is_none() {
            return p.err("`}`");
        }
        let line = p.line();
        if p.eat_kw("struct") {
            let sname = p.ident()?;
            p.want_sym("{")?;
            let sfields = parse_body(p, structs)?;
            structs.push(AstStruct { name: sname, fields: sfields });
            continue;
        }
        if p.eat_kw("bytes") {
            let name = p.ident()?;
            let at = if p.eat_sym("@") { Some(p.u16v()?) } else { None };
            let mut len = None;
            // `..len` is the rest of the payload; `..N` is a fixed span.
            if p.eat_sym("..") && !p.eat_kw("len") {
                len = Some(p.u16v()?);
            }
            fields.push(AstField::Span { name, at, len, line });
            continue;
        }
        if p.eat_kw("repeat") {
            let name = p.ident()?;
            p.want_sym("[")?;
            let count = if p.eat_kw("count_from") {
                p.want_sym(":")?;
                AstCount::From(p.ident()?)
            } else {
                AstCount::Literal(p.u16v()?)
            };
            p.want_sym("]")?;
            p.want_sym(":")?;
            let elem = p.ident()?;
            fields.push(AstField::Repeat { name, count, elem, line });
            continue;
        }
        if p.eat_kw("bitpack") {
            let name = p.ident()?;
            p.want_sym("[")?;
            p.want_kw("count_from")?;
            p.want_sym(":")?;
            let count_from = p.ident()?;
            p.want_sym("]")?;
            p.want_kw("width_from")?;
            p.want_sym(":")?;
            let width_from = p.ident()?;
            let mut delta = false;
            let mut zigzag = false;
            let mut seed = None;
            loop {
                if p.eat_kw("delta") {
                    delta = true;
                } else if p.eat_kw("zigzag") {
                    zigzag = true;
                } else if p.eat_kw("seed") {
                    p.want_sym(":")?;
                    let mut path = p.ident()?;
                    while p.eat_sym(".") {
                        path.push('.');
                        path.push_str(&p.ident()?);
                    }
                    seed = Some(path);
                } else {
                    break;
                }
            }
            fields.push(AstField::Bitpack {
                name,
                count_from,
                width_from,
                delta,
                zigzag,
                seed,
                line,
            });
            continue;
        }
        if p.is_kw("crc16") {
            return Err(EapError { line, kind: EapErrorKind::Crc16Unsupported });
        }
        if p.eat_kw("crc32") {
            // `ieee` is accepted and required as documentation of which
            // CRC-32 this is; there is no other, so it selects nothing.
            p.want_kw("ieee")?;
            p.want_kw("policy")?;
            p.want_sym(":")?;
            let policy = if p.eat_kw("skip") {
                CrcPolicy::Skip
            } else if p.eat_kw("error") {
                CrcPolicy::Error
            } else if p.eat_kw("retry") {
                CrcPolicy::Retry
            } else {
                return p.err("`skip`, `error` or `retry`");
            };
            fields.push(AstField::Crc32 { policy, line });
            continue;
        }
        // Otherwise: a scalar, spelled by its width.
        let word = p.ident()?;
        let Some(ty) = ScalarType::parse(&word) else {
            return Err(EapError {
                line,
                kind: EapErrorKind::Expected {
                    want: "a field kind or scalar width".into(),
                    got: format!("`{word}`"),
                },
            });
        };
        let name = p.ident()?;
        let at = if p.eat_sym("@") { Some(p.u16v()?) } else { None };
        // `signed` is accepted where the draft writes it (`i32be first
        // signed`). It is redundant with the width's own signedness, which
        // is what actually decides the read — kept so a manifest copied from
        // the design doc parses, and deliberately not given a second meaning.
        let _ = p.eat_kw("signed");
        let mut fixed = None;
        if p.eat_kw("fixed") {
            p.want_sym("(")?;
            let scale = parse_number(p)?;
            p.eat_sym(",");
            let unit = p.ident()?;
            p.want_sym(")")?;
            fixed = Some((scale, unit));
        }
        fields.push(AstField::Scalar { name, ty, at, fixed, line });
    }
    Ok(fields)
}

/// `0.005` — a decimal scale for `fixed(…)`. The only place the grammar
/// admits a non-integer, and it is render-only: no guard can reach it,
/// because [`Operand::Literal`] is an `i64` and nothing lowers a `fixed`
/// modifier into a [`ProtocolDef`].
fn parse_number(p: &mut P) -> R<f64> {
    let neg = p.eat_sym("-");
    let v = match p.peek().cloned() {
        Some(Tok::Float(v)) => {
            p.i += 1;
            v
        }
        Some(Tok::Int(v)) => {
            p.i += 1;
            v as f64
        }
        _ => return p.err("a number"),
    };
    Ok(if neg { -v } else { v })
}

fn parse_state(p: &mut P) -> R<AstState> {
    let line = p.line();
    let name = p.ident()?;
    let mut st = AstState {
        name,
        terminal: None,
        on_enter: None,
        on_event: Vec::new(),
        on_timeout: None,
        line,
    };
    // `state done outcome: pass` — a terminal state has no block at all.
    if p.eat_kw("outcome") {
        p.want_sym(":")?;
        st.terminal = Some(if p.eat_kw("pass") {
            TerminalOutcome::Pass
        } else if p.eat_kw("fail") {
            TerminalOutcome::Fail
        } else {
            return p.err("`pass` or `fail`");
        });
        return Ok(st);
    }
    p.want_sym("{")?;
    while !p.eat_sym("}") {
        if p.peek().is_none() {
            return p.err("`}`");
        }
        if p.eat_kw("on_enter") {
            p.want_sym(":")?;
            st.on_enter = Some(parse_write(p)?);
        } else if p.eat_kw("on_event") {
            st.on_event.push(parse_event_arm(p)?);
        } else if p.eat_kw("on_timeout") {
            let after = p.int()? as u32;
            // `1500ms` lexes as `1500` then `ms`; the suffix is documentation.
            let _ = p.eat_kw("ms");
            let retry = if p.eat_kw("retry") { p.int()? as u8 } else { 0 };
            p.want_sym(":")?;
            p.want_kw("goto")?;
            let target = p.ident()?;
            st.on_timeout = Some((after, retry, target));
        } else {
            return p.err("`on_enter`, `on_event`, `on_timeout` or `}`");
        }
    }
    Ok(st)
}

fn parse_write(p: &mut P) -> R<AstWrite> {
    let line = p.line();
    p.want_kw("write")?;
    let source = p.ident()?;
    p.want_sym("{")?;
    let mut fields = Vec::new();
    while !p.eat_sym("}") {
        if p.peek().is_none() {
            return p.err("`}`");
        }
        let wline = p.line();
        let word = p.ident()?;
        let Some(ty) = ScalarType::parse(&word) else {
            return Err(EapError {
                line: wline,
                kind: EapErrorKind::Expected {
                    want: "a scalar width".into(),
                    got: format!("`{word}`"),
                },
            });
        };
        p.want_sym(":")?;
        let value = parse_operand(p)?;
        fields.push((ty, value));
        p.eat_sym(",");
    }
    let with_response = p.eat_kw("with_response");
    Ok(AstWrite { source, fields, with_response, line })
}

fn parse_event_arm(p: &mut P) -> R<AstEventArm> {
    let line = p.line();
    let frame = p.ident()?;
    p.want_sym(":")?;
    let mut arm = AstEventArm {
        frame,
        remember: Vec::new(),
        when: Vec::new(),
        otherwise: None,
        line,
    };
    loop {
        if p.eat_kw("remember") {
            let var = p.ident()?;
            p.want_sym("=")?;
            arm.remember.push((var, parse_expr(p)?));
        } else if p.eat_kw("when") {
            let cond = parse_cond(p)?;
            p.want_sym(":")?;
            p.want_kw("goto")?;
            arm.when.push((cond, p.ident()?));
        } else if p.eat_kw("otherwise") {
            p.want_sym(":")?;
            p.want_kw("goto")?;
            arm.otherwise = Some(p.ident()?);
            break;
        } else if p.eat_kw("goto") {
            // `on_event progress: goto done` — an unconditional transition,
            // which the worked BDS example writes without an `otherwise`.
            arm.otherwise = Some(p.ident()?);
            break;
        } else {
            break;
        }
    }
    Ok(arm)
}

fn parse_expr(p: &mut P) -> R<AstExpr> {
    let a = parse_operand(p)?;
    if p.eat_sym("+") {
        let b = parse_operand(p)?;
        return Ok(AstExpr::Add(a, b));
    }
    Ok(AstExpr::Term(a))
}

fn parse_cond(p: &mut P) -> R<AstCond> {
    let lhs = parse_operand(p)?;
    let op = if p.eat_sym("==") {
        CompareOp::Eq
    } else if p.eat_sym("!=") {
        CompareOp::Ne
    } else if p.eat_sym("<=") {
        CompareOp::Le
    } else if p.eat_sym(">=") {
        CompareOp::Ge
    } else if p.eat_sym("<") {
        CompareOp::Lt
    } else if p.eat_sym(">") {
        CompareOp::Gt
    } else {
        return p.err("a comparison operator");
    };
    let rhs = parse_operand(p)?;
    Ok(AstCond { lhs, op, rhs })
}

fn parse_operand(p: &mut P) -> R<AstOperand> {
    if matches!(p.peek(), Some(Tok::Int(_))) || matches!(p.peek(), Some(Tok::Sym("-"))) {
        return Ok(AstOperand::Literal(p.int()?));
    }
    if p.eat_kw("len") {
        p.want_sym("(")?;
        let frame = p.ident()?;
        p.want_sym(".")?;
        let field = p.ident()?;
        p.want_sym(")")?;
        return Ok(AstOperand::SpanLen { frame, field });
    }
    let head = p.ident()?;
    if p.eat_sym(".") {
        let field = p.ident()?;
        if head == "session" {
            return Ok(AstOperand::Session(field));
        }
        return Ok(AstOperand::Field { frame: head, field });
    }
    // A bare identifier is a session variable. `remember received = received
    // + len(chunk.payload)` reads better than forcing `session.` on both
    // sides, and there is nothing else a bare name could be: a frame field
    // always needs its frame, because two frames may declare the same name.
    Ok(AstOperand::Session(head))
}

/// `"0021"`, `"1910"`, `"0x180f"`, or a full hyphenated 128-bit form.
///
/// Delegates entirely to [`Uuid::parse`], which already accepts every
/// spelling a firmware engineer types and already expands the 16-/32-bit
/// shorthand through the Bluetooth Base UUID. Re-deriving that here would be
/// a second implementation of a Core-Spec fact, and the two would agree
/// until one of them was edited.
fn parse_uuid(s: &str, line: u32) -> R<Uuid> {
    Uuid::parse(s).ok_or_else(|| EapError { line, kind: EapErrorKind::BadUuid(s.to_string()) })
}

// --- Lowering -----------------------------------------------------------
//
// §3 decision 59's split, made concrete. `resolve` produces the two halves
// together, from one parse, so they cannot describe different manifests.

/// One `.eap` protocol, lowered.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedProtocol {
    /// The half that crosses the wire and that dev-bench executes.
    pub def: ProtocolDef,
    /// The half that stays here: render-time primitives, keyed by frame
    /// name. Empty for a protocol whose frames are all guard-reachable.
    pub render: Vec<FrameRender>,
}

/// The render-only primitives one frame declares (§3 decision 59).
///
/// These never reach dev-bench. They are applied host-side, after the fact,
/// over the raw bytes the tap already wrote — which is where they were always
/// going to be applied anyway, since [`crate::result::ProtocolOutcome`]
/// reports a state name and nothing a `bitpack` could fill in.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameRender {
    pub frame: String,
    /// Present when the frame is flat enough to lower into §3 decision 52's
    /// own type — see [`ResolvedProtocol::struct_layouts`].
    pub layout: Option<StructLayout>,
    pub repeats: Vec<AstField>,
    pub bitpacks: Vec<AstField>,
    pub crc: Option<CrcPolicy>,
}

impl ResolvedProtocol {
    /// Every frame that lowered cleanly into a decision-52 [`StructLayout`].
    ///
    /// **This is the whole relationship between the two mechanisms.** §3
    /// decision 52's `StreamEncoding::Struct { decoder }` shipped first and
    /// is unchanged: it is still how a captured `GattNotify` payload becomes
    /// CSV rows, still resolved out of `study-structs.toml`, still indexed by
    /// a `u8` on the tap. An `.eap` file does not replace it and does not add
    /// a second rendering path — it becomes a **second front end** to the
    /// same one, so a frame an engineer already described for a state machine
    /// does not have to be described again in TOML to also be rendered.
    ///
    /// A frame lowers only if it is scalars, optionally followed by one
    /// repeating group — the shape `StructLayout` describes. A frame with a
    /// `bitpack`, a `count_from` repeat, or two repeating groups does not,
    /// and gets no layout rather than a wrong one.
    pub fn struct_layouts(&self) -> Vec<&StructLayout> {
        self.render.iter().filter_map(|r| r.layout.as_ref()).collect()
    }
}

fn bounded_str<const N: usize>(s: &str, what: &'static str, line: u32) -> R<HString<N>> {
    HString::try_from(s).map_err(|_| EapError {
        line,
        kind: EapErrorKind::NameTooLong { what, limit: N },
    })
}

fn push<T, const N: usize>(v: &mut HVec<T, N>, x: T, what: &'static str, line: u32) -> R<()> {
    v.push(x).map_err(|_| EapError { line, kind: EapErrorKind::TooMany { what, limit: N } })
}

/// Lower one parsed protocol into the definition dev-bench executes plus the
/// render-only remainder, resolving every name to an index and checking
/// every structural rule on the way.
pub fn resolve(a: &AstProtocol) -> R<ResolvedProtocol> {
    let line0 = a.states.first().map(|s| s.line).unwrap_or(1);

    // --- sources
    let mut source_ix: HashMap<&str, u8> = HashMap::new();
    let mut sources = HVec::new();
    for (i, (name, svc, chr)) in a.sources.iter().enumerate() {
        if source_ix.insert(name.as_str(), i as u8).is_some() {
            return Err(EapError {
                line: line0,
                kind: EapErrorKind::Duplicate { what: "source", name: name.clone() },
            });
        }
        push(
            &mut sources,
            ProtocolSource {
                name: bounded_str(name, "source", line0)?,
                service_uuid: *svc,
                characteristic_uuid: *chr,
            },
            "sources",
            line0,
        )?;
    }

    // --- frames: the guard-reachable half, plus the render remainder
    let mut frame_ix: HashMap<&str, u8> = HashMap::new();
    let mut frames = HVec::new();
    let mut render = Vec::new();
    for (i, f) in a.frames.iter().enumerate() {
        if frame_ix.insert(f.name.as_str(), i as u8).is_some() {
            return Err(EapError {
                line: f.line,
                kind: EapErrorKind::Duplicate { what: "frame", name: f.name.clone() },
            });
        }
        let source = *source_ix.get(f.source.as_str()).ok_or_else(|| EapError {
            line: f.line,
            kind: EapErrorKind::Unknown { what: "source", name: f.source.clone() },
        })?;

        let mut fields = HVec::new();
        let mut spans = HVec::new();
        let mut repeats = Vec::new();
        let mut bitpacks = Vec::new();
        let mut crc = None;
        // Offsets accumulate for fields written without an explicit `@`,
        // exactly as `StructLayout` packs them: declaration order, no
        // padding. A field after a variable-length primitive has no static
        // offset and must state one.
        let mut at = 0u16;
        let mut static_offset = true;
        for field in &f.fields {
            match field {
                AstField::Scalar { name, ty, at: explicit, fixed, line } => {
                    if !ty.is_integer() {
                        return Err(EapError {
                            line: *line,
                            kind: EapErrorKind::NonIntegerField(name.clone()),
                        });
                    }
                    let offset = match explicit {
                        Some(o) => *o,
                        None if static_offset => at,
                        // Silently guessing an offset after a variable-length
                        // field is the inference this suite refuses; the
                        // author has to say.
                        None => {
                            return Err(EapError {
                                line: *line,
                                kind: EapErrorKind::Expected {
                                    want: format!("`@ <offset>` on `{name}`, which follows a variable-length field"),
                                    got: "no offset".into(),
                                },
                            })
                        }
                    };
                    at = offset.saturating_add(ty.width() as u16);
                    // A `fixed(…)` modifier is render-only and the field is
                    // still readable as the integer it is, so the scalar is
                    // carried either way.
                    let _ = fixed;
                    push(
                        &mut fields,
                        ScalarRead {
                            name: bounded_str(name, "field", *line)?,
                            offset,
                            ty: *ty,
                        },
                        "frame fields",
                        *line,
                    )?;
                }
                AstField::Span { name, at: explicit, len, line } => {
                    let offset = explicit.unwrap_or(at);
                    match len {
                        Some(n) => at = offset.saturating_add(*n),
                        None => static_offset = false,
                    }
                    push(
                        &mut spans,
                        SpanRead { name: bounded_str(name, "field", *line)?, offset, len: *len },
                        "frame spans",
                        *line,
                    )?;
                }
                AstField::Repeat { .. } => {
                    static_offset = false;
                    repeats.push(field.clone());
                }
                AstField::Bitpack { .. } => {
                    static_offset = false;
                    bitpacks.push(field.clone());
                }
                AstField::Crc32 { policy, .. } => crc = Some(*policy),
            }
        }

        let select_if = match &f.select_if {
            Some((off, bytes)) => {
                let mut eq = HVec::new();
                for b in bytes {
                    push(&mut eq, *b, "select_if bytes", f.line)?;
                }
                Some(FrameMatch { offset: *off, eq })
            }
            None => None,
        };

        push(
            &mut frames,
            FrameDef {
                name: bounded_str(&f.name, "frame", f.line)?,
                source,
                select_if,
                fields,
                spans,
            },
            "frames",
            f.line,
        )?;
        render.push(FrameRender {
            frame: f.name.clone(),
            layout: lower_layout(f, &a.structs),
            repeats,
            bitpacks,
            crc,
        });
    }

    // --- session
    let mut session_ix: HashMap<&str, u8> = HashMap::new();
    let mut session = HVec::new();
    for (i, (name, init)) in a.session.iter().enumerate() {
        if session_ix.insert(name.as_str(), i as u8).is_some() {
            return Err(EapError {
                line: line0,
                kind: EapErrorKind::Duplicate { what: "session variable", name: name.clone() },
            });
        }
        push(
            &mut session,
            SessionVarDef { name: bounded_str(name, "session variable", line0)?, initial: *init },
            "session variables",
            line0,
        )?;
    }

    // --- states: names first, so a `goto` may point forward
    let mut state_ix: HashMap<&str, u8> = HashMap::new();
    for (i, s) in a.states.iter().enumerate() {
        if state_ix.insert(s.name.as_str(), i as u8).is_some() {
            return Err(EapError {
                line: s.line,
                kind: EapErrorKind::Duplicate { what: "state", name: s.name.clone() },
            });
        }
    }
    let goto = |name: &str, line: u32| -> R<u8> {
        state_ix.get(name).copied().ok_or_else(|| EapError {
            line,
            kind: EapErrorKind::Unknown { what: "state", name: name.to_string() },
        })
    };

    let mut states = crate::bounded::Bounded::new();
    for s in &a.states {
        let kind = if let Some(t) = s.terminal {
            StateKind::Terminal(t)
        } else {
            let on_enter = match &s.on_enter {
                Some(w) => Some(lower_write(w, &source_ix, &session_ix, None)?),
                None => None,
            };
            let mut on_event = HVec::new();
            for arm in &s.on_event {
                let fi = *frame_ix.get(arm.frame.as_str()).ok_or_else(|| EapError {
                    line: arm.line,
                    kind: EapErrorKind::Unknown { what: "frame", name: arm.frame.clone() },
                })?;
                let fdef = &a.frames[fi as usize];
                let ctx = Some((arm.frame.as_str(), fdef));
                let mut remember = HVec::new();
                for (var, e) in &arm.remember {
                    let vi = *session_ix.get(var.as_str()).ok_or_else(|| EapError {
                        line: arm.line,
                        kind: EapErrorKind::Unknown { what: "session variable", name: var.clone() },
                    })?;
                    push(
                        &mut remember,
                        Remember { var: vi, value: lower_expr(e, &session_ix, ctx, arm.line)? },
                        "remember clauses",
                        arm.line,
                    )?;
                }
                let mut when = HVec::new();
                for (cond, target) in &arm.when {
                    push(
                        &mut when,
                        GuardedGoto {
                            cond: Condition {
                                lhs: lower_operand(&cond.lhs, &session_ix, ctx, arm.line)?,
                                op: cond.op,
                                rhs: lower_operand(&cond.rhs, &session_ix, ctx, arm.line)?,
                            },
                            goto: goto(target, arm.line)?,
                        },
                        "when clauses",
                        arm.line,
                    )?;
                }
                let otherwise = match &arm.otherwise {
                    Some(t) => Some(goto(t, arm.line)?),
                    None => None,
                };
                push(
                    &mut on_event,
                    EventArm { frame: fi, remember, when, otherwise },
                    "on_event arms",
                    arm.line,
                )?;
            }
            let on_timeout = match &s.on_timeout {
                Some((ms, retry, target)) => Some(TimeoutArm {
                    after_ms: *ms,
                    retry: *retry,
                    goto: goto(target, s.line)?,
                }),
                None => None,
            };
            StateKind::Active(ActiveState { on_enter, on_event, on_timeout })
        };
        states
            .push(StateDef { name: bounded_str(&s.name, "state", s.line)?, kind })
            .map_err(|_| EapError {
                line: s.line,
                kind: EapErrorKind::TooMany {
                    what: "states",
                    limit: crate::limits::MAX_STATES_PER_PROTOCOL,
                },
            })?;
    }

    let def = ProtocolDef {
        name: bounded_str(&a.name, "protocol", line0)?,
        sources,
        frames,
        session,
        states,
    };
    // The same check every other consumer runs, run once here so a manifest
    // that cannot execute fails at authoring time rather than on a bench.
    crate::eap::validate_protocol(&def)
        .map_err(|e| EapError { line: line0, kind: EapErrorKind::Invalid(e) })?;
    Ok(ResolvedProtocol { def, render })
}

fn lower_write(
    w: &AstWrite,
    source_ix: &HashMap<&str, u8>,
    session_ix: &HashMap<&str, u8>,
    ctx: Option<(&str, &AstFrame)>,
) -> R<WriteAction> {
    let source = *source_ix.get(w.source.as_str()).ok_or_else(|| EapError {
        line: w.line,
        kind: EapErrorKind::Unknown { what: "source", name: w.source.clone() },
    })?;
    let mut fields = HVec::new();
    for (ty, op) in &w.fields {
        push(
            &mut fields,
            WriteField { ty: *ty, value: lower_operand(op, session_ix, ctx, w.line)? },
            "write fields",
            w.line,
        )?;
    }
    Ok(WriteAction { source, fields, with_response: w.with_response })
}

fn lower_expr(
    e: &AstExpr,
    session_ix: &HashMap<&str, u8>,
    ctx: Option<(&str, &AstFrame)>,
    line: u32,
) -> R<Expr> {
    Ok(match e {
        AstExpr::Term(a) => Expr::Term(lower_operand(a, session_ix, ctx, line)?),
        AstExpr::Add(a, b) => Expr::Add(
            lower_operand(a, session_ix, ctx, line)?,
            lower_operand(b, session_ix, ctx, line)?,
        ),
    })
}

fn lower_operand(
    o: &AstOperand,
    session_ix: &HashMap<&str, u8>,
    ctx: Option<(&str, &AstFrame)>,
    line: u32,
) -> R<Operand> {
    match o {
        AstOperand::Literal(v) => Ok(Operand::Literal(*v)),
        AstOperand::Session(name) => session_ix
            .get(name.as_str())
            .copied()
            .map(Operand::Session)
            .ok_or_else(|| EapError {
                line,
                kind: EapErrorKind::Unknown { what: "session variable", name: name.clone() },
            }),
        AstOperand::Field { frame, field } => {
            let (cname, cframe) = ctx.ok_or_else(|| EapError {
                line,
                kind: EapErrorKind::Unknown { what: "frame in this context", name: frame.clone() },
            })?;
            if cname != frame {
                return Err(EapError {
                    line,
                    kind: EapErrorKind::Unknown { what: "frame in this context", name: frame.clone() },
                });
            }
            // Only the guard-reachable scalars were lowered, and the index
            // has to be into *that* list rather than into the source order,
            // so it is recomputed here the same way.
            let ix = cframe
                .fields
                .iter()
                .filter(|f| matches!(f, AstField::Scalar { .. }))
                .position(|f| matches!(f, AstField::Scalar { name, .. } if name == field))
                .ok_or_else(|| EapError {
                    line,
                    kind: EapErrorKind::Unknown { what: "field", name: field.clone() },
                })?;
            Ok(Operand::Field(ix as u8))
        }
        AstOperand::SpanLen { frame, field } => {
            let (cname, cframe) = ctx.ok_or_else(|| EapError {
                line,
                kind: EapErrorKind::Unknown { what: "frame in this context", name: frame.clone() },
            })?;
            if cname != frame {
                return Err(EapError {
                    line,
                    kind: EapErrorKind::Unknown { what: "frame in this context", name: frame.clone() },
                });
            }
            let ix = cframe
                .fields
                .iter()
                .filter(|f| matches!(f, AstField::Span { .. }))
                .position(|f| matches!(f, AstField::Span { name, .. } if name == field))
                .ok_or_else(|| EapError {
                    line,
                    kind: EapErrorKind::Unknown { what: "span", name: field.clone() },
                })?;
            Ok(Operand::SpanLen(ix as u8))
        }
    }
}

/// Lower a frame into §3 decision 52's [`StructLayout`], when it is flat
/// enough to be one.
///
/// Returns `None` — no layout at all — rather than an approximate one, for
/// the reason decision 52 already gives about a payload that doesn't fit its
/// layout: the raw `.bin` is on disk either way, so a missing rendering can
/// be redone and a wrong one silently misreads every row.
fn lower_layout(f: &AstFrame, structs: &[AstStruct]) -> Option<StructLayout> {
    let mut header = HVec::new();
    let mut repeat = HVec::new();
    let mut seen_repeat = false;
    for field in &f.fields {
        match field {
            AstField::Scalar { name, ty, .. } => {
                if seen_repeat {
                    // A scalar after the repeating group is not something
                    // `StructLayout` can express.
                    return None;
                }
                header.push(StructField { name: HString::try_from(name.as_str()).ok()?, ty: *ty }).ok()?;
            }
            AstField::Repeat { count: AstCount::Literal(_), elem, .. } if !seen_repeat => {
                seen_repeat = true;
                let s = structs.iter().find(|s| &s.name == elem)?;
                for sf in &s.fields {
                    let AstField::Scalar { name, ty, .. } = sf else { return None };
                    repeat
                        .push(StructField { name: HString::try_from(name.as_str()).ok()?, ty: *ty })
                        .ok()?;
                }
            }
            // A `count_from` repeat, a bitpack, a span, a second repeating
            // group, or a CRC all put the frame outside what `StructLayout`
            // describes.
            _ => return None,
        }
    }
    if header.is_empty() && repeat.is_empty() {
        return None;
    }
    Some(StructLayout { name: HString::try_from(f.name.as_str()).ok()?, header, repeat })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(src: &str) -> R<ResolvedProtocol> {
        let f = parse(src)?;
        resolve(&f.protocols[0])
    }

    const SHELL: &str = r#"
protocol p {
    source s = characteristic(service: "1910", char: "0002")
    frame f on s { u8 kind @ 0 }
    session { var n: u32 = 0 }
    state go {
        on_enter: write s { u8: 0x01 }
        on_event f: goto done
        on_timeout 100ms: goto bad
    }
    state done outcome: pass
    state bad  outcome: fail
}
"#;

    #[test]
    fn the_shell_protocol_parses_resolves_and_validates() {
        let r = one(SHELL).unwrap();
        assert_eq!(r.def.name.as_str(), "p");
        assert_eq!(r.def.sources.len(), 1);
        assert_eq!(r.def.states.len(), 3);
    }

    #[test]
    fn a_16_bit_uuid_expands_through_the_bluetooth_base_uuid() {
        // The same expansion `BT_UUID_DECLARE_16` performs — a Core-Spec
        // fact, so a manifest can name a characteristic the way its own
        // header does.
        let r = one(SHELL).unwrap();
        assert_eq!(
            r.def.sources[0].service_uuid.to_hyphenated().as_str(),
            "00001910-0000-1000-8000-00805f9b34fb"
        );
    }

    #[test]
    fn comments_and_both_write_field_styles_are_accepted() {
        let src = r#"
protocol p {
    source s = characteristic(service: "1910", char: "0002")  # trailing comment
    frame f on s { u8 kind @ 0 }
    session { var n: u32 = 0 }
    state go {
        # one field per line, no commas
        on_enter: write s { u8: 0x01
                            u16le: 0x0203 }
        on_event f: goto done
    }
    state done outcome: pass
}
"#;
        let r = one(src).unwrap();
        let StateKind::Active(a) = &r.def.states[0].kind else { panic!() };
        assert_eq!(a.on_enter.as_ref().unwrap().fields.len(), 2);
    }

    #[test]
    fn crc16_is_refused_by_name_rather_than_guessed_at() {
        // Zephyr ships several mutually incompatible CRC-16s and the design
        // named none of them. Guessing would be the inference this suite
        // refuses; shipping all four would be four primitives with no caller.
        let src = SHELL.replace("u8 kind @ 0", "u8 kind @ 0\n        crc16 ieee policy: skip");
        let e = parse(&src).unwrap_err();
        assert_eq!(e.kind, EapErrorKind::Crc16Unsupported);
    }

    #[test]
    fn a_select_if_whose_declared_len_disagrees_with_its_literal_is_refused() {
        // Refused rather than trusting one over the other: either could be
        // the typo, and a silently-shortened magic matches frames it should
        // not.
        let src = SHELL.replace(
            "frame f on s {",
            "frame f on s select_if { offset: 0, len: 2, eq: magic(\"GWF1\") } {",
        );
        let e = parse(&src).unwrap_err();
        assert_eq!(e.kind, EapErrorKind::MatchLenMismatch { declared: 2, actual: 4 });
    }

    #[test]
    fn a_hex_escape_in_a_magic_survives_to_the_matcher() {
        // `BSS\x03` is how a firmware's own header writes a format tag with
        // a non-printable byte.
        let src = SHELL.replace(
            "frame f on s {",
            "frame f on s select_if { offset: 0, len: 4, eq: magic(\"BSS\\x03\") } {",
        );
        let r = one(&src).unwrap();
        let m = r.def.frames[0].select_if.as_ref().unwrap();
        assert_eq!(&m.eq[..], b"BSS\x03");
    }

    #[test]
    fn an_unknown_name_is_named_rather_than_silently_dropped() {
        for (bad, want) in [
            ("goto done", "goto nowhere"),
            ("write s {", "write nosuch {"),
            ("on_event f:", "on_event nosuch:"),
        ] {
            let e = parse(&SHELL.replace(bad, want))
                .and_then(|f| resolve(&f.protocols[0]))
                .unwrap_err();
            assert!(matches!(e.kind, EapErrorKind::Unknown { .. }), "{want}: {e}");
        }
    }

    #[test]
    fn a_duplicate_declaration_is_refused_in_every_scope() {
        for dup in [
            "    source s = characteristic(service: \"1911\", char: \"0003\")\n",
            "    frame f on s { u8 other @ 0 }\n",
            "    state done outcome: fail\n",
        ] {
            let src = SHELL.replace("    state done outcome: pass", &format!("{dup}    state done outcome: pass"));
            let e = parse(&src).and_then(|f| resolve(&f.protocols[0])).unwrap_err();
            assert!(matches!(e.kind, EapErrorKind::Duplicate { .. }), "{dup}: {e}");
        }
    }

    #[test]
    fn a_field_after_a_variable_length_one_must_state_its_offset() {
        // The alternative is guessing an offset, which is exactly the
        // inference this suite refuses everywhere else.
        let src = SHELL.replace("u8 kind @ 0", "bytes rest @ 0..len\n        u8 kind");
        let e = parse(&src).and_then(|f| resolve(&f.protocols[0])).unwrap_err();
        assert!(matches!(e.kind, EapErrorKind::Expected { .. }), "{e}");

        // Stating it is fine.
        let ok = SHELL.replace("u8 kind @ 0", "bytes rest @ 0..len\n        u8 kind @ 4");
        assert!(one(&ok).is_ok());
    }

    #[test]
    fn offsets_accumulate_in_declaration_order_when_not_stated() {
        let src = SHELL.replace("u8 kind @ 0", "u8 a\n        u32be b\n        u16le c");
        let r = one(&src).unwrap();
        let offsets: Vec<u16> = r.def.frames[0].fields.iter().map(|f| f.offset).collect();
        assert_eq!(offsets, [0, 1, 5], "packed with no padding, like StructLayout");
    }

    #[test]
    fn a_capacity_is_a_named_error_rather_than_a_truncation() {
        let mut src = String::from("protocol p {\n");
        for i in 0..crate::limits::MAX_SOURCES_PER_PROTOCOL + 1 {
            src.push_str(&format!(
                "    source s{i} = characteristic(service: \"1910\", char: \"000{}\")\n",
                i % 10
            ));
        }
        src.push_str("    state done outcome: pass\n}\n");
        let e = parse(&src).and_then(|f| resolve(&f.protocols[0])).unwrap_err();
        assert!(matches!(e.kind, EapErrorKind::TooMany { what: "sources", .. }), "{e}");
    }

    #[test]
    fn every_error_carries_the_line_it_happened_on() {
        let e = parse("protocol p {\n\n\n    source s = characteristic(oops)\n}").unwrap_err();
        assert_eq!(e.line, 4);
        assert!(e.to_string().starts_with("line 4: "));
    }

    #[test]
    fn a_fixed_scale_keeps_its_leading_zeros() {
        // `0.005` lexed as `Int . Int` would be 0.5 -- a hundredfold error in
        // a skin-temperature reading, and exactly the plausible-wrong-number
        // failure this crate keeps refusing.
        let src = SHELL.replace("u8 kind @ 0", "i16le temp @ 0 fixed(0.005, celsius)");
        let f = parse(&src).unwrap();
        let scale = f.protocols[0].frames[0]
            .fields
            .iter()
            .find_map(|x| match x {
                AstField::Scalar { fixed: Some((s, u)), .. } => Some((*s, u.clone())),
                _ => None,
            })
            .unwrap();
        assert_eq!(scale, (0.005, "celsius".to_string()));
    }

    #[test]
    fn a_range_still_lexes_as_a_range_and_not_as_a_decimal() {
        let src = SHELL.replace("u8 kind @ 0", "bytes rest @ 0..len");
        let r = one(&src).unwrap();
        assert_eq!(r.def.frames[0].spans[0].len, None);

        let src2 = SHELL.replace("u8 kind @ 0", "bytes head @ 0..4");
        let r2 = one(&src2).unwrap();
        assert_eq!(r2.def.frames[0].spans[0].len, Some(4));
    }

    #[test]
    fn a_manifest_that_cannot_execute_fails_at_authoring_time() {
        // `resolve` runs the same `validate_protocol` every other consumer
        // does, so a structural problem surfaces here rather than on a bench.
        let none_terminal = SHELL
            .replace("    state done outcome: pass\n", "")
            .replace("    state bad  outcome: fail\n", "")
            .replace("goto done", "goto go")
            .replace("goto bad", "goto go");
        let e = parse(&none_terminal)
            .and_then(|f| resolve(&f.protocols[0]))
            .unwrap_err();
        assert_eq!(
            e.kind,
            EapErrorKind::Invalid(crate::eap::ProtocolError::NoTerminalState)
        );
    }
}
