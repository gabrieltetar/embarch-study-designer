//! CLI wrapper for `ZephyrBleDefExtractor` (design.md §3 decision 33,
//! embarch-study-designer/milestone-9.md §3.7) — the authoring-time
//! convenience that decision exists to provide: run this against a checked-out
//! `reference-dut-fw` repo to see its GATT table as JSON, before ever
//! connecting dev-bench to it.
//!
//! Prints `{ "services": [...], "names": { "<characteristic-uuid>": {...} } }`
//! — the names half added by design.md §3 decision 56.
//!
//! ```text
//! cargo run --features gatt-extract --bin extract-gatt-config -- --repo <path>
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

use embarch_study_designer::{
    ZephyrBleDefExtractor, GattConfigExtractor, GattName, GattNameBook, GattServiceInfo,
};
use serde::Serialize;

/// What this prints. An object rather than decision 33's original bare
/// `services` array, so decision 56's names have somewhere to go — the
/// characteristic UUIDs alone are what made a `numbers, not names` picker the
/// only thing a UI could render, and eyeballing the extraction is exactly
/// where a wrong or missing name should be caught.
#[derive(Serialize)]
struct Output {
    services: Vec<GattServiceInfo>,
    /// Keyed by hyphenated characteristic UUID — the form the rest of the
    /// suite already renders and compares in.
    names: std::collections::BTreeMap<String, GattName>,
}

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

    match ZephyrBleDefExtractor.extract_labeled(&repo) {
        Ok(extracted) => {
            let book = GattNameBook::new().with_symbols(
                extracted.symbols.iter().map(|s| (s.uuid, s.identifier.clone())),
            );
            let output = Output {
                names: extracted
                    .services
                    .iter()
                    .flat_map(|service| service.characteristics.iter())
                    .filter_map(|chrc| {
                        book.get(chrc.uuid)
                            .map(|name| (chrc.uuid.to_hyphenated().to_string(), name))
                    })
                    .collect(),
                services: extracted.services.iter().cloned().collect(),
            };
            match serde_json::to_string_pretty(&output) {
                Ok(json) => {
                    println!("{json}");
                    ExitCode::SUCCESS
                }
                Err(err) => {
                    eprintln!("failed to render extracted GATT table as JSON: {err}");
                    ExitCode::FAILURE
                }
            }
        }
        Err(err) => {
            eprintln!("extraction failed: {err}");
            ExitCode::FAILURE
        }
    }
}
