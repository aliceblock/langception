//! Model Packs (ADR-0008, deferred-model-packs ticket 01): the pure
//! foundation every later ticket of the feature stands on — the models
//! manifest shipped with the app, the effective-selection → required-packs
//! rule, per-pack on-disk status, and the assembly that populates the
//! **active models directory** the sidecar launches into. No UI, no
//! network here; the downloader is ticket 03, the Engines tab is ticket
//! 04, and the packaging that generates the manifest and archives is
//! ticket 05.
//!
//! The pieces and their contracts:
//!
//! - **Manifest** ([`Manifest::parse`]): the JSON file packaging drops
//!   beside the model resources pins, per Model Pack, the file list with
//!   the SHA-256 hash and byte size of every file, the pack version, and
//!   the ordered URL list (GitHub Releases asset first, pinned upstream
//!   mirrors after). The bundled Tiny files are pinned under `bundled`,
//!   so a mutated bundle is detected the same way a bad download is. The
//!   app never fetches the manifest over the network — a release's
//!   manifest is the release's truth. Anything malformed is a named
//!   [`ManifestError`], never a guess.
//! - **Required packs** ([`required_packs`]): derives from the EFFECTIVE
//!   selection, not the literal one — Japanese + tiny runs Small's files
//!   (the substitution rule of [`crate::ppocr`]), so it requires the
//!   Small pack; Korean reads the tier-independent Korean recognizer at
//!   every tier, so any Korean selection requires the Korean pack plus
//!   the tier's own pack (its detection file). English/Chinese + tiny
//!   needs nothing beyond the bundled baseline.
//! - **Status** ([`pack_status`]): per pack, computed from the active
//!   directory's contents — every file present and hash-matching is
//!   [`PackStatus::Ready`], any file absent is [`PackStatus::Missing`],
//!   everything present with a hash or size mismatch is
//!   [`PackStatus::Stale`].
//! - **Assembly** ([`install_bundled_files`] / [`install_pack_archive`]):
//!   populates the active models directory — the bundled Tiny files are
//!   copied in on first engine use, a pack archive unpacks there, stale
//!   files are replaced. Both verify BEFORE anything lands: an archive
//!   whose files fail hash or size is rejected WHOLE (staged extraction
//!   outside the active directory, moved in only once every file
//!   verified). A model file path only ever holds the old bytes or the
//!   new bytes — a failed move restores the old copy, and a crash
//!   leaves the file absent (status Missing), which the next install
//!   heals — so the directory never holds a partial pack. Every
//!   transient artifact (staging dirs, copy temps, replacement backups)
//!   lives under a [`RESIDUE_PREFIX`] name the sidecar would never read
//!   as a model file, and the next install sweeps leftovers from a
//!   crashed run.
//!
//! The sidecar's launch contract is untouched: everything assembles into
//! one flat directory of model files, exactly the layout the
//! `-models_path=` launch argument always pointed at (ADR-0008: the
//! sidecar is not modified at all).

use crate::ports::SourceLanguage;
use crate::ppocr::{effective_selection, tier_for_engine_value, ModelTier};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};

/// The manifest format this build understands. Packaging bumps the number
/// when the format changes; an app reading a higher version reports a
/// named error instead of guessing at unknown fields (they are rejected
/// too).
pub const MANIFEST_VERSION: u32 = 1;

/// The manifest's file name inside the bundled model directory — the
/// single file the shell reads to build a [`Manifest`] (ticket 05 drops
/// it there at packaging time).
pub const MANIFEST_FILE_NAME: &str = "models-manifest.json";

/// The prefix naming every transient file or directory the assembly
/// creates (staging dirs, copy temps, replacement backups) — and the
/// downloader's staged archive temp, which shares the contract (and the
/// sweep surface). Residue names are dot-prefixed, so they can never
/// collide with a manifest file name (both validators reject dotfiles)
/// and the sidecar's flat directory never mistakes one for a model
/// file; the sweep removes leftovers from a crashed install on the
/// next one.
pub(crate) const RESIDUE_PREFIX: &str = ".langception-";

/// One Model Pack (CONTEXT.md): the Small set, the Medium set, or the
/// Korean recognizer set. The spelling is the manifest's `packs` key —
/// and the downloader events' wire spelling (serde lowercase), pinned
/// equal to [`PackId::as_str`] by the pack_downloader tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PackId {
    Small,
    Medium,
    Korean,
}

impl PackId {
    /// Every pack the manifest must describe, in display order.
    pub const ALL: [PackId; 3] = [PackId::Small, PackId::Medium, PackId::Korean];

    /// The pack's spelling as the manifest's `packs` key.
    pub fn as_str(self) -> &'static str {
        match self {
            PackId::Small => "small",
            PackId::Medium => "medium",
            PackId::Korean => "korean",
        }
    }

    /// The pack's user-facing name (CONTEXT.md's Model Pack entry), for
    /// notices and log lines composed at the notices seam.
    pub fn display_name(self) -> &'static str {
        match self {
            PackId::Small => "Small set",
            PackId::Medium => "Medium set",
            PackId::Korean => "Korean recognizer set",
        }
    }

    fn parse(key: &str) -> Option<Self> {
        PackId::ALL.iter().copied().find(|id| id.as_str() == key)
    }
}

impl std::fmt::Display for PackId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One model file as the manifest pins it: the name it carries inside the
/// flat models directory (the sidecar's launch-contract layout), its
/// SHA-256 (lowercase hex), and its size in bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackFile {
    pub name: String,
    pub sha256: String,
    pub size: u64,
}

/// One Model Pack's pins: an opaque version string, the ordered URL list
/// (primary first, fallback mirrors after), and the file list above.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackSpec {
    pub version: String,
    pub urls: Vec<String>,
    pub files: Vec<PackFile>,
}

/// A parsed, fully validated manifest: the bundled Tiny baseline plus a
/// spec for every one of the three packs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    bundled: Vec<PackFile>,
    packs: BTreeMap<PackId, PackSpec>,
}

impl Manifest {
    /// The bundled Tiny files (detection, recognition, dictionary).
    pub fn bundled(&self) -> &[PackFile] {
        &self.bundled
    }

    /// One pack's pins. Validated manifests cover all three packs, so
    /// the lookup cannot miss.
    pub fn pack(&self, id: PackId) -> &PackSpec {
        self.packs.get(&id).expect("validated manifests cover every pack")
    }

    /// Parses the manifest text (read from [`MANIFEST_FILE_NAME`] inside
    /// the bundle). Every structural problem is a named
    /// [`ManifestError`] — the manifest is machine-generated, so a
    /// mismatch means the release was built wrong and must say so, never
    /// degrade to a guess.
    pub fn parse(text: &str) -> Result<Self, ManifestError> {
        let raw: RawManifest =
            serde_json::from_str(text).map_err(|e| ManifestError::Parse(e.to_string()))?;
        if raw.manifest_version != MANIFEST_VERSION {
            return Err(ManifestError::UnsupportedVersion {
                found: raw.manifest_version,
                supported: MANIFEST_VERSION,
            });
        }

        let bundled = validate_file_set("bundled", &raw.bundled.files)?;

        let mut packs = BTreeMap::new();
        for (key, raw_pack) in &raw.packs {
            let id = PackId::parse(key)
                .ok_or_else(|| ManifestError::UnknownPack { name: key.clone() })?;
            if raw_pack.urls.is_empty() {
                return Err(ManifestError::EmptyUrls { pack: id });
            }
            let files = validate_file_set(id.as_str(), &raw_pack.files)?;
            packs.insert(
                id,
                PackSpec {
                    version: raw_pack.version.clone(),
                    urls: raw_pack.urls.clone(),
                    files,
                },
            );
        }
        for id in PackId::ALL {
            if !packs.contains_key(&id) {
                return Err(ManifestError::MissingPack { pack: id });
            }
        }

        // The active models directory is flat — one file name, one copy.
        // The same name across sets is expected exactly once (small and
        // medium share the full dictionary) and only tolerable when both
        // pins agree byte-for-byte; disagreement is a manifest bug.
        let mut seen: BTreeMap<&str, (&str, &str, u64)> = BTreeMap::new();
        check_conflicts(&mut seen, "bundled", &bundled)?;
        for id in PackId::ALL {
            check_conflicts(&mut seen, id.as_str(), &packs[&id].files)?;
        }

        Ok(Self { bundled, packs })
    }

    /// Reads and parses the manifest shipped inside the bundle
    /// directory ([`MANIFEST_FILE_NAME`] beside the model files — the
    /// file ticket 05's packaging step generates). This is the app-side
    /// half of "the app reads the models manifest attached to the app";
    /// the network is never consulted (ADR-0008).
    pub fn load_from_bundle(bundle_dir: &Path) -> Result<Self, ManifestError> {
        let path = bundle_dir.join(MANIFEST_FILE_NAME);
        let text = fs::read_to_string(&path).map_err(|source| ManifestError::Read {
            path: path.clone(),
            detail: source.to_string(),
        })?;
        Self::parse(&text).map_err(|e| e.at_path(&path))
    }
}

/// Registers one file set into the flat-directory conflict table (the
/// first registration wins; a second, differing pin for the same name is
/// a [`ManifestError::ConflictingFile`]).
fn check_conflicts<'a>(
    seen: &mut BTreeMap<&'a str, (&'a str, &'a str, u64)>,
    context: &'a str,
    files: &'a [PackFile],
) -> Result<(), ManifestError> {
    for file in files {
        match seen.get(file.name.as_str()) {
            Some((first, sha, size)) => {
                if sha != &file.sha256 || size != &file.size {
                    return Err(ManifestError::ConflictingFile {
                        name: file.name.clone(),
                        first_context: (*first).to_string(),
                        second_context: context.to_string(),
                    });
                }
            }
            None => {
                seen.insert(file.name.as_str(), (context, &file.sha256, file.size));
            }
        }
    }
    Ok(())
}

// ---- the raw manifest shape (strict: unknown fields are format drift,
// and drift must be a named error, never silently ignored) ----

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    manifest_version: u32,
    bundled: RawBundled,
    packs: BTreeMap<String, RawPack>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBundled {
    files: Vec<RawPackFile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPack {
    version: String,
    urls: Vec<String>,
    files: Vec<RawPackFile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPackFile {
    name: String,
    sha256: String,
    size: u64,
}

/// A manifest file's hash must be 64 hex digits; case is normalized so a
/// generator that prints uppercase still parses (the hash's semantics
/// are case-insensitive; structure is not, and structure is what the
/// named errors guard).
fn normalized_sha256(context: &str, hash: &str) -> Result<String, ManifestError> {
    let is_hex = hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit());
    if !is_hex {
        return Err(ManifestError::BadHash {
            context: context.to_string(),
            hash: hash.to_string(),
        });
    }
    Ok(hash.to_lowercase())
}

/// The one escape rule both seams share: a path component must be a
/// plain model file name — non-empty, never a dotfile (which covers
/// ".", "..", and this module's own [`RESIDUE_PREFIX`] names, so a
/// manifest name can never collide with transient residue).
fn is_plain_component(part: &str) -> bool {
    !part.is_empty() && !part.starts_with('.')
}

/// The flat-directory guard on manifest file names: a name that leaves or
/// hides inside the models directory (`..`, separators, drive/ADS colons,
/// dotfiles) would break the launch contract's layout — a named error at
/// parse time, never a surprise at extraction time. Manifest names carry
/// no separators at all: the models directory is flat, so one plain
/// component is the whole name.
fn validate_file_name(context: &str, name: &str) -> Result<(), ManifestError> {
    let ok = !name.contains(['/', '\\', ':']) && is_plain_component(name);
    if ok {
        Ok(())
    } else {
        Err(ManifestError::BadFileName {
            context: context.to_string(),
            name: name.to_string(),
        })
    }
}

fn validate_file_set(context: &str, files: &[RawPackFile]) -> Result<Vec<PackFile>, ManifestError> {
    if files.is_empty() {
        return Err(ManifestError::EmptyFileList {
            context: context.to_string(),
        });
    }
    let mut out = Vec::with_capacity(files.len());
    let mut seen = std::collections::BTreeSet::new();
    for raw in files {
        validate_file_name(context, &raw.name)?;
        if !seen.insert(raw.name.clone()) {
            return Err(ManifestError::DuplicateFile {
                context: context.to_string(),
                name: raw.name.clone(),
            });
        }
        out.push(PackFile {
            name: raw.name.clone(),
            sha256: normalized_sha256(context, &raw.sha256)?,
            size: raw.size,
        });
    }
    Ok(out)
}

/// The manifest's named errors. Display strings name the offending
/// context (the pack or "bundled") so a packaging bug is diagnosable
/// from the message alone.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ManifestError {
    #[error("cannot read the models manifest at {path}: {detail}")]
    Read {
        path: PathBuf,
        detail: String,
    },
    #[error("models manifest is not valid JSON: {0}")]
    Parse(String),
    #[error(
        "models manifest version {found} is not understood by this app (supported: {supported})"
    )]
    UnsupportedVersion { found: u32, supported: u32 },
    #[error("models manifest names unknown pack \"{name}\" (known: small, medium, korean)")]
    UnknownPack { name: String },
    #[error("models manifest is missing the {pack} pack")]
    MissingPack { pack: PackId },
    #[error("models manifest pins no URL for the {pack} pack")]
    EmptyUrls { pack: PackId },
    #[error("models manifest lists no files for {context}")]
    EmptyFileList { context: String },
    #[error("models manifest pins a malformed SHA-256 for {context}: \"{hash}\" (expected 64 hex digits)")]
    BadHash { context: String, hash: String },
    #[error("models manifest names a file that leaves the models directory for {context}: \"{name}\"")]
    BadFileName { context: String, name: String },
    #[error("models manifest lists \"{name}\" twice in {context}")]
    DuplicateFile { context: String, name: String },
    #[error(
        "models manifest pins {name} differently in {first_context} and {second_context} — the flat models directory holds one copy, so the pins must agree"
    )]
    ConflictingFile {
        name: String,
        first_context: String,
        second_context: String,
    },
}

impl ManifestError {
    /// Attaches the manifest's path to a structural error, so a
    /// packaging bug is diagnosable from the message alone (the plain
    /// text parse is used on the string directly; every file-backed
    /// parse names its file).
    fn at_path(self, path: &Path) -> Self {
        match self {
            // The read error already carries the path.
            ManifestError::Read { .. } => self,
            ManifestError::Parse(detail) => {
                ManifestError::Parse(format!("{}: {detail}", path.display()))
            }
            other => other,
        }
    }
}

/// The packs an effective OCR Engine selection needs on disk (ADR-0008's
/// resolver rule): derived from the EFFECTIVE selection, so Japanese +
/// tiny requires the Small set (the substitution already runs Small's
/// files) and Korean always requires the Korean recognizer set plus the
/// tier's own pack (its detection file). English/Chinese + tiny needs
/// nothing — the bundled baseline covers it. The result is ordered for
/// stable display; empty means "bundled only".
pub fn required_packs(source: SourceLanguage, tier: ModelTier) -> &'static [PackId] {
    let (source, tier) = effective_selection(source, tier);
    // Exhaustive over the tier so a ModelTier reorder in ppocr.rs cannot
    // silently misroute a pack to the wrong tier.
    match (source, tier) {
        (SourceLanguage::Ko, ModelTier::Tiny) => &[PackId::Korean],
        (SourceLanguage::Ko, ModelTier::Small) => &[PackId::Small, PackId::Korean],
        (SourceLanguage::Ko, ModelTier::Medium) => &[PackId::Medium, PackId::Korean],
        (_, ModelTier::Tiny) => &[],
        (_, ModelTier::Small) => &[PackId::Small],
        (_, ModelTier::Medium) => &[PackId::Medium],
    }
}

/// The same rule keyed by an `[ocr] engine` config value: `None` when the
/// value selects another engine (Windows OCR) or is unknown — those need
/// no pack of this engine.
pub fn required_packs_for_engine_value(
    engine_value: &str,
    source: SourceLanguage,
) -> Option<&'static [PackId]> {
    tier_for_engine_value(engine_value).map(|tier| required_packs(source, tier))
}

/// One manifest file's state on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileState {
    /// Present with the pinned size and hash.
    Ready,
    /// Present but wrong — a hash or size mismatch against the manifest.
    Stale,
    /// Not on disk.
    Missing,
}

/// Reads and hashes one file (lowercase hex SHA-256, streamed — the
/// biggest model file is tens of MB and never loads whole).
pub fn file_sha256(path: &Path) -> Result<String, PackError> {
    let mut file = fs::File::open(path).map_err(|e| PackError::io(path, e))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(|e| PackError::io(path, e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(digest: &[u8]) -> String {
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Compares one file on disk against its pin. The size is checked first —
/// a mismatch is stale evidence on its own and skips hashing the rest.
pub fn file_state(dir: &Path, file: &PackFile) -> FileState {
    let path = dir.join(&file.name);
    let Ok(meta) = fs::metadata(&path) else {
        return FileState::Missing;
    };
    if meta.len() != file.size {
        return FileState::Stale;
    }
    match file_sha256(&path) {
        Ok(actual) if actual == file.sha256 => FileState::Ready,
        _ => FileState::Stale,
    }
}

/// One pack's (or the bundled baseline's) status, computed from the
/// active directory's contents. Missing wins over stale: any absent file
/// means the pack never landed and the whole archive must be fetched
/// (pack files arrive together, as one archive).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackStatus {
    Ready,
    /// At least one file is absent; the names are listed for debug logs.
    Missing { files: Vec<String> },
    /// Every file is present but at least one fails its pin; the
    /// offending names are listed for debug logs.
    Stale { files: Vec<String> },
}

fn file_set_status(files: &[PackFile], dir: &Path) -> PackStatus {
    let mut missing = Vec::new();
    let mut stale = Vec::new();
    for file in files {
        match file_state(dir, file) {
            FileState::Ready => {}
            FileState::Missing => missing.push(file.name.clone()),
            FileState::Stale => stale.push(file.name.clone()),
        }
    }
    if !missing.is_empty() {
        PackStatus::Missing { files: missing }
    } else if !stale.is_empty() {
        PackStatus::Stale { files: stale }
    } else {
        PackStatus::Ready
    }
}

/// The status of one pack inside the active models directory.
pub fn pack_status(pack: &PackSpec, active_dir: &Path) -> PackStatus {
    file_set_status(&pack.files, active_dir)
}

/// The status of the bundled Tiny baseline inside the active models
/// directory.
pub fn bundled_status(manifest: &Manifest, active_dir: &Path) -> PackStatus {
    file_set_status(manifest.bundled(), active_dir)
}

/// The assembly's named errors: which file, what was expected, what
/// arrived — diagnosable from the message alone.
#[derive(Debug, thiserror::Error)]
pub enum PackError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{archive}: not a readable ZIP archive: {source}")]
    Archive {
        archive: PathBuf,
        #[source]
        source: zip::result::ZipError,
    },
    #[error("pack archive entry \"{name}\" would leave the models directory — rejected")]
    UnsafeEntry { name: String },
    #[error("pack archive layout is not the packaging layout: {0}")]
    BadArchiveLayout(String),
    #[error("pack archive is missing manifest file \"{file}\" — the whole pack is rejected")]
    EntryMissing { file: String },
    #[error("pack archive file \"{file}\" is {actual} bytes, manifest pins {expected}")]
    SizeMismatch {
        file: String,
        expected: u64,
        actual: u64,
    },
    #[error(
        "pack archive file \"{file}\" fails its SHA-256 pin (expected {expected}, got {actual}) — the whole pack is rejected"
    )]
    HashMismatch {
        file: String,
        expected: String,
        actual: String,
    },
    #[error("bundled model file \"{file}\" is missing from the app bundle: {path}")]
    BundleFileMissing { file: String, path: PathBuf },
    #[error(
        "bundled model file \"{file}\" fails its SHA-256 pin (expected {expected}, got {actual}) — the bundle is mutated, refusing to copy"
    )]
    BundleHashMismatch {
        file: String,
        expected: String,
        actual: String,
    },
}

impl PackError {
    /// The named wrapper for every filesystem error in this module —
    /// one place that attaches the path the operation was acting on.
    fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        PackError::Io {
            path: path.into(),
            source,
        }
    }
}

/// Copies the bundled Tiny files into the active models directory (first
/// engine use per ADR-0008): a file already on disk and hash-matching is
/// left alone; a missing or stale file is re-copied from the bundle.
/// Every bundle source is verified against the manifest BEFORE anything
/// is copied, so a mutated bundle changes nothing on disk. Returns the
/// number of files copied.
pub fn install_bundled_files(
    bundle_dir: &Path,
    manifest: &Manifest,
    active_dir: &Path,
) -> Result<u32, PackError> {
    sweep_residue(active_dir);
    let bundled = manifest.bundled();
    // Phase 1: verify every file that needs copying against its pin.
    let mut to_copy = Vec::new();
    for file in bundled {
        if file_state(active_dir, file) == FileState::Ready {
            continue;
        }
        let source = bundle_dir.join(&file.name);
        if !source.is_file() {
            return Err(PackError::BundleFileMissing {
                file: file.name.clone(),
                path: source,
            });
        }
        let actual = file_sha256(&source)?;
        if actual != file.sha256 {
            return Err(PackError::BundleHashMismatch {
                file: file.name.clone(),
                expected: file.sha256.clone(),
                actual,
            });
        }
        to_copy.push(file);
    }
    // Phase 2: copy through a residue-named temp + [`replace_file`], so
    // the destination path only ever holds old or new bytes.
    fs::create_dir_all(active_dir).map_err(|e| PackError::io(active_dir, e))?;
    let mut backups = Vec::new();
    let result = (|| -> Result<(), PackError> {
        for file in &to_copy {
            let source = bundle_dir.join(&file.name);
            let dest = active_dir.join(&file.name);
            let temp = copy_to_temp(&source, &dest)?;
            replace_file(&temp, &dest, &mut backups)?;
        }
        Ok(())
    })();
    settle_backups(backups);
    result?;
    Ok(to_copy.len() as u32)
}

/// The parent directory a transient artifact stages beside: the parent
/// of the active models directory (or "." when it has none), so staging
/// stays on the active directory's volume and out of the sidecar's flat
/// layout.
fn active_dir_parent(active_dir: &Path) -> PathBuf {
    active_dir
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()
}

/// A [`RESIDUE_PREFIX`]-named directory beside the active models
/// directory, pid-tagged so a concurrent assembly's sweep skips it —
/// the one composer every staging-style artifact names through (the
/// pack archive's staging dir, the downloader's staged download temp).
pub(crate) fn residue_dir_sibling(active_dir: &Path, name: &str) -> PathBuf {
    active_dir_parent(active_dir).join(format!(
        "{RESIDUE_PREFIX}{name}-{}",
        std::process::id()
    ))
}

/// Removes the transient residue of a crashed install: copy temps and
/// replacement backups inside the active directory, staging directories
/// beside it — every one [`RESIDUE_PREFIX`]-named. Entries carrying this
/// process's pid are skipped (the app is single-instance; the guard is
/// re-entrancy paranoia, never a race the sweep needs to win).
fn sweep_residue(active_dir: &Path) {
    let pid_tag = format!("-{}", std::process::id());
    let ours = |name: &str| name.contains(&pid_tag);
    if let Ok(entries) = fs::read_dir(active_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(RESIDUE_PREFIX) && !ours(&name) {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    let parent = active_dir_parent(active_dir);
    if let Ok(entries) = fs::read_dir(&parent) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path == active_dir {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(RESIDUE_PREFIX) && !ours(&name) {
                let _ = fs::remove_dir_all(&path);
            }
        }
    }
}

/// A [`RESIDUE_PREFIX`]-named sibling of `dest` for one transient role —
/// same directory as the destination, so the later rename stays on one
/// volume.
fn residue_sibling(dest: &Path, role: &str) -> PathBuf {
    let name = dest
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    dest.with_file_name(format!("{RESIDUE_PREFIX}{role}-{}-{name}", std::process::id()))
}

/// Copies `source`'s bytes into a residue-named temp beside `dest` and
/// returns the temp path for [`replace_file`]. The caller has already
/// verified the source against its pin; a write error removes the temp.
fn copy_to_temp(source: &Path, dest: &Path) -> Result<PathBuf, PackError> {
    let temp = residue_sibling(dest, "copying");
    let result = (|| {
        let mut reader = fs::File::open(source).map_err(|e| PackError::io(source, e))?;
        let mut writer =
            BufWriter::new(fs::File::create(&temp).map_err(|e| PackError::io(&temp, e))?);
        let mut buf = [0u8; 64 * 1024];
        loop {
            let n = reader.read(&mut buf).map_err(|e| PackError::io(source, e))?;
            if n == 0 {
                break;
            }
            writer.write_all(&buf[..n]).map_err(|e| PackError::io(&temp, e))?;
        }
        writer.flush().map_err(|e| PackError::io(&temp, e))?;
        Ok(())
    })();
    if let Err(e) = result {
        let _ = fs::remove_file(&temp);
        return Err(e);
    }
    Ok(temp)
}

/// Replaces `dest` with `staged` (content already verified), keeping the
/// old copy until the new one is in place: the old file renames to a
/// residue-named backup, the staged file renames in, and
/// [`settle_backups`] deletes the backups once every file has landed —
/// or restores them if the install failed midway. std::fs::rename fails
/// onto an existing destination on Windows, hence the dance; it buys the
/// module's claim that a model file path only ever holds the old bytes
/// or the new bytes, never a partial file.
fn replace_file(
    staged: &Path,
    dest: &Path,
    backups: &mut Vec<(PathBuf, PathBuf)>,
) -> Result<(), PackError> {
    if dest.exists() {
        let backup = residue_sibling(dest, "backup");
        fs::rename(dest, &backup).map_err(|e| PackError::io(dest, e))?;
        backups.push((backup, dest.to_path_buf()));
    }
    fs::rename(staged, dest).map_err(|e| PackError::io(dest, e))
}

/// Settles the backups [`replace_file`] collected: a backup whose
/// destination received its new file is deleted; one whose destination is
/// missing (the install failed between the two renames) is renamed back,
/// restoring the pre-install state.
fn settle_backups(backups: Vec<(PathBuf, PathBuf)>) {
    for (backup, dest) in backups {
        if dest.exists() {
            let _ = fs::remove_file(&backup);
        } else {
            let _ = fs::rename(&backup, &dest);
        }
    }
}

/// Installs one Model Pack from its downloaded archive into the active
/// models directory: extract to a staging directory OUTSIDE the active
/// directory, verify every manifest file's size and hash, and only then
/// move the files in (replacing stale copies). Any verification failure
/// rejects the WHOLE pack — the active directory is untouched and the
/// staging residue is removed, so the directory never holds a partial
/// pack (spec story 23). A failed move restores the replaced files'
/// old copies ([`settle_backups`]).
///
/// The archive layout is the one packaging produces: entries named
/// exactly as the manifest pins them, either flat or under one shared
/// top-level directory (an archive of the pack folder). Anything else —
/// mixed prefixes, entries escaping the directory — is a named error
/// before any byte is written.
pub fn install_pack_archive(
    archive: &Path,
    id: PackId,
    pack: &PackSpec,
    active_dir: &Path,
) -> Result<(), PackError> {
    let file = fs::File::open(archive).map_err(|e| PackError::io(archive, e))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|source| PackError::Archive {
        archive: archive.to_path_buf(),
        source,
    })?;

    // Index the entries: exact names after stripping the shared prefix.
    let mut entry_names: Vec<(usize, String)> = Vec::new();
    for i in 0..zip.len() {
        let entry = zip.by_index(i).map_err(|source| PackError::Archive {
            archive: archive.to_path_buf(),
            source,
        })?;
        if entry.is_dir() {
            continue;
        }
        // The name decode is fallible in zip 9 (a corrupt encoding field);
        // it lands in the same named error as any other unreadable archive.
        let name = entry.name().map_err(|source| PackError::Archive {
            archive: archive.to_path_buf(),
            source,
        })?;
        let name = name.into_owned();
        validate_entry_name(&name)?;
        entry_names.push((i, name));
    }
    let prefix = shared_prefix(&entry_names)?;

    let lookup = |stripped: &str| -> Option<usize> {
        entry_names
            .iter()
            .find(|(_, name)| strip_prefix_dir(name, prefix.as_deref()) == stripped)
            .map(|(i, _)| *i)
    };

    // Phase 1: extract + verify every manifest file into staging. The
    // staging directory is removed on every failure path, so a rejected
    // pack leaves no residue anywhere.
    let staging = staging_dir(active_dir, id.as_str());
    if staging.exists() {
        let _ = fs::remove_dir_all(&staging);
    }
    let staged: Result<Vec<(PathBuf, &PackFile)>, PackError> = (|| {
        fs::create_dir_all(&staging).map_err(|e| PackError::io(&staging, e))?;
        let mut staged = Vec::with_capacity(pack.files.len());
        for file in &pack.files {
            let index = lookup(&file.name).ok_or_else(|| PackError::EntryMissing {
                file: file.name.clone(),
            })?;
            let mut entry = zip.by_index(index).map_err(|source| PackError::Archive {
                archive: archive.to_path_buf(),
                source,
            })?;
            let path = extract_verified(&mut entry, file, &staging)?;
            staged.push((path, file));
        }
        Ok(staged)
    })();
    let staged = match staged {
        Ok(staged) => staged,
        Err(e) => {
            let _ = fs::remove_dir_all(&staging);
            return Err(e);
        }
    };

    // Phase 2: everything verified — move into the active directory
    // through [`replace_file`]: a model file path only ever holds the
    // old bytes or the new bytes, and a failed move restores the old
    // copy, so the active directory keeps its pre-install state.
    fs::create_dir_all(active_dir).map_err(|e| PackError::io(active_dir, e))?;
    let mut backups = Vec::new();
    let move_result = (|| -> Result<(), PackError> {
        for (staged_path, file) in &staged {
            let dest = active_dir.join(&file.name);
            replace_file(staged_path, &dest, &mut backups)?;
        }
        Ok(())
    })();
    settle_backups(backups);
    let _ = fs::remove_dir_all(&staging);
    move_result
}

/// Removes the pack files the `keep` selection does not use (ADR-0008:
/// nothing deletes a pack automatically — the Engines tab's explicit
/// delete-unused button, deferred-model-packs ticket 04, is the only
/// caller). A file is removed only when an unkept pack pins it AND no
/// kept pack and the bundled baseline pin the same name — the shared
/// dictionary repeating across Small and Medium survives while either
/// pack is kept, and the bundled Tiny files are guarded the same way
/// even though no pack pins them today. Files already absent are fine
/// (the pack may never have downloaded); every kept pack's files —
/// including stale ones, which the next selection-triggered download
/// refreshes — are in use and stay. Returns the removed names in
/// manifest order (the tests pin them; the shell's command logs the
/// count for the page).
pub fn delete_unused_packs(
    manifest: &Manifest,
    active_dir: &Path,
    keep: &[PackId],
) -> Result<Vec<String>, PackError> {
    let mut guarded: BTreeSet<&str> = manifest
        .bundled()
        .iter()
        .map(|file| file.name.as_str())
        .collect();
    for id in keep {
        for file in &manifest.pack(*id).files {
            guarded.insert(file.name.as_str());
        }
    }
    let mut removed = Vec::new();
    for id in PackId::ALL {
        if keep.contains(&id) {
            continue;
        }
        for file in &manifest.pack(id).files {
            if guarded.contains(file.name.as_str()) {
                continue;
            }
            let path = active_dir.join(&file.name);
            match fs::remove_file(&path) {
                Ok(()) => removed.push(file.name.clone()),
                // Never downloaded (or already reclaimed): the outcome
                // the caller asked for holds either way.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(PackError::io(&path, e)),
            }
        }
    }
    Ok(removed)
}

/// The staging directory for one pack install: a residue-named sibling
/// of the active directory, so the verified-file rename stays on one
/// volume and the sidecar's flat directory never sees staging residue.
fn staging_dir(active_dir: &Path, pack: &str) -> PathBuf {
    residue_dir_sibling(active_dir, &format!("pack-{pack}-staging"))
}

/// Extracts one zip entry into the staging directory under its manifest
/// name, streaming with a hard byte cap at the pinned size (an oversized
/// entry never writes past its pin) and hashing on the way. Returns the
/// staged path when size and hash both verify.
fn extract_verified(
    entry: &mut dyn Read,
    file: &PackFile,
    staging: &Path,
) -> Result<PathBuf, PackError> {
    let path = staging.join(&file.name);
    let dest_file = fs::File::create(&path).map_err(|e| PackError::io(&path, e))?;
    let mut writer = BufWriter::new(dest_file);
    let mut hasher = Sha256::new();
    let mut written: u64 = 0;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = entry
            .read(&mut buf)
            .map_err(|e| PackError::io(&path, std::io::Error::other(e)))?;
        if n == 0 {
            break;
        }
        written += n as u64;
        if written > file.size {
            let _ = fs::remove_file(&path);
            return Err(PackError::SizeMismatch {
                file: file.name.clone(),
                expected: file.size,
                actual: written,
            });
        }
        hasher.update(&buf[..n]);
        writer.write_all(&buf[..n]).map_err(|e| PackError::io(&path, e))?;
    }
    writer.flush().map_err(|e| PackError::io(&path, e))?;
    if written != file.size {
        let _ = fs::remove_file(&path);
        return Err(PackError::SizeMismatch {
            file: file.name.clone(),
            expected: file.size,
            actual: written,
        });
    }
    let actual = hex(&hasher.finalize());
    if actual != file.sha256 {
        let _ = fs::remove_file(&path);
        return Err(PackError::HashMismatch {
            file: file.name.clone(),
            expected: file.sha256.clone(),
            actual,
        });
    }
    Ok(path)
}

/// Rejects entry names that would leave the models directory before any
/// prefix logic runs: the same plain-component rule the manifest
/// enforces (no dotfiles, no empty components — a doubled slash or a
/// `..` smuggle), plus no backslashes (a Windows path smuggled through
/// a zip), no drive/ADS colons, no absolute or trailing-slash paths.
fn validate_entry_name(name: &str) -> Result<(), PackError> {
    let ok = !name.is_empty()
        && !name.starts_with('/')
        && !name.ends_with('/')
        && !name.contains('\\')
        && !name.contains(':')
        && name.split('/').all(is_plain_component);
    if ok {
        Ok(())
    } else {
        Err(PackError::UnsafeEntry {
            name: name.to_string(),
        })
    }
}

/// The one shared top-level directory all entries sit under, if any —
/// the layout `Compress-Archive` of the pack folder produces. Mixed
/// layouts (some entries prefixed, some not) are not what packaging
/// builds and are named errors rather than guesses.
fn shared_prefix(entries: &[(usize, String)]) -> Result<Option<String>, PackError> {
    let prefixes: Vec<Option<&str>> = entries
        .iter()
        .map(|(_, name)| name.split_once('/').map(|(dir, _)| dir))
        .collect();
    let all_prefixed = prefixes.iter().all(|p| p.is_some());
    let none_prefixed = prefixes.iter().all(|p| p.is_none());
    if none_prefixed {
        return Ok(None);
    }
    if !all_prefixed {
        return Err(PackError::BadArchiveLayout(
            "some entries sit in a top-level directory and some do not".to_string(),
        ));
    }
    let first = prefixes[0].expect("all_prefixed");
    if prefixes.iter().any(|p| *p != Some(first)) {
        return Err(PackError::BadArchiveLayout(
            "entries sit under different top-level directories".to_string(),
        ));
    }
    Ok(Some(first.to_string()))
}

fn strip_prefix_dir(name: &str, prefix: Option<&str>) -> String {
    match prefix {
        Some(dir) => name
            .strip_prefix(&format!("{dir}/"))
            .unwrap_or(name)
            .to_string(),
        None => name.to_string(),
    }
}

// ---- The spawn gate (ADR-0008, deferred-model-packs ticket 02) ----

/// Why a pass cannot run its OCR engine right now. `NotReady` is the
/// deferred-pack feature's normal waiting state — the pack's files are
/// missing or stale and a download heals it; `Broken` is a broken
/// manifest or bundle, which no download fixes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Readiness {
    /// Every file the effective selection needs is present and
    /// hash-valid in the active models directory; the sidecar can spawn.
    Ready,
    /// The not-ready state: report loading, never spawn, never fall back
    /// to another engine (only the user switching engines switches).
    NotReady(NotReady),
    /// The manifest or the bundled baseline is broken — the old
    /// loud-failure path applies instead.
    Broken(String),
}

/// What a not-ready pass reports: the packs whose files are missing or
/// stale, with their statuses — structured so the user-facing wording
/// composes at the notices seam in the UI language, and the per-pass
/// error line composes from [`NotReady::summary`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotReady {
    pub packs: Vec<(PackId, PackStatus)>,
}

impl NotReady {
    /// The distinct pack names, in the verdict's order — the list the
    /// notices seam interpolates.
    pub fn pack_names(&self) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = Vec::new();
        for (id, _) in &self.packs {
            let name = id.display_name();
            if !names.contains(&name) {
                names.push(name);
            }
        }
        names
    }

    /// The one-line English summary for per-pass error lines and the app
    /// log: every offending pack with its reason and representative
    /// files. User-facing notices compose their own wording from the
    /// structured fields instead.
    pub fn summary(&self) -> String {
        let parts: Vec<String> = self
            .packs
            .iter()
            .map(|(id, status)| match status {
                PackStatus::Ready => format!("{} is ready", id.display_name()),
                PackStatus::Missing { files } => {
                    format!("{} is missing {}", id.display_name(), files.join(", "))
                }
                PackStatus::Stale { files } => format!(
                    "{} fails verification {}",
                    id.display_name(),
                    files.join(", ")
                ),
            })
            .collect();
        parts.join("; ")
    }
}

/// The cached verdict for one selection, valid while every pinned file's
/// on-disk size still matches what the verdict observed — and while the
/// rotating re-verification (one pinned file re-hashed per pass) hasn't
/// contradicted it. The size scan is what makes a per-pass gate cheap:
/// a pass costs one stat per pinned file plus ONE file hash, never tens
/// of MB of SHA-256.
#[derive(Debug)]
struct CachedVerdict {
    profile: Vec<(String, Option<u64>)>,
    verdict: Readiness,
    /// The next pinned file the cache-hit path re-hashes, rotating
    /// through the set.
    next_check: usize,
}

/// The OCR Engine's spawn gate: decides, per pass and per EFFECTIVE
/// selection, whether the sidecar may spawn into the active models
/// directory. A `Ready` verdict has also populated the directory (the
/// bundled baseline is assembled on first engine use — ADR-0008), so the
/// gate is the only thing the launch waits on; once a pack's files land,
/// the next pass's stat scan changes and the recomputed verdict spawns
/// without any app restart.
///
/// Between recomputes a cached verdict is guarded by a size scan over
/// every pinned file plus a rotating re-hash of ONE file per pass — so
/// a same-size corruption is caught within one rotation (spec story 7:
/// a corrupted or tampered download never feeds the OCR engine) while a
/// pass never re-hashes the whole set.
///
/// The manifest is read once per gate (a release's manifest is the
/// release's truth — it cannot change under a running app); an unreadable
/// or malformed one is a `Broken` verdict, the same loud path a mutated
/// bundle takes through [`install_bundled_files`]'s named errors.
pub struct ReadinessGate {
    bundle_dir: PathBuf,
    active_dir: PathBuf,
    cached: std::sync::Mutex<GateCache>,
}

#[derive(Default)]
struct GateCache {
    manifest: Option<Result<Manifest, String>>,
    verdicts: BTreeMap<(SourceLanguage, ModelTier), CachedVerdict>,
}

impl ReadinessGate {
    pub fn new(bundle_dir: impl Into<PathBuf>, active_dir: impl Into<PathBuf>) -> Self {
        Self {
            bundle_dir: bundle_dir.into(),
            active_dir: active_dir.into(),
            cached: std::sync::Mutex::new(GateCache::default()),
        }
    }

    /// The verdict for one (literal) selection; the pack requirement
    /// derives from the effective selection (Japanese + tiny needs the
    /// Small set — [`required_packs`]).
    pub fn verdict(&self, source: SourceLanguage, tier: ModelTier) -> Readiness {
        let key = (source, tier);
        let mut cache = self.cached.lock().unwrap_or_else(|p| p.into_inner());
        // The manifest reads once per gate (a release's manifest cannot
        // change under a running app); the small struct is cloned out so
        // the cache lock borrows stay field-local.
        let manifest = match cache.manifest.get_or_insert_with(|| {
            Manifest::load_from_bundle(&self.bundle_dir).map_err(|e| e.to_string())
        }) {
            Ok(manifest) => manifest.clone(),
            Err(reason) => {
                return Readiness::Broken(reason.clone());
            }
        };

        // The profile this verdict keys on: every pinned file the
        // selection can need, with the on-disk size observed now. A
        // cached verdict whose profile still matches is returned as-is.
        let mut files: Vec<&PackFile> = manifest.bundled().iter().collect();
        for id in required_packs(source, tier) {
            files.extend(manifest.pack(*id).files.iter());
        }
        let profile: Vec<(String, Option<u64>)> = files
            .iter()
            .map(|file| {
                let size = fs::metadata(self.active_dir.join(&file.name))
                    .ok()
                    .map(|meta| meta.len());
                (file.name.clone(), size)
            })
            .collect();
        // The cache-hit path still re-hashes ONE pinned file per pass,
        // rotating through the set: sizes never change under same-size
        // corruption, so the size scan alone would trust a Ready verdict
        // forever and feed the engine bytes its pins reject (spec story
        // 7 — hash before use). A check that contradicts the cached
        // verdict — a file gone bad, or a healed file under a stale
        // NotReady — falls through to the full recompute below.
        let mut rotation: Option<usize> = None;
        if let Some(cached) = cache.verdicts.get_mut(&key) {
            if cached.profile == profile {
                let idx = cached.next_check % files.len();
                let checked = files[idx];
                let checked_ready = file_state(&self.active_dir, checked) == FileState::Ready;
                if checked_ready && cached.verdict == Readiness::Ready {
                    cached.next_check = (idx + 1) % files.len();
                    return cached.verdict.clone();
                }
                rotation = Some(idx);
            }
        }

        // Recompute: assemble the bundled baseline first (first engine
        // use, or a stale file to refresh — both verify before anything
        // lands), then the required packs' statuses.
        let verdict = match install_bundled_files(&self.bundle_dir, &manifest, &self.active_dir) {
            Err(e) => Readiness::Broken(e.to_string()),
            Ok(_) => {
                let mut not_ready = Vec::new();
                for id in required_packs(source, tier) {
                    match pack_status(manifest.pack(*id), &self.active_dir) {
                        PackStatus::Ready => {}
                        status => not_ready.push((*id, status)),
                    }
                }
                if not_ready.is_empty() {
                    Readiness::Ready
                } else {
                    Readiness::NotReady(NotReady { packs: not_ready })
                }
            }
        };
        cache.verdicts.insert(
            key,
            CachedVerdict {
                profile,
                verdict: verdict.clone(),
                // A cache-hit fall-through continues its rotation from
                // where it stopped, so the steady-state cost stays one
                // hash per pass; a fresh verdict starts the rotation
                // over (the recompute itself re-hashed everything).
                next_check: match rotation {
                    Some(idx) => (idx + 1) % files.len(),
                    None => 0,
                },
            },
        );
        verdict
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A unique temp directory per test, removed on drop.
    struct Scratch {
        path: PathBuf,
    }

    impl Scratch {
        fn new(tag: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("model-packs-{}-{tag}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }

        fn dir(&self, name: &str) -> PathBuf {
            let dir = self.path.join(name);
            fs::create_dir_all(&dir).unwrap();
            dir
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn write_file(dir: &Path, name: &str, bytes: &[u8]) {
        fs::write(dir.join(name), bytes).unwrap();
    }

    /// A PackFile pin matching the bytes just written to `dir`.
    fn pin(dir: &Path, name: &str) -> PackFile {
        let path = dir.join(name);
        PackFile {
            name: name.to_string(),
            sha256: file_sha256(&path).unwrap(),
            size: fs::metadata(&path).unwrap().len(),
        }
    }

    fn spec(version: &str, urls: &[&str], files: Vec<PackFile>) -> PackSpec {
        PackSpec {
            version: version.to_string(),
            urls: urls.iter().map(|u| u.to_string()).collect(),
            files,
        }
    }

    /// A full valid manifest over tiny fake files, all written to
    /// `bundle` (packaging stages every file once; the archive builder
    /// reads them back from there).
    fn fixture(bundle: &Path) -> Manifest {
        write_file(bundle, "pp-ocrv6_tiny_det.onnx", b"tiny-det-v1");
        write_file(bundle, "pp-ocrv6_tiny_rec.onnx", b"tiny-rec-v1");
        write_file(bundle, "ppocrv6_tiny_dict.txt", b"tiny-dict-v1");
        write_file(bundle, "pp-ocrv6_small_det.onnx", b"small-det-v1");
        write_file(bundle, "pp-ocrv6_small_rec.onnx", b"small-rec-v1");
        write_file(bundle, "ppocrv6_dict.txt", b"shared-dict-v1");
        write_file(bundle, "pp-ocrv6_medium_det.onnx", b"medium-det-v1");
        write_file(bundle, "pp-ocrv6_medium_rec.onnx", b"medium-rec-v1");
        write_file(bundle, "korean_pp-ocrv5_mobile_rec.onnx", b"korean-rec-v1");
        write_file(bundle, "ppocrv5_korean_dict.txt", b"korean-dict-v1");

        let mut packs = BTreeMap::new();
        packs.insert(
            PackId::Small,
            spec(
                "1",
                &[
                    "https://github.com/example/releases/download/v0.3.0/pp-ocrv6-small.zip",
                    "https://huggingface.co/PaddlePaddle/PP-OCRv6_small_det_onnx/resolve/main/inference.onnx",
                ],
                vec![
                    pin(bundle, "pp-ocrv6_small_det.onnx"),
                    pin(bundle, "pp-ocrv6_small_rec.onnx"),
                    pin(bundle, "ppocrv6_dict.txt"),
                ],
            ),
        );
        packs.insert(
            PackId::Medium,
            spec(
                "1",
                &["https://github.com/example/releases/download/v0.3.0/pp-ocrv6-medium.zip"],
                vec![
                    pin(bundle, "pp-ocrv6_medium_det.onnx"),
                    pin(bundle, "pp-ocrv6_medium_rec.onnx"),
                    pin(bundle, "ppocrv6_dict.txt"),
                ],
            ),
        );
        packs.insert(
            PackId::Korean,
            spec(
                "1",
                &["https://github.com/example/releases/download/v0.3.0/pp-ocrv6-korean.zip"],
                vec![
                    pin(bundle, "korean_pp-ocrv5_mobile_rec.onnx"),
                    pin(bundle, "ppocrv5_korean_dict.txt"),
                ],
            ),
        );
        Manifest {
            bundled: vec![
                pin(bundle, "pp-ocrv6_tiny_det.onnx"),
                pin(bundle, "pp-ocrv6_tiny_rec.onnx"),
                pin(bundle, "ppocrv6_tiny_dict.txt"),
            ],
            packs,
        }
    }

    // ---- JSON fixtures, built structurally (the manifest is
    // machine-generated; the tests generate it the same way instead of
    // string-surgery on one blob). ----

    fn json_file(name: &str, sha256: &str, size: u64) -> String {
        // serde_json escapes the name (a backslash case must survive as
        // JSON, not as a raw control character).
        let name = serde_json::to_string(name).unwrap();
        format!("{{\"name\": {name}, \"sha256\": \"{sha256}\", \"size\": {size}}}")
    }

    fn json_pack(version: &str, urls: &[&str], files: &[String]) -> String {
        let urls: Vec<String> = urls.iter().map(|u| format!("\"{u}\"")).collect();
        format!(
            "{{\"version\": \"{version}\", \"urls\": [{}], \"files\": [{}]",
            urls.join(", "),
            files.join(", ")
        ) + "}"
    }

    /// The manifest JSON exactly as given — no packs filled in. For
    /// structural tests that pin which error fires.
    fn json_manifest_raw(bundled_files: &[String], packs: &[(&str, String)]) -> String {
        let packs_json: Vec<String> = packs
            .iter()
            .map(|(key, body)| format!("\"{key}\": {body}"))
            .collect();
        format!(
            "{{\"manifest_version\": {MANIFEST_VERSION}, \"bundled\": {{\"files\": [{}]}}, \"packs\": {{{}}}}}",
            bundled_files.join(", "),
            packs_json.join(", ")
        )
    }

    /// Like [`json_manifest_raw`], with any missing pack replaced by a
    /// valid dummy so the tweak under test is the only structural
    /// deviation.
    fn json_manifest(bundled_files: &[String], packs: &[(&str, String)]) -> String {
        let mut all: Vec<(&str, String)> = Vec::new();
        for key in ["small", "medium", "korean"] {
            if let Some((_, body)) = packs.iter().find(|(k, _)| *k == key) {
                all.push((key, body.clone()));
            } else {
                all.push((
                    key,
                    json_pack("1", &["https://dummy"], &[json_file("dummy.onnx", HEX_A, 1)]),
                ));
            }
        }
        json_manifest_raw(bundled_files, &all)
    }

    const HEX_A: &str = "1111111111111111111111111111111111111111111111111111111111111111";
    const HEX_B: &str = "2222222222222222222222222222222222222222222222222222222222222222";

    /// The manifest JSON packaging must emit — the real hash pins from
    /// fetch-pp-ocrv6.ps1 over the launch contract's ten files. Ticket 05
    /// generates this file; this fixture pins the exact shape it must
    /// produce (sizes here are placeholders; the generator writes the
    /// real byte sizes).
    fn realistic_manifest_json() -> String {
        let tiny_det = "193bab7a04fca699a6c82e6abb5b81bdb28177f0abd4062552b04908dafb19f8";
        let tiny_rec = "9ef676d6ed3c88256a2d92c640c44f25b0c40947e111b14b8be8f594091563e6";
        let tiny_dict = "c5cbe34ef40c29c4df07ed012bf96569cb69a2d2a01a07027e9f13cb832bd9cd";
        let small_det = "d73e0058b7a8086bbd57f3d10b8bcd4ff95363f67e06e2762b5e814fe9c9410e";
        let small_rec = "5435fd747c9e0efe15a96d0b378d5bd157e9492ed8fd80edf08f30d02fa24634";
        let shared_dict = "b5f2bfe2bdd9448429e3e82b51c789775d9b42f2403d082b00662eb77e401c5d";
        let medium_det = "eb13b44b25bb36f89528b68720af8a61d9cf381176107f465db1757b65d086e1";
        let medium_rec = "9c09abf0957f7968c7586464b7397b84ad2387a0497a351af40e9acc71b673ba";
        let ko_rec = "92f0b7785e64fc9090106a241cf4c1eb97472824558272751b88a2a4476d3a08";
        let ko_dict = "a88071c68c01707489baa79ebe0405b7beb5cca229f4fc94cc3ef992328802d7";

        json_manifest(
            &[
                json_file("pp-ocrv6_tiny_det.onnx", tiny_det, 1_234_567),
                json_file("pp-ocrv6_tiny_rec.onnx", tiny_rec, 2_345_678),
                json_file("ppocrv6_tiny_dict.txt", tiny_dict, 12_345),
            ],
            &[
                (
                    "small",
                    json_pack(
                        "1",
                        &[
                            "https://github.com/intception/langception/releases/download/v0.3.0/pp-ocrv6-small.zip",
                            "https://huggingface.co/PaddlePaddle/PP-OCRv6_small_det_onnx/resolve/main/inference.onnx",
                            "https://www.modelscope.cn/api/v1/models/greatv/oar-ocr/repo?Revision=master&FilePath=ppocrv6_dict.txt",
                        ],
                        &[
                            json_file("pp-ocrv6_small_det.onnx", small_det, 11_111_111),
                            json_file("pp-ocrv6_small_rec.onnx", small_rec, 22_222_222),
                            json_file("ppocrv6_dict.txt", shared_dict, 333_333),
                        ],
                    ),
                ),
                (
                    "medium",
                    json_pack(
                        "1",
                        &["https://github.com/intception/langception/releases/download/v0.3.0/pp-ocrv6-medium.zip"],
                        &[
                            json_file("pp-ocrv6_medium_det.onnx", medium_det, 44_444_444),
                            json_file("pp-ocrv6_medium_rec.onnx", medium_rec, 55_555_555),
                            json_file("ppocrv6_dict.txt", shared_dict, 333_333),
                        ],
                    ),
                ),
                (
                    "korean",
                    json_pack(
                        "1",
                        &[
                            "https://github.com/intception/langception/releases/download/v0.3.0/pp-ocrv6-korean.zip",
                            "https://huggingface.co/PaddlePaddle/korean_PP-OCRv5_mobile_rec_onnx/resolve/main/inference.onnx",
                            "https://www.modelscope.cn/api/v1/models/greatv/oar-ocr/repo?Revision=master&FilePath=ppocrv5_korean_dict.txt",
                        ],
                        &[
                            json_file("korean_pp-ocrv5_mobile_rec.onnx", ko_rec, 6_666_666),
                            json_file("ppocrv5_korean_dict.txt", ko_dict, 44_444),
                        ],
                    ),
                ),
            ],
        )
    }

    // ---- manifest parsing ----

    #[test]
    fn the_pinned_manifest_fixture_parses_with_every_pack_complete() {
        let manifest = Manifest::parse(&realistic_manifest_json()).unwrap();
        // Every pack present, each with version, URLs, and files pinned.
        for id in PackId::ALL {
            let pack = manifest.pack(id);
            assert!(!pack.urls.is_empty(), "{id:?} must pin at least one URL");
            assert!(!pack.version.is_empty());
            assert!(!pack.files.is_empty());
            for file in &pack.files {
                assert_eq!(file.sha256.len(), 64);
            }
        }
        // The primary URL is the GitHub Releases asset; mirrors follow.
        let small = manifest.pack(PackId::Small);
        assert!(small.urls[0].starts_with("https://github.com/"));
        assert!(small.urls.len() >= 2, "fallback mirrors follow the primary");
        // The bundled baseline is exactly the three Tiny files.
        let bundled_names: Vec<&str> =
            manifest.bundled().iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            bundled_names,
            [
                "pp-ocrv6_tiny_det.onnx",
                "pp-ocrv6_tiny_rec.onnx",
                "ppocrv6_tiny_dict.txt"
            ],
        );
        // The shared dictionary repeats across packs with identical pins.
        let dict = |id| manifest
            .pack(id)
            .files
            .iter()
            .find(|f| f.name == "ppocrv6_dict.txt")
            .unwrap();
        assert_eq!(dict(PackId::Small), dict(PackId::Medium));
    }

    #[test]
    fn a_malformed_manifest_is_a_named_parse_error() {
        let err = Manifest::parse("{ not json").unwrap_err();
        assert!(matches!(err, ManifestError::Parse(_)), "{err:?}");
        assert!(err.to_string().contains("not valid JSON"), "{err}");
    }

    #[test]
    fn a_missing_field_is_a_named_parse_error() {
        // A pack without its version: serde names the field in the message.
        let json = realistic_manifest_json().replace("\"version\": \"1\",", "");
        let err = Manifest::parse(&json).unwrap_err();
        assert!(matches!(err, ManifestError::Parse(_)), "{err:?}");
        assert!(err.to_string().contains("version"), "{err}");
    }

    #[test]
    fn an_unsupported_manifest_version_is_named() {
        let json = json_manifest(
            &[json_file("a.onnx", HEX_A, 1)],
            &[("small", json_pack("1", &["https://u"], &[json_file("a.onnx", HEX_A, 1)]))],
        );
        let json = json.replace(
            &format!("\"manifest_version\": {MANIFEST_VERSION}"),
            "\"manifest_version\": 2",
        );
        let err = Manifest::parse(&json).unwrap_err();
        assert_eq!(
            err,
            ManifestError::UnsupportedVersion { found: 2, supported: MANIFEST_VERSION },
            "{err:?}"
        );
    }

    #[test]
    fn an_unknown_pack_key_is_named_not_ignored() {
        let json = json_manifest_raw(
            &[json_file("a.onnx", HEX_A, 1)],
            &[
                ("small", json_pack("1", &["https://u"], &[json_file("a.onnx", HEX_A, 1)])),
                ("koreen", json_pack("1", &["https://u"], &[json_file("a.onnx", HEX_A, 1)])),
            ],
        );
        let err = Manifest::parse(&json).unwrap_err();
        assert_eq!(err, ManifestError::UnknownPack { name: "koreen".to_string() }, "{err:?}");
    }

    #[test]
    fn a_missing_pack_is_named() {
        let json = json_manifest_raw(
            &[json_file("a.onnx", HEX_A, 1)],
            &[
                ("small", json_pack("1", &["https://u"], &[json_file("a.onnx", HEX_A, 1)])),
                ("medium", json_pack("1", &["https://u"], &[json_file("b.onnx", HEX_B, 1)])),
            ],
        );
        let err = Manifest::parse(&json).unwrap_err();
        assert_eq!(err, ManifestError::MissingPack { pack: PackId::Korean }, "{err:?}");
    }

    #[test]
    fn an_empty_url_list_is_named() {
        let json = json_manifest(
            &[json_file("a.onnx", HEX_A, 1)],
            &[("small", json_pack("1", &[], &[json_file("a.onnx", HEX_A, 1)]))],
        );
        let err = Manifest::parse(&json).unwrap_err();
        assert!(matches!(err, ManifestError::EmptyUrls { pack: PackId::Small }), "{err:?}");
    }

    #[test]
    fn an_empty_file_list_is_named() {
        // No bundled files: the first file-set validation names the
        // empty set.
        let json = json_manifest_raw(
            &[],
            &[("small", json_pack("1", &["https://u"], &[]))],
        );
        let err = Manifest::parse(&json).unwrap_err();
        assert!(
            matches!(err, ManifestError::EmptyFileList { ref context } if context == "bundled"),
            "{err:?}"
        );
        // A pack with an empty file list names the pack.
        let json = json_manifest_raw(
            &[json_file("a.onnx", HEX_A, 1)],
            &[("small", json_pack("1", &["https://u"], &[]))],
        );
        let err = Manifest::parse(&json).unwrap_err();
        assert!(
            matches!(err, ManifestError::EmptyFileList { ref context } if context == "small"),
            "{err:?}"
        );
    }

    #[test]
    fn a_malformed_hash_is_named() {
        let json = json_manifest(
            &[json_file("a.onnx", "not-a-hash", 1)],
            &[("small", json_pack("1", &["https://u"], &[json_file("a.onnx", HEX_A, 1)]))],
        );
        let err = Manifest::parse(&json).unwrap_err();
        assert!(
            matches!(err, ManifestError::BadHash { ref hash, .. } if hash == "not-a-hash"),
            "{err:?}"
        );
    }

    #[test]
    fn a_truncated_hash_is_named() {
        let json = json_manifest(
            &[json_file("a.onnx", &HEX_A[..63], 1)],
            &[("small", json_pack("1", &["https://u"], &[json_file("a.onnx", HEX_A, 1)]))],
        );
        let err = Manifest::parse(&json).unwrap_err();
        assert!(matches!(err, ManifestError::BadHash { .. }), "{err:?}");
    }

    #[test]
    fn uppercase_hashes_normalize_to_lowercase() {
        let json = json_manifest(
            &[json_file("a.onnx", &HEX_A.to_uppercase(), 1)],
            &[("small", json_pack("1", &["https://u"], &[json_file("a.onnx", HEX_A, 1)]))],
        );
        let manifest = Manifest::parse(&json).unwrap();
        assert_eq!(manifest.bundled()[0].sha256, HEX_A);
    }

    #[test]
    fn a_file_name_that_leaves_the_models_dir_is_named() {
        for bad_name in ["../escape.onnx", "a/b.onnx", "a\\b.onnx", "C:/evil.onnx", ".hidden", ""] {
            let json = json_manifest(
                &[json_file(bad_name, HEX_A, 1)],
                &[("small", json_pack("1", &["https://u"], &[json_file("a.onnx", HEX_A, 1)]))],
            );
            let err = Manifest::parse(&json).unwrap_err();
            assert!(
                matches!(err, ManifestError::BadFileName { .. }),
                "\"{bad_name}\" must be a named error, got {err:?}"
            );
        }
    }

    #[test]
    fn a_duplicate_file_inside_one_pack_is_named() {
        let json = json_manifest(
            &[json_file("a.onnx", HEX_A, 1)],
            &[(
                "small",
                json_pack(
                    "1",
                    &["https://u"],
                    &[json_file("a.onnx", HEX_A, 1), json_file("a.onnx", HEX_A, 1)],
                ),
            )],
        );
        let err = Manifest::parse(&json).unwrap_err();
        assert!(
            matches!(err, ManifestError::DuplicateFile { ref name, .. } if name == "a.onnx"),
            "{err:?}"
        );
    }

    #[test]
    fn the_shared_dict_may_repeat_across_packs_only_with_identical_hash() {
        // Identical pins: parse succeeds (the realistic fixture proves
        // it; assert the structural rule directly too).
        let dict = || json_file("ppocrv6_dict.txt", HEX_B, 9);
        let ok = json_manifest(
            &[json_file("a.onnx", HEX_A, 1)],
            &[
                ("small", json_pack("1", &["https://u"], &[dict()])),
                ("medium", json_pack("1", &["https://u"], &[dict()])),
                ("korean", json_pack("1", &["https://u"], &[json_file("k.onnx", HEX_A, 1)])),
            ],
        );
        Manifest::parse(&ok).unwrap();

        // Divergent pins for the same name: the flat dir can hold one
        // copy, so this is a named manifest bug.
        let divergent = json_manifest(
            &[json_file("a.onnx", HEX_A, 1)],
            &[
                ("small", json_pack("1", &["https://u"], &[dict()])),
                ("medium", json_pack("1", &["https://u"], &[json_file("ppocrv6_dict.txt", HEX_A, 9)])),
                ("korean", json_pack("1", &["https://u"], &[json_file("k.onnx", HEX_A, 1)])),
            ],
        );
        let err = Manifest::parse(&divergent).unwrap_err();
        assert!(
            matches!(err, ManifestError::ConflictingFile { ref name, .. } if name == "ppocrv6_dict.txt"),
            "{err:?}"
        );
    }

    #[test]
    fn an_unknown_field_is_a_named_parse_error_not_silently_ignored() {
        let json = realistic_manifest_json().replace(
            &format!("\"manifest_version\": {MANIFEST_VERSION}"),
            &format!("\"manifest_version\": {MANIFEST_VERSION}, \"surprise\": true"),
        );
        let err = Manifest::parse(&json).unwrap_err();
        assert!(matches!(err, ManifestError::Parse(_)), "{err:?}");
    }

    // ---- required packs ----

    #[test]
    fn the_pack_matrix_follows_the_effective_selection() {
        use crate::ports::SourceLanguage as Lang;
        use ModelTier::*;
        let cases: &[(Lang, ModelTier, &[PackId])] = &[
            (Lang::En, Tiny, &[]),
            (Lang::Zh, Tiny, &[]),
            // ja+tiny is the substitution: the effective small set.
            (Lang::Ja, Tiny, &[PackId::Small]),
            (Lang::Ko, Tiny, &[PackId::Korean]),
            (Lang::Ko, Small, &[PackId::Small, PackId::Korean]),
            (Lang::Ko, Medium, &[PackId::Medium, PackId::Korean]),
            (Lang::En, Small, &[PackId::Small]),
            (Lang::Ja, Small, &[PackId::Small]),
            (Lang::Zh, Small, &[PackId::Small]),
            (Lang::En, Medium, &[PackId::Medium]),
            (Lang::Ja, Medium, &[PackId::Medium]),
            (Lang::Zh, Medium, &[PackId::Medium]),
        ];
        for (source, tier, expected) in cases {
            assert_eq!(
                required_packs(*source, *tier),
                *expected,
                "{source:?}+{tier:?} must require {expected:?}"
            );
        }
    }

    #[test]
    fn engine_values_outside_this_engine_need_no_pack() {
        use crate::ports::SourceLanguage as Lang;
        assert_eq!(required_packs_for_engine_value("windows", Lang::En), None);
        assert_eq!(required_packs_for_engine_value("pp-ocrv6-xl", Lang::En), None);
        assert_eq!(
            required_packs_for_engine_value("pp-ocrv6-tiny", Lang::En),
            Some(&[][..])
        );
        assert_eq!(
            required_packs_for_engine_value("pp-ocrv6-small", Lang::Ko),
            Some(&[PackId::Small, PackId::Korean][..])
        );
        // The legacy value maps to small (ppocr's rule).
        assert_eq!(
            required_packs_for_engine_value("paddle", Lang::Ja),
            Some(&[PackId::Small][..])
        );
    }

    // ---- on-disk status ----

    #[test]
    fn an_untouched_directory_reads_as_missing() {
        let scratch = Scratch::new("status-empty");
        let active = scratch.dir("active");
        let bundle = scratch.dir("bundle");
        let manifest = fixture(&bundle);
        let status = pack_status(manifest.pack(PackId::Small), &active);
        assert_eq!(
            status,
            PackStatus::Missing {
                files: manifest
                    .pack(PackId::Small)
                    .files
                    .iter()
                    .map(|f| f.name.clone())
                    .collect(),
            }
        );
    }

    #[test]
    fn a_directory_with_every_file_verifying_reads_as_ready() {
        let scratch = Scratch::new("status-ready");
        let active = scratch.dir("active");
        let bundle = scratch.dir("bundle");
        let manifest = fixture(&bundle);
        let pack = manifest.pack(PackId::Small);
        for file in &pack.files {
            fs::copy(bundle.join(&file.name), active.join(&file.name)).unwrap();
        }
        assert_eq!(pack_status(pack, &active), PackStatus::Ready);
        // The bundled baseline is a different file set: not copied yet.
        assert_eq!(
            bundled_status(&manifest, &active),
            PackStatus::Missing {
                files: manifest.bundled().iter().map(|f| f.name.clone()).collect(),
            }
        );
    }

    #[test]
    fn a_present_file_with_wrong_bytes_is_stale_and_names_the_file() {
        let scratch = Scratch::new("status-stale");
        let active = scratch.dir("active");
        let bundle = scratch.dir("bundle");
        let manifest = fixture(&bundle);
        let pack = manifest.pack(PackId::Small);
        for file in &pack.files {
            fs::copy(bundle.join(&file.name), active.join(&file.name)).unwrap();
        }
        // Corrupt one file in place, keeping its size (hash must decide).
        fs::write(active.join("pp-ocrv6_small_rec.onnx"), b"small-rec-XXX").unwrap();
        let status = pack_status(pack, &active);
        assert_eq!(
            status,
            PackStatus::Stale {
                files: vec!["pp-ocrv6_small_rec.onnx".to_string()]
            },
            "{status:?}"
        );
    }

    #[test]
    fn an_absent_file_is_missing_even_when_the_others_verify() {
        let scratch = Scratch::new("status-half");
        let active = scratch.dir("active");
        let bundle = scratch.dir("bundle");
        let manifest = fixture(&bundle);
        let pack = manifest.pack(PackId::Small);
        // Copy only two of the three files.
        for file in pack.files.iter().take(2) {
            fs::copy(bundle.join(&file.name), active.join(&file.name)).unwrap();
        }
        let status = pack_status(pack, &active);
        assert_eq!(
            status,
            PackStatus::Missing {
                files: vec!["ppocrv6_dict.txt".to_string()]
            },
            "{status:?}"
        );
    }

    #[test]
    fn a_size_mismatch_is_stale() {
        let scratch = Scratch::new("status-size");
        let active = scratch.dir("active");
        let bundle = scratch.dir("bundle");
        let manifest = fixture(&bundle);
        let pack = manifest.pack(PackId::Small);
        for file in &pack.files {
            fs::copy(bundle.join(&file.name), active.join(&file.name)).unwrap();
        }
        // Different length: enough evidence on its own.
        fs::write(active.join("pp-ocrv6_small_det.onnx"), b"short").unwrap();
        let status = pack_status(pack, &active);
        assert_eq!(
            status,
            PackStatus::Stale {
                files: vec!["pp-ocrv6_small_det.onnx".to_string()]
            },
            "{status:?}"
        );
    }

    // ---- delete-unused (the Engines tab's cleanup button, ticket 04) ----

    /// Copies every pinned file — bundled and all three packs — into
    /// `active`, the everything-downloaded starting point.
    fn populate_everything(bundle: &Path, active: &Path, manifest: &Manifest) {
        fs::create_dir_all(active).unwrap();
        for file in manifest.bundled() {
            fs::copy(bundle.join(&file.name), active.join(&file.name)).unwrap();
        }
        for id in PackId::ALL {
            for file in &manifest.pack(id).files {
                fs::copy(bundle.join(&file.name), active.join(&file.name)).unwrap();
            }
        }
    }

    #[test]
    fn delete_unused_removes_only_files_no_kept_set_pins() {
        let scratch = Scratch::new("delete-small-kept");
        let active = scratch.dir("active");
        let bundle = scratch.dir("bundle");
        let manifest = fixture(&bundle);
        populate_everything(&bundle, &active, &manifest);

        let removed = delete_unused_packs(&manifest, &active, &[PackId::Small]).unwrap();
        // Medium's and Korean's exclusively-owned files are gone, in
        // manifest order.
        assert_eq!(
            removed,
            vec![
                "pp-ocrv6_medium_det.onnx".to_string(),
                "pp-ocrv6_medium_rec.onnx".to_string(),
                "korean_pp-ocrv5_mobile_rec.onnx".to_string(),
                "ppocrv5_korean_dict.txt".to_string(),
            ]
        );
        for name in &removed {
            assert!(!active.join(name).exists(), "{name} must be gone");
        }
        // The shared dictionary repeats across Small and Medium: Small is
        // kept, so the file it both pin survives even though the removed
        // Medium pins it too.
        assert!(active.join("ppocrv6_dict.txt").is_file());
        // The kept pack's own files and the bundled baseline survive.
        for file in &manifest.pack(PackId::Small).files {
            assert!(active.join(&file.name).is_file(), "{} must stay", file.name);
        }
        for file in manifest.bundled() {
            assert!(active.join(&file.name).is_file(), "{} must stay", file.name);
        }
        // The kept pack still verifies Ready after the sweep.
        assert_eq!(pack_status(manifest.pack(PackId::Small), &active), PackStatus::Ready);
    }

    #[test]
    fn delete_unused_with_nothing_kept_removes_every_pack_file_but_never_bundled() {
        let scratch = Scratch::new("delete-none-kept");
        let active = scratch.dir("active");
        let bundle = scratch.dir("bundle");
        let manifest = fixture(&bundle);
        populate_everything(&bundle, &active, &manifest);

        // Windows OCR selected: no pack is required, every pack file goes
        // (the shared dictionary is one name across Small and Medium).
        let removed = delete_unused_packs(&manifest, &active, &[]).unwrap();
        assert_eq!(removed.len(), 7, "every non-bundled pin: {removed:?}");
        for id in PackId::ALL {
            for file in &manifest.pack(id).files {
                assert!(!active.join(&file.name).exists(), "{} must be gone", file.name);
            }
        }
        // The bundled Tiny baseline is never a pack's to delete.
        for file in manifest.bundled() {
            assert!(active.join(&file.name).is_file(), "{} must stay", file.name);
        }
    }

    #[test]
    fn delete_unused_keeps_a_required_packs_stale_files() {
        let scratch = Scratch::new("delete-kept-stale");
        let active = scratch.dir("active");
        let bundle = scratch.dir("bundle");
        let manifest = fixture(&bundle);
        populate_everything(&bundle, &active, &manifest);
        // The kept pack has a stale file — in use, the download refreshes
        // it; the delete button must not touch it.
        fs::write(active.join("pp-ocrv6_small_det.onnx"), b"corrupted").unwrap();

        let removed = delete_unused_packs(&manifest, &active, &[PackId::Small]).unwrap();
        assert!(!removed.contains(&"pp-ocrv6_small_det.onnx".to_string()));
        assert!(active.join("pp-ocrv6_small_det.onnx").is_file());
    }

    #[test]
    fn delete_unused_on_a_never_downloaded_directory_removes_nothing() {
        let scratch = Scratch::new("delete-empty-dir");
        let active = scratch.dir("active");
        let bundle = scratch.dir("bundle");
        let manifest = fixture(&bundle);

        let removed = delete_unused_packs(&manifest, &active, &[PackId::Small]).unwrap();
        assert!(removed.is_empty(), "nothing on disk, nothing removed");
    }

    #[test]
    fn delete_unused_with_every_pack_kept_removes_nothing() {
        let scratch = Scratch::new("delete-all-kept");
        let active = scratch.dir("active");
        let bundle = scratch.dir("bundle");
        let manifest = fixture(&bundle);
        populate_everything(&bundle, &active, &manifest);

        // Korean + medium: every pack the manifest describes is required.
        let removed =
            delete_unused_packs(&manifest, &active, &[PackId::Small, PackId::Medium, PackId::Korean])
                .unwrap();
        assert!(removed.is_empty(), "kept packs pin every file: {removed:?}");
        for id in PackId::ALL {
            for file in &manifest.pack(id).files {
                assert!(active.join(&file.name).is_file(), "{} must stay", file.name);
            }
        }
    }

    // ---- assembly: bundled Tiny baseline ----

    #[test]
    fn first_engine_use_copies_the_whole_tiny_baseline() {
        let scratch = Scratch::new("install-bundled-first");
        let active = scratch.dir("active");
        let bundle = scratch.dir("bundle");
        let manifest = fixture(&bundle);
        let copied = install_bundled_files(&bundle, &manifest, &active).unwrap();
        assert_eq!(copied, manifest.bundled().len() as u32);
        assert_eq!(bundled_status(&manifest, &active), PackStatus::Ready);
    }

    #[test]
    fn bundled_copying_is_idempotent_when_files_verify() {
        let scratch = Scratch::new("install-bundled-idempotent");
        let active = scratch.dir("active");
        let bundle = scratch.dir("bundle");
        let manifest = fixture(&bundle);
        install_bundled_files(&bundle, &manifest, &active).unwrap();
        let copied = install_bundled_files(&bundle, &manifest, &active).unwrap();
        assert_eq!(copied, 0, "verified files must not be re-copied");
    }

    #[test]
    fn a_stale_tiny_file_is_replaced_from_the_bundle() {
        let scratch = Scratch::new("install-bundled-stale");
        let active = scratch.dir("active");
        let bundle = scratch.dir("bundle");
        let manifest = fixture(&bundle);
        install_bundled_files(&bundle, &manifest, &active).unwrap();
        // Mutate one active copy in place.
        fs::write(active.join("ppocrv6_tiny_dict.txt"), b"corrupted").unwrap();
        let copied = install_bundled_files(&bundle, &manifest, &active).unwrap();
        assert_eq!(copied, 1);
        assert_eq!(bundled_status(&manifest, &active), PackStatus::Ready);
    }

    #[test]
    fn a_mutated_bundle_source_is_rejected_before_anything_is_copied() {
        let scratch = Scratch::new("install-bundled-mutated");
        let active = scratch.dir("active");
        let bundle = scratch.dir("bundle");
        let manifest = fixture(&bundle);
        // Corrupt one bundle source the active dir needs.
        fs::write(bundle.join("pp-ocrv6_tiny_rec.onnx"), b"tampered bytes").unwrap();
        let err = install_bundled_files(&bundle, &manifest, &active).unwrap_err();
        assert!(
            matches!(err, PackError::BundleHashMismatch { ref file, .. }
                if file == "pp-ocrv6_tiny_rec.onnx"),
            "{err}"
        );
        // Nothing landed, not even the still-good files.
        assert_eq!(
            fs::read_dir(&active).unwrap().count(),
            0,
            "a mutated bundle must copy nothing"
        );
    }

    #[test]
    fn a_missing_bundle_file_is_a_named_error() {
        let scratch = Scratch::new("install-bundled-missing");
        let active = scratch.dir("active");
        let bundle = scratch.dir("bundle");
        let manifest = fixture(&bundle);
        fs::remove_file(bundle.join("ppocrv6_tiny_dict.txt")).unwrap();
        let err = install_bundled_files(&bundle, &manifest, &active).unwrap_err();
        assert!(
            matches!(err, PackError::BundleFileMissing { ref file, .. }
                if file == "ppocrv6_tiny_dict.txt"),
            "{err}"
        );
    }

    // ---- assembly: pack archives ----

    /// Builds a zip archive from (name, bytes) entries.
    fn write_zip<N: AsRef<str>, B: AsRef<[u8]>>(path: &Path, entries: &[(N, B)]) {
        let file = fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, bytes) in entries {
            zip.start_file(name.as_ref(), options).unwrap();
            zip.write_all(bytes.as_ref()).unwrap();
        }
        zip.finish().unwrap();
    }

    /// The pack's manifest files as archive entries, read from `bundle`.
    fn archive_entries<'a>(bundle: &Path, pack: &'a PackSpec) -> Vec<(&'a str, Vec<u8>)> {
        pack.files
            .iter()
            .map(|f| (f.name.as_str(), fs::read(bundle.join(&f.name)).unwrap()))
            .collect()
    }

    #[test]
    fn a_good_archive_installs_every_pack_file_and_replaces_stale_files() {
        let scratch = Scratch::new("install-pack-good");
        let active = scratch.dir("active");
        let bundle = scratch.dir("bundle");
        let manifest = fixture(&bundle);
        let pack = manifest.pack(PackId::Small);

        // Pre-seed the active dir with stale versions of every file.
        fs::create_dir_all(&active).unwrap();
        for file in &pack.files {
            fs::write(active.join(&file.name), b"old-and-stale").unwrap();
        }

        let archive = scratch.path().join("small.zip");
        let mut entries = archive_entries(&bundle, pack);
        entries.push(("EXTRA-license.txt", b"ignored".to_vec()));
        write_zip(&archive, &entries);

        install_pack_archive(&archive, PackId::Small, pack, &active).unwrap();
        assert_eq!(pack_status(pack, &active), PackStatus::Ready, "stale files replaced");
        assert!(
            !active.join("EXTRA-license.txt").exists(),
            "extra archive entries are not installed"
        );
    }

    #[test]
    fn an_archive_with_one_shared_prefix_dir_installs_the_same() {
        let scratch = Scratch::new("install-pack-prefix");
        let active = scratch.dir("active");
        let bundle = scratch.dir("bundle");
        let manifest = fixture(&bundle);
        let pack = manifest.pack(PackId::Small);

        let archive = scratch.path().join("small-folder.zip");
        let entries: Vec<(String, Vec<u8>)> = archive_entries(&bundle, pack)
            .into_iter()
            .map(|(name, bytes)| (format!("small/{name}"), bytes))
            .collect();
        write_zip(&archive, &entries);

        install_pack_archive(&archive, PackId::Small, pack, &active).unwrap();
        assert_eq!(pack_status(pack, &active), PackStatus::Ready);
    }

    #[test]
    fn an_archive_missing_a_manifest_file_is_rejected_whole() {
        let scratch = Scratch::new("install-pack-missing");
        let active = scratch.dir("active");
        let bundle = scratch.dir("bundle");
        let manifest = fixture(&bundle);
        let pack = manifest.pack(PackId::Small);

        // Pre-seed stale files; the rejection must leave them untouched.
        fs::create_dir_all(&active).unwrap();
        for file in &pack.files {
            fs::write(active.join(&file.name), b"old-and-stale").unwrap();
        }

        // Archive omits one manifest file.
        let archive = scratch.path().join("incomplete.zip");
        let entries: Vec<(&str, Vec<u8>)> = archive_entries(&bundle, pack)
            .into_iter()
            .take(2)
            .collect();
        write_zip(&archive, &entries);

        let err = install_pack_archive(&archive, PackId::Small, pack, &active).unwrap_err();
        assert!(
            matches!(err, PackError::EntryMissing { ref file } if file == "ppocrv6_dict.txt"),
            "{err}"
        );
        for file in &pack.files {
            assert_eq!(
                fs::read(active.join(&file.name)).unwrap(),
                b"old-and-stale",
                "a rejected archive must not touch the active directory"
            );
        }
        assert_no_residue(&active);
    }

    #[test]
    fn an_archive_with_corrupted_bytes_is_rejected_whole() {
        let scratch = Scratch::new("install-pack-corrupt");
        let active = scratch.dir("active");
        let bundle = scratch.dir("bundle");
        let manifest = fixture(&bundle);
        let pack = manifest.pack(PackId::Small);
        fs::create_dir_all(&active).unwrap();
        fs::write(active.join(&pack.files[0].name), b"old-and-stale").unwrap();

        // One entry carries same-length bytes that fail its hash pin
        // (the size check passes so the hash is what rejects).
        let archive = scratch.path().join("corrupt.zip");
        let mut entries = archive_entries(&bundle, pack);
        assert_eq!(entries[1].1.len(), 12, "fixture: small-rec-v1 is 12 bytes");
        entries[1].1 = b"tamperedrec!".to_vec();
        write_zip(&archive, &entries);

        let err = install_pack_archive(&archive, PackId::Small, pack, &active).unwrap_err();
        assert!(
            matches!(err, PackError::HashMismatch { ref file, .. } if *file == pack.files[1].name),
            "{err}"
        );
        assert_eq!(
            fs::read(active.join(&pack.files[0].name)).unwrap(),
            b"old-and-stale",
            "a rejected archive must not touch the active directory"
        );
        assert_no_residue(&active);
    }

    #[test]
    fn an_archive_file_larger_than_its_pinned_size_is_rejected() {
        let scratch = Scratch::new("install-pack-bomb");
        let active = scratch.dir("active");
        let bundle = scratch.dir("bundle");
        let manifest = fixture(&bundle);
        let pack = manifest.pack(PackId::Small);

        let archive = scratch.path().join("bomb.zip");
        let entries: Vec<(&str, &[u8])> = pack
            .files
            .iter()
            .map(|f| (f.name.as_str(), &b"pad-pad-pad-pad-pad"[..]))
            .collect();
        write_zip(&archive, &entries);

        let err = install_pack_archive(&archive, PackId::Small, pack, &active).unwrap_err();
        assert!(matches!(err, PackError::SizeMismatch { .. }), "{err}");
        assert_no_residue(&active);
    }

    #[test]
    fn an_entry_escaping_the_models_dir_is_named_unsafe() {
        let scratch = Scratch::new("install-pack-escape");
        let active = scratch.dir("active");
        let bundle = scratch.dir("bundle");
        let manifest = fixture(&bundle);
        let pack = manifest.pack(PackId::Small);

        let archive = scratch.path().join("escape.zip");
        let mut entries = archive_entries(&bundle, pack);
        entries.push(("../escape.txt", b"evil".to_vec()));
        write_zip(&archive, &entries);

        let err = install_pack_archive(&archive, PackId::Small, pack, &active).unwrap_err();
        assert!(matches!(err, PackError::UnsafeEntry { .. }), "{err}");
        // The escape must not have written anywhere.
        assert!(!scratch.path().join("escape.txt").exists());
        assert!(!active.parent().unwrap().join("escape.txt").exists());
        assert_no_residue(&active);
    }

    #[test]
    fn a_mixed_prefix_layout_is_a_named_layout_error() {
        let scratch = Scratch::new("install-pack-mixed");
        let active = scratch.dir("active");
        let bundle = scratch.dir("bundle");
        let manifest = fixture(&bundle);
        let pack = manifest.pack(PackId::Small);

        let archive = scratch.path().join("mixed.zip");
        let entries: Vec<(String, Vec<u8>)> = archive_entries(&bundle, pack)
            .into_iter()
            .enumerate()
            .map(|(i, (name, bytes))| {
                if i == 0 {
                    (format!("small/{name}"), bytes)
                } else {
                    (name.to_string(), bytes)
                }
            })
            .collect();
        write_zip(&archive, &entries);

        let err = install_pack_archive(&archive, PackId::Small, pack, &active).unwrap_err();
        assert!(matches!(err, PackError::BadArchiveLayout(_)), "{err}");
        assert_no_residue(&active);
    }

    #[test]
    fn the_active_directory_stays_flat_after_install() {
        let scratch = Scratch::new("install-flat");
        let active = scratch.dir("active");
        let bundle = scratch.dir("bundle");
        let manifest = fixture(&bundle);
        install_bundled_files(&bundle, &manifest, &active).unwrap();
        let pack = manifest.pack(PackId::Korean);
        let archive = scratch.path().join("korean.zip");
        let entries = archive_entries(&bundle, pack);
        write_zip(&archive, &entries);
        install_pack_archive(&archive, PackId::Korean, pack, &active).unwrap();

        // The sidecar's contract: one flat directory of model files —
        // the three tiny files plus the two Korean ones, nothing nested,
        // no staging residue.
        let mut names: Vec<String> = fs::read_dir(&active)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        let mut expected: Vec<String> = manifest
            .bundled()
            .iter()
            .chain(pack.files.iter())
            .map(|f| f.name.clone())
            .collect();
        expected.sort();
        assert_eq!(names, expected, "the active directory must stay flat");
        assert_no_residue(&active);
    }

    /// No transient artifact may survive an install, success or failure:
    /// no staging sibling beside the active directory, no copy temp or
    /// replacement backup inside it — every one carries the module's
    /// residue prefix.
    fn assert_no_residue(active: &Path) {
        let parent = active.parent().unwrap();
        let outside: Vec<String> = fs::read_dir(parent)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(RESIDUE_PREFIX))
            .collect();
        assert!(outside.is_empty(), "staging residue left behind: {outside:?}");
        let inside: Vec<String> = fs::read_dir(active)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(RESIDUE_PREFIX))
            .collect();
        assert!(inside.is_empty(), "copy/backup residue left behind: {inside:?}");
    }

    // ---- the shipped manifest ----

    #[test]
    fn load_from_bundle_reads_the_manifest_beside_the_models() {
        let scratch = Scratch::new("load-manifest");
        let bundle = scratch.dir("bundle");
        fs::write(
            bundle.join(MANIFEST_FILE_NAME),
            realistic_manifest_json(),
        )
        .unwrap();
        let manifest = Manifest::load_from_bundle(&bundle).unwrap();
        assert_eq!(manifest.bundled().len(), 3);
        assert_eq!(manifest.pack(PackId::Small).urls.len(), 3);
    }

    #[test]
    fn a_missing_manifest_file_is_a_named_error_carrying_the_path() {
        let scratch = Scratch::new("load-manifest-missing");
        let bundle = scratch.dir("bundle");
        let err = Manifest::load_from_bundle(&bundle).unwrap_err();
        assert!(
            matches!(&err, ManifestError::Read { path, .. }
                if path == &bundle.join(MANIFEST_FILE_NAME)),
            "{err}"
        );
        assert!(err.to_string().contains("cannot read the models manifest"), "{err}");
    }
}
