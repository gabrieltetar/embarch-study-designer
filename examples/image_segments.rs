//! `cargo run --features hil-image --example image_segments -- <file> [base]`
//! prints a hex, ELF or bin file's load segments and identity as JSON. The
//! format is read from the extension (`.hex`/`.ihex`, `.elf`, anything else is
//! a bin, which needs its load address as `base`, decimal or `0x` hex).
use embarch_study_designer::hil_image::{describe_json, image_segments, ImageFormat};

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: image_segments <file> [base]");
    let base = args.next().map(|s| match s.strip_prefix("0x") {
        Some(h) => u64::from_str_radix(h, 16).expect("base"),
        None => s.parse().expect("base"),
    });
    let bytes = std::fs::read(&path).expect("read file");
    let format = match path.rsplit('.').next().unwrap_or("") {
        "hex" | "ihex" => ImageFormat::Hex,
        "elf" => ImageFormat::Elf,
        _ => ImageFormat::Bin,
    };
    match image_segments(&bytes, format, base) {
        Ok(segs) => println!("{}", describe_json(&segs)),
        Err(e) => {
            eprintln!("{path}: {e}");
            std::process::exit(1);
        }
    }
}
