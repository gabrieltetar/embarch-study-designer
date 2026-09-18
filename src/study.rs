//! `Study`/`Step`/`Action` — interfaces/types.md.
//!
//! `PowerSampleWindow` was here and is **retired** by decision
//! 39's 2026-08-25 amendment: a `StreamSource::PowerFrontEnd { sample_hz }`
//! tap scoped to a step range says the same thing, and was already the only
//! one of the two anything read.

use heapless::{String, Vec};
use serde::{Deserialize, Serialize};

use crate::bounded::Bounded;
use crate::gatt::GattTarget;
use crate::ids::{BleAddress, Uuid};
use crate::limits::{
    MAX_BUILD_EXTRA_ARGS, MAX_BUILD_EXTRA_ARG_LEN, MAX_BUILD_TARGET_FIELD_LEN,
    MAX_DECODERS_PER_STUDY, MAX_FIRMWARE_VERSION_LEN, MAX_LOCAL_NAME_LEN, MAX_MONITOR_TARGETS,
    MAX_NAME_LEN, MAX_PAYLOAD_LEN, MAX_PROTOCOLS_PER_STUDY, MAX_SERVICE_UUIDS,
    MAX_SNIPPETS_PER_BUILD, MAX_SNIPPET_NAME_LEN, MAX_STREAMS_PER_STUDY, MAX_STUDY_NAME_LEN,
};
use crate::streams::StreamTap;

/// How loud dev-bench's own firmware should be **for the duration of one
/// study** (embarch-dev-bench decision 39).
///
/// **Why this is per-study and not a build-time setting.** embarch-dev-bench
/// decision 38 turned `CONFIG_LOG` on in the bench firmware and forwarded
/// every record to Core as a `LogLine`. That made the bench's own account of
/// a run available for the first time, and it made it available *always* —
/// which is the wrong default for a link the study protocol shares: at
/// `Info` the Zephyr BT host is
/// genuinely chatty, and every 128-byte `LogLine` is ~1.3 ms of a 1 Mbaud wire
/// that a timing measurement is also using. A compile-time level forced the
/// choice to be made once, for every study, by whoever last edited `prj.conf`.
///
/// So the study says. The level a study asks for is applied by dev-bench when
/// the study starts and reverted when it ends, so the bench is quiet again
/// before the next one — see that decision for the revert rules.
///
/// [`Self::Warn`] is the default rather than [`Self::Off`], and that is a
/// deliberate asymmetry: an `<err>`/`<wrn>` line is rare by construction and
/// is *exactly* what someone wants to read about the run that just failed, so
/// paying for it on every study is worth it. `Off` exists for the study that
/// genuinely needs a clear link and is willing to be blind.
///
/// Fieldless, so postcard encodes it as a single varint discriminant. Variants
/// are appended, never reordered — the same positional-encoding rule every
/// other enum on this wire follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DevBenchLogLevel {
    /// Send nothing. Not even errors, and not even the fatal-error dump —
    /// the only setting under which a crash mid-study goes unreported.
    Off,
    /// Errors only.
    Error,
    /// Errors and warnings. The default (see above).
    #[default]
    Warn,
    /// Adds informational records, including the Zephyr BT host's own account
    /// of connecting, pairing and discovering. The level for "this study is
    /// failing and I want to know what the radio thinks."
    Info,
    /// Everything the firmware was built with. Expect this to cost real link
    /// bandwidth during BLE-heavy steps.
    Debug,
}

impl DevBenchLogLevel {
    /// Every level, quietest first — which is also the order a picker should
    /// offer them, because the axis is one-dimensional and the cost rises
    /// monotonically along it.
    ///
    /// **Served, not restated.** Same reason [`crate::decoder::ScalarType`]'s
    /// `ALL` is public and `BuiltInActionKind::label` lives on this side: a
    /// browser-side copy of a name drifts the day a variant is appended, and
    /// these variants are appended rather than reordered by rule, so a stale
    /// copy would map a label onto the wrong discriminant rather than merely
    /// omitting one.
    pub const ALL: [DevBenchLogLevel; 5] = [
        DevBenchLogLevel::Off,
        DevBenchLogLevel::Error,
        DevBenchLogLevel::Warn,
        DevBenchLogLevel::Info,
        DevBenchLogLevel::Debug,
    ];

    /// What a picker shows for this level: the variant's own JSON spelling,
    /// then what choosing it costs.
    ///
    /// The variant name is the spelling deliberately — these serialize as
    /// bare PascalCase (`"Warn"`), with **no `rename_all`**, and every saved
    /// study on disk carries that spelling. A label that renamed them in the
    /// picker would be a second vocabulary for the same five values.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Off => "Off — nothing, not even the fatal-error dump",
            Self::Error => "Error — errors only",
            Self::Warn => "Warn — errors and warnings (the default)",
            Self::Info => "Info — adds the BT host's own account of the link",
            Self::Debug => "Debug — everything the firmware was built with",
        }
    }

    /// The Zephyr severity number this maps to (`LOG_LEVEL_NONE` = 0 through
    /// `LOG_LEVEL_DBG` = 4), which is what dev-bench passes to
    /// `log_filter_set`.
    ///
    /// Kept here rather than in the firmware so both ends read the mapping
    /// from one place — it is small, but it is exactly the sort of hand-mirrored
    /// constant this project has already had go stale twice
    /// (`embarch-dev-bench/app/CMakeLists.txt`'s own comment on
    /// `STUDY_FFI_STUB_SCHEMA_VERSION`).
    pub const fn zephyr_level(self) -> u8 {
        match self {
            Self::Off => 0,
            Self::Error => 1,
            Self::Warn => 2,
            Self::Info => 3,
            Self::Debug => 4,
        }
    }
}

/// interfaces/types.md. Sealed by two sibling CRCs: `steps_crc` over `steps`
/// (decision 17) and `streams_crc` over `streams` (decision
/// 39's 2026-08-25 amendment) — see [`crate::crc`] for why there are two
/// rather than one widened one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Study {
    /// Human-readable identifier; not required to be unique.
    pub name: String<MAX_STUDY_NAME_LEN>,
    /// The builds this study is meant to run against (decision 40,
    /// interfaces/types.md). **Host-side only — never transmitted to
    /// dev-bench**, dev-bench has no use for a
    /// requirement it cannot check about itself, and `steps_crc` seals what
    /// dev-bench actually executes, which is unchanged.
    ///
    /// Mandatory, with no `#[serde(default)]` on purpose: "I don't care
    /// which build" is a real and legitimate answer — a dev-bench self-test
    /// involves no DUT at all — but it has to be *said*
    /// ([`REQUIREMENT_ANY`]), not achieved by leaving a field out, because
    /// the failure this exists to prevent is precisely the one where nobody
    /// thought about it.
    pub requires: Requirements,
    /// Run in order. Entirely static once submitted for v1.
    pub steps: crate::bounded::StepList,
    /// Declared capture channels for this study (decision 39,
    /// interfaces/taps.md) — the one generic inbound stream pipeline that replaced power,
    /// sensor-waveform, and GATT-transcript capture as three separate ones.
    ///
    /// Unlike `requires`, this **does** cross the wire to
    /// dev-bench, on `DevBenchMessage::StudyStart`: four of the five
    /// [`StreamSource`](crate::streams::StreamSource) variants are
    /// dev-bench-mediated, so dev-bench has to know which taps to open and
    /// which `id` each one answers to.
    ///
    /// `#[serde(default)]` so a saved study (decision 38)
    /// authored before taps existed still loads — as a study that captures
    /// nothing, which is exactly what it did.
    #[serde(default)]
    pub streams: Vec<StreamTap, MAX_STREAMS_PER_STUDY>,
    /// CRC-32 over `steps` (decision 17), computed by whoever
    /// submits this `Study` via [`crate::crc::steps_crc`].
    pub steps_crc: u32,
    /// CRC-32 over `streams` (decision 39's 2026-08-25
    /// amendment), computed by the same submitter via
    /// [`crate::crc::streams_crc`]. A **sibling** of `steps_crc`, not a
    /// widening of it: `steps_crc`'s own definition is unchanged, and each
    /// seal is checked independently at both hops, so a mismatch says which
    /// half is corrupt.
    ///
    /// `#[serde(default)]` — and, unlike `requires`, that default is
    /// *correct* rather than merely permissive: a saved study (decision 38)
    /// authored before taps existed has no `streams`, and `0`
    /// is the genuine CRC-32/ISO-HDLC of zero bytes, not a sentinel standing
    /// in for one. Every submitter recomputes and overwrites it anyway
    /// (embarch-api decisions 27/28).
    #[serde(default)]
    pub streams_crc: u32,
    /// `.eap` protocol manifests resolved into this study at build time
    /// (decision 58, interfaces/eap.md), reachable only through an
    /// [`Action::RunProtocol`] step.
    ///
    /// **Resolved, not referenced** — the posture decision 52 settled for
    /// payload layouts, adopted here for the same reason and not by analogy:
    /// Core cannot read the firmware repo, so a study naming an `.eap` file
    /// rather than carrying it would run on its author's machine and nowhere
    /// else, and would run *differently* after an unrelated edit to that
    /// file. The draft this decision came from bound the manifest by a CRC
    /// instead; that is the write-ahead staleness pattern
    /// embarch-topology decision 3 exists to eliminate, and
    /// the one `StreamEncoding::OutpostTrace` was corrected for
    /// (`embarch-decision-reversals.md` row 37).
    ///
    /// **Where this parts company with `decoders`, and why:** `Study.decoders`
    /// is host-only and unsealed, because a layout only decides how the host
    /// *renders* a captured byte. A protocol decides what dev-bench
    /// *executes*, so it crosses the wire like `steps` — and therefore gets
    /// a seal like `steps`, [`Study::protocols_crc`].
    #[serde(default)]
    pub protocols: crate::bounded::Bounded<crate::eap::ProtocolDef, MAX_PROTOCOLS_PER_STUDY>,
    /// CRC-32 over `protocols` (decision 58) — the study's
    /// third seal, computed via [`crate::crc::protocols_crc`].
    ///
    /// A **sibling** of `steps_crc`/`streams_crc` rather than a widening of
    /// either, for the structural reason decision 17 already settled: each
    /// seal is carried immediately after the one contiguous
    /// span it covers, so dev-bench's hand-written C digests one run of
    /// bytes per seal and a mismatch names which of the three is corrupt.
    ///
    /// `#[serde(default)]` — and correct rather than permissive, as with
    /// `streams_crc`: a study authored before protocols existed has none,
    /// and `0` is the genuine CRC-32/ISO-HDLC of zero bytes.
    #[serde(default)]
    pub protocols_crc: u32,
    /// How loud dev-bench's firmware should be while this study runs
    /// (embarch-dev-bench decision 39). Crosses the wire to
    /// dev-bench on `DevBenchMessage::StudyStart`, unlike `requires`, because
    /// it is an instruction dev-bench acts on rather than a fact about the
    /// host's expectations.
    ///
    /// **Sealed by neither CRC, on purpose.** `steps_crc` covers what
    /// dev-bench executes and `streams_crc` covers what it captures; how
    /// verbose it is about doing so changes neither, and a study re-run at a
    /// louder level must stay the same study by every check that matters.
    ///
    /// `#[serde(default)]` so every study authored before this field existed
    /// still loads, at [`DevBenchLogLevel::Warn`] — which is what those
    /// studies already effectively ran at, so the default is *correct* here
    /// and not merely permissive.
    #[serde(default)]
    pub dev_bench_log_level: DevBenchLogLevel,
    /// Named payload layouts this study's `Struct`-encoded taps decode with
    /// (decision 52, [`crate::decoder`]). A tap's
    /// `StreamEncoding::Struct { decoder }` is an index into this list.
    ///
    /// **Host-side only — never transmitted to dev-bench**, the same posture
    /// as `requires` and for a stronger reason: what a payload means is
    /// precisely the knowledge decision 39 took away from dev-bench, and it
    /// captures and stamps bytes perfectly well without it. Core is the only
    /// consumer, at render time.
    ///
    /// Resolved out of the firmware repo's own `embarch/study-structs.toml`
    /// when the study is built ([`crate::registry::StructRegistry`]), so the
    /// submitted `Study` is self-contained and a saved study replays
    /// identically — Core cannot read that repo, and a study that named a
    /// layout without carrying it would render nothing on a machine that
    /// isn't the author's.
    ///
    /// **Sealed by neither CRC, deliberately**, for the same reason
    /// `dev_bench_log_level` isn't: `steps_crc` covers what dev-bench
    /// executes and `streams_crc` covers what it captures, and how the host
    /// later renders a captured byte changes neither. A re-render with a
    /// corrected layout must stay the same study.
    ///
    /// `#[serde(default)]` so every study authored before this field existed
    /// still loads, as a study that decodes nothing — which is what those
    /// studies did.
    #[serde(default)]
    pub decoders: crate::bounded::Bounded<crate::decoder::StructLayout, MAX_DECODERS_PER_STUDY>,
    /// Per-tap record framing, checked by Core against the capture after the
    /// run — see [`crate::records`].
    ///
    /// **Host-only, exactly like `requires` and `decoders`**: never
    /// transmitted to dev-bench, and sealed by neither `steps_crc` nor
    /// `streams_crc`. What a captured byte means is knowledge decision 39 took
    /// away from dev-bench, and whether the host checks a checksum afterwards
    /// changes neither what dev-bench executes nor what it captures. Naming
    /// the tap by its `id` rather than referencing this from
    /// `StreamEncoding` is what keeps it that way — a tap's encoding crosses
    /// the wire inside `StudyStart`, so pointing at this from there would
    /// make a host-side check cost a firmware reflash.
    ///
    /// `#[serde(default)]` so every study authored before this existed still
    /// loads, as a study that checks nothing — which is what those studies
    /// did, and why a 10 h drain could report a short capture as complete.
    #[serde(default)]
    pub record_checks:
        crate::bounded::Bounded<crate::records::RecordCheck, MAX_STREAMS_PER_STUDY>,
}

/// The explicit "I don't care which build" value for either
/// [`Requirements`] field (decision 40).
pub const REQUIREMENT_ANY: &str = "any";

/// The dev-bench and DUT firmware builds a `Study` is meant to run against
/// (decision 40, interfaces/types.md).
///
/// Two free-form strings, matching the *shape*
/// [`HelloAck`](crate::protocol::DevBenchMessage::HelloAck)'s
/// `firmware_version` already uses
/// (embarch-dev-bench decision 18: whatever the build embeds, typically
/// `git describe --always --dirty --abbrev=8`). Both are mandatory and
/// [`REQUIREMENT_ANY`] is an explicit legal value.
///
/// **A shared shape is not a shared subject, and this doc comment used to
/// read as though it were** (decision 74, `tasks/suite/010`). `HelloAck`'s
/// `firmware_version` is the **bench's** build; the field of that name here
/// is the **DUT's**. The one that corresponds to `HelloAck`'s is
/// [`Requirements::dev_bench_version`] — and `embarch-core` and
/// `embarch-api` both do that mapping by hand, assigning
/// `hello.firmware_version` into `dev_bench_version`. Copying `HelloAck`'s
/// value into `firmware_version` instead is the mistake the name invites,
/// and it is silent: the DUT requirement is only compared when the run
/// supplies a `flashed_firmware_version`, so in the normal no-reflash case
/// a wrong value is accepted and recorded as `Declared`.
///
/// **The verification asymmetry is real and cannot be designed away.**
/// dev-bench self-reports its version over `HelloAck`, so a dev-bench
/// requirement is genuinely *checked*. The DUT reports nothing at all —
/// Core flashes it through a debug probe with no readback path — so a
/// `firmware_version` requirement is only verifiable when the outpost is
/// compiled in or the run just flashed it. That is what
/// [`Provenance`](crate::result::Provenance)'s source fields exist to record
/// rather than paper over.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Requirements {
    pub dev_bench_version: String<MAX_FIRMWARE_VERSION_LEN>,
    pub firmware_version: String<MAX_FIRMWARE_VERSION_LEN>,
    /// The DUT firmware this study builds and flashes for itself before it
    /// runs, if it does (`embarch-ui` decision 11, reversed).
    ///
    /// **A declaration, resolved every time, never a pinned artifact.**
    /// What is stored is the *selection* — board, app, snippets — not an
    /// identifier for a build directory somebody produced once. A pinned
    /// build id would be the write-ahead staleness pattern
    /// `embarch-topology` decision 3 exists to eliminate: a study would
    /// carry a claim about a directory that a later `west build`, a moved
    /// tree, or a pruned build root can quietly falsify, and nothing would
    /// notice until the wrong image was on the board.
    ///
    /// **It does not name a project**, and that is what keeps a study
    /// portable. Which configured `[[projects]]` entry is the DUT is a
    /// property of the bench the run happens on — `embarch-api`'s
    /// `reflash::dut_project` already takes it per-run for the same reason
    /// — so a study carries the target selection and the run supplies the
    /// repo it is selected within.
    ///
    /// `None`, the default, is every study that existed before this field:
    /// the DUT is whatever somebody already flashed, which is what those
    /// studies always did.
    #[serde(default)]
    pub build: Option<BuildSpec>,
    /// Which outpost trace mode this study needs the DUT to be running in,
    /// checked against the header frame's `flags` byte before step 1.
    ///
    /// **This is the readback path the doc comment above says does not
    /// exist.** The asymmetry it describes — dev-bench self-reports, the
    /// DUT reports nothing — is true of a DUT with no outpost compiled in.
    /// One that *has* an outpost puts a header frame on the wire carrying
    /// both a `build_id` and this flags byte, so a study can verify the
    /// firmware *and* its mode in a single pre-flight read, and
    /// `firmware_version` stops being unverifiable for that class of run.
    ///
    /// `None` is every study that does not care, which includes every
    /// study that existed before this field.
    #[serde(default)]
    pub outpost: Option<OutpostModeRequirement>,
}

/// The DUT firmware a study builds for itself (`Requirements.build`).
///
/// Field-for-field the narrowing selection `embarch-firmware-build`'s
/// `resolve::Selection` already takes, in owned, bounded form so it can sit
/// inside a `Study`. Keeping the shapes identical is deliberate: the
/// resolver is the one thing that knows what a selection means, and a study
/// that carried some other set of axes would need a translation layer whose
/// only job would be to lose information.
///
/// **Snippets are an ordered list, not a set** — see
/// [`BuildSpec::snippets`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildSpec {
    /// Each `None` means "don't narrow on this axis", which the resolver
    /// then fills from the project's own `default_target` — the same three
    /// states an omitted call-time parameter already has. An empty string
    /// is not that; it is refused by [`BuildSpec::validate`].
    #[serde(default)]
    pub board: Option<String<MAX_BUILD_TARGET_FIELD_LEN>>,
    #[serde(default)]
    pub variant: Option<String<MAX_BUILD_TARGET_FIELD_LEN>>,
    #[serde(default)]
    pub revision: Option<String<MAX_BUILD_TARGET_FIELD_LEN>>,
    #[serde(default)]
    pub app: Option<String<MAX_BUILD_TARGET_FIELD_LEN>>,
    /// The `-S` flags, **in the order west will apply them**.
    ///
    /// `embarch-decision-reversals.md` row 109 is why the order is stored
    /// rather than normalised: the BLE-shell snippet re-points the shell
    /// backend and switches the traced UART off, so the outpost overlay has
    /// to come second or the tracer loses its own UART. Two orderings of
    /// the same names are two different images, and the resolver treats
    /// them that way down to the build directory.
    ///
    /// Empty means "take the project's configured `default_snippets`", not
    /// "build with none" — the reserved literal `"none"` alone is how a
    /// study says none over a configured default (`embarch-api` decision
    /// 21). Both readings are legitimate, which is why there is a literal
    /// instead of an empty list standing in for one.
    #[serde(default)]
    pub snippets: Vec<String<MAX_SNIPPET_NAME_LEN>, MAX_SNIPPETS_PER_BUILD>,
    /// Opaque `west build` passthrough flags, also in order. Same
    /// empty-means-the-configured-default rule as `snippets`.
    #[serde(default)]
    pub extra_args: Vec<String<MAX_BUILD_EXTRA_ARG_LEN>, MAX_BUILD_EXTRA_ARGS>,
}

impl BuildSpec {
    /// A blank axis, a blank snippet name or a blank flag is the
    /// nobody-filled-this-in case, refused here for the same reason a blank
    /// `firmware_version` is: it is not the same statement as leaving the
    /// field out, and accepting it would make the two indistinguishable.
    ///
    /// **What this does not check is anything that needs the repo.**
    /// Whether a board exists, whether an app declares a snippet by that
    /// name, whether the composition builds at all — those are the
    /// resolver's, against a live scan, and restating any of them here
    /// would be a second copy that goes stale the moment the tree does.
    pub fn validate(&self) -> Result<(), RequirementsError> {
        for axis in [&self.board, &self.variant, &self.revision, &self.app] {
            if axis.as_ref().is_some_and(|v| v.trim().is_empty()) {
                return Err(RequirementsError::BlankBuildAxis);
            }
        }
        if self.snippets.iter().any(|s| s.trim().is_empty()) {
            return Err(RequirementsError::BlankSnippetName);
        }
        if self.extra_args.iter().any(|a| a.trim().is_empty()) {
            return Err(RequirementsError::BlankBuildArg);
        }
        // The reserved literal means "no snippets"; mixed with real names
        // it means neither thing clearly. `embarch-api` decision 21 refuses
        // that at resolve time, and refusing it here too is what stops an
        // authoring UI from saving a study that can only ever fail at the
        // moment the build starts.
        if self.snippets.len() > 1 && self.snippets.iter().any(|s| s == NO_SNIPPETS) {
            return Err(RequirementsError::MixedNoSnippetsLiteral);
        }
        Ok(())
    }
}

/// The reserved snippet literal meaning "build with genuinely no snippets",
/// mirroring `embarch-firmware-build`'s `resolve::NO_SNIPPETS`
/// (`embarch-api` decision 21).
///
/// **Mirrored rather than imported, because the dependency direction does
/// not exist**: this crate is `no_std` and sits *below* the build machinery
/// — `embarch-firmware-build` depends on nothing here and nothing here can
/// depend on it. What that buys is that an authoring UI refuses the
/// ambiguous list at save time instead of at build time; what it costs is a
/// second copy of one word, named here so the next reader can check it.
pub const NO_SNIPPETS: &str = "none";

/// Which outpost trace mode a study needs the DUT to be in
/// (`Requirements.outpost`), as two masks over
/// [`crate::outpost::OutpostHeader::flags`].
///
/// **Two masks rather than one, because a flag's clear state can be the
/// requirement.** `TRACE_SELF` is the standing example and its own doc
/// comment says so: clear means the trace deliberately omits the outpost's
/// own drain thread and UART interrupt, so a study that reasons about
/// unaccounted-for intervals needs it *off*, and a single "required" mask
/// could not ask for that.
///
/// A bit named in neither mask is genuinely not cared about — the third
/// state, and the common one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutpostModeRequirement {
    /// Bits that must be **set** in the header's flags byte.
    #[serde(default)]
    pub required_set: u8,
    /// Bits that must be **clear**.
    #[serde(default)]
    pub required_clear: u8,
}

impl OutpostModeRequirement {
    /// Whether a header's flags byte satisfies this requirement.
    ///
    /// Lives here, beside the declaration, for the same reason
    /// [`requirement_satisfied`] does: Core's pre-flight holds no
    /// independent copy of the comparison rule.
    pub const fn satisfied_by(&self, flags: u8) -> bool {
        (flags & self.required_set) == self.required_set && (flags & self.required_clear) == 0
    }

    /// Which required-set bits are missing from `flags`, and which
    /// required-clear bits are present — the two halves a refusal names, so
    /// the message says what is wrong rather than only that something is.
    pub const fn unmet(&self, flags: u8) -> (u8, u8) {
        (self.required_set & !flags, self.required_clear & flags)
    }

    /// A bit cannot be required both set and clear. That is not a firmware
    /// that will never be built — it is a declaration with no satisfying
    /// firmware at all, which is a typo, and it is caught here rather than
    /// after a rebuild and a reset.
    pub fn validate(&self) -> Result<(), RequirementsError> {
        if self.required_set & self.required_clear != 0 {
            return Err(RequirementsError::ContradictoryOutpostFlags);
        }
        Ok(())
    }

    /// True when this requirement says nothing at all. An empty requirement
    /// is legal and harmless, but it is also indistinguishable in effect
    /// from `None`, so a caller checking "does this study need an outpost"
    /// has to ask this rather than just whether the field is present.
    pub const fn is_empty(&self) -> bool {
        self.required_set == 0 && self.required_clear == 0
    }
}

impl Requirements {
    /// Both fields explicitly [`REQUIREMENT_ANY`] — a dev-bench self-test
    /// with no DUT involved, said out loud.
    pub fn any() -> Self {
        let any = String::try_from(REQUIREMENT_ANY).expect("REQUIREMENT_ANY fits");
        Requirements {
            dev_bench_version: any.clone(),
            firmware_version: any,
            build: None,
            outpost: None,
        }
    }

    /// `POST /study`'s pre-flight check (decision 18): a blank
    /// requirement is the not-thought-about case decision 40 exists to
    /// reject, and is not the same thing as [`REQUIREMENT_ANY`].
    pub fn validate(&self) -> Result<(), RequirementsError> {
        if self.dev_bench_version.trim().is_empty() {
            return Err(RequirementsError::BlankDevBenchVersion);
        }
        if self.firmware_version.trim().is_empty() {
            return Err(RequirementsError::BlankFirmwareVersion);
        }
        if let Some(build) = &self.build {
            build.validate()?;
        }
        if let Some(outpost) = &self.outpost {
            outpost.validate()?;
        }
        Ok(())
    }
}

/// Whether a study's outpost mode requirement is one this study could ever
/// satisfy, given the taps it declares.
///
/// **A mode requirement with no outpost trace tap can never be met, and
/// that is a typo rather than a run that fails informatively.** The flags
/// byte arrives in the header frame of an outpost capture; a study that
/// declares no such tap opens nothing to read it from, so the pre-flight
/// has nothing to check and the requirement is a statement with no subject.
/// Refusing it at submit is the difference between "you forgot the tap" at
/// authoring time and a reset DUT timing out waiting for a frame nobody
/// asked for.
///
/// Separate from [`Requirements::validate`] because it is the only rule
/// here that needs a field outside `requires` — Core and an authoring UI
/// both call it alongside, rather than each holding their own copy of the
/// cross-check.
pub fn outpost_requirement_is_satisfiable(
    requires: &Requirements,
    streams: &[StreamTap],
) -> Result<(), RequirementsError> {
    let Some(outpost) = &requires.outpost else {
        return Ok(());
    };
    if outpost.is_empty() {
        return Ok(());
    }
    let has_trace_tap = streams
        .iter()
        .any(|tap| matches!(tap.encoding, crate::streams::StreamEncoding::OutpostTrace));
    if has_trace_tap {
        Ok(())
    } else {
        Err(RequirementsError::OutpostRequirementWithoutTrace)
    }
}

/// Whether an actual version satisfies a declared requirement — exact match,
/// or [`REQUIREMENT_ANY`]. Lives here so Core's version gate (decision 40)
/// holds no independent copy of the comparison rule.
pub fn requirement_satisfied(required: &str, actual: &str) -> bool {
    required == REQUIREMENT_ANY || required == actual
}

/// Why a `Study.requires` isn't usable (decision 40).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequirementsError {
    BlankDevBenchVersion,
    BlankFirmwareVersion,
    /// A `build` axis was present but blank.
    BlankBuildAxis,
    /// A `build.snippets` entry was blank.
    BlankSnippetName,
    /// A `build.extra_args` entry was blank.
    BlankBuildArg,
    /// `build.snippets` mixes the reserved `"none"` literal with real names
    /// (`embarch-api` decision 21).
    MixedNoSnippetsLiteral,
    /// `outpost` requires the same bit both set and clear.
    ContradictoryOutpostFlags,
    /// `outpost` declares a mode, but the study opens no outpost trace tap
    /// to read a header frame from — see
    /// [`outpost_requirement_is_satisfiable`].
    OutpostRequirementWithoutTrace,
}

impl core::fmt::Display for RequirementsError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let field = match self {
            RequirementsError::BlankDevBenchVersion => "dev_bench_version",
            RequirementsError::BlankFirmwareVersion => "firmware_version",
            // The four below are not blank-version errors and each says its
            // own thing; the shared sentence would be wrong for them.
            RequirementsError::BlankBuildAxis => {
                return write!(
                    f,
                    "requires.build names a board/variant/revision/app that is blank; omit the \
                     field to leave that axis unnarrowed, which is not the same statement as \
                     naming an empty one"
                )
            }
            RequirementsError::BlankSnippetName => {
                return write!(f, "requires.build.snippets contains a blank entry")
            }
            RequirementsError::BlankBuildArg => {
                return write!(f, "requires.build.extra_args contains a blank entry")
            }
            RequirementsError::MixedNoSnippetsLiteral => {
                return write!(
                    f,
                    "requires.build.snippets mixes the reserved literal \"{NO_SNIPPETS}\" with \
                     real snippet names, which could mean either \"build with no snippets\" or \
                     \"build with those\" — pass [\"{NO_SNIPPETS}\"] alone to force none over the \
                     project's configured default_snippets, or pass just the names you want"
                )
            }
            RequirementsError::ContradictoryOutpostFlags => {
                return write!(
                    f,
                    "requires.outpost asks for the same flag bit both set and clear, which no \
                     firmware can satisfy"
                )
            }
            RequirementsError::OutpostRequirementWithoutTrace => {
                return write!(
                    f,
                    "requires.outpost declares a trace mode, but this study opens no outpost \
                     trace tap — the mode is read from that capture's header frame, so there \
                     would be nothing to check it against"
                )
            }
        };
        write!(
            f,
            "requires.{field} is blank; state the build this study needs, or              '{REQUIREMENT_ANY}' if it genuinely doesn't matter"
        )
    }
}

#[cfg(feature = "std")]
impl std::error::Error for RequirementsError {}

/// interfaces/types.md.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Step {
    /// Label surfaced in results (`StepResult.step_name`); never used for
    /// machine correlation (decision 14 uses array position).
    pub name: String<MAX_NAME_LEN>,
    pub action: Action,
    /// Max wall-clock time dev-bench allows this step before reporting
    /// `Outcome::TimedOut`.
    pub timeout_ms: u32,
    /// `false` (default) aborts the `Study` on this step's `Fail`/`TimedOut`;
    /// `true` continues to the next step regardless. decision 13.
    #[serde(default)]
    pub continue_on_fail: bool,
    /// How long dev-bench waits *before* starting this step's action —
    /// decision 42, the "when" half of authoring a stimulus.
    ///
    /// Steps run strictly in sequence, so until this existed the only
    /// expressible timing was "immediately after the previous step
    /// finished". That is not enough to author a stimulus: letting a DUT
    /// settle after a connect, or waiting inside an open
    /// [`Action::GattMonitorStart`] window before writing so the transcript
    /// clearly separates unsolicited traffic from the response to the
    /// write, both need a delay that isn't a side effect of some other
    /// step's `timeout_ms`.
    ///
    /// Deliberately *not* folded into `timeout_ms`: this is time spent
    /// before the action starts, so it doesn't consume the action's own
    /// budget, and a step's `Outcome::TimedOut` keeps meaning "the action
    /// took too long" rather than "the delay was too long".
    ///
    /// Declared last, and encoded last, on purpose. Postcard is a
    /// field-order-sensitive format with no field names on the wire, and
    /// dev-bench hand-decodes `Step` in C (`serial_protocol.c`); appending
    /// rather than inserting means that decoder gained one trailing varint
    /// read instead of a re-shuffled sequence. Wire-format change all the
    /// same — hence the v6 bump in
    /// [`crate::schema_version`].
    #[serde(default)]
    pub delay_before_ms: u32,
}

/// interfaces/types.md. There is no post-hoc content validation anywhere in
/// this suite — decision 48 removed it outright — and no on-device
/// validation `Action` variant; the real-time `Outcome` a step reports
/// (decision 19's surviving half) is the whole of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Action {
    BleAdvertise {
        local_name: Option<String<MAX_LOCAL_NAME_LEN>>,
        service_uuids: Vec<Uuid, MAX_SERVICE_UUIDS>,
        adv_interval_ms: u16,
    },
    BleConnect {
        role: BleRole,
        /// `None` accepts/connects to whichever DUT shows up first.
        target_address: Option<BleAddress>,
        /// Connect only to an advertiser whose advertised local name equals
        /// this, exactly — decision 43.
        ///
        /// Added because "whichever DUT shows up first" is not a usable
        /// default on a real bench. Found live running roadmap Milestone 6:
        /// with both `target_address` and this unset, consecutive runs of the
        /// *same* study connected to visibly different peripherals — one run
        /// discovered a GATT table with a `0x1910` service, the next an
        /// entirely different table carrying two Apple 128-bit services —
        /// neither of them the DUT under test. Every study then failed with
        /// "service not found on DUT", which is true and completely
        /// misleading: the service wasn't on the device dev-bench happened to
        /// reach.
        ///
        /// `target_address` already existed and remains the precise filter,
        /// but it can't be authored ahead of time for a DUT that advertises
        /// a resolvable private address, and nobody knows their DUT's MAC by
        /// heart. A name is what an engineer actually knows
        /// (`CONFIG_BT_DEVICE_NAME`). Both may be set; both must then match.
        ///
        /// Matched against the advertised name only — never against the GAP
        /// Device Name characteristic (`0x2A00`), which would require
        /// connecting first, i.e. exactly what this exists to avoid.
        #[serde(default)]
        target_name: Option<String<MAX_LOCAL_NAME_LEN>>,
    },
    DataExchange {
        service_uuid: Uuid,
        characteristic_uuid: Uuid,
        operation: GattOperation,
    },
    /// Walks the connected DUT's entire GATT table via wildcard discovery
    /// (every primary service, every characteristic, each characteristic's
    /// raw ATT properties byte) rather than requiring a caller to already
    /// know a `service_uuid`/`characteristic_uuid` pair. Reports its result
    /// in `StepResult.gatt_services` (interfaces/gatt-types.md); doesn't subscribe or capture
    /// anything itself. decision 31.
    GattDiscover {},
    /// Runs the same discovery as `GattDiscover` internally, then subscribes
    /// to every characteristic whose discovered properties include Notify or
    /// Indicate, then captures every notification/indication that arrives
    /// until the step's `timeout_ms` expires. Reports both `gatt_services`
    /// and `gatt_activity` (interfaces/gatt-types.md) — self-sufficient, doesn't depend on a
    /// preceding `GattDiscover` step's result. decision 32.
    GattMonitorAll {},
    /// Opens a capture window that deliberately **outlives its own step**:
    /// runs the same wildcard discovery as `GattDiscover`, subscribes to
    /// every Notify/Indicate characteristic, and returns immediately, leaving
    /// every subscription armed. Every step that runs afterwards — including
    /// `DataExchange` writes that stimulate the DUT — has its GATT traffic
    /// recorded into the streamed transcript until a `GattMonitorStop`
    /// closes the window. decision 36.
    ///
    /// This is the one action that makes "stimulate the DUT and capture what
    /// comes back" expressible at all: `GattMonitorAll` tears its own
    /// subscriptions down when its step ends, and steps run strictly in
    /// sequence, so a write step and a `GattMonitorAll` step can never
    /// overlap.
    GattMonitorStart {},
    /// Closes the window a preceding `GattMonitorStart` opened: unsubscribes
    /// everything it armed and reports the window's own `gatt_services` plus
    /// the (capped) inline `gatt_activity` summary in this step's
    /// `StepResult`. The full, uncapped record is the streamed transcript,
    /// not this summary. A `GattMonitorStop` with no open window is a
    /// no-op `Pass`, not a `Fail` — a study that ends without one still has
    /// its window closed implicitly when the study does. decision 36.
    GattMonitorStop {},
    /// Elevates the live BLE link to at least `level`, answering the pairing
    /// prompts itself — decision 44.
    ///
    /// Until this existed a study could not ask for security at all, which
    /// made a DUT that requires an encrypted link before it will answer GATT
    /// simply un-testable: every later step failed, and the failure named
    /// the wrong thing ("service not found", "disconnected during service
    /// discovery"). The "when" half needs nothing new — `Step`'s
    /// `delay_before_ms` and `timeout_ms` already express "settle, then
    /// establish security inside this budget", which is what a DUT-side
    /// security timeout needs authoring against.
    ///
    /// **The step fails when the level actually reached is lower than
    /// `level`, and `StepResult.security_level` reports what was reached
    /// either way.** There is deliberately no separate "request it but
    /// don't insist" flag: `Step.continue_on_fail` is already exactly that
    /// knob, so a study that wants to observe an attempted elevation
    /// without aborting sets it, and a study that leaves it at its default
    /// gets the strict reading. A step named "establish L4" that passes at
    /// L2 is the silent-degradation failure this suite keeps arriving at
    /// from other directions.
    ///
    /// Reaching [`BleSecurityLevel::L4`] requires a pairing method the
    /// Bluetooth spec counts as *authenticated*, and the method selection
    /// takes **both** peers' IO capabilities — see that variant's own docs
    /// for what dev-bench does about its half and what it cannot do about
    /// the DUT's.
    BleSecurity { level: BleSecurityLevel },
    /// Drops the bond established by a preceding [`Action::BleSecurity`],
    /// inside the study — decision 50.
    ///
    /// dev-bench already clears bonds at the *end* of every study, so a
    /// second run of a study behaves like the first. This is the other
    /// half: "pair, do work, drop the bond, pair again" is a real test, and
    /// without an authorable unbond the only way to reach the second
    /// pairing was to end the study.
    ///
    /// **This drops the link.** Clearing a bond for a peer dev-bench is
    /// connected to disconnects that connection — Zephyr's `bt_unpair`
    /// does it, not dev-bench, and it is the correct behavior (a link whose
    /// keys just went away is not a link). A study that unbonds mid-run
    /// therefore needs its own [`Action::BleConnect`] afterwards, which is
    /// what "pair again" meant in the first place.
    BleUnbond {},
    /// [`Action::GattMonitorAll`], narrowed to the characteristics the study
    /// names — decision 53.
    ///
    /// Discovery still runs exactly as `GattMonitorAll`'s does, so
    /// `gatt_services` reports the whole table either way; what narrows is
    /// **what gets subscribed**. Subscribing to every notify-capable
    /// characteristic on a DUT that streams a high-rate waveform floods the
    /// serial link with traffic nobody asked for, and buries the two
    /// characteristics the study is actually about.
    ///
    /// A target naming a characteristic the DUT doesn't have, or one that is
    /// neither notify- nor indicate-capable, is reported as a `Fail` naming
    /// it — not skipped. A study that names a characteristic has said it
    /// expects one, and a silently-empty capture is the failure this whole
    /// family of decisions keeps being opened by.
    GattMonitorSelected { targets: Bounded<GattTarget, MAX_MONITOR_TARGETS> },
    /// [`Action::GattMonitorStart`], narrowed the same way
    /// [`Action::GattMonitorSelected`] narrows `GattMonitorAll` — decision 53.
    ///
    /// Closed by the same [`Action::GattMonitorStop`]: a window is a window
    /// regardless of how many characteristics it armed, and a second stop
    /// action would be two names for one thing.
    GattMonitorSelectedStart { targets: Bounded<GattTarget, MAX_MONITOR_TARGETS> },
    /// Hand the link to a declared protocol state machine for the length of
    /// this step (decision 60, interfaces/eap.md).
    ///
    /// **This is the write direction decision 39 left open**, and the
    /// thing its own rejected `StreamSend`/`StreamExpect` proposal was
    /// reaching for. That proposal was turned down as premature because
    /// nothing in the model had conditional logic, branching or multi-step
    /// state; a handshake is mostly those three. `RunProtocol` spans steps
    /// the way `GattMonitorStart`/`GattMonitorStop` (decision 36) does,
    /// but where that pair opens a time window, this one runs a machine.
    ///
    /// **Both fields are indices, not names.** `protocol` indexes
    /// [`Study::protocols`] — the same shape
    /// `StreamEncoding::Struct { decoder }` uses against `Study.decoders`
    /// (decision 52) — and `entry_state` indexes that protocol's own
    /// `states`, so one manifest can be entered at more than one point
    /// without a firmware comparing strings.
    ///
    /// **Neither index is checked by [`crate::eap::validate_protocol`]**,
    /// which never sees an `Action`: it takes a `ProtocolDef` and checks
    /// that protocol's own internal references (frame sources, `goto`
    /// targets, session variables). An earlier version of this comment
    /// claimed it range-checked both of these, which it could not have.
    /// What does check them, both before either reaches a hand-written C
    /// array subscript:
    ///
    /// - `study_builder::build_study`, which resolves both from names an
    ///   author picked and therefore cannot emit an out-of-range index in
    ///   the first place, and which additionally refuses a terminal entry
    ///   state.
    /// - Core's own pre-flight, which range-checks a submitted `Study`
    ///   however it was authored — including one written by hand.
    RunProtocol { protocol: u8, entry_state: u8 },
}

/// LE security mode 1's levels, as [`Action::BleSecurity`] asks for one and
/// [`crate::result::StepResult`] reports one — decision 44.
///
/// Numbered by the spec's own level numbers rather than renamed, so a value
/// here and a Zephyr `BT_SECURITY_L*` constant and a line in a Bluetooth
/// Core Spec table are all obviously the same thing. Append-only, like every
/// other enum that crosses the dev-bench wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BleSecurityLevel {
    /// No encryption, no authentication — a plain connection.
    ///
    /// **Authorable, and deliberately so** (decision 44): `L1`
    /// is "this DUT needs no security", *said out loud*, rather than reached
    /// by leaving the step out and hoping. It is the same distinction
    /// `REQUIREMENT_ANY` draws for [`crate::study::Requirements`] — a real
    /// answer that has to be given. Since a connected link is already at
    /// L1, a step asking for it is a `Pass` under the same
    /// already-at-or-above rule every other level uses; it is not a special
    /// case in dev-bench.
    ///
    /// It is also the level a `StepResult` most needs to be able to
    /// *report*: a link that never got encrypted is exactly the answer a
    /// study debugging a security-requiring DUT is looking for.
    L1,
    /// Encrypted with an unauthenticated key — what Just Works reaches.
    L2,
    /// Encrypted with an authenticated key.
    L3,
    /// Authenticated LE Secure Connections with a 128-bit key.
    ///
    /// **Just Works cannot reach this**, which is the whole reason
    /// dev-bench had to change rather than merely gain an action: Level 4
    /// requires an authenticated key, Just Works produces an
    /// unauthenticated one, so no `bt_conn_set_security(L4)` against the
    /// old posture could ever have succeeded. dev-bench now declares a
    /// DisplayYesNo-class IO capability and auto-confirms, which selects LE
    /// Secure Connections Numeric Comparison — authenticated, and
    /// answerable without a human.
    ///
    /// **What that L4 is, said plainly.** The MITM flag is set, the key is
    /// authenticated in the stack's own bookkeeping, and
    /// `bt_conn_get_security` reports Level 4. It provides **no real
    /// man-in-the-middle protection**, because nothing compared the numbers
    /// — dev-bench confirmed them to itself. That is the right trade for an
    /// unattended bench and it is what was asked for; it is written down
    /// here so a result reporting "L4" is never later read as evidence the
    /// link was humanly verified.
    ///
    /// **Selection takes both peers.** The spec's matrix is indexed by the
    /// local *and* remote IO capability, so a DUT presenting
    /// NoInputNoOutput forces Just Works whatever dev-bench declares, and
    /// L4 becomes unreachable against that DUT. dev-bench cannot fix that
    /// from its side and does not pretend to: the step fails, and
    /// `StepResult.security_level` says which level it actually got.
    L4,
}

impl BleSecurityLevel {
    /// The spec's own level number — `2` for `L2`, and so on. For a message
    /// or a rendered result, so no call site writes the digit itself.
    pub const fn number(self) -> u8 {
        match self {
            BleSecurityLevel::L1 => 1,
            BleSecurityLevel::L2 => 2,
            BleSecurityLevel::L3 => 3,
            BleSecurityLevel::L4 => 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BleRole {
    Central,
    Peripheral,
}

/// interfaces/types.md.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GattOperation {
    Read,
    Write {
        payload: Vec<u8, MAX_PAYLOAD_LEN>,
    },
    /// Wait for a notification, bounded independently of the step's own
    /// `timeout_ms`.
    Notify {
        timeout_ms: u32,
    },
    /// Same wait-for-a-pushed-value shape as `Notify`, over BLE's
    /// acknowledged indication mechanism instead.
    Indicate {
        timeout_ms: u32,
    },
    /// Enable notifications/indications without waiting for one.
    Subscribe,
    // `StreamCapture` was here (decision 21) and is
    // **retired** by decision 39: a continuous capture of what a
    // characteristic streams is now a declared
    // `StreamSource::GattNotify` tap (interfaces/taps.md), not a per-step action kind.
    // Removed rather than kept as a dead trailing variant so nothing can
    // author one — the schema break is already paid for by v8's
    // Hello/HelloAck handshake, and a variant nothing dispatches is the
    // silently-captures-nothing failure decision 36 was opened by.
}

// `PowerSampleWindow` was here and is **retired** by
// decision 39's 2026-08-25 amendment, along with `Step.power_sample` above.
// It carried one field, `sample_rate_hz`, naming dev-bench's power-sampling
// rate for a step-bounded window; a `StreamSource::PowerFrontEnd
// { sample_hz }` tap with a `StreamScope::Steps { from, to }` covering the
// same step (interfaces/taps.md) expresses exactly that.
//
// Retired on evidence, not on symmetry: nothing consumed it. `embarch-core`
// took a power capture's rate from the tap, dev-bench's C encoder wrote its
// `Option` byte as `None` unconditionally while its decoder read-and-
// discarded it, and `study_builder::build_study` always emitted `None`.
// Milestone 4 — the first study that would author a power capture, and one
// that has never run — now finds one way to do it rather than two.

#[cfg(test)]
mod tests {
    // `#![no_std]` is on for the default feature set, so `std` isn't in the
    // prelude even though the test harness links it. Only the JSON test
    // below needs it.
    extern crate std;

    use super::*;

    #[test]
    fn any_is_an_explicit_legal_value_for_both_requirements() {
        let requires = Requirements::any();
        assert_eq!(requires.dev_bench_version.as_str(), REQUIREMENT_ANY);
        assert_eq!(requires.firmware_version.as_str(), REQUIREMENT_ANY);
        assert_eq!(requires.validate(), Ok(()));
    }

    #[test]
    fn a_blank_requirement_is_not_the_same_thing_as_any() {
        // Decision 40's whole point: "I don't care which build" is a real
        // answer, but it has to be *said*. Blank is the nobody-thought-about-
        // it case, and it is a pre-flight failure.
        let mut requires = Requirements::any();
        requires.firmware_version = String::try_from("  ").unwrap();
        assert_eq!(requires.validate(), Err(RequirementsError::BlankFirmwareVersion));

        let mut requires = Requirements::any();
        requires.dev_bench_version = String::new();
        assert_eq!(requires.validate(), Err(RequirementsError::BlankDevBenchVersion));
    }

    #[test]
    fn requirement_matching_is_exact_or_any() {
        assert!(requirement_satisfied("any", "g1a2b3c-dirty"));
        assert!(requirement_satisfied("g1a2b3c", "g1a2b3c"));
        assert!(!requirement_satisfied("g1a2b3c", "g1a2b3c-dirty"));
        assert!(!requirement_satisfied("g1a2b3c", ""));
        // "Any" is a value, not a wildcard syntax — nothing else matches
        // loosely, because a result attributed to the wrong firmware is
        // worse than no result.
        assert!(!requirement_satisfied("g*", "g1a2b3c"));
    }

    #[test]
    fn a_study_json_without_requires_is_rejected_rather_than_defaulted() {
        // Run on a deliberately larger stack: deserializing a `Study` at all
        // needs ~75 KiB of inline `heapless` arrays plus serde's own frames,
        // and overflows libtest's default stack in a debug build. That is
        // spec.md §7's long-standing note, not something this test
        // introduces — `embarch-api` closed its own exposure the same way
        // (embarch-api decision 36).
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                // Mandatory on purpose (decision 40): omitting it must fail,
                // not silently become "any".
                let without = r#"{"name":"s","steps":[],"steps_crc":0}"#;
                assert!(serde_json::from_str::<Study>(without).is_err());

                let with = r#"{"name":"s","requires":{"dev_bench_version":"any",
                    "firmware_version":"any"},"steps":[],"steps_crc":0}"#;
                let study: Study = serde_json::from_str(with).unwrap();
                // This equality is also what pins the 2026-09-18 additions
                // as backward-compatible: `Requirements::any()` has
                // `build: None, outpost: None`, so a saved study written
                // before either field existed still parses to exactly it.
                assert_eq!(study.requires, Requirements::any());
                // `streams`, unlike `requires`, defaults: a saved study
                // authored before taps existed captured nothing, and still
                // does.
                assert!(study.streams.is_empty());
                // And so does its sibling seal — with the defaulted value
                // being the *correct* one, since 0 really is the CRC of the
                // empty tap list this study has (crate::crc::streams_crc).
                assert_eq!(study.streams_crc, 0);
                assert_eq!(crate::crc::streams_crc(&study.streams).unwrap(), 0);
            })
            .unwrap()
            .join()
            .unwrap();
    }

    fn snippets(names: &[&str]) -> Vec<String<MAX_SNIPPET_NAME_LEN>, MAX_SNIPPETS_PER_BUILD> {
        names.iter().map(|n| String::try_from(*n).unwrap()).collect()
    }

    fn spec_with(snippet_names: &[&str]) -> BuildSpec {
        BuildSpec {
            board: None,
            variant: None,
            revision: None,
            app: None,
            snippets: snippets(snippet_names),
            extra_args: Vec::new(),
        }
    }

    #[test]
    fn a_build_spec_that_narrows_nothing_is_legal() {
        // Every axis `None` means "take the project's own default_target",
        // which is the common case on a single-board bench — not an
        // under-specified study.
        let mut requires = Requirements::any();
        requires.build = Some(spec_with(&[]));
        assert_eq!(requires.validate(), Ok(()));
    }

    #[test]
    fn a_blank_build_axis_is_not_the_same_as_an_absent_one() {
        let mut spec = spec_with(&[]);
        spec.board = Some(String::try_from("   ").unwrap());
        let mut requires = Requirements::any();
        requires.build = Some(spec);
        assert_eq!(requires.validate(), Err(RequirementsError::BlankBuildAxis));
    }

    #[test]
    fn a_blank_snippet_or_flag_is_refused() {
        let mut requires = Requirements::any();
        requires.build = Some(spec_with(&[""]));
        assert_eq!(requires.validate(), Err(RequirementsError::BlankSnippetName));

        let mut spec = spec_with(&[]);
        spec.extra_args.push(String::try_from(" ").unwrap()).unwrap();
        let mut requires = Requirements::any();
        requires.build = Some(spec);
        assert_eq!(requires.validate(), Err(RequirementsError::BlankBuildArg));
    }

    /// `embarch-api` decision 21's ambiguity, refused at authoring time
    /// instead of at the moment the build starts.
    #[test]
    fn the_none_literal_mixed_with_real_names_is_refused_here_too() {
        let mut requires = Requirements::any();
        requires.build = Some(spec_with(&[NO_SNIPPETS, "ble-shell"]));
        assert_eq!(requires.validate(), Err(RequirementsError::MixedNoSnippetsLiteral));

        // Alone it is the legal way to say "no snippets over a configured
        // default", so it must not be caught by the same rule.
        let mut requires = Requirements::any();
        requires.build = Some(spec_with(&[NO_SNIPPETS]));
        assert_eq!(requires.validate(), Ok(()));
    }

    /// Two orderings of the same names are two different specs, because
    /// west applies `-S` in order (reversals row 109). Nothing in this type
    /// or its validation may normalise that away.
    #[test]
    fn snippet_order_is_part_of_the_spec() {
        let forwards = spec_with(&["ble-shell", "outpost"]);
        let backwards = spec_with(&["outpost", "ble-shell"]);
        assert_ne!(forwards, backwards);
        assert_eq!(forwards.validate(), Ok(()));
        assert_eq!(backwards.validate(), Ok(()));
    }

    #[test]
    fn an_outpost_requirement_reads_both_set_and_clear_bits() {
        use crate::outpost::HeaderFlags;
        let needs = OutpostModeRequirement {
            required_set: HeaderFlags::TRACE_THREADS | HeaderFlags::TRACE_ISRS,
            required_clear: HeaderFlags::TRACE_SELF,
        };
        assert_eq!(needs.validate(), Ok(()));

        // Exactly right.
        assert!(needs.satisfied_by(HeaderFlags::TRACE_THREADS | HeaderFlags::TRACE_ISRS));
        // More than asked for, on a bit nobody named — still satisfied, and
        // that is the third state doing its job.
        assert!(needs.satisfied_by(
            HeaderFlags::TRACE_THREADS | HeaderFlags::TRACE_ISRS | HeaderFlags::TRACE_GPIO
        ));
        // A required bit missing.
        assert!(!needs.satisfied_by(HeaderFlags::TRACE_THREADS));
        // **The clear half is the point**: this firmware traces everything
        // asked for and also itself, which is what the study said it must
        // not do.
        assert!(!needs.satisfied_by(
            HeaderFlags::TRACE_THREADS | HeaderFlags::TRACE_ISRS | HeaderFlags::TRACE_SELF
        ));
    }

    #[test]
    fn unmet_names_which_half_failed() {
        use crate::outpost::HeaderFlags;
        let needs = OutpostModeRequirement {
            required_set: HeaderFlags::TRACE_THREADS | HeaderFlags::TRACE_MARKERS,
            required_clear: HeaderFlags::TRACE_SELF,
        };
        let flags = HeaderFlags::TRACE_THREADS | HeaderFlags::TRACE_SELF;
        assert_eq!(needs.unmet(flags), (HeaderFlags::TRACE_MARKERS, HeaderFlags::TRACE_SELF));
        // Satisfied means nothing unmet, in both halves.
        assert_eq!(
            needs.unmet(HeaderFlags::TRACE_THREADS | HeaderFlags::TRACE_MARKERS),
            (0, 0)
        );
    }

    #[test]
    fn a_bit_required_both_set_and_clear_has_no_satisfying_firmware() {
        use crate::outpost::HeaderFlags;
        let mut requires = Requirements::any();
        requires.outpost = Some(OutpostModeRequirement {
            required_set: HeaderFlags::TRACE_SELF,
            required_clear: HeaderFlags::TRACE_SELF,
        });
        assert_eq!(requires.validate(), Err(RequirementsError::ContradictoryOutpostFlags));
    }

    #[test]
    fn a_mode_requirement_needs_a_trace_tap_to_be_read_from() {
        use crate::outpost::HeaderFlags;
        use crate::streams::{StreamEncoding, StreamScope, StreamSource, StreamTap};

        let mut requires = Requirements::any();
        requires.outpost = Some(OutpostModeRequirement {
            required_set: HeaderFlags::TRACE_THREADS,
            required_clear: 0,
        });

        assert_eq!(
            outpost_requirement_is_satisfiable(&requires, &[]),
            Err(RequirementsError::OutpostRequirementWithoutTrace)
        );

        let trace = StreamTap {
            id: 0,
            name: String::try_from("trace").unwrap(),
            source: StreamSource::Signal { name: String::try_from("outpost").unwrap() },
            encoding: StreamEncoding::OutpostTrace,
            scope: StreamScope::WholeStudy,
        };
        assert_eq!(
            outpost_requirement_is_satisfiable(&requires, core::slice::from_ref(&trace)),
            Ok(())
        );

        // A tap that is not a trace does not supply a header frame.
        let mut text = trace;
        text.encoding = StreamEncoding::Text;
        assert_eq!(
            outpost_requirement_is_satisfiable(&requires, &[text]),
            Err(RequirementsError::OutpostRequirementWithoutTrace)
        );

        // And a requirement that asks for nothing is not a requirement, so
        // it needs no tap.
        let mut empty = Requirements::any();
        empty.outpost = Some(OutpostModeRequirement { required_set: 0, required_clear: 0 });
        assert_eq!(outpost_requirement_is_satisfiable(&empty, &[]), Ok(()));
        assert_eq!(outpost_requirement_is_satisfiable(&Requirements::any(), &[]), Ok(()));
    }

    /// One bit-to-name table, so a refusal in Core and a picker in the UI
    /// cannot disagree about what a flag is called.
    #[test]
    fn every_header_flag_bit_is_named_exactly_once() {
        use crate::outpost::HeaderFlags;
        let mut covered = 0u8;
        for (bit, name) in HeaderFlags::NAMED {
            assert_eq!(bit.count_ones(), 1, "{name} is not a single bit");
            assert_eq!(covered & bit, 0, "{name} repeats a bit already named");
            covered |= bit;
            assert_eq!(HeaderFlags::bit(name), Some(bit));
        }
        assert_eq!(covered, 0xFF, "the table does not cover the whole flags byte");
        assert_eq!(HeaderFlags::bit("trace_everything"), None);
    }
}

#[cfg(test)]
mod log_level_vocabulary_tests {
    use super::DevBenchLogLevel;

    /// **The five levels serialize as bare PascalCase, and must keep doing
    /// so.** There is no `rename_all` behind `"Warn"` — it is the variant
    /// name — and every saved study on disk carries that spelling in its
    /// `dev_bench_log_level` field. Adding `rename_all = "snake_case"` here
    /// would be a one-line change that silently stops every one of them from
    /// loading, so the spelling is pinned rather than left to nobody
    /// noticing.
    #[test]
    fn the_json_spelling_is_bare_pascal_case() {
        for (level, spelling) in [
            (DevBenchLogLevel::Off, "Off"),
            (DevBenchLogLevel::Error, "Error"),
            (DevBenchLogLevel::Warn, "Warn"),
            (DevBenchLogLevel::Info, "Info"),
            (DevBenchLogLevel::Debug, "Debug"),
        ] {
            assert_eq!(serde_json::to_value(level).unwrap(), spelling);
        }
    }

    /// `ALL` is every variant, quietest first, and its order is the
    /// discriminant order — which is also the postcard encoding's, so a
    /// picker built from it cannot offer a label against the wrong value.
    #[test]
    fn all_is_every_level_quietest_first() {
        assert_eq!(DevBenchLogLevel::ALL.len(), 5);
        for (i, level) in DevBenchLogLevel::ALL.into_iter().enumerate() {
            assert_eq!(level.zephyr_level() as usize, i);
            assert!(level.label().starts_with(match level {
                DevBenchLogLevel::Off => "Off",
                DevBenchLogLevel::Error => "Error",
                DevBenchLogLevel::Warn => "Warn",
                DevBenchLogLevel::Info => "Info",
                DevBenchLogLevel::Debug => "Debug",
            }));
        }
    }

    /// The default is `Warn`, and a picker built from `ALL` offers it.
    #[test]
    fn warn_is_the_default_and_is_offered() {
        assert_eq!(DevBenchLogLevel::default(), DevBenchLogLevel::Warn);
        assert!(DevBenchLogLevel::ALL.contains(&DevBenchLogLevel::default()));
    }
}
