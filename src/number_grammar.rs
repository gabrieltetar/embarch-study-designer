//! The number grammar of `interfaces/hil-pdf.md`: what is a number in text
//! extracted from a PDF, and in source text (the prose files and `report.typ`)
//! that `verify`'s check 4 rescans with the same rules. Host-only, behind the
//! `pdf-text` feature (decision 101).
//!
//! Nothing is exempted by its shape: a year, a version and a page count are
//! numbers here. The exemptions (an opaque word, a citation, a measurement
//! match, the footer and the axis ticks) are the caller's, not this module's,
//! except the opaque-word rule, which the grammar applies first.

use std::collections::HashSet;
use std::string::String;
use std::vec::Vec;

/// An exact decimal as written: `(-1)^negative * coefficient * 10^exp10`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decimal {
    pub negative: bool,
    /// The mantissa's digits with the point removed and leading zeros stripped
    /// (`"0"` for zero); trailing zeros are kept as written.
    pub coefficient: String,
    pub exp10: i32,
}

impl Decimal {
    /// The value as an `f64`, for display or a coarse compare; never for a match.
    pub fn to_f64(&self) -> f64 {
        let mut s = String::new();
        if self.negative {
            s.push('-');
        }
        s.push_str(&self.coefficient);
        s.push('e');
        s.push_str(&self.exp10.to_string());
        s.parse().unwrap_or(f64::NAN)
    }

    /// The same value with trailing zeros of the coefficient moved into the
    /// exponent, so `1.50` and `1.5` compare equal. Zero has no sign.
    pub fn normalised(&self) -> Decimal {
        if self.coefficient == "0" {
            return Decimal { negative: false, coefficient: "0".into(), exp10: 0 };
        }
        let trimmed = self.coefficient.trim_end_matches('0');
        let removed = (self.coefficient.len() - trimmed.len()) as i32;
        Decimal { negative: self.negative, coefficient: trimmed.into(), exp10: self.exp10 + removed }
    }
}

/// One number token found in a word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NumberToken {
    /// The token's text as written, sign and exponent included.
    pub text: String,
    /// Byte offset of the token's first character in the word.
    pub start: usize,
    pub value: Decimal,
    /// The written digits after stripping leading zeros, trailing zeros after
    /// a point included, an integer's all counted (`1200` is four); the
    /// exponent's digits are not counted; a zero is one.
    pub sig_digits: u32,
    /// Digits after the decimal point in the mantissa.
    pub decimals: u32,
}

fn is_sign(c: char) -> bool {
    c == '-' || c == '+' || c == '\u{2212}'
}

/// Strips the opaque-word wrapper: leading `( [ " '`, trailing `) ] " ' , ; :`
/// and one trailing `.` (taken wherever it falls among the trailing marks).
pub fn strip_opaque_word(word: &str) -> &str {
    let mut w = word.trim_start_matches(['(', '[', '"', '\'']);
    let mut dot_taken = false;
    loop {
        let before = w.len();
        w = w.trim_end_matches([')', ']', '"', '\'', ',', ';', ':']);
        if !dot_taken {
            if let Some(rest) = w.strip_suffix('.') {
                w = rest;
                dot_taken = true;
            }
        }
        if w.len() == before {
            return w;
        }
    }
}

/// Whether the word, stripped, is byte-equal to a string `report-data.json` holds.
pub fn is_opaque(word: &str, opaque_values: &HashSet<String>) -> bool {
    let w = strip_opaque_word(word);
    !w.is_empty() && opaque_values.contains(w)
}

/// The number tokens of one word, longest match, left to right, no overlap.
/// The opaque-word rule is NOT applied here; see [`numbers_in_word`].
pub fn tokens(word: &str) -> Vec<NumberToken> {
    let chars: Vec<(usize, char)> = word.char_indices().collect();
    let n = chars.len();
    let digit = |i: usize| i < n && chars[i].1.is_ascii_digit();
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        let c = chars[i].1;
        let starts_mantissa = c.is_ascii_digit() || (c == '.' && digit(i + 1));
        if !starts_mantissa {
            i += 1;
            continue;
        }
        // A sign belongs only if it directly precedes and what is before it is
        // not a letter or digit.
        let mut start = i;
        if i >= 1 && is_sign(chars[i - 1].1) {
            let before_ok = i < 2 || !chars[i - 2].1.is_alphanumeric();
            if before_ok && out.last().is_none_or(|t: &NumberToken| t.start + t.text.len() <= chars[i - 1].0) {
                start = i - 1;
            }
        }
        let mut j = i;
        let mut int_digits = String::new();
        while digit(j) {
            int_digits.push(chars[j].1);
            j += 1;
        }
        let mut frac_digits = String::new();
        if j < n && chars[j].1 == '.' && digit(j + 1) {
            j += 1;
            while digit(j) {
                frac_digits.push(chars[j].1);
                j += 1;
            }
        }
        let mut exp: i32 = 0;
        if j < n && (chars[j].1 == 'e' || chars[j].1 == 'E') {
            let mut k = j + 1;
            let mut eneg = false;
            if k < n && is_sign(chars[k].1) {
                eneg = chars[k].1 != '+';
                k += 1;
            }
            if digit(k) {
                let mut e: i64 = 0;
                while digit(k) {
                    e = (e * 10 + i64::from(chars[k].1 as u8 - b'0')).min(1_000_000);
                    k += 1;
                }
                exp = if eneg { -(e as i32) } else { e as i32 };
                j = k;
            }
        }
        let end_byte = if j < n { chars[j].0 } else { word.len() };
        let text = &word[chars[start].0..end_byte];
        let negative = is_sign(chars[start].1) && chars[start].1 != '+';
        let all: String = int_digits.chars().chain(frac_digits.chars()).collect();
        let stripped = all.trim_start_matches('0');
        let (coefficient, sig_digits) = if stripped.is_empty() {
            ("0".to_string(), 1)
        } else {
            (stripped.to_string(), stripped.len() as u32)
        };
        let decimals = frac_digits.len() as u32;
        out.push(NumberToken {
            text: text.to_string(),
            start: chars[start].0,
            value: Decimal { negative, coefficient, exp10: exp - decimals as i32 },
            sig_digits,
            decimals,
        });
        i = j;
    }
    out
}

/// The numbers of one word: none if it is opaque, else its tokens.
pub fn numbers_in_word(word: &str, opaque_values: &HashSet<String>) -> Vec<NumberToken> {
    if is_opaque(word, opaque_values) {
        Vec::new()
    } else {
        tokens(word)
    }
}

/// The numbers of source text (a prose file or `report.typ`), split into words
/// at whitespace and read with the same rules as extracted text.
pub fn numbers_in_text(text: &str, opaque_values: &HashSet<String>) -> Vec<NumberToken> {
    text.split_whitespace().flat_map(|w| numbers_in_word(w, opaque_values)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(w: &str) -> Vec<String> {
        tokens(w).into_iter().map(|t| t.text).collect()
    }

    #[test]
    fn longest_match_examples_from_the_doc() {
        assert_eq!(texts("4.75\u{2013}5.25"), ["4.75", "5.25"]);
        assert_eq!(texts("R3.2"), ["3.2"]);
        assert_eq!(texts("SHA-256"), ["256"]);
        assert_eq!(texts("v2"), ["2"]);
        assert_eq!(texts("1,234"), ["1", "234"]);
        assert_eq!(texts("12."), ["12"]);
        assert_eq!(texts("1.2.3"), ["1.2", ".3"]);
    }

    #[test]
    fn sign_rules() {
        assert_eq!(texts("A-3"), ["3"]);
        assert_eq!(texts("x=-3"), ["-3"]);
        assert_eq!(texts("5-3"), ["5", "3"]);
        assert_eq!(texts("(-3)"), ["-3"]);
        assert_eq!(texts("-.5"), ["-.5"]);
        assert_eq!(texts("\u{2212}4.5"), ["\u{2212}4.5"]);
        assert!(tokens("\u{2212}4.5")[0].value.negative);
        assert_eq!(texts("+7"), ["+7"]);
        assert!(!tokens("+7")[0].value.negative);
    }

    #[test]
    fn exponent_only_with_a_digit() {
        assert_eq!(texts("1e"), ["1"]);
        assert_eq!(texts("1e5"), ["1e5"]);
        assert_eq!(texts("2.5E-3"), ["2.5E-3"]);
        assert_eq!(texts("1e+"), ["1"]);
        let t = &tokens("2.5E-3")[0];
        assert_eq!(t.value.to_f64(), 0.0025);
        assert_eq!(t.sig_digits, 2);
        assert_eq!(t.decimals, 1);
    }

    #[test]
    fn significant_digits_and_decimals() {
        let sig = |w: &str| tokens(w)[0].sig_digits;
        assert_eq!(sig("1200"), 4);
        assert_eq!(sig("0.0120"), 3);
        assert_eq!(sig("4.50"), 3);
        assert_eq!(sig("007"), 1);
        assert_eq!(sig("0"), 1);
        assert_eq!(sig("0.00"), 1);
        assert_eq!(tokens("4.50")[0].decimals, 2);
        assert_eq!(tokens("1200")[0].decimals, 0);
    }

    #[test]
    fn value_is_exact_and_normalises() {
        let v = &tokens("4.50")[0].value;
        assert_eq!(v.coefficient, "450");
        assert_eq!(v.exp10, -2);
        assert_eq!(v.normalised(), tokens("4.5")[0].value.normalised());
        assert!(!tokens("-0")[0].value.normalised().negative);
    }

    #[test]
    fn a_year_a_version_and_a_page_count_are_numbers() {
        assert_eq!(texts("2026"), ["2026"]);
        assert_eq!(texts("v1.2.0"), ["1.2", ".0"]);
        assert_eq!(texts("12"), ["12"]);
        let none = HashSet::new();
        assert_eq!(numbers_in_text("built in 2026 on 3 pages, rev 1.4", &none).len(), 3);
    }

    #[test]
    fn opaque_word_is_consumed_whole() {
        let mut set = HashSet::new();
        set.insert("2026-11-20".to_string());
        set.insert("a3f9c0".to_string());
        assert!(numbers_in_word("2026-11-20", &set).is_empty());
        assert!(numbers_in_word("(2026-11-20),", &set).is_empty());
        assert!(numbers_in_word("\"a3f9c0\".", &set).is_empty());
        assert!(numbers_in_word("(a3f9c0).", &set).is_empty());
        // Not byte-equal: its digits are numbers.
        assert_eq!(numbers_in_word("2026-11-21", &set).len(), 3);
        // Half of a split hash is not equal to it.
        assert!(!numbers_in_word("a3f9", &set).is_empty());
    }

    #[test]
    fn source_text_uses_the_same_rules() {
        let none = HashSet::new();
        let t = numbers_in_text("Supply is 3.3 V,\n-0.5 mV drift", &none);
        let s: Vec<&str> = t.iter().map(|n| n.text.as_str()).collect();
        assert_eq!(s, ["3.3", "-0.5"]);
    }
}
