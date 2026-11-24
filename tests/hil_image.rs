#![cfg(feature = "hil-image")]
//! Fixtures in `tests/fixtures/hil-image/`: `fixture.elf` is `a.S` linked by
//! `l.ld` (two PT_LOAD at 0x20000 and 0x20400, a gap between); `fixture.hex` is
//! `objcopy -O ihex` of it; `fixture_a.bin` is `objcopy -O binary
//! --only-section=.a` of it.
use embarch_study_designer::hil_image::*;

const ELF: &[u8] = include_bytes!("fixtures/hil-image/fixture.elf");
const HEX: &[u8] = include_bytes!("fixtures/hil-image/fixture.hex");
const BIN: &[u8] = include_bytes!("fixtures/hil-image/fixture_a.bin");

#[test]
fn hex_and_elf_of_one_build_agree() {
    let h = image_segments(HEX, ImageFormat::Hex, None).unwrap();
    let e = image_segments(ELF, ImageFormat::Elf, None).unwrap();
    assert_eq!(h, e);
    assert_eq!(identity(&h), identity(&e));
    assert_eq!(identity(&h).len(), 64);
}

#[test]
fn hex_with_a_gap_gives_two_segments() {
    let h = image_segments(HEX, ImageFormat::Hex, None).unwrap();
    assert_eq!(h.len(), 2);
    assert_eq!((h[0].address, h[0].length), (0x20000, 0x3c));
    assert_eq!((h[1].address, h[1].length), (0x20400, 0x1a));
}

#[test]
fn bin_with_a_base_gives_one_segment_there() {
    let b = image_segments(BIN, ImageFormat::Bin, Some(0x20000)).unwrap();
    assert_eq!(b.len(), 1);
    assert_eq!(b[0].address, 0x20000);
    let h = image_segments(HEX, ImageFormat::Hex, None).unwrap();
    assert_eq!(b[0], h[0]);
    assert_eq!(image_segments(BIN, ImageFormat::Bin, None), Err(ImageError::NoBase));
}

#[test]
fn sha256_is_known() {
    let b = image_segments(b"abc", ImageFormat::Bin, Some(0)).unwrap();
    assert_eq!(
        b[0].sha256,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(canonical_json(&b), format!("[{{\"address\":0,\"length\":3,\"sha256\":\"{}\"}}]", b[0].sha256));
}

#[test]
fn touching_records_merge_and_bad_hex_is_refused() {
    let s = image_segments(HEX, ImageFormat::Hex, None).unwrap();
    assert_eq!(s[0].length, 0x10 * 3 + 0x0c);
    let bad = String::from_utf8_lossy(HEX).replace(":0400000320000000D9", ":0400000320000000D8");
    assert!(image_segments(bad.as_bytes(), ImageFormat::Hex, None).is_err());
    assert!(image_segments(b"nope", ImageFormat::Elf, None).is_err());
}

#[test]
fn bench_list_round_trips() {
    let segs = image_segments(HEX, ImageFormat::Hex, None).unwrap();
    let list = BenchFirmwareList {
        schema: BENCH_FIRMWARE_SCHEMA,
        release: "0.1.0".into(),
        images: vec![BenchImage {
            app: "dev-bench".into(),
            board: "b".into(),
            version: "1".into(),
            commit: "abc".into(),
            identity: identity(&segs),
            segments: segs,
        }],
    };
    let j = serde_json::to_string(&list).unwrap();
    let back: BenchFirmwareList = serde_json::from_str(&j).unwrap();
    assert_eq!(back, list);
    assert!(back.images[0].identity_consistent());
}
