#![cfg(feature = "pdf-text")]
//! Fixture PDFs for the text extractor, written out by hand (objects, xref and
//! all) so each test controls exactly what the content stream says.

use embarch_study_designer::pdf_text::{extract_pages, words, ExtractError};

/// Assembles a PDF from numbered object bodies (object N is `objs[N-1]`).
fn assemble(objs: &[Vec<u8>]) -> Vec<u8> {
    let mut out = b"%PDF-1.5\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend(format!("{} 0 obj\n", i + 1).bytes());
        out.extend(o);
        out.extend(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).bytes());
    for o in &offs {
        out.extend(format!("{o:010} 00000 n \n").bytes());
    }
    out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).bytes());
    out
}

fn stream(dict: &str, data: &str) -> Vec<u8> {
    format!("<< {dict} /Length {} >>\nstream\n{data}\nendstream", data.len()).into_bytes()
}

/// ToUnicode: ASCII identity, plus 0x80 = FULLWIDTH DIGIT ONE, 0x81 = LATIN SMALL LIGATURE FI,
/// 0x82 = MINUS SIGN.
const CMAP: &str = "/CIDInit /ProcSet findresource begin 12 dict begin begincmap\n\
1 begincodespacerange <00> <FF> endcodespacerange\n\
1 beginbfrange <20> <7E> <0020> endbfrange\n\
3 beginbfchar <80> <FF11> <81> <FB01> <82> <2212> endbfchar\nendcmap";
/// Same, but 0x90 and above are not mapped.
const CMAP_NARROW: &str = "1 begincodespacerange <00> <FF> endcodespacerange\n1 beginbfrange <20> <7E> <0020> endbfrange";

fn widths() -> String {
    format!("[{}]", vec!["500"; 256].join(" "))
}

/// Pages' content strings, in the order the page tree lists them. The page
/// objects are numbered in the reverse of that order so object order differs.
fn pdf(contents_in_tree_order: &[&str], cmap: &str) -> Vec<u8> {
    let n = contents_in_tree_order.len();
    // 1 catalog, 2 pages, 3 font, 4 cmap, then per page: content, page.
    let mut objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        Vec::new(),
        format!("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar 0 /LastChar 255 /Widths {} /ToUnicode 4 0 R >>", widths()).into_bytes(),
        stream("", cmap),
    ];
    let mut page_ids = vec![0usize; n];
    for (k, c) in contents_in_tree_order.iter().enumerate() {
        objs.push(stream("", c));
        let content_id = objs.len();
        objs.push(
            format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 3 0 R >> >> /Contents {content_id} 0 R >>")
                .into_bytes(),
        );
        page_ids[k] = objs.len();
    }
    // Kids in the reverse of creation order for the first two pages, to prove
    // the page tree and not object order decides.
    let kids: Vec<String> = page_ids.iter().map(|i| format!("{i} 0 R")).collect();
    objs[1] = format!("<< /Type /Pages /Kids [{}] /Count {n} >>", kids.join(" ")).into_bytes();
    assemble(&objs)
}

#[test]
fn page_order_is_the_page_trees_and_runs_are_in_stream_order() {
    // The runs are drawn bottom-to-top, so content order differs from reading order.
    let bytes = pdf(
        &["BT /F1 10 Tf 50 100 Td (second-drawn-first) Tj 1 0 0 1 50 700 Tm (top) Tj ET", "BT /F1 10 Tf 50 100 Td (page two) Tj ET"],
        CMAP,
    );
    let pages = extract_pages(&bytes).unwrap();
    assert_eq!(pages.iter().map(|p| p.number).collect::<Vec<_>>(), [1, 2]);
    assert_eq!(pages[0].runs.iter().map(|r| r.text.as_str()).collect::<Vec<_>>(), ["second-drawn-first", "top"]);
    assert_eq!(pages[1].runs[0].text, "page two");
}

#[test]
fn coordinates_and_size_follow_the_matrices() {
    let bytes = pdf(&["q 2 0 0 2 10 20 cm BT /F1 12 Tf 1 0 0 1 5 7 Tm (ab) Tj ET Q"], CMAP);
    let r = &extract_pages(&bytes).unwrap()[0].runs[0];
    assert_eq!(r.text, "ab");
    assert!((r.x - 20.0).abs() < 1e-9, "x = {}", r.x); // 5*2 + 10
    assert!((r.y - 34.0).abs() < 1e-9, "y = {}", r.y); // 7*2 + 20
    assert!((r.size - 24.0).abs() < 1e-9, "size = {}", r.size);
    // two glyphs at 500/1000 em, scaled by 2
    assert!((r.x_end - (20.0 + 2.0 * 0.5 * 24.0)).abs() < 1e-9, "x_end = {}", r.x_end);
}

#[test]
fn td_t_star_and_quote_move_the_origin() {
    let bytes = pdf(&["BT /F1 10 Tf 12 TL 100 500 Td (a) Tj T* (b) Tj (c) ' ET"], CMAP);
    let runs = &extract_pages(&bytes).unwrap()[0].runs;
    let ys: Vec<f64> = runs.iter().map(|r| r.y).collect();
    assert_eq!(ys, [500.0, 488.0, 476.0]);
}

#[test]
fn an_unmapped_glyph_fails_naming_its_page() {
    let bytes = pdf(&["BT /F1 10 Tf 50 100 Td (fine) Tj ET", "BT /F1 10 Tf 50 100 Td (ok \\220) Tj ET"], CMAP_NARROW);
    let err = extract_pages(&bytes).unwrap_err();
    assert!(matches!(err, ExtractError::Unmapped { page: 2, code: 0x90, .. }), "{err:?}");
    assert_eq!(err.to_string(), "unextractable text on page 2");
}

#[test]
fn nfkc_is_applied_and_minus_sign_is_kept() {
    // 0x80 full-width 1, 0x81 fi ligature, 0x82 U+2212.
    let bytes = pdf(&["BT /F1 10 Tf 50 100 Td (\\200 \\201 \\2024) Tj ET"], CMAP);
    let r = &extract_pages(&bytes).unwrap()[0].runs[0];
    assert_eq!(r.text, "1 fi \u{2212}4");
}

#[test]
fn a_tj_gap_over_the_threshold_splits_the_run_and_the_word() {
    // A TJ number of -400 opens a 4 pt gap at 10 pt, over 0.3 * 10; -100 is 1 pt, under it.
    let bytes = pdf(&["BT /F1 10 Tf 50 100 Td [(12) -100 (34) -400 (56)] TJ ET"], CMAP);
    let page = &extract_pages(&bytes).unwrap()[0];
    assert_eq!(page.runs.iter().map(|r| r.text.as_str()).collect::<Vec<_>>(), ["1234", "56"]);
    let w = words(page);
    assert_eq!(w.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(), ["1234", "56"]);
}

#[test]
fn words_split_at_whitespace_and_at_a_gap_and_join_when_touching() {
    // "ab" ends at x=60 (2 glyphs * 5 pt); "cd" starts at 60: touching, one word.
    // "cd" ends at 70; "ef" starts at 90: a 20 pt gap over 3, a new word. Whitespace splits "g h".
    let bytes = pdf(&["BT /F1 10 Tf 50 100 Td (ab) Tj 1 0 0 1 60 100 Tm (cd) Tj 1 0 0 1 90 100 Tm (ef) Tj 1 0 0 1 120 100 Tm (g h) Tj ET"], CMAP);
    let w = words(&extract_pages(&bytes).unwrap()[0]);
    assert_eq!(w.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(), ["abcd", "ef", "g", "h"]);
    assert!(w.iter().all(|w| w.line == 0));
}

#[test]
fn a_change_of_y_over_half_the_size_is_a_new_line() {
    // 4 pt apart at size 10 stays one line; 6 pt is a new line.
    let bytes = pdf(&["BT /F1 10 Tf 50 100 Td (a) Tj 1 0 0 1 100 104 Tm (b) Tj 1 0 0 1 150 110 Tm (c) Tj ET"], CMAP);
    let w = words(&extract_pages(&bytes).unwrap()[0]);
    assert_eq!(w.iter().map(|w| w.line).collect::<Vec<_>>(), [0, 0, 1]);
}

#[test]
fn a_font_the_resources_lack_is_an_error() {
    let bytes = pdf(&["BT /F9 10 Tf 50 100 Td (a) Tj ET"], CMAP);
    assert!(matches!(extract_pages(&bytes), Err(ExtractError::MissingFont { page: 1, .. })));
}

#[test]
fn non_pdf_bytes_are_a_parse_error() {
    assert!(matches!(extract_pages(b"not a pdf"), Err(ExtractError::Parse(_))));
}
