# embarch-study-designer

Shared `no_std` Rust data types — and the narrow set of tools to work with them
identically everywhere — for [EmbArch](https://github.com/gabrieltetar/embarch-doc)
hardware-in-the-loop studies. Compiled by `embarch-api` and `embarch-core`, and
read by `embarch-dev-bench` firmware's build, which takes the wire schema
version, the limits and the wire enum values out of this crate's source at
configure time, so a `Study` crossing between them can't drift into three
independently-maintained, slowly-diverging definitions.

This crate is a mechanical translation of
[`embarch-doc`'s `embarch-study-designer/spec.md`](https://github.com/gabrieltetar/embarch-doc/blob/main/embarch-study-designer/spec.md)
— that document (with `decisions.md`, `open.md`, and `interfaces/*.md`) is the
durable architecture record; this repo just implements it. Doc comments
throughout `src/` cite it by decision number (`decision 17`) or by file
(`interfaces/types.md`); a bare `spec.md §N` cites that file's own numbered
section.

## Layout

| Module | Contents |
|---|---|
| `study` | `Study`, `Step`, `Action` (interfaces/types.md) |
| `result` | `StudyResult`, `StepResult`, `Outcome` (interfaces/result-types.md) |
| `sample` | `Sample`, the shared power/waveform CSV row record (interfaces/decoders.md) |
| `protocol` | `DevBenchMessage`, the Core<->dev-bench serial wire protocol (decisions 10, 12, 20) |
| `crc` | `steps_crc`, the CRC-32 integrity seal over `Study.steps` (decision 17) |
| `schema_version` | `STUDY_DESIGNER_SCHEMA_VERSION` (decision 12) |
| `limits` | Fixed-capacity bounds for every `heapless` collection (decision 15) |
| `ids` | `Uuid`/`BleAddress` newtypes |
| `bounded` | Capacity-bounded sequence storage, one shape per target (decisions 46, 49) |
| `streams` | Stream taps — source, scope, and rendering encoding for one capture (decision 39, interfaces/taps.md) |
| `decoder` | Engineer-declared struct decoding for a stream tap's payload bytes (decision 52, interfaces/decoders.md) |
| `records` | Engineer-declared record framing for a stream tap, and Core's post-capture check (decision 70) |
| `outpost` | The `embarch-outpost` trace wire format and its manifest |
| `gatt` | GATT discovery types shared by live discovery and static extraction (interfaces/gatt-types.md, decisions 31/32/33) |
| `gatt_names` (feature `std`) | Naming a discovered characteristic something a human recognizes (decision 56) |
| `vendor` | Vendor-defined GATT service identities, e.g. Nordic UART Service (decision 41) |
| `eap` | `.eap` protocol manifests in the form dev-bench executes (decisions 58-62, interfaces/eap.md) |
| `eap_interp` (feature `eap-parse`) | Host-side reference interpreter for a `ProtocolDef` (decision 60) |
| `eap_parse` (feature `eap-parse`) | The `.eap` text grammar: lexer, parser, and lowering (decisions 58/59) |
| `gatt_extract` (feature `gatt-extract`) | Static GATT-config extraction from firmware source (decisions 33, 56, 57) |
| `registry` (feature `study-ui`) | User-authored custom-action registry (decision 35) |
| `merged_actions` (feature `study-ui`) | The merged action list a Study Designer UI row picks from (decisions 34/35) |
| `study_builder` (feature `study-ui`) | Table-row -> `Study` conversion for the Study Designer UI (decision 34) |

## Features

- **default** — `#![no_std]`, no allocator, no floating-point-heavy code.
  Nothing links it today: dev-bench firmware stopped linking the crate when
  the FFI staticlib was retired (decision 7), and the shape stays open for a
  firmware consumer.
- **`alloc`** — heap-backed `Study.steps` and result containers instead of a
  fixed-capacity `heapless` collection. Host consumers (`embarch-api`,
  `embarch-core`, `embarch-ui`) enable it.
- **`std`** — enables `alloc` plus the standard library, for the
  authoring-time tools (`gatt-extract`, `study-ui`). dev-bench firmware never
  enables this.
- **`gatt-extract`** — the repo-walking GATT extractor (needs `regex`,
  `ignore`): the `GattConfigExtractor` trait, `ZephyrBleDefExtractor`, and the
  `extract-gatt-config` CLI binary. An authoring-time binary; `std`-only.
- **`study-ui`** — table-authoring types, the study builder, and the
  custom-action registry that `embarch-ui`'s Study Designer tab depends on.
  `std`-only.
- **`eap-parse`** — the `.eap` protocol-manifest parser and a host-side
  reference interpreter, for authoring and for pinning the semantics C must
  match. `std`-only.

Run the full test suite with `cargo test --all-features`; a plain
`cargo build` (no features) exercises the `#![no_std]`, no-allocator path.

## Status

Types, wire format, and CRC/CSV tooling are implemented and tested.
**Dev-bench firmware links no Rust.** Its C codec is written by hand and
pinned to this crate by the cross-side byte tests
(`tests/firmware_test_vectors.rs`), and its build reads the wire schema
version, every limit and every wire enum's values out of this crate's source
at configure time (`embarch-dev-bench` decisions 20 and 56). The `extern "C"`
staticlib it used to link was retired on 2026-11-10: its only live call was
the schema version.

## License

MIT — see [LICENSE](LICENSE).
