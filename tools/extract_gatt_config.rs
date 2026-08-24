//! CLI wrapper for `ZephyrBleDefExtractor` (design.md §3 decision 33,
//! embarch-study-designer/milestone-9.md §3.7) — the authoring-time
//! convenience that decision exists to provide: run this against a checked-out
//! `reference-dut-fw` repo to see its GATT table as JSON, before ever
//! connecting dev-bench to it.
//!
//! ```text
//! cargo run --features gatt-extract --bin extract-gatt-config -- --repo <path>
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

use embarch_study_designer::{ZephyrBleDefExtractor, GattConfigExtractor};

fn main() -> ExitCode {
    let mut repo: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--repo" => repo = args.next().map(PathBuf::from),
            other => {
                eprintln!("unrecognized argument: {other}");
                eprintln!("usage: extract-gatt-config --repo <path>");
                return ExitCode::FAILURE;
            }
        }
    }

    let Some(repo) = repo else {
        eprintln!("usage: extract-gatt-config --repo <path>");
        return ExitCode::FAILURE;
    };

    match ZephyrBleDefExtractor.extract(&repo) {
        Ok(services) => match serde_json::to_string_pretty(&services) {
            Ok(json) => {
                println!("{json}");
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("failed to render extracted GATT table as JSON: {err}");
                ExitCode::FAILURE
            }
        },
        Err(err) => {
            eprintln!("extraction failed: {err}");
            ExitCode::FAILURE
        }
    }
}
