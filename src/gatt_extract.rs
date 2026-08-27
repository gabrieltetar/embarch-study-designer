//! Static GATT-config extraction — design.md §3 decisions 33, 56, 57.
//!
//! Answers "what UUIDs does this DUT firmware actually expose" from source,
//! ahead of ever connecting to real hardware — distinct from the live
//! `Action::GattDiscover`/`Action::GattMonitorAll` (§3 decisions 31/32),
//! which answer the same question over an actual BLE connection. `std`-only
//! (needs filesystem access via `std::path::Path`, and `regex` for the
//! text-scan) — gated behind the `gatt-extract` feature, never linked by
//! dev-bench firmware or embarch-core/embarch-api's plain Cargo-dependency
//! use (design.md §2's scope note).
//!
//! # What gets scanned (§3 decision 57)
//!
//! Every `.c`/`.h` file in the firmware repo, not a hardcoded list of two.
//! This module used to name `lib/ble/ble_def.h` and `lib/ble/ble.c`
//! outright, and had therefore been missing a third of the reference DUT's
//! GATT table — `lib/bds/bds.c`'s `sensor_bds` service and its three
//! characteristics — silently, for as long as it had existed. A bounded
//! read of two files cannot report that it is incomplete; a walk can.
//!
//! The walk honors the firmware repo's **own** `.gitignore` (and `.ignore`,
//! `.git/info/exclude`, the user's global gitignore) and skips hidden
//! directories, rather than carrying a skip list this crate would have to
//! guess and maintain per project. That is not tidiness: on the reference
//! DUT a naive `**/*.{c,h}` glob reads 1663 files and finds
//! `BT_GATT_SERVICE_DEFINE` **six** times, because `.claude/worktrees/`
//! holds two whole extra copies of the repo — and six services fit under
//! [`MAX_DISCOVERED_SERVICES`] with room to spare, so it would have emitted
//! three duplicated services without a word. Honoring the repo's ignore
//! files reads 218 files and finds the three real ones.
//!
//! [`SCAN_BLOCKED_DIR_NAMES`] is pruned unconditionally on top of that, for
//! a directory this suite itself plants inside a firmware repo and cannot
//! rely on that repo having ignored.
//!
//! # Failing loudly, at the point of use
//!
//! Text-scan, not a full C parser (embarch-study-designer/milestone-9.md
//! §3.6): this deliberately doesn't evaluate preprocessor conditionals (e.g.
//! `IF_ENABLED(CONFIG_AIR_TEMP_ENABLE, (...))` in `reference-dut-fw`'s
//! `ble_def.h`) — a characteristic wrapped in one is reported regardless of
//! whether that Kconfig option is actually set for a given build.
//!
//! It still fails loudly (a named [`ExtractError`]) on an unrecognized
//! identifier or `BT_GATT_CHRC_*` token rather than silently
//! under-extracting — the defensive posture milestone-9.md §5 calls for.
//! Decision 57 moves *where* that loudness happens: a `#define X_UUID_VAL`
//! that resolves to nothing, in a file no service definition references, is
//! now recorded as unresolvable rather than aborting the extraction, and
//! only becomes an error if a `BT_GATT_SERVICE_DEFINE` actually reaches for
//! it. Under a two-file read every declaration scanned was a declaration in
//! use; under a repo-wide walk that stopped being true, and a malformed
//! macro in some unrelated corner must not be able to blank the GATT table.
//!
//! Two failure modes decision 57 adds outright, both of which a repo-wide
//! walk creates and a two-file read could not have:
//! [`ExtractError::DuplicateService`] (two definitions resolving to one
//! service UUID — the `.claude/worktrees/` case, caught even if some future
//! repo has it tracked rather than ignored) and
//! [`ExtractError::AmbiguousSymbol`] (one name defined with two different
//! values in two files, resolved by a coin flip on walk order otherwise).
//!
//! # Reporting what was scanned
//!
//! [`ExtractedGatt::scan`] carries the walk's own account of itself — how
//! many files it read, which ones actually contributed, what it pruned,
//! what it could not decode. Silent under-extraction is the failure mode
//! this module exists to avoid and had been committing; a report an
//! engineer can eyeball is the part that makes "the file you expected isn't
//! in here" visible instead of inferred.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use regex::Regex;
use serde::Serialize;

use crate::gatt::{GattCharacteristicInfo, GattServiceInfo};
use crate::ids::Uuid;
use crate::limits::MAX_DISCOVERED_SERVICES;

/// Directory names pruned from the walk no matter what the firmware repo's
/// ignore files say (§3 decision 57).
///
/// `embarch/` is this suite's own per-engineer build/flash directory, which
/// `embarch-core` plants *inside* the firmware repo being worked on. On the
/// reference DUT it holds 917 `.c`/`.h` files of Zephyr build output and is
/// gitignored — but it is EmbArch that put it there, so EmbArch does not get
/// to depend on the firmware repo having remembered to ignore it. The block
/// is by directory name at any depth, since where the repo owner points
/// `embarch-core` at is their call.
pub const SCAN_BLOCKED_DIR_NAMES: [&str; 1] = ["embarch"];

/// File extensions the text-scan understands. C, because
/// `BT_GATT_SERVICE_DEFINE` is a C macro; widening this to C++ is a one-line
/// change the day a firmware repo needs it, and is deliberately not done
/// speculatively.
const SCANNED_EXTENSIONS: [&str; 2] = ["c", "h"];

/// Names the specific failure rather than surfacing a raw I/O/parse error
/// (design.md §3 decisions 18/23's "name the specific failure" discipline,
/// applied here per milestone-9.md §3.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtractError {
    /// A required source file or the repo root itself didn't exist or
    /// couldn't be read.
    FileNotFound(PathBuf),
    /// The walk reached the repo root but came back with no `.c`/`.h` file
    /// at all — an extraction that would have returned an empty table for a
    /// reason that has nothing to do with the DUT's GATT (§3 decision 57).
    NoSourceFilesFound(PathBuf),
    /// A `BT_UUID_INIT_128(...)`/`BT_GATT_PRIMARY_SERVICE(...)`/
    /// `BT_GATT_CHARACTERISTIC(...)` call referenced a macro, constant, or
    /// variable this scanner never found a definition for.
    MacroNotFound(String),
    /// One name carries two different values in two scanned files, and a
    /// definition in use reached for it (§3 decision 57). Reported rather
    /// than resolved by whichever file the walk happened to read last.
    AmbiguousSymbol(String),
    /// Two `BT_GATT_SERVICE_DEFINE` blocks resolved to the same service
    /// UUID (§3 decision 57) — a vendored or duplicated copy of the source
    /// tree that the repo's ignore files didn't exclude. Reported rather
    /// than emitted twice, since a duplicated service fits happily under
    /// [`MAX_DISCOVERED_SERVICES`] and would otherwise pass unremarked.
    DuplicateService(Uuid),
    /// A `BT_UUID_128_ENCODE(...)` invocation didn't have the expected
    /// 5-argument shape, or one of its arguments wasn't a parseable hex
    /// literal or known constant.
    UnparseableUuidEncode(String),
    /// A characteristic's properties expression contained a token that
    /// isn't one of the recognized `BT_GATT_CHRC_*` macros — reported rather
    /// than silently treated as contributing zero bits.
    UnparseableProperties(String),
    /// A `BT_GATT_SERVICE_DEFINE(...)`/`BT_GATT_CHARACTERISTIC(...)` call's
    /// parentheses never balanced within the source text.
    UnbalancedCall(&'static str),
    /// More services/characteristics were extracted than `limits` allows.
    CapacityExceeded(&'static str),
}

impl core::fmt::Display for ExtractError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ExtractError::FileNotFound(p) => write!(f, "file not found: {}", p.display()),
            ExtractError::NoSourceFilesFound(p) => write!(
                f,
                "no .c/.h source files found under {} — check the repo path, and whether its \
                 ignore files exclude the source",
                p.display()
            ),
            ExtractError::MacroNotFound(name) => {
                write!(f, "macro, constant, or variable not found: {name}")
            }
            ExtractError::AmbiguousSymbol(name) => write!(
                f,
                "{name} is defined with two different values in the scanned files — \
                 exclude the duplicate copy rather than letting walk order decide"
            ),
            ExtractError::DuplicateService(uuid) => write!(
                f,
                "service {} is defined more than once in the scanned files — \
                 a duplicated or vendored copy of the source tree is being scanned",
                uuid.to_hyphenated()
            ),
            ExtractError::UnparseableUuidEncode(msg) => {
                write!(f, "unparseable BT_UUID_128_ENCODE invocation: {msg}")
            }
            ExtractError::UnparseableProperties(tok) => {
                write!(f, "unrecognized characteristic-properties token: {tok}")
            }
            ExtractError::UnbalancedCall(which) => write!(f, "unbalanced parentheses in {which}"),
            ExtractError::CapacityExceeded(which) => write!(f, "extracted data exceeds {which}"),
        }
    }
}

impl std::error::Error for ExtractError {}

/// Whether a recovered identifier named a service or a characteristic
/// (§3 decision 56, extended by decision 57).
///
/// One `symbols` list rather than two: a name lookup is keyed by UUID, and a
/// service UUID and a characteristic UUID never collide, so a consumer that
/// only wants characteristic names can ignore this field entirely. It exists
/// for the consumer that wants to *group* by service — which is the reason
/// service identifiers stopped being thrown away (embarch-ui/design.md §3
/// decision 17).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GattSymbolKind {
    Service,
    Characteristic,
}

/// The C identifier a service or characteristic was declared under, paired
/// with the UUID it resolved to — design.md §3 decision 56.
///
/// **A label, not semantics.** `sds_hrm_rrm_char_uuid` says what the
/// firmware's authors called this characteristic in their own source; it says
/// nothing about what its bytes mean, when it notifies, or what writing to it
/// does. Those remain knowledge only that repo's engineers have, supplied
/// through [`crate::registry`] and [`crate::decoder`]. This module's entire
/// claim is "the declaration you are looking at is spelled this way", which
/// is a fact about the text it just scanned rather than an inference about
/// the device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GattSymbol {
    pub uuid: Uuid,
    /// Verbatim, including whatever suffix convention the repo uses —
    /// trimming it for display is [`crate::gatt_names`]'s job, and the
    /// untrimmed form stays available so a UI can show exactly what is in
    /// the source.
    pub identifier: String,
    pub kind: GattSymbolKind,
}

/// One scanned file that actually contributed something, and what it
/// contributed (§3 decision 57).
///
/// Counts rather than the parsed values themselves: this is the report an
/// engineer reads to answer "did it look at the file I expected", not a
/// second copy of the table.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct ScannedSource {
    /// Repo-relative, so the report is comparable between machines.
    pub path: PathBuf,
    /// `#define <NAME>_UUID_VAL BT_UUID_128_ENCODE(...)` definitions.
    pub uuid_macros: usize,
    /// `static struct bt_uuid_128 <var> = BT_UUID_INIT_128(...)` definitions.
    pub uuid_vars: usize,
    /// `BT_GATT_SERVICE_DEFINE(...)` blocks.
    pub services: usize,
}

/// The walk's account of itself (§3 decision 57) — what an engineer looks at
/// to see that a file they expected was actually read, rather than assuming
/// it from a table that came back non-empty.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct ScanReport {
    /// The repo root the walk started from.
    pub root: PathBuf,
    /// How many `.c`/`.h` files were read in total, including the ones that
    /// had nothing to contribute.
    pub files_read: usize,
    /// Only the files that contributed, sorted by path — on the reference
    /// DUT, three of two hundred and eighteen.
    pub sources: Vec<ScannedSource>,
    /// Directories pruned by [`SCAN_BLOCKED_DIR_NAMES`], repo-relative.
    /// Listed rather than assumed: a hard block is exactly the kind of rule
    /// that is invisible until it excludes something it shouldn't have.
    pub blocked_dirs: Vec<PathBuf>,
    /// Files with a scanned extension that weren't valid UTF-8, repo-relative.
    /// Skipping them is right; skipping them quietly is not.
    pub unreadable: Vec<PathBuf>,
}

/// What an extraction produced: the wire-shaped GATT table, plus the source
/// identifiers behind it (§3 decision 56) and the walk's own account of
/// itself (§3 decision 57).
///
/// `services`/`symbols` are two fields rather than a name field on
/// [`GattCharacteristicInfo`]: that type is the `no_std`, `heapless`,
/// wire-comparable shape a *live* `GattDiscover` fills in from an ATT
/// response, and an ATT response carries no names. Hanging a source-only
/// field off it would put a field on the wire that hardware can never
/// populate, and break the byte-for-byte comparability between a static
/// extraction and a live discovery that decision 33 exists to provide.
// No `Eq`: `GattServiceInfo` is only `PartialEq`, matching every other
// wire type in this crate.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExtractedGatt {
    /// Services in scan order: sorted repo-relative path, then source order
    /// within a file.
    ///
    /// **A stable order, not a claim about handle order.** Within one file,
    /// source order was a fair proxy for the order a live `GattDiscover`
    /// walks the table. Across files it is not: which
    /// `BT_GATT_SERVICE_DEFINE` gets the lower ATT handle is decided by the
    /// linker's section ordering, which is a build fact this scanner cannot
    /// read out of source and does not attempt to guess. A caller comparing
    /// a static extraction against a live discovery (§3 decision 33) should
    /// compare them as sets.
    pub services: heapless::Vec<GattServiceInfo, MAX_DISCOVERED_SERVICES>,
    /// One entry per service and characteristic whose declaring identifier
    /// was recovered, in the same order. Not necessarily one per entry in
    /// `services`: an extractor that can't recover names leaves this empty,
    /// and every consumer treats a missing name as "unnamed" rather than as
    /// an error.
    pub symbols: Vec<GattSymbol>,
    pub scan: ScanReport,
}

impl ExtractedGatt {
    /// Just the characteristic symbols, as `(uuid, identifier)` pairs —
    /// what [`crate::gatt_names::GattNameBook::with_symbols`] takes.
    pub fn characteristic_symbols(&self) -> impl Iterator<Item = (Uuid, String)> + '_ {
        self.symbols
            .iter()
            .filter(|s| s.kind == GattSymbolKind::Characteristic)
            .map(|s| (s.uuid, s.identifier.clone()))
    }

    /// Just the service symbols, as `(uuid, identifier)` pairs.
    pub fn service_symbols(&self) -> impl Iterator<Item = (Uuid, String)> + '_ {
        self.symbols
            .iter()
            .filter(|s| s.kind == GattSymbolKind::Service)
            .map(|s| (s.uuid, s.identifier.clone()))
    }
}

/// Extracts a DUT firmware's GATT table from its source, ahead of ever
/// connecting to real hardware (design.md §3 decision 33). Generic at the
/// trait boundary, narrow at the implementation, per the user's explicit
/// call during Milestone 3's design pass — a second firmware project's own
/// extractor is a new `impl`, not a redesign of this trait or its output
/// shape.
pub trait GattConfigExtractor {
    /// `repo_root` is the checked-out firmware repo's root directory.
    ///
    /// [`ExtractedGatt::services`] is the same [`GattServiceInfo`] shape a
    /// live `Action::GattDiscover`/`Action::GattMonitorAll` result uses, so a
    /// static extraction and a live discovery are comparable;
    /// [`ExtractedGatt::symbols`] carries what only source can know
    /// (§3 decision 56) and [`ExtractedGatt::scan`] what only the walk can
    /// (§3 decision 57).
    ///
    /// This is the required method rather than [`Self::extract`] because a
    /// single text-scan produces all three — an extractor asked for the
    /// table and then for the names would read and re-parse the same files
    /// twice to answer one request.
    fn extract_labeled(&self, repo_root: &Path) -> Result<ExtractedGatt, ExtractError>;

    /// The table alone, for a caller that has no use for the names.
    /// Provided in terms of [`Self::extract_labeled`] — every existing
    /// caller predates decision 56 and keeps compiling unchanged.
    fn extract(
        &self,
        repo_root: &Path,
    ) -> Result<heapless::Vec<GattServiceInfo, MAX_DISCOVERED_SERVICES>, ExtractError> {
        self.extract_labeled(repo_root).map(|extracted| extracted.services)
    }
}

/// Scoped to the Zephyr BLE conventions `reference-dut-fw` actually
/// uses (`..._UUID_VAL` macros expanding to `BT_UUID_128_ENCODE`,
/// `BT_UUID_INIT_128` variables, `BT_GATT_SERVICE_DEFINE` /
/// `BT_GATT_PRIMARY_SERVICE(&var)` / `BT_GATT_CHARACTERISTIC(&var.uuid,
/// PROPS, ...)` calls) — confirmed against that real source, not guessed
/// against a generic Zephyr BLE peripheral layout other projects might use
/// differently (design.md §3 decision 33).
///
/// The name is kept as-is through decision 57 even though the two files it
/// was named for are no longer hardcoded: `static_extractor = "zephyr-ble-def"`
/// is a value in real `embarch-ui` configs, and the remaining project-specific
/// assumption — that a 128-bit UUID reaches a `bt_uuid_128` through a
/// `#define <NAME>_UUID_VAL` — is real enough to keep the narrow name honest.
#[derive(Debug, Clone, Copy, Default)]
pub struct ZephyrBleDefExtractor;

impl GattConfigExtractor for ZephyrBleDefExtractor {
    fn extract_labeled(&self, repo_root: &Path) -> Result<ExtractedGatt, ExtractError> {
        let (files, mut report) = walk_sources(repo_root)?;
        if files.is_empty() {
            return Err(ExtractError::NoSourceFilesFound(repo_root.to_path_buf()));
        }
        extract_from_files(&files, &mut report)
    }
}

/// One scanned file's text, keyed by the repo-relative path the report and
/// the ordering both use.
struct SourceFile {
    path: PathBuf,
    text: String,
}

/// Every `.c`/`.h` file the firmware repo considers its own source
/// (§3 decision 57): the repo's ignore files decide, plus a hard prune of
/// [`SCAN_BLOCKED_DIR_NAMES`] and hidden directories.
///
/// `require_git(false)` so a `.gitignore` is honored in an exported tree
/// with no `.git` directory too — an extractor that quietly widened its
/// scan whenever it was pointed at a tarball would be the same silent
/// failure in a different disguise.
fn walk_sources(repo_root: &Path) -> Result<(Vec<SourceFile>, ScanReport), ExtractError> {
    if !repo_root.is_dir() {
        return Err(ExtractError::FileNotFound(repo_root.to_path_buf()));
    }

    // `filter_entry`'s closure has to be `'static`, so pruned directories
    // come back out through a handle rather than a borrow.
    let blocked: Arc<Mutex<Vec<PathBuf>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&blocked);
    let root_for_filter = repo_root.to_path_buf();

    let walker = ignore::WalkBuilder::new(repo_root)
        .hidden(true)
        .parents(true)
        .ignore(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .require_git(false)
        .follow_links(false)
        .filter_entry(move |entry| {
            let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
            if !is_dir {
                return true;
            }
            // Never the root itself: pointing the extractor at a directory
            // that happens to be named `embarch` is a deliberate act, not the
            // accident this block exists for.
            if entry.depth() == 0 {
                return true;
            }
            let name = entry.file_name().to_string_lossy();
            if !SCAN_BLOCKED_DIR_NAMES.contains(&name.as_ref()) {
                return true;
            }
            let rel = entry
                .path()
                .strip_prefix(&root_for_filter)
                .unwrap_or(entry.path())
                .to_path_buf();
            if let Ok(mut guard) = sink.lock() {
                guard.push(rel);
            }
            false
        })
        .build();

    let mut files: Vec<SourceFile> = Vec::new();
    let mut unreadable: Vec<PathBuf> = Vec::new();
    for entry in walker {
        // A directory that vanished or can't be read mid-walk is skipped
        // rather than fatal: the report's own counts are what a caller
        // checks, and one unreadable subdirectory shouldn't blank a table
        // the rest of the tree can still produce.
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let has_ext = entry
            .path()
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| SCANNED_EXTENSIONS.contains(&e));
        if !has_ext {
            continue;
        }
        let rel = entry.path().strip_prefix(repo_root).unwrap_or(entry.path()).to_path_buf();
        match std::fs::read_to_string(entry.path()) {
            Ok(text) => files.push(SourceFile { path: rel, text }),
            Err(_) => unreadable.push(rel),
        }
    }

    // Sorted so the extraction order — and therefore `ExtractedGatt`'s own
    // service order — is a property of the repo rather than of the walker's
    // directory-read order.
    files.sort_by(|a, b| a.path.cmp(&b.path));
    unreadable.sort();
    let mut blocked_dirs = Arc::try_unwrap(blocked)
        .map(|m| m.into_inner().unwrap_or_default())
        .unwrap_or_default();
    blocked_dirs.sort();
    blocked_dirs.dedup();

    let report = ScanReport {
        root: repo_root.to_path_buf(),
        files_read: files.len(),
        sources: Vec::new(),
        blocked_dirs,
        unreadable,
    };
    Ok((files, report))
}

/// A name gathered from every scanned file at once (§3 decision 57).
///
/// Defined twice with the same value is fine — a constant spelled in two
/// headers, a repo that keeps a mirror of a macro. Defined twice with
/// *different* values makes the name ambiguous, and reaching for it is an
/// [`ExtractError::AmbiguousSymbol`] rather than a coin flip on which file
/// the walk read last. The ambiguity is only reported when something
/// actually resolves the name: a repo-wide walk reads plenty of
/// declarations nothing uses, and those must not be able to fail an
/// extraction.
#[derive(Debug)]
struct RepoWide<T> {
    entries: HashMap<String, Option<T>>,
}

impl<T: PartialEq> RepoWide<T> {
    fn new() -> RepoWide<T> {
        RepoWide { entries: HashMap::new() }
    }

    fn insert(&mut self, name: String, value: T) {
        match self.entries.get_mut(&name) {
            None => {
                self.entries.insert(name, Some(value));
            }
            Some(Some(existing)) if *existing == value => {}
            Some(slot) => *slot = None,
        }
    }

    fn get(&self, name: &str) -> Result<Option<&T>, ExtractError> {
        match self.entries.get(name) {
            None => Ok(None),
            Some(Some(value)) => Ok(Some(value)),
            Some(None) => Err(ExtractError::AmbiguousSymbol(name.to_string())),
        }
    }
}

/// A UUID that may not have resolved. Held rather than raised so the failure
/// surfaces at the definition that uses it — see the module doc's "failing
/// loudly, at the point of use".
type MaybeUuidBytes = Result<[u8; 16], ExtractError>;

/// The pure, file-I/O-free core — split out so this crate's own tests can
/// exercise the text-scan against literal fixture strings without touching
/// the filesystem.
fn extract_from_files(
    files: &[SourceFile],
    report: &mut ScanReport,
) -> Result<ExtractedGatt, ExtractError> {
    // Scalars first, repo-wide: a `#define ES_BASE 0x...` in one header is
    // what a `_UUID_VAL` macro in another resolves against.
    let mut consts: RepoWide<u64> = RepoWide::new();
    for file in files {
        for (name, value) in parse_scalar_constants(&file.text) {
            consts.insert(name, value);
        }
    }

    let mut uuid_macros: RepoWide<MaybeUuidBytes> = RepoWide::new();
    let mut counts: HashMap<&Path, ScannedSource> = HashMap::new();
    for file in files {
        let entry = counts
            .entry(file.path.as_path())
            .or_insert_with(|| ScannedSource { path: file.path.clone(), ..Default::default() });
        for (name, resolved) in parse_uuid_macros(&file.text, &consts) {
            entry.uuid_macros += 1;
            uuid_macros.insert(name, resolved);
        }
    }

    // C `static`s are file-scoped, so a variable is resolved against its own
    // file first and only then repo-wide. Two files each declaring a
    // `static struct bt_uuid_128 service_uuid` is ordinary C, and must not
    // make either one resolve to the other's value.
    let mut vars_repo_wide: RepoWide<MaybeUuidBytes> = RepoWide::new();
    let mut vars_by_file: HashMap<&Path, HashMap<String, MaybeUuidBytes>> = HashMap::new();
    for file in files {
        let local = vars_by_file.entry(file.path.as_path()).or_default();
        for (var, resolved) in parse_uuid_vars(&file.text, &uuid_macros) {
            if let Some(entry) = counts.get_mut(file.path.as_path()) {
                entry.uuid_vars += 1;
            }
            local.insert(var.clone(), resolved.clone());
            vars_repo_wide.insert(var, resolved);
        }
    }

    let mut services: heapless::Vec<GattServiceInfo, MAX_DISCOVERED_SERVICES> = heapless::Vec::new();
    let mut symbols: Vec<GattSymbol> = Vec::new();
    for file in files {
        let empty = HashMap::new();
        let local = vars_by_file.get(file.path.as_path()).unwrap_or(&empty);
        let found = parse_gatt_services(
            &file.text,
            local,
            &vars_repo_wide,
            &mut services,
            &mut symbols,
        )?;
        if let Some(entry) = counts.get_mut(file.path.as_path()) {
            entry.services = found;
        }
    }

    report.sources = {
        let mut contributing: Vec<ScannedSource> = counts
            .into_values()
            .filter(|s| s.uuid_macros + s.uuid_vars + s.services > 0)
            .collect();
        contributing.sort_by(|a, b| a.path.cmp(&b.path));
        contributing
    };

    Ok(ExtractedGatt { services, symbols, scan: std::mem::take(report) })
}

/// Simple `#define NAME 0x...` scalar constants (e.g. `ES_BASE`), used to
/// resolve `BT_UUID_128_ENCODE` arguments that reference a named constant
/// rather than a literal.
fn parse_scalar_constants(src: &str) -> Vec<(String, u64)> {
    let flattened = join_backslash_continuations(src);
    let re = Regex::new(r"#define\s+(\w+)\s+0[xX]([0-9A-Fa-f]+)(?:[uUlL]*)\b").unwrap();
    re.captures_iter(&flattened)
        .filter_map(|caps| {
            u64::from_str_radix(&caps[2], 16).ok().map(|value| (caps[1].to_string(), value))
        })
        .collect()
}

/// `#define <NAME>_UUID_VAL \` `BT_UUID_128_ENCODE(w32, w1, w2, w3, w48)`
/// macros → each resolved to 16 raw bytes, big-endian (design.md §4's
/// documented `Uuid` byte order — the same order the macro's own arguments
/// are written in, `w32-w1-w2-w3-w48`).
///
/// A macro whose arguments don't resolve comes back as an `Err` value rather
/// than aborting: under a repo-wide walk it may well be a macro nothing
/// uses (§3 decision 57).
fn parse_uuid_macros(src: &str, consts: &RepoWide<u64>) -> Vec<(String, MaybeUuidBytes)> {
    let flattened = join_backslash_continuations(src);
    let re = Regex::new(r"#define\s+(\w+_UUID_VAL)\s+BT_UUID_128_ENCODE\(([^)]*)\)").unwrap();
    re.captures_iter(&flattened)
        .map(|caps| {
            let name = caps[1].to_string();
            let resolved = resolve_uuid_encode(&name, &caps[2], consts);
            (name, resolved)
        })
        .collect()
}

fn resolve_uuid_encode(name: &str, args: &str, consts: &RepoWide<u64>) -> MaybeUuidBytes {
    let args: Vec<&str> = args.split(',').map(str::trim).collect();
    if args.len() != 5 {
        return Err(ExtractError::UnparseableUuidEncode(format!(
            "{name}: expected 5 BT_UUID_128_ENCODE arguments, found {}",
            args.len()
        )));
    }
    let w32 = parse_hex_or_const(args[0], consts, name)?;
    let w1 = parse_hex_or_const(args[1], consts, name)?;
    let w2 = parse_hex_or_const(args[2], consts, name)?;
    let w3 = parse_hex_or_const(args[3], consts, name)?;
    let w48 = parse_hex_or_const(args[4], consts, name)?;

    let mut bytes = [0u8; 16];
    bytes[0..4].copy_from_slice(&(w32 as u32).to_be_bytes());
    bytes[4..6].copy_from_slice(&(w1 as u16).to_be_bytes());
    bytes[6..8].copy_from_slice(&(w2 as u16).to_be_bytes());
    bytes[8..10].copy_from_slice(&(w3 as u16).to_be_bytes());
    bytes[10..16].copy_from_slice(&w48.to_be_bytes()[2..8]);
    Ok(bytes)
}

fn parse_hex_or_const(
    token: &str,
    consts: &RepoWide<u64>,
    macro_name: &str,
) -> Result<u64, ExtractError> {
    let token = token.trim();
    if let Some(hex) = token.strip_prefix("0x").or_else(|| token.strip_prefix("0X")) {
        let hex = hex.trim_end_matches(['u', 'U', 'l', 'L']);
        u64::from_str_radix(hex, 16).map_err(|_| {
            ExtractError::UnparseableUuidEncode(format!("{macro_name}: bad hex literal {token:?}"))
        })
    } else {
        consts
            .get(token)?
            .copied()
            .ok_or_else(|| ExtractError::MacroNotFound(token.to_string()))
    }
}

/// `static struct bt_uuid_128 <var> = BT_UUID_INIT_128(<NAME>_UUID_VAL);` →
/// C variable name to that macro's already-resolved UUID bytes, or to the
/// reason it didn't resolve.
fn parse_uuid_vars(
    src: &str,
    uuid_macros: &RepoWide<MaybeUuidBytes>,
) -> Vec<(String, MaybeUuidBytes)> {
    let re = Regex::new(r"struct\s+bt_uuid_128\s+(\w+)\s*=\s*BT_UUID_INIT_128\((\w+)\)").unwrap();
    re.captures_iter(src)
        .map(|caps| {
            let var = caps[1].to_string();
            let macro_name = &caps[2];
            let resolved = match uuid_macros.get(macro_name) {
                Err(err) => Err(err),
                Ok(None) => Err(ExtractError::MacroNotFound(macro_name.to_string())),
                Ok(Some(bytes)) => bytes.clone(),
            };
            (var, resolved)
        })
        .collect()
}

/// Resolves a `&var` reference inside a service definition: the file's own
/// `static`s first, then repo-wide, then a named failure.
fn resolve_var(
    var: &str,
    local: &HashMap<String, MaybeUuidBytes>,
    repo_wide: &RepoWide<MaybeUuidBytes>,
) -> Result<[u8; 16], ExtractError> {
    if let Some(resolved) = local.get(var) {
        return resolved.clone();
    }
    match repo_wide.get(var)? {
        Some(resolved) => resolved.clone(),
        None => Err(ExtractError::MacroNotFound(var.to_string())),
    }
}

/// Every `BT_GATT_SERVICE_DEFINE(...)` block in one file, each appending one
/// [`GattServiceInfo`] (design.md §4.3a) — characteristics in source order
/// within a service. Also records the C identifier the service and each
/// characteristic were declared under (§3 decisions 56/57): both are already
/// in hand here to resolve the UUIDs at all, and both used to be dropped on
/// the floor.
///
/// Returns how many blocks this file held, for the scan report.
fn parse_gatt_services(
    src: &str,
    local_vars: &HashMap<String, MaybeUuidBytes>,
    repo_wide_vars: &RepoWide<MaybeUuidBytes>,
    services: &mut heapless::Vec<GattServiceInfo, MAX_DISCOVERED_SERVICES>,
    symbols: &mut Vec<GattSymbol>,
) -> Result<usize, ExtractError> {
    let primary_re = Regex::new(r"BT_GATT_PRIMARY_SERVICE\(\s*&(\w+)\s*\)").unwrap();
    let chrc_re =
        Regex::new(r"BT_GATT_CHARACTERISTIC\(\s*&(\w+)(?:\.uuid)?\s*,\s*([^,]+),").unwrap();

    let marker = "BT_GATT_SERVICE_DEFINE(";
    let mut search_from = 0usize;
    let mut found = 0usize;
    while let Some(rel_idx) = src[search_from..].find(marker) {
        let paren_start = search_from + rel_idx + marker.len() - 1;
        let block = extract_balanced_call(src, paren_start)
            .ok_or(ExtractError::UnbalancedCall("BT_GATT_SERVICE_DEFINE"))?;
        search_from = paren_start + block.len();
        found += 1;
        let inner = &block[1..block.len() - 1];

        let service_var = primary_re
            .captures(inner)
            .map(|c| c[1].to_string())
            .ok_or_else(|| ExtractError::MacroNotFound("BT_GATT_PRIMARY_SERVICE".to_string()))?;
        let service_uuid = resolve_var(&service_var, local_vars, repo_wide_vars)?;

        // A duplicated source tree the repo's ignore files didn't exclude
        // would otherwise emit the same service two or three times and stay
        // comfortably under the cap, which is precisely the silent
        // under/over-extraction decision 57 exists to end.
        if services.iter().any(|s| s.uuid == Uuid(service_uuid)) {
            return Err(ExtractError::DuplicateService(Uuid(service_uuid)));
        }

        let mut characteristics: heapless::Vec<
            GattCharacteristicInfo,
            { crate::limits::MAX_CHARS_PER_SERVICE },
        > = heapless::Vec::new();
        let mut block_symbols: Vec<GattSymbol> = vec![GattSymbol {
            uuid: Uuid(service_uuid),
            identifier: service_var,
            kind: GattSymbolKind::Service,
        }];
        for caps in chrc_re.captures_iter(inner) {
            let char_var = caps[1].to_string();
            let props_expr = caps[2].trim();
            let uuid = resolve_var(&char_var, local_vars, repo_wide_vars)?;

            let mut properties = 0u8;
            for token in props_expr.split('|') {
                let token = token.trim();
                properties |= chrc_property_bit(token)
                    .ok_or_else(|| ExtractError::UnparseableProperties(token.to_string()))?;
            }

            characteristics
                .push(GattCharacteristicInfo { uuid: Uuid(uuid), properties })
                .map_err(|_| ExtractError::CapacityExceeded("MAX_CHARS_PER_SERVICE"))?;
            // Uncapped `Vec`, unlike the `heapless` table beside it: symbols
            // are host-only display data that never crosses the wire, so
            // there is no buffer on the far end for them to have to fit.
            block_symbols.push(GattSymbol {
                uuid: Uuid(uuid),
                identifier: char_var,
                kind: GattSymbolKind::Characteristic,
            });
        }

        // Pushed only once the service itself fits, so a `CapacityExceeded`
        // doesn't leave names behind for a service that isn't in the table.
        services
            .push(GattServiceInfo { uuid: Uuid(service_uuid), characteristics })
            .map_err(|_| ExtractError::CapacityExceeded("MAX_DISCOVERED_SERVICES"))?;
        symbols.append(&mut block_symbols);
    }

    Ok(found)
}
/// Bluetooth Core Spec characteristic-properties bits — the same encoding
/// design.md §4.3a documents for the live `GattDiscover` path (bit 0 =
/// broadcast ... bit 7 = extended-properties). Returns `None` on an
/// unrecognized token so the caller can fail loudly rather than silently
/// contribute zero bits (milestone-9.md §5's defensive-posture call).
fn chrc_property_bit(token: &str) -> Option<u8> {
    match token {
        "BT_GATT_CHRC_BROADCAST" => Some(0x01),
        "BT_GATT_CHRC_READ" => Some(0x02),
        "BT_GATT_CHRC_WRITE_WITHOUT_RESP" => Some(0x04),
        "BT_GATT_CHRC_WRITE" => Some(0x08),
        "BT_GATT_CHRC_NOTIFY" => Some(0x10),
        "BT_GATT_CHRC_INDICATE" => Some(0x20),
        "BT_GATT_CHRC_AUTH" => Some(0x40),
        "BT_GATT_CHRC_EXT_PROP" => Some(0x80),
        _ => None,
    }
}
/// Returns the slice of `src` starting at `src[open_paren_idx] == '('`
/// through its matching close, inclusive of both parens — tracking nesting
/// depth so a call containing its own nested parenthesized arguments (as
/// every `BT_GATT_SERVICE_DEFINE` block does) is captured whole.
fn extract_balanced_call(src: &str, open_paren_idx: usize) -> Option<&str> {
    let bytes = src.as_bytes();
    if bytes.get(open_paren_idx) != Some(&b'(') {
        return None;
    }
    let mut depth = 0i32;
    let mut i = open_paren_idx;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&src[open_paren_idx..=i]);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Joins a `#define`'s backslash-newline continuation lines into one
/// logical line, so the argument-capturing regexes above don't need to
/// reason about where a multi-line macro invocation happens to wrap.
fn join_backslash_continuations(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut chars = src.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.peek() {
                Some('\n') => {
                    chars.next();
                    out.push(' ');
                    continue;
                }
                Some('\r') => {
                    chars.next();
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    out.push(' ');
                    continue;
                }
                _ => {}
            }
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // A minimal fixture mirroring reference-dut-fw's real
    // conventions (confirmed against that actual checkout,
    // embarch-study-designer/milestone-9.md §3.6) — enough to exercise every
    // parsing stage without touching the filesystem or a sibling repo this
    // crate has no dependency on.
    const FIXTURE_DEF_H: &str = r#"
#define ES_BASE  0xE58100000000ULL

#define SDS_SERVICE_UUID_VAL \
    BT_UUID_128_ENCODE(0x00000001, 0x853F, 0x4A00, 0x8000, ES_BASE)
#define SDS_HRM_RRM_CHAR_UUID_VAL \
    BT_UUID_128_ENCODE(0x00000002, 0x853F, 0x4A00, 0x8000, ES_BASE)

#define DMS_SERVICE_UUID_VAL \
    BT_UUID_128_ENCODE(0x00000010, 0x853F, 0x4A00, 0x8000, ES_BASE)
#define DMS_SENSOR_CFG_UUID_VAL \
    BT_UUID_128_ENCODE(0x00000011, 0x853F, 0x4A00, 0x8000, ES_BASE)

#define BDS_SERVICE_UUID_VAL \
    BT_UUID_128_ENCODE(0x00000020, 0x853F, 0x4A00, 0x8000, ES_BASE)
#define BDS_DATA_CHAR_UUID_VAL \
    BT_UUID_128_ENCODE(0x00000021, 0x853F, 0x4A00, 0x8000, ES_BASE)
"#;

    const FIXTURE_BLE_C: &str = r#"
static struct bt_uuid_128 sds_service_uuid      = BT_UUID_INIT_128(SDS_SERVICE_UUID_VAL);
static struct bt_uuid_128 sds_hrm_rrm_char_uuid = BT_UUID_INIT_128(SDS_HRM_RRM_CHAR_UUID_VAL);
static struct bt_uuid_128 dms_service_uuid      = BT_UUID_INIT_128(DMS_SERVICE_UUID_VAL);
static struct bt_uuid_128 dms_sensor_cfg_uuid   = BT_UUID_INIT_128(DMS_SENSOR_CFG_UUID_VAL);

BT_GATT_SERVICE_DEFINE(s11_sds,
    BT_GATT_PRIMARY_SERVICE(&sds_service_uuid),

    BT_GATT_CHARACTERISTIC(&sds_hrm_rrm_char_uuid.uuid,
                           BT_GATT_CHRC_NOTIFY,
                           BT_GATT_PERM_NONE,
                           NULL, NULL, NULL),
    BT_GATT_CCC(hrm_rrm_ccc_changed, BT_GATT_PERM_READ_ENCRYPT | BT_GATT_PERM_WRITE_ENCRYPT),
);

BT_GATT_SERVICE_DEFINE(s11_dms,
    BT_GATT_PRIMARY_SERVICE(&dms_service_uuid),

    BT_GATT_CHARACTERISTIC(&dms_sensor_cfg_uuid.uuid,
                           BT_GATT_CHRC_READ | BT_GATT_CHRC_WRITE | BT_GATT_CHRC_NOTIFY,
                           BT_GATT_PERM_READ_ENCRYPT | BT_GATT_PERM_WRITE_ENCRYPT,
                           sensor_cfg_read_handler, sensor_cfg_write_handler, NULL),
    BT_GATT_CCC(sensor_cfg_ccc_changed, BT_GATT_PERM_READ_ENCRYPT | BT_GATT_PERM_WRITE_ENCRYPT),
);
"#;

    /// The shape decision 57 exists for: a *second* `.c` file, holding its
    /// own service, whose UUID macros live in the header the old two-file
    /// read already looked at. This is `lib/bds/bds.c` in miniature — the
    /// service the extractor had been missing.
    const FIXTURE_BDS_C: &str = r#"
static struct bt_uuid_128 bds_service_uuid   = BT_UUID_INIT_128(BDS_SERVICE_UUID_VAL);
static struct bt_uuid_128 bds_data_char_uuid = BT_UUID_INIT_128(BDS_DATA_CHAR_UUID_VAL);

BT_GATT_SERVICE_DEFINE(sensor_bds,
    BT_GATT_PRIMARY_SERVICE(&bds_service_uuid),

    BT_GATT_CHARACTERISTIC(&bds_data_char_uuid.uuid,
                           BT_GATT_CHRC_NOTIFY,
                           BT_GATT_PERM_NONE,
                           NULL, NULL, NULL),
    BT_GATT_CCC(data_ccc_changed, BT_GATT_PERM_READ_ENCRYPT | BT_GATT_PERM_WRITE_ENCRYPT),
);
"#;

    /// Runs the text-scan over named in-memory files, exactly as the walk
    /// hands them over — sorted by path, so a test's expectations are the
    /// same ones a real repo produces.
    fn extract_files(files: &[(&str, &str)]) -> Result<ExtractedGatt, ExtractError> {
        let mut sources: Vec<SourceFile> = files
            .iter()
            .map(|(path, text)| SourceFile {
                path: PathBuf::from(path),
                text: (*text).to_string(),
            })
            .collect();
        sources.sort_by(|a, b| a.path.cmp(&b.path));
        let mut report = ScanReport {
            root: PathBuf::from("/fixture"),
            files_read: sources.len(),
            ..Default::default()
        };
        extract_from_files(&sources, &mut report)
    }

    fn two_file_fixture() -> Result<ExtractedGatt, ExtractError> {
        extract_files(&[("lib/ble/ble_def.h", FIXTURE_DEF_H), ("lib/ble/ble.c", FIXTURE_BLE_C)])
    }

    #[test]
    fn extracts_expected_services_and_uuids_from_fixture() {
        let services = two_file_fixture().unwrap().services;
        assert_eq!(services.len(), 2);

        let sds = &services[0];
        assert_eq!(
            sds.uuid.0,
            [0x00, 0x00, 0x00, 0x01, 0x85, 0x3F, 0x4A, 0x00, 0x80, 0x00, 0xE5, 0x81, 0x00, 0x00, 0x00, 0x00]
        );
        assert_eq!(sds.characteristics.len(), 1);
        assert_eq!(sds.characteristics[0].properties, 0x10); // NOTIFY

        let dms = &services[1];
        assert_eq!(dms.characteristics.len(), 1);
        assert_eq!(dms.characteristics[0].properties, 0x02 | 0x08 | 0x10); // READ|WRITE|NOTIFY
    }

    /// §3 decision 56: the C identifier each characteristic was declared
    /// under comes back paired with the UUID it resolved to — and the UUID
    /// it is paired with is the *same* one the table carries, so a UI
    /// looking a name up by UUID can't miss.
    #[test]
    fn extraction_recovers_the_declaring_c_identifiers() {
        let extracted = two_file_fixture().unwrap();
        let identifiers: Vec<&str> = extracted
            .symbols
            .iter()
            .filter(|s| s.kind == GattSymbolKind::Characteristic)
            .map(|s| s.identifier.as_str())
            .collect();
        assert_eq!(identifiers, ["sds_hrm_rrm_char_uuid", "dms_sensor_cfg_uuid"]);
        let chrc: Vec<Uuid> = extracted.characteristic_symbols().map(|(uuid, _)| uuid).collect();
        assert_eq!(chrc[0], extracted.services[0].characteristics[0].uuid);
        assert_eq!(chrc[1], extracted.services[1].characteristics[0].uuid);
    }

    /// §3 decision 57 extends 56 one level up: the *service*'s declaring
    /// identifier was already in hand to resolve its UUID at all, and was
    /// thrown away for exactly as long as the characteristic's was.
    #[test]
    fn extraction_recovers_the_declaring_service_identifiers() {
        let extracted = two_file_fixture().unwrap();
        let services: Vec<(String, String)> = extracted
            .service_symbols()
            .map(|(uuid, id)| (uuid.to_hyphenated().to_string(), id))
            .collect();
        assert_eq!(
            services,
            [
                ("00000001-853f-4a00-8000-e58100000000".to_string(), "sds_service_uuid".to_string()),
                ("00000010-853f-4a00-8000-e58100000000".to_string(), "dms_service_uuid".to_string()),
            ]
        );
        // A service symbol and a characteristic symbol are told apart by
        // `kind`, not by which list they came out of.
        assert_eq!(extracted.symbols.len(), 4);
    }

    /// The bug decision 57 fixes, in miniature: the service block lives in a
    /// file the old hardcoded pair never opened, and its macros in the file
    /// it did. A repo-wide scan finds all three services; the old read found
    /// two.
    #[test]
    fn a_service_defined_in_a_third_file_is_found() {
        let extracted = extract_files(&[
            ("lib/ble/ble_def.h", FIXTURE_DEF_H),
            ("lib/ble/ble.c", FIXTURE_BLE_C),
            ("lib/bds/bds.c", FIXTURE_BDS_C),
        ])
        .unwrap();
        assert_eq!(extracted.services.len(), 3);
        let names: Vec<String> = extracted.service_symbols().map(|(_, id)| id).collect();
        // Sorted repo-relative path, then source order within a file:
        // `lib/bds/bds.c` sorts before `lib/ble/ble.c`. A stable order, not
        // a claim about ATT handle order — see `ExtractedGatt::services`.
        assert_eq!(names, ["bds_service_uuid", "sds_service_uuid", "dms_service_uuid"]);
    }

    /// The scan report is the part that makes a missing file visible rather
    /// than inferred: it names the files that contributed, and a file that
    /// contributed nothing is absent rather than listed as empty.
    #[test]
    fn the_scan_report_names_what_actually_contributed() {
        let extracted = extract_files(&[
            ("lib/ble/ble_def.h", FIXTURE_DEF_H),
            ("lib/ble/ble.c", FIXTURE_BLE_C),
            ("lib/bds/bds.c", FIXTURE_BDS_C),
            ("app/main.c", "int main(void) { return 0; }"),
        ])
        .unwrap();
        assert_eq!(extracted.scan.files_read, 4);
        let paths: Vec<&str> =
            extracted.scan.sources.iter().filter_map(|s| s.path.to_str()).collect();
        assert_eq!(paths, ["lib/bds/bds.c", "lib/ble/ble.c", "lib/ble/ble_def.h"]);
        let header = &extracted.scan.sources[2];
        assert_eq!(header.uuid_macros, 6);
        assert_eq!(header.uuid_vars, 0);
        assert_eq!(header.services, 0);
        let bds = &extracted.scan.sources[0];
        assert_eq!((bds.uuid_macros, bds.uuid_vars, bds.services), (0, 2, 1));
    }

    /// A duplicated source tree the repo's ignore files didn't exclude —
    /// the `.claude/worktrees/` shape, which on the reference DUT produces
    /// six `BT_GATT_SERVICE_DEFINE` blocks for three services and would fit
    /// under the cap without a word.
    #[test]
    fn a_duplicated_source_tree_is_reported_not_emitted_twice() {
        let err = extract_files(&[
            ("lib/ble/ble_def.h", FIXTURE_DEF_H),
            ("lib/ble/ble.c", FIXTURE_BLE_C),
            ("vendored/lib/ble/ble.c", FIXTURE_BLE_C),
        ])
        .unwrap_err();
        assert!(matches!(err, ExtractError::DuplicateService(_)), "{err}");
    }

    /// A repo-wide walk reads plenty of declarations nothing uses. A
    /// malformed `_UUID_VAL` in a corner of the tree no service definition
    /// reaches for must not be able to blank the GATT table — the loudness
    /// moves to the point of use (§3 decision 57).
    #[test]
    fn an_unused_unresolvable_macro_does_not_fail_the_extraction() {
        let extracted = extract_files(&[
            ("lib/ble/ble_def.h", FIXTURE_DEF_H),
            ("lib/ble/ble.c", FIXTURE_BLE_C),
            (
                "third_party/other.h",
                "#define OTHER_THING_UUID_VAL BT_UUID_128_ENCODE(0x1, 0x2, 0x3, NOPE_BASE, 0x5)",
            ),
        ])
        .unwrap();
        assert_eq!(extracted.services.len(), 2);
    }

    /// ...but the same macro, *used*, still fails loudly.
    #[test]
    fn the_same_macro_used_by_a_service_still_fails_loudly() {
        let err = extract_files(&[
            ("lib/ble/ble_def.h", FIXTURE_DEF_H),
            ("lib/ble/ble.c", FIXTURE_BLE_C),
            (
                "lib/other/other.c",
                r#"
#define OTHER_SERVICE_UUID_VAL BT_UUID_128_ENCODE(0x1, 0x2, 0x3, NOPE_BASE, 0x5)
static struct bt_uuid_128 other_service_uuid = BT_UUID_INIT_128(OTHER_SERVICE_UUID_VAL);
BT_GATT_SERVICE_DEFINE(other, BT_GATT_PRIMARY_SERVICE(&other_service_uuid),);
"#,
            ),
        ])
        .unwrap_err();
        assert!(matches!(err, ExtractError::MacroNotFound(ref n) if n == "NOPE_BASE"), "{err}");
    }

    /// C `static`s are file-scoped, so two files each declaring their own
    /// `service_uuid` must not cross-resolve — the file's own declaration
    /// wins over the repo-wide fallback.
    #[test]
    fn a_file_scoped_static_wins_over_a_same_named_one_elsewhere() {
        let one = r#"
#define ONE_UUID_VAL BT_UUID_128_ENCODE(0x00000001, 0x0002, 0x0003, 0x0004, 0x000000000005)
static struct bt_uuid_128 service_uuid = BT_UUID_INIT_128(ONE_UUID_VAL);
BT_GATT_SERVICE_DEFINE(one, BT_GATT_PRIMARY_SERVICE(&service_uuid),);
"#;
        let two = r#"
#define TWO_UUID_VAL BT_UUID_128_ENCODE(0x000000AA, 0x0002, 0x0003, 0x0004, 0x000000000005)
static struct bt_uuid_128 service_uuid = BT_UUID_INIT_128(TWO_UUID_VAL);
BT_GATT_SERVICE_DEFINE(two, BT_GATT_PRIMARY_SERVICE(&service_uuid),);
"#;
        let extracted = extract_files(&[("a/one.c", one), ("b/two.c", two)]).unwrap();
        assert_eq!(extracted.services.len(), 2);
        assert_eq!(extracted.services[0].uuid.0[3], 0x01);
        assert_eq!(extracted.services[1].uuid.0[3], 0xAA);
    }

    /// The same name with two different values, reached for from a file that
    /// declares neither: reported rather than resolved by whichever file the
    /// walk happened to read last.
    #[test]
    fn an_ambiguous_repo_wide_name_is_reported() {
        let def_a = "#define SHARED_UUID_VAL BT_UUID_128_ENCODE(0x00000001, 0x2, 0x3, 0x4, 0x5)";
        let def_b = "#define SHARED_UUID_VAL BT_UUID_128_ENCODE(0x000000AA, 0x2, 0x3, 0x4, 0x5)";
        let user = r#"
static struct bt_uuid_128 shared_service_uuid = BT_UUID_INIT_128(SHARED_UUID_VAL);
BT_GATT_SERVICE_DEFINE(shared, BT_GATT_PRIMARY_SERVICE(&shared_service_uuid),);
"#;
        let err = extract_files(&[("a.h", def_a), ("b.h", def_b), ("c.c", user)]).unwrap_err();
        assert!(matches!(err, ExtractError::AmbiguousSymbol(ref n) if n == "SHARED_UUID_VAL"), "{err}");
    }

    /// The caps are reached, not bypassed: nine services is one more than
    /// [`MAX_DISCOVERED_SERVICES`], and a repo-wide walk is exactly what
    /// makes finding nine plausible.
    #[test]
    fn more_services_than_the_cap_is_reported() {
        let mut files: Vec<(String, String)> = Vec::new();
        for i in 0..(MAX_DISCOVERED_SERVICES + 1) {
            files.push((
                format!("lib/s{i:02}/s{i:02}.c"),
                format!(
                    r#"
#define S{i:02}_UUID_VAL BT_UUID_128_ENCODE(0x000000{i:02}, 0x0002, 0x0003, 0x0004, 0x000000000005)
static struct bt_uuid_128 s{i:02}_service_uuid = BT_UUID_INIT_128(S{i:02}_UUID_VAL);
BT_GATT_SERVICE_DEFINE(s{i:02}, BT_GATT_PRIMARY_SERVICE(&s{i:02}_service_uuid),);
"#
                ),
            ));
        }
        let borrowed: Vec<(&str, &str)> =
            files.iter().map(|(p, t)| (p.as_str(), t.as_str())).collect();
        let err = extract_files(&borrowed).unwrap_err();
        assert_eq!(err, ExtractError::CapacityExceeded("MAX_DISCOVERED_SERVICES"));
    }

    #[test]
    fn more_characteristics_than_the_cap_is_reported() {
        let mut src = String::from(
            "#define SVC_UUID_VAL BT_UUID_128_ENCODE(0x00000001, 0x2, 0x3, 0x4, 0x5)\n\
             static struct bt_uuid_128 svc_uuid = BT_UUID_INIT_128(SVC_UUID_VAL);\n",
        );
        for i in 0..(crate::limits::MAX_CHARS_PER_SERVICE + 1) {
            src.push_str(&format!(
                "#define C{i:02}_UUID_VAL BT_UUID_128_ENCODE(0x000001{i:02}, 0x2, 0x3, 0x4, 0x5)\n\
                 static struct bt_uuid_128 c{i:02}_uuid = BT_UUID_INIT_128(C{i:02}_UUID_VAL);\n"
            ));
        }
        src.push_str("BT_GATT_SERVICE_DEFINE(big, BT_GATT_PRIMARY_SERVICE(&svc_uuid),\n");
        for i in 0..(crate::limits::MAX_CHARS_PER_SERVICE + 1) {
            src.push_str(&format!(
                "  BT_GATT_CHARACTERISTIC(&c{i:02}_uuid.uuid, BT_GATT_CHRC_NOTIFY, \
                 BT_GATT_PERM_NONE, NULL, NULL, NULL),\n"
            ));
        }
        src.push_str(");\n");
        let err = extract_files(&[("lib/big/big.c", &src)]).unwrap_err();
        assert_eq!(err, ExtractError::CapacityExceeded("MAX_CHARS_PER_SERVICE"));
    }

    /// `extract` is a provided method, so a second extractor — the exact
    /// thing this trait's doc comment says a new firmware project adds —
    /// implements one method and gets the other. Asserted against a stub
    /// rather than against `ZephyrBleDefExtractor`, since the provided
    /// method is the trait's behavior, not that impl's.
    #[test]
    fn a_new_impl_only_writes_extract_labeled() {
        struct Stub;
        impl GattConfigExtractor for Stub {
            fn extract_labeled(&self, _repo_root: &Path) -> Result<ExtractedGatt, ExtractError> {
                two_file_fixture()
            }
        }
        let services = Stub.extract(Path::new("/does/not/matter")).unwrap();
        assert_eq!(services, Stub.extract_labeled(Path::new("/does/not/matter")).unwrap().services);
        assert_eq!(services.len(), 2);
    }

    #[test]
    fn a_repo_root_that_does_not_exist_reports_file_not_found() {
        let err = ZephyrBleDefExtractor.extract(Path::new("/nonexistent/repo/root")).unwrap_err();
        assert!(matches!(err, ExtractError::FileNotFound(_)), "{err}");
    }

    /// A real directory with no C in it: an empty table would be a plausible
    /// answer to the wrong question, so the walk says so instead
    /// (§3 decision 57).
    #[test]
    fn a_repo_root_with_no_c_sources_is_reported() {
        let dir = std::env::temp_dir().join(format!(
            "embarch-gatt-extract-empty-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("README.md"), "no C here").unwrap();
        let err = ZephyrBleDefExtractor.extract(&dir).unwrap_err();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(matches!(err, ExtractError::NoSourceFilesFound(_)), "{err}");
    }

    /// The walk's two exclusion rules, on a real temp tree: the repo's own
    /// `.gitignore` prunes `build/`, and `embarch/` is pruned whether or not
    /// the repo remembered to ignore it ([`SCAN_BLOCKED_DIR_NAMES`]).
    #[test]
    fn the_walk_honors_gitignore_and_hard_blocks_embarch() {
        let dir = std::env::temp_dir().join(format!(
            "embarch-gatt-extract-walk-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        for sub in ["lib/ble", "lib/bds", "build/zephyr", "embarch/build", ".hidden"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        std::fs::write(dir.join(".gitignore"), "build/\n").unwrap();
        std::fs::write(dir.join("lib/ble/ble_def.h"), FIXTURE_DEF_H).unwrap();
        std::fs::write(dir.join("lib/ble/ble.c"), FIXTURE_BLE_C).unwrap();
        std::fs::write(dir.join("lib/bds/bds.c"), FIXTURE_BDS_C).unwrap();
        // Three copies of a service the scan must never see: gitignored,
        // hard-blocked, and hidden. Any one of them reaching the parser is a
        // `DuplicateService`, so this test fails loudly rather than by count.
        std::fs::write(dir.join("build/zephyr/ble.c"), FIXTURE_BLE_C).unwrap();
        std::fs::write(dir.join("embarch/build/ble.c"), FIXTURE_BLE_C).unwrap();
        std::fs::write(dir.join(".hidden/ble.c"), FIXTURE_BLE_C).unwrap();

        let extracted = ZephyrBleDefExtractor.extract_labeled(&dir).unwrap();
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(extracted.services.len(), 3);
        assert_eq!(extracted.scan.files_read, 3);
        assert_eq!(
            extracted.scan.blocked_dirs,
            vec![PathBuf::from("embarch")],
            "the hard block reports what it pruned rather than pruning silently"
        );
    }

    #[test]
    fn unrecognized_property_token_fails_loudly() {
        let bad_c = FIXTURE_BLE_C.replace(
            "BT_GATT_CHRC_NOTIFY,\n                           BT_GATT_PERM_NONE",
            "BT_GATT_CHRC_BOGUS,\n                           BT_GATT_PERM_NONE",
        );
        let err =
            extract_files(&[("lib/ble/ble_def.h", FIXTURE_DEF_H), ("lib/ble/ble.c", &bad_c)])
                .unwrap_err();
        assert!(matches!(err, ExtractError::UnparseableProperties(_)), "{err}");
    }

    #[test]
    fn unknown_macro_reference_is_reported() {
        let bad_def = FIXTURE_DEF_H.replace("ES_BASE", "UNKNOWN_BASE");
        // Rewriting every ES_BASE occurrence also rewrites the #define
        // itself into `#define UNKNOWN_BASE 0x...` — undo that one so the
        // lookup genuinely fails to resolve `UNKNOWN_BASE`.
        let bad_def = bad_def.replacen("#define UNKNOWN_BASE", "#define ES_BASE", 1);
        let err = extract_files(&[("lib/ble/ble_def.h", &bad_def), ("lib/ble/ble.c", FIXTURE_BLE_C)])
            .unwrap_err();
        assert!(matches!(err, ExtractError::MacroNotFound(_)), "{err}");
    }
}
