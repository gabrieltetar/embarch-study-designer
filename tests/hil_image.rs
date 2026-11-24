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

/// An ELF64 header with one program header, built by hand, so a field can be
/// pushed past `u64::MAX`.
fn elf64(phoff: u64, phentsize: u16, phnum: u16, off: u64, filesz: u64) -> Vec<u8> {
    let mut b = vec![0u8; 64 + 56];
    b[..4].copy_from_slice(b"\x7fELF");
    b[4] = 2;
    b[5] = 1;
    b[32..40].copy_from_slice(&phoff.to_le_bytes());
    b[54..56].copy_from_slice(&phentsize.to_le_bytes());
    b[56..58].copy_from_slice(&phnum.to_le_bytes());
    b[64..68].copy_from_slice(&1u32.to_le_bytes());
    b[72..80].copy_from_slice(&off.to_le_bytes());
    b[88..96].copy_from_slice(&0x1000u64.to_le_bytes());
    b[96..104].copy_from_slice(&filesz.to_le_bytes());
    b
}

#[test]
fn elf_segment_offset_plus_size_overflow_is_an_error() {
    let ok = elf64(64, 56, 1, 0, 8);
    assert!(image_segments(&ok, ImageFormat::Elf, None).is_ok());
    let bad = elf64(64, 56, 1, 8, u64::MAX);
    assert!(matches!(image_segments(&bad, ImageFormat::Elf, None), Err(ImageError::Elf(_))));
}

#[test]
fn elf_program_header_position_overflow_is_an_error() {
    let bad = elf64(u64::MAX, 56, 1, 0, 8);
    assert!(matches!(image_segments(&bad, ImageFormat::Elf, None), Err(ImageError::Elf(_))));
    let bad = elf64(u64::MAX - 10, 56, 2, 0, 8);
    assert!(matches!(image_segments(&bad, ImageFormat::Elf, None), Err(ImageError::Elf(_))));
}

#[test]
fn the_hil_json_types_refuse_an_unknown_key() {
    let seg = r#"{"address":0,"length":1,"sha256":"00","extra":1}"#;
    assert!(serde_json::from_str::<Segment>(seg).is_err());
    let img = r#"{"app":"a","board":"b","version":"1","commit":"c","identity":"i","segments":[],"extra":1}"#;
    assert!(serde_json::from_str::<BenchImage>(img).is_err());
    let list = r#"{"schema":1,"release":"r","images":[],"extra":1}"#;
    assert!(serde_json::from_str::<BenchFirmwareList>(list).is_err());
    assert!(serde_json::from_str::<BenchFirmwareList>(r#"{"schema":1,"release":"r","images":[]}"#).is_ok());
}

/// An ELF64 with two `PT_LOAD` segments of 8 bytes each at the given physical
/// addresses.
fn elf64_two(paddr_a: u64, paddr_b: u64) -> Vec<u8> {
    let mut b = vec![0u8; 64 + 2 * 56];
    b[..4].copy_from_slice(b"\x7fELF");
    b[4] = 2;
    b[5] = 1;
    b[32..40].copy_from_slice(&64u64.to_le_bytes());
    b[54..56].copy_from_slice(&56u16.to_le_bytes());
    b[56..58].copy_from_slice(&2u16.to_le_bytes());
    for (i, paddr) in [paddr_a, paddr_b].into_iter().enumerate() {
        let p = 64 + i * 56;
        b[p..p + 4].copy_from_slice(&1u32.to_le_bytes());
        b[p + 24..p + 32].copy_from_slice(&paddr.to_le_bytes());
        b[p + 32..p + 40].copy_from_slice(&8u64.to_le_bytes());
    }
    b
}

#[test]
fn elf_load_address_near_u64_max_is_an_error() {
    let ok = elf64_two(0x1000, 0x2000);
    assert_eq!(image_segments(&ok, ImageFormat::Elf, None).unwrap().len(), 2);
    let bad = elf64_two(u64::MAX - 4, 0x1000);
    assert!(matches!(image_segments(&bad, ImageFormat::Elf, None), Err(ImageError::Overlap(_))));
    let bad = elf64_two(0x1000, u64::MAX - 4);
    assert!(matches!(image_segments(&bad, ImageFormat::Elf, None), Err(ImageError::Overlap(_))));
}

#[test]
fn bin_base_near_u64_max_is_an_error() {
    let bytes = [0u8; 16];
    assert!(matches!(
        image_segments(&bytes, ImageFormat::Bin, Some(u64::MAX - 4)),
        Err(ImageError::Overlap(_))
    ));
    assert!(image_segments(&bytes, ImageFormat::Bin, Some(u64::MAX - 16)).is_ok());
}
