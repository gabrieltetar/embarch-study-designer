# embarch-study-designer

Shared `no_std` Rust data types — and the narrow set of tools to work with them
identically everywhere — for [EmbArch](https://github.com/gabrieltetar/embarch-doc)
hardware-in-the-loop studies. Compiled independently by `embarch-api`,
`embarch-core`, and `embarch-dev-bench` firmware, so a `Study` crossing between
them can't drift into three independently-maintained, slowly-diverging
definitions.

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
| `result` | `StudyResult`, `StepResult`, `Outcome` (interfaces/types.md) |
| `sample` | `Sample`, the shared power/waveform CSV row record (interfaces/decoders.md) |
| `protocol` | `DevBenchMessage`, the Core<->dev-bench serial wire protocol (decisions 10, 12, 20) |
| `crc` | `steps_crc`, the CRC-32 integrity seal over `Study.steps` (decision 17) |
| `schema_version` | `STUDY_DESIGNER_SCHEMA_VERSION` (decision 12) |
| `limits` | Fixed-capacity bounds for every `heapless` collection (decision 15) |
| `ids` | `Uuid`/`BleAddress` newtypes |
| `ffi` (feature `ffi`) | `extern "C"` surface for dev-bench firmware (decisions 7, 23) |

## Features

- **default** — `#![no_std]`, no allocator, no floating-point-heavy code. What
  dev-bench firmware links.
- **`alloc`** — heap-backed `Study.steps` and result containers instead of a
  fixed-capacity `heapless` collection. Host consumers (`embarch-api`,
  `embarch-core`, `embarch-ui`) enable it.
- **`std`** — enables `alloc` plus the standard library, for the
  authoring-time tools (`gatt-extract`, `study-ui`). dev-bench firmware never
  enables this.
- **`ffi`** — enables the `ffi` module's `extern "C"` functions. Only dev-bench
  firmware's build turns this on; today this is a minimal, representative
  slice of the eventual surface (see `ffi.rs`'s module docs) — it locks in the
  calling convention, not the full set of functions real dev-bench firmware
  will eventually need.
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
`cargo test`/`cargo build` (no features) exercises the actual `#![no_std]`,
no-allocator path every consumer besides the host crates compiles.

## Status

Types, wire format, and CRC/CSV tooling are implemented and tested against a
stand-in host target (decision 3's accepted posture — no real
`embarch-dev-bench` hardware exists yet). The nRF54 cross-compilation
toolchain and `cbindgen` header generation needed to actually link this crate
into dev-bench firmware remain open, blocked on that hardware existing
(see open.md).
