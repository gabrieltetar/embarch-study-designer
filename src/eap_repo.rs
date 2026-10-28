//! The `.eap` files of one firmware repo, on disk — the layer between
//! [`crate::eap_parse`] and an editor that has to render what is wrong with
//! them.
//!
//! `embarch/protocols/` sits beside `study-actions.toml` and
//! `study-structs.toml` for the reason [`crate::registry::struct_registry_path`]
//! gives for those two: a protocol manifest is engineer-authored knowledge
//! about *this* DUT, so it travels with the firmware repo and is shared
//! across engineers exactly as they already are. One directory rather than
//! one file because an `.eap` file is a text document an engineer edits by
//! hand, and a repo with three handshakes should have three of them.
//!
//! Three properties of this module are load-bearing, and each is the
//! opposite of what the obvious implementation would do:
//!
//! **[`scan`] never fails on a bad file.** A directory holding one
//! unparseable `.eap` still scans, with that file carrying its error and
//! every other file carrying its protocols. This is what makes an editor
//! possible at all: the reason to open one is that a file is wrong, and a
//! scan that returned `Err` for the whole directory would leave the editor
//! with nothing to show. It is the same posture `embarch-ui`'s actions
//! response already takes for a malformed `study-structs.toml` — report it
//! beside the things that did load, rather than failing the request.
//!
//! **[`RepoProtocols::defs`] is where a duplicate protocol name is
//! refused**, and it is the only place. That refusal is what lets a study
//! row name a protocol by **name alone**, with no filename: within one repo
//! a protocol name resolves to exactly one block, or the repo does not
//! resolve at all. Two files declaring the same protocol name is a real
//! situation an engineer can create by copying a file, and it is refused
//! rather than resolved by directory order.
//!
//! **[`save`] parses and resolves before it writes**, exactly as both
//! registries' `save` validate before writing. A file that cannot become a
//! `ProtocolDef` is not written, so the directory on disk never holds text
//! this crate would refuse — which is what lets [`scan`]'s per-file errors
//! mean "somebody edited this outside the tool" rather than "the tool wrote
//! it that way".

use std::fs;
use std::path::{Path, PathBuf};

use crate::eap::ProtocolDef;
use crate::eap_parse::{parse, resolve, AstProtocol, EapError, EapErrorKind, ResolvedProtocol};

/// Every error one scanned file can carry, and the only two kinds there are:
/// the grammar refused it, or the filesystem did.
///
/// Distinct from a bare [`EapError`] because an unreadable file has no line
/// to point at, and inventing line 0 for it would put an I/O message into the
/// editor's line bands. An editor renders a `FileError::Eap` against the text
/// and a `FileError::Io` above it.
pub type FileError = RepoError;

/// `<firmware-repo>/embarch/protocols` — the directory `.eap` files live in.
pub fn protocols_dir(firmware_repo_root: &Path) -> PathBuf {
    firmware_repo_root.join("embarch").join("protocols")
}

/// The path one `.eap` file lives at, given its stem (`bds` →
/// `embarch/protocols/bds.eap`).
///
/// Takes a stem rather than a filename so no caller ever writes `.eap`
/// itself, and so a stem is the only thing a route has to validate.
pub fn protocol_path(firmware_repo_root: &Path, stem: &str) -> PathBuf {
    protocols_dir(firmware_repo_root).join(format!("{stem}.eap"))
}

/// Why a stem is not usable as a filename.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoError {
    /// The stem is empty, carries a path separator or a `.`, or holds a
    /// character outside `[A-Za-z0-9_-]`.
    BadFileName { stem: String, why: &'static str },
    /// The text does not parse, or a block in it does not resolve.
    Eap(EapError),
    /// The file could not be read or written.
    Io(String),
}

impl std::fmt::Display for RepoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RepoError::BadFileName { stem, why } => {
                write!(f, "'{stem}' is not a usable protocol file name: {why}")
            }
            RepoError::Eap(e) => write!(f, "{e}"),
            RepoError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for RepoError {}

/// Refuses a stem that is not a bare, flat file name.
///
/// **Traversal is refused here and again by whatever route calls in**, on
/// purpose: this is the check that protects the filesystem, so it lives
/// beside the `join` it protects rather than only at the edge of a web
/// server that might grow a second caller.
///
/// The allowed set is deliberately narrower than the filesystem's: letters,
/// digits, `_` and `-`. A protocol file's stem is a short identifier, and
/// every character that is not one is a character that behaves differently
/// on one of the three platforms this suite runs on.
pub fn validate_stem(stem: &str) -> Result<(), RepoError> {
    let bad = |why: &'static str| Err(RepoError::BadFileName { stem: stem.to_string(), why });
    if stem.is_empty() {
        return bad("it is empty");
    }
    if stem.len() > 64 {
        return bad("it is longer than 64 characters");
    }
    if stem.contains('/') || stem.contains('\\') {
        return bad("it contains a path separator");
    }
    if stem.contains('.') {
        return bad("it contains a '.' — pass the stem, not the file name");
    }
    if !stem.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return bad("only letters, digits, '_' and '-' are allowed");
    }
    Ok(())
}

/// One `.eap` file as it is on disk right now.
#[derive(Debug, Clone)]
pub struct ProtocolFile {
    /// The file's stem — `bds` for `bds.eap`.
    pub stem: String,
    /// The file's whole text, as read. Kept so an editor can open the file
    /// it was told about rather than re-reading a path that may since have
    /// changed underneath it.
    pub text: String,
    /// `Err` when the file did not parse, or could not be read at all.
    /// **At most one parse error**: the parser stops at the first thing it
    /// cannot read, so this is never a list — see the module docs' note
    /// about not promising one.
    pub parsed: Result<Vec<AstProtocol>, FileError>,
    /// One entry per block the file parsed, in declaration order, each
    /// resolved or carrying its own single resolve error. Empty when
    /// `parsed` is `Err`.
    pub blocks: Vec<BlockOutcome>,
}

/// One `protocol <name> { … }` block, resolved or not.
#[derive(Debug, Clone)]
pub struct BlockOutcome {
    pub name: String,
    /// The line `protocol <name> {` opened on.
    pub line: u32,
    pub resolved: Result<ResolvedProtocol, EapError>,
}

impl ProtocolFile {
    /// Every block that resolved, in declaration order.
    pub fn resolved(&self) -> impl Iterator<Item = (&str, &ResolvedProtocol)> {
        self.blocks
            .iter()
            .filter_map(|b| b.resolved.as_ref().ok().map(|r| (b.name.as_str(), r)))
    }

    /// Every error this file carries: its parse error, or one per block that
    /// failed to resolve. A file that parsed and whose blocks all resolved
    /// yields none.
    pub fn errors(&self) -> Vec<FileError> {
        match &self.parsed {
            Err(e) => vec![e.clone()],
            Ok(_) => self
                .blocks
                .iter()
                .filter_map(|b| b.resolved.as_ref().err())
                .map(|e| RepoError::Eap(e.clone()))
                .collect(),
        }
    }
}

/// Every `.eap` file under one firmware repo's `embarch/protocols/`.
#[derive(Debug, Clone, Default)]
pub struct RepoProtocols {
    /// Sorted by stem, so a listing is stable across calls and across
    /// filesystems whose directory order is not.
    pub files: Vec<ProtocolFile>,
}

/// Reads every `*.eap` file under `<root>/embarch/protocols/`.
///
/// **A missing directory is an empty repo, `Ok`** — the same starting state
/// [`crate::registry::ActionRegistry::load`] treats a missing
/// `study-actions.toml` as, and for the same reason: there is no bootstrap
/// step that creates it, so "not there" is what a firmware repo that has
/// never authored a protocol looks like.
///
/// **A file that does not parse does not fail the scan.** It comes back with
/// its error attached. See the module docs.
///
/// A file that cannot be *read* — a permission error, a directory named
/// `x.eap` — is reported the same way, as a file whose text is empty and
/// whose `parsed` carries the I/O message on line 0. It is never silently
/// omitted: a file the scan cannot see is not the same as a file that is not
/// there, and an editor listing the second when the first is true would hide
/// a protocol a study already names.
pub fn scan(firmware_repo_root: &Path) -> Result<RepoProtocols, RepoError> {
    let dir = protocols_dir(firmware_repo_root);
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(RepoProtocols::default()),
        Err(e) => return Err(RepoError::Io(format!("{}: {e}", dir.display()))),
    };

    let mut files = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            // A single unreadable entry does not fail the directory, for the
            // same reason a single unparseable file does not.
            Err(_) => continue,
        };
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("eap") {
            continue;
        }
        let stem = match path.file_stem().and_then(|s| s.to_str()) {
            Some(stem) => stem.to_string(),
            None => continue,
        };
        files.push(read_one(&path, stem));
    }
    files.sort_by(|a, b| a.stem.cmp(&b.stem));
    Ok(RepoProtocols { files })
}

fn read_one(path: &Path, stem: String) -> ProtocolFile {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) => {
            return ProtocolFile {
                stem,
                text: String::new(),
                parsed: Err(RepoError::Io(format!("{}: {e}", path.display()))),
                blocks: Vec::new(),
            }
        }
    };
    let (parsed, blocks) = match parse(&text) {
        Ok(file) => {
            let blocks = file
                .protocols
                .iter()
                .map(|a| BlockOutcome { name: a.name.clone(), line: a.line, resolved: resolve(a) })
                .collect();
            (Ok(file.protocols), blocks)
        }
        Err(e) => (Err(RepoError::Eap(e)), Vec::new()),
    };
    ProtocolFile { stem, text, parsed, blocks }
}

impl RepoProtocols {
    /// Every protocol this repo defines, ready to be placed in
    /// `Study.protocols`.
    ///
    /// **This is where a duplicate protocol name is refused** — see the
    /// module docs for why that refusal is what lets a study row carry a
    /// name and no filename. The error names the protocol, not the files,
    /// because the name is what the study said and the files are what the
    /// engineer has to reconcile; both file stems are in the message.
    ///
    /// A file that did not parse, or a block that did not resolve,
    /// contributes nothing and is **not** an error here: `defs` answers
    /// "what can a study reach", and an editor that wants to say why a file
    /// is missing from that answer reads [`ProtocolFile::errors`]. A row
    /// naming a protocol that is missing for either reason is refused by
    /// `build_study`, where the row is.
    pub fn defs(&self) -> Result<Vec<ProtocolDef>, RepoError> {
        let mut defs: Vec<ProtocolDef> = Vec::new();
        let mut origin: Vec<(String, &str)> = Vec::new();
        for file in &self.files {
            for (name, resolved) in file.resolved() {
                if let Some((_, first)) = origin.iter().find(|(n, _)| n == name) {
                    return Err(RepoError::Eap(EapError {
                        line: 0,
                        kind: EapErrorKind::Duplicate {
                            what: "protocol name across files",
                            name: format!("{name} (in {first}.eap and {}.eap)", file.stem),
                        },
                    }));
                }
                origin.push((name.to_string(), file.stem.as_str()));
                defs.push(resolved.def.clone());
            }
        }
        Ok(defs)
    }

    /// The file with this stem, if the scan found one.
    pub fn file(&self, stem: &str) -> Option<&ProtocolFile> {
        self.files.iter().find(|f| f.stem == stem)
    }

    /// Protocol names declared by more than one file, each with the stems
    /// declaring it — what an editor renders repo-wide, above the files, so
    /// the situation [`defs`](Self::defs) refuses is visible before a study
    /// is built rather than only when one is.
    pub fn duplicate_names(&self) -> Vec<(String, Vec<String>)> {
        let mut by_name: Vec<(String, Vec<String>)> = Vec::new();
        for file in &self.files {
            for (name, _) in file.resolved() {
                match by_name.iter_mut().find(|(n, _)| n == name) {
                    Some((_, stems)) => stems.push(file.stem.clone()),
                    None => by_name.push((name.to_string(), vec![file.stem.clone()])),
                }
            }
        }
        by_name.retain(|(_, stems)| stems.len() > 1);
        by_name
    }
}

/// Writes one `.eap` file, **after** parsing and resolving every block in it.
///
/// Same order both registries' `save` use, and for the same reason: a file
/// this crate would refuse to read is never a file this crate wrote. On a
/// refusal nothing is written — an existing file is left byte-identical, so
/// a failed save cannot cost an engineer the working version they were
/// editing away from.
///
/// The stem is re-validated here even though a caller is expected to have
/// validated it. See [`validate_stem`].
pub fn save(firmware_repo_root: &Path, stem: &str, text: &str) -> Result<(), RepoError> {
    validate_stem(stem)?;

    let file = parse(text).map_err(RepoError::Eap)?;
    for a in &file.protocols {
        resolve(a).map_err(RepoError::Eap)?;
    }

    let path = protocol_path(firmware_repo_root, stem);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| RepoError::Io(format!("{}: {e}", parent.display())))?;
    }
    fs::write(&path, text).map_err(|e| RepoError::Io(format!("{}: {e}", path.display())))
}

/// Deletes one `.eap` file. A missing file is `Ok` — the end state the
/// caller asked for is the end state.
pub fn delete(firmware_repo_root: &Path, stem: &str) -> Result<(), RepoError> {
    validate_stem(stem)?;
    let path = protocol_path(firmware_repo_root, stem);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(RepoError::Io(format!("{}: {e}", path.display()))),
    }
}

/// What the repo's current manifests say about one protocol a saved study
/// carries (decision 87).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Divergence {
    /// The current manifest resolves this name to exactly the definition the
    /// study carries.
    Unchanged,
    /// The name still resolves, to a different definition. Lists what
    /// differs, one entry per source, frame, session variable or state.
    Changed(Vec<Difference>),
    /// The name no longer resolves: the block was renamed or deleted, its
    /// file no longer parses, or two files now declare it.
    Gone,
}

/// One part of a protocol that is not the same on both sides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Difference {
    /// Every value it takes: `"source"`, `"frame"`, `"session variable"` or
    /// `"state"` (a named entry added, removed or modified, or the name
    /// `(order)` when only that part's order changed), or `"protocol"` with
    /// name `(name)` when nothing in those four differs and the protocol's
    /// own name does.
    pub part: &'static str,
    pub name: String,
    pub how: DifferenceKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DifferenceKind {
    /// In the current manifest, not in the study.
    Added,
    /// In the study, not in the current manifest.
    Removed,
    /// Under the same name on both sides, with different contents.
    Modified,
}

fn diff_named<'a, T: PartialEq + 'a>(
    part: &'static str,
    carried: impl Iterator<Item = (&'a str, &'a T)> + Clone,
    current: impl Iterator<Item = (&'a str, &'a T)> + Clone,
    out: &mut Vec<Difference>,
) {
    for (name, old) in carried.clone() {
        let how = match current.clone().find(|(n, _)| *n == name) {
            None => DifferenceKind::Removed,
            Some((_, new)) if new != old => DifferenceKind::Modified,
            Some(_) => continue,
        };
        out.push(Difference { part, name: name.to_string(), how });
    }
    for (name, _) in current {
        if !carried.clone().any(|(n, _)| n == name) {
            out.push(Difference { part, name: name.to_string(), how: DifferenceKind::Added });
        }
    }
}

/// Like [`diff_named`], and when that finds nothing for the part but the two
/// lists are still unequal, only their order changed: one `(order)` entry
/// under this part (indices are positional, so that is a real difference).
fn diff_part<'a, T: PartialEq + 'a>(
    part: &'static str,
    carried: &'a [T],
    current: &'a [T],
    name_of: impl Fn(&'a T) -> &'a str + Copy,
    out: &mut Vec<Difference>,
) {
    let before = out.len();
    diff_named(part, carried.iter().map(|x| (name_of(x), x)), current.iter().map(|x| (name_of(x), x)), out);
    if out.len() == before && carried != current {
        out.push(Difference { part, name: String::from("(order)"), how: DifferenceKind::Modified });
    }
}

/// Compares one carried definition with the current one, part by part.
fn compare(carried: &ProtocolDef, current: &ProtocolDef) -> Divergence {
    if carried == current {
        return Divergence::Unchanged;
    }
    let mut out = Vec::new();
    diff_part("source", &carried.sources, &current.sources, |x| x.name.as_str(), &mut out);
    diff_part("frame", &carried.frames, &current.frames, |x| x.name.as_str(), &mut out);
    diff_part("session variable", &carried.session, &current.session, |x| x.name.as_str(), &mut out);
    diff_part("state", &carried.states, &current.states, |x| x.name.as_str(), &mut out);
    // Unequal overall with no named part or order differing: the only field
    // left is the protocol's own name.
    if out.is_empty() {
        out.push(Difference { part: "protocol", name: String::from("(name)"), how: DifferenceKind::Modified });
    }
    Divergence::Changed(out)
}

/// For each protocol a saved study carries, whether the current manifests
/// still resolve its name to the same definition (decision 87).
///
/// `carried` is `&study.protocols`; `current` is [`RepoProtocols::defs`]. The
/// comparison is structural, never a digest: nothing is added to the study,
/// and the study is not changed or refused — a diverged study still runs what
/// it carries (decision 58). Answers come back in the study's own order; a
/// study carrying no protocol gets an empty list.
pub fn divergence(carried: &[ProtocolDef], current: &[ProtocolDef]) -> Vec<(String, Divergence)> {
    carried
        .iter()
        .map(|c| {
            let d = match current.iter().find(|d| d.name == c.name) {
                None => Divergence::Gone,
                Some(d) => compare(c, d),
            };
            (c.name.as_str().to_string(), d)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch repo under the system temp dir, unique per test. No
    /// `tempfile` dev-dependency for four lines — the same posture
    /// `embarch-ui`'s own `Scratch` helper takes.
    struct Repo(PathBuf);

    impl Repo {
        fn new(tag: &str) -> Repo {
            static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!("eap-repo-{tag}-{}-{n}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(protocols_dir(&root)).unwrap();
            Repo(root)
        }

        fn write(&self, stem: &str, text: &str) {
            fs::write(protocol_path(&self.0, stem), text).unwrap();
        }
    }

    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const ONE: &str = "protocol one {\n    state go {\n        on_timeout 1000ms: goto done\n    }\n\n    state done outcome: pass\n}\n";

    fn named(name: &str) -> String {
        ONE.replace("protocol one", &format!("protocol {name}"))
    }

    #[test]
    fn a_missing_directory_is_an_empty_repo() {
        let root = std::env::temp_dir().join("eap-repo-absent-does-not-exist");
        let _ = fs::remove_dir_all(&root);
        let repo = scan(&root).unwrap();
        assert!(repo.files.is_empty());
        assert!(repo.defs().unwrap().is_empty());
    }

    #[test]
    fn scan_does_not_fail_on_a_bad_file_and_the_good_ones_still_resolve() {
        let repo = Repo::new("mixed");
        repo.write("good", &named("good"));
        repo.write("bad", "protocol broken {\n  state go {\n}\n");

        let scanned = scan(&repo.0).unwrap();
        assert_eq!(scanned.files.len(), 2);
        // Sorted by stem: bad, good.
        assert_eq!(scanned.files[0].stem, "bad");
        assert!(scanned.files[0].parsed.is_err());
        assert_eq!(scanned.files[0].errors().len(), 1);
        assert!(scanned.files[1].parsed.is_ok());
        assert!(scanned.files[1].errors().is_empty());

        // The bad file contributes nothing and is not an error here.
        let defs = scanned.defs().unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name.as_str(), "good");
    }

    #[test]
    fn defs_refuses_the_same_protocol_name_in_two_files() {
        let repo = Repo::new("dup");
        repo.write("a", &named("shared"));
        repo.write("b", &named("shared"));

        let scanned = scan(&repo.0).unwrap();
        let err = scanned.defs().unwrap_err();
        let text = err.to_string();
        assert!(text.contains("shared"), "{text}");
        assert!(text.contains("a.eap") && text.contains("b.eap"), "{text}");

        let dups = scanned.duplicate_names();
        assert_eq!(dups.len(), 1);
        assert_eq!(dups[0].0, "shared");
        assert_eq!(dups[0].1, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn a_refused_save_leaves_the_file_byte_identical() {
        let repo = Repo::new("refuse");
        let original = named("keep");
        repo.write("keep", &original);

        let err = save(&repo.0, "keep", "protocol keep {\n  state go {\n").unwrap_err();
        assert!(matches!(err, RepoError::Eap(_)), "{err}");
        assert_eq!(fs::read_to_string(protocol_path(&repo.0, "keep")).unwrap(), original);
    }

    #[test]
    fn save_writes_a_file_scan_can_read_back() {
        let repo = Repo::new("save");
        save(&repo.0, "fresh", &named("fresh")).unwrap();
        let scanned = scan(&repo.0).unwrap();
        assert_eq!(scanned.file("fresh").unwrap().text, named("fresh"));
        assert_eq!(scanned.defs().unwrap().len(), 1);
    }

    #[test]
    fn a_traversing_stem_is_refused_by_save_and_delete() {
        let repo = Repo::new("stem");
        for stem in ["..", "a/b", "a\\b", "", "x.eap", "a b", "a;b"] {
            assert!(
                matches!(validate_stem(stem), Err(RepoError::BadFileName { .. })),
                "{stem:?} should be refused"
            );
            assert!(save(&repo.0, stem, &named("x")).is_err(), "{stem:?}");
            assert!(delete(&repo.0, stem).is_err(), "{stem:?}");
        }
        assert!(validate_stem("bds_v2-1").is_ok());
    }

    /// A file the scan cannot *read* is listed carrying an I/O error, never
    /// silently omitted — a file the scan cannot see is not the same as a
    /// file that is not there, and an editor listing the second when the
    /// first is true would hide a protocol a study already names. A
    /// directory named `x.eap` is the portable way to be unreadable.
    #[test]
    fn an_unreadable_file_is_listed_with_its_error_not_omitted() {
        let repo = Repo::new("unreadable");
        repo.write("good", &named("good"));
        fs::create_dir_all(protocol_path(&repo.0, "weird")).unwrap();

        let scanned = scan(&repo.0).unwrap();
        assert_eq!(scanned.files.len(), 2);
        let weird = scanned.file("weird").unwrap();
        assert!(matches!(weird.parsed, Err(RepoError::Io(_))), "{:?}", weird.parsed);
        assert_eq!(weird.errors().len(), 1);
        // It still contributes nothing to what a study can reach.
        assert_eq!(scanned.defs().unwrap().len(), 1);
    }

    #[test]
    fn deleting_a_missing_file_is_ok() {
        let repo = Repo::new("del");
        assert!(delete(&repo.0, "never-existed").is_ok());
        repo.write("gone", &named("gone"));
        assert!(delete(&repo.0, "gone").is_ok());
        assert!(scan(&repo.0).unwrap().files.is_empty());
    }

    fn def_of(repo: &Repo, name: &str) -> ProtocolDef {
        scan(&repo.0).unwrap().defs().unwrap().into_iter().find(|d| d.name == name).unwrap()
    }

    #[test]
    fn divergence_unchanged_changed_gone_and_none() {
        let repo = Repo::new("div");
        repo.write("one", &named("one"));
        repo.write("two", &named("two"));
        let carried = vec![def_of(&repo, "one"), def_of(&repo, "two")];

        // Edit one (add a state), delete two.
        repo.write(
            "one",
            "protocol one {\n    state go {\n        on_timeout 1000ms: goto done\n    }\n\n    state done outcome: pass\n\n    state extra outcome: fail\n}\n",
        );
        delete(&repo.0, "two").unwrap();
        let current = scan(&repo.0).unwrap().defs().unwrap();

        let got = divergence(&carried, &current);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].0, "one");
        match &got[0].1 {
            Divergence::Changed(d) => assert_eq!(
                d,
                &vec![Difference { part: "state", name: "extra".into(), how: DifferenceKind::Added }]
            ),
            other => panic!("{other:?}"),
        }
        assert_eq!(got[1], ("two".to_string(), Divergence::Gone));

        // Unrelated edit elsewhere: unchanged.
        let same = divergence(&carried[..1], &[carried[0].clone()]);
        assert_eq!(same[0].1, Divergence::Unchanged);

        // A study carrying no protocol has nothing to report.
        assert!(divergence(&[], &current).is_empty());
    }

    const RICH: &str = "protocol rich {
    source a = characteristic(service: \"0000aaaa-0000-1000-8000-00805f9b34fb\", char: \"0000aaa1-0000-1000-8000-00805f9b34fb\")
    source b = characteristic(service: \"0000bbbb-0000-1000-8000-00805f9b34fb\", char: \"0000bbb1-0000-1000-8000-00805f9b34fb\")
    frame fa on a {
        u8 x
    }
    frame fb on b {
        u8 y
    }
    session {
        var p: u32 = 0
        var q: u32 = 0
    }
    state go {
        on_timeout 1000ms: goto done
    }
    state done outcome: pass
}
";

    fn rich() -> ProtocolDef {
        let repo = Repo::new("rich");
        repo.write("rich", RICH);
        def_of(&repo, "rich")
    }

    fn changed(a: &ProtocolDef, b: &ProtocolDef) -> Vec<(&'static str, String)> {
        match compare(a, b) {
            Divergence::Changed(d) => d.into_iter().map(|x| (x.part, x.name)).collect(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_pure_reordering_names_the_part_that_moved() {
        let base = rich();
        let mut d = base.clone();
        d.sources.reverse();
        assert_eq!(changed(&base, &d), vec![("source", "(order)".to_string())]);
        let mut d = base.clone();
        d.frames.reverse();
        assert_eq!(changed(&base, &d), vec![("frame", "(order)".to_string())]);
        let mut d = base.clone();
        d.session.reverse();
        assert_eq!(changed(&base, &d), vec![("session variable", "(order)".to_string())]);
        let mut d = base.clone();
        d.states.reverse();
        assert_eq!(changed(&base, &d), vec![("state", "(order)".to_string())]);
    }

    #[test]
    fn reordering_two_parts_reports_both() {
        let base = rich();
        let mut d = base.clone();
        d.frames.reverse();
        d.states.reverse();
        assert_eq!(
            changed(&base, &d),
            vec![("frame", "(order)".to_string()), ("state", "(order)".to_string())]
        );
    }

    #[test]
    fn a_difference_outside_the_four_lists_is_not_filed_under_state() {
        let base = rich();
        let mut d = base.clone();
        d.name = heapless::String::try_from("renamed").unwrap();
        assert_eq!(changed(&base, &d), vec![("protocol", "(name)".to_string())]);
    }
}
