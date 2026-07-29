# embarch-study-designer

Shared `no_std` Rust data types — and the narrow set of tools to work with them
identically everywhere — for [EmbArch](https://github.com/gabrieltetar/embarch-doc)
hardware-in-the-loop studies. Compiled independently by `embarch-api`,
`embarch-core`, and `embarch-dev-bench` firmware, so a `Study` crossing between
them can't drift into three independently-maintained, slowly-diverging
definitions.

This crate is a mechanical translation of
[`embarch-doc`'s `embarch-study-designer/design.md`](https://github.com/gabrieltetar/embarch-doc/blob/main/embarch-study-designer/design.md)
— that document is the durable architecture record; this repo just implements it.
Doc comments throughout `src/` cite it by section (`§4.1`, `§3 decision 17`, etc.).

## Layout

| Module | Contents |
|---|---|
| `study` | `Study`, `Step`, `Action`, `PowerSampleWindow` (design.md §4.1-§4.4) |
| `result` | `StudyResult`, `StepResult`, `Outcome` (§4.5) |
| `validation` | Post-hoc validation types: `PostHocValidation`, `SignalCheck`, `ContentValidity`, etc. (§4.6) |
| `sample` | `Sample`, the shared power/waveform CSV row record (§4.7) |
| `protocol` | `DevBenchMessage`, the Core<->dev-bench serial wire protocol (§3 decisions 10, 12, 20) |
| `crc` | `steps_crc`, the CRC-32 integrity seal over `Study.steps` (§3 decision 17) |
| `schema_version` | `STUDY_DESIGNER_SCHEMA_VERSION` (§3 decision 12) |
| `limits` | Fixed-capacity bounds for every `heapless` collection (§3 decision 15) |
| `ids` | `Uuid`/`BleAddress` newtypes |
| `signal` (feature `core-validation`) | `SignalCheck` evaluation logic (§3 decision 19) |
| `ffi` (feature `ffi`) | `extern "C"` surface for dev-bench firmware (§3 decisions 7, 23) |

## Features

- **default** — `#![no_std]`, no allocator, no floating-point-heavy code. What
  `embarch-api` and dev-bench firmware link.
- **`core-validation`** — enables `std` and the `signal` module's `SignalCheck`
  evaluation logic. Only `embarch-core`'s build turns this on.
- **`ffi`** — enables the `ffi` module's `extern "C"` functions. Only dev-bench
  firmware's build turns this on; today this is a minimal, representative
  slice of the eventual surface (see `ffi.rs`'s module docs) — it locks in the
  calling convention, not the full set of functions real dev-bench firmware
  will eventually need.

Run the full test suite with `cargo test --features core-validation,ffi`; a
plain `cargo test`/`cargo build` (no features) exercises the actual
`#![no_std]`, no-allocator path every consumer besides `embarch-core` compiles.

## Status

Types, wire format, and CRC/CSV tooling are implemented and tested against a
stand-in host target (design.md §3 decision 3's accepted posture — no real
`embarch-dev-bench` hardware exists yet). The nRF54 cross-compilation
toolchain and `cbindgen` header generation needed to actually link this crate
into dev-bench firmware remain open, blocked on that hardware existing
(design.md §7).
