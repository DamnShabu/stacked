//! Keeping more than one Roblox build, and choosing between them.
//!
//! There was one slot. `~/.cache/cordial/lib/<abi>` held one `libroblox.so`
//! and a stamp naming the APK it came out of, and a new build overwrote the
//! old one. That is fine until a Roblox build regresses, and then it is the
//! whole problem: the engine is the one component nobody here controls, and a
//! user whose game stopped working had no way back short of finding an APK
//! themselves.
//!
//! So the slot becomes a store. `~/.cache/cordial/builds/<version>/` holds the
//! extracted library and its stamp, and the old single-slot path becomes a
//! symlink into whichever entry is current. Everything that already computes
//! that path -- `justfile`, `cordial-shell`'s `install::engine_cache`,
//! `cordial_update::install::engine_dir`, and every `--lib-dir` anybody has
//! typed -- keeps working without being told, because a symlink is a directory
//! to everything that opens a file through it.
//! [ADR-033](../../../docs/adr/ADR-033-roblox-versions-are-a-keyed-store.md)
//! records the decision and what it deliberately leaves out.
//!
//! **The version is a directory name, and it comes from scanning a binary.**
//! [`crate::engine::scan`] finds it by looking for a plausible run of digits
//! and dots in `libroblox.so`, which is a heuristic over bytes Cordial did not
//! write. Anything derived that way and then joined onto a path is a directory
//! traversal waiting to be reported as one, so [`is_valid_version`] is checked
//! at every entry point here rather than trusted to a caller -- and it is a
//! whitelist of digits and dots, not a blacklist of `..`.
//!
//! **Ordering is component-wise and numeric, never lexicographic.** Roblox's
//! versions look like `2.738.0.1393`, and sorted as strings `2.99` comes after
//! `2.738` -- so a store sorted the obvious way offers the wrong build as the
//! newest, and prunes the right one. That is a one-line bug with a
//! months-later symptom.

use crate::sha256::{Hasher, Sha256Hash};
use std::cmp::Ordering;
use std::io;
use std::path::{Path, PathBuf};

/// Where the entries live, under the cache root.
pub const BUILDS: &str = "builds";

/// The Cordial version that last loaded this entry, written beside it.
///
/// A compatibility record and not a decoration. Cordial's own shim is
/// versioned too: an old Roblox build can import a symbol the current shim
/// does not answer, and that fails at `dlopen` with `cannot locate symbol`
/// before any window appears. A picker that offers a build nothing here has
/// ever loaded is offering a crash, so an entry says whether it has been run
/// and by what -- and an entry with no record is offered *with that said*,
/// never hidden.
pub const LOADED_BY: &str = ".loaded-by";

/// A SHA-256 of the entry's own `libroblox.so`, written beside it.
///
/// Recorded once, by [`ensure_content_hash`], and trusted after that rather
/// than recomputed on every launch. It hashes the engine rather than the APK
/// it came out of because the APK is not a stable name for one build: ADR-025
/// measured Google Play's split bundle and APKPure's monolithic archive
/// carrying the same signed engine in containers 150 MB and 229 MB — a hash of
/// either container would call those two downloads different builds, which is
/// exactly backwards. [ADR-037](../../../docs/adr/ADR-037-one-lock-and-a-content-hash-for-the-build-store.md)
/// records why the directory itself stays named by version rather than by this.
pub const CONTENT_SHA256: &str = ".content-sha256";

/// How many entries to keep, counting the current one.
///
/// By count rather than by age, because age says nothing about how much disk
/// this is using and an engine directory is not small -- about 115 MB each on
/// the build measured here. An unbounded store is a disk-full bug reported as
/// something else entirely.
pub const KEEP: usize = 3;

/// `~/.cache/cordial/builds`.
pub fn root() -> PathBuf {
    crate::install::cache_root().join(BUILDS)
}

/// Whether `version` may be used as a directory name.
///
/// Digits and dots, nothing else, and neither end may be a dot. That rejects
/// `..` and `/` without naming them, which is the point: a blacklist of the
/// traversal spellings somebody thought of is how the next spelling gets
/// through. The length cap is the same one [`crate::engine`] applies to a run
/// of digits when it scans, so nothing this accepts is longer than something
/// that could have been scanned.
pub fn is_valid_version(version: &str) -> bool {
    !version.is_empty()
        && version.len() <= 32
        && !version.starts_with('.')
        && !version.ends_with('.')
        && version.bytes().all(|b| b.is_ascii_digit() || b == b'.')
}

/// Order two Roblox version strings, oldest first.
///
/// Component-wise and numeric. A component that does not parse -- which
/// [`is_valid_version`] should already have excluded, and which an empty
/// component from `1..2` would produce -- compares as zero rather than
/// panicking, because the ordering of a string that should not exist is not
/// worth an unwrap.
pub fn compare(a: &str, b: &str) -> Ordering {
    let mut left = a.split('.');
    let mut right = b.split('.');
    loop {
        match (left.next(), right.next()) {
            (None, None) => return Ordering::Equal,
            (l, r) => {
                let l = l.unwrap_or("").parse::<u64>().unwrap_or(0);
                let r = r.unwrap_or("").parse::<u64>().unwrap_or(0);
                match l.cmp(&r) {
                    Ordering::Equal => continue,
                    other => return other,
                }
            }
        }
    }
}

/// One build in the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub version: String,
    pub dir: PathBuf,
    /// The Cordial version that last loaded this build, if one ever has.
    ///
    /// `None` is the honest answer for an entry that has been fetched and
    /// never launched, and for every entry that existed before this was
    /// recorded. It is not "incompatible" and must not be presented as one.
    pub loaded_by: Option<String>,
    /// What the entry occupies, so a picker can say what deleting it frees.
    ///
    /// Hard-linked archives are counted here even though they share their
    /// inode with `build/<abi>`, so this over-reports the current entry by the
    /// size of the APKs. Deliberately: the number a user wants is "how big is
    /// this build", and explaining inode sharing in a picker helps nobody.
    pub bytes: u64,
    /// Whether this entry holds the APKs as well as the engine, and can
    /// therefore be launched on its own.
    ///
    /// False for every entry keyed before the archives were kept, and for one
    /// whose linking failed. Such an entry is not offered as a pin -- an engine
    /// paired with another version's assets is the mismatch
    /// [`crate::cache`] exists to prevent -- but it is still listed, because
    /// "you have this build and cannot select it" is information and hiding it
    /// is not.
    pub complete: bool,
    /// A SHA-256 of this entry's `libroblox.so`, if one has been recorded.
    ///
    /// `None` for an entry kept before [ADR-037] shipped, and for one whose
    /// hash could not be computed -- neither is fatal to launching it. See
    /// [`CONTENT_SHA256`].
    ///
    /// [ADR-037]: ../../../docs/adr/ADR-037-one-lock-and-a-content-hash-for-the-build-store.md
    pub content_hash: Option<Sha256Hash>,
}

impl Entry {
    /// The `base.apk` in this entry, if it has one.
    pub fn base_apk(&self) -> Option<PathBuf> {
        let base = self.dir.join(crate::install::BASE_APK);
        base.is_file().then_some(base)
    }
}

/// The directory an entry would occupy, or `None` if the version is not a name
/// this will write.
pub fn entry_dir_in(root: &Path, version: &str) -> Option<PathBuf> {
    is_valid_version(version).then(|| root.join(version))
}

pub fn entry_dir(version: &str) -> Option<PathBuf> {
    entry_dir_in(&root(), version)
}

/// What the store holds, newest first.
///
/// A directory whose name is not a version is skipped rather than reported.
/// The cache is a place users and packagers poke at, and a stray directory
/// there is not an error worth surfacing -- but it is also not something to
/// offer as a build, and it is emphatically not something [`prune_in`] should
/// feel free to delete.
pub fn list_in(root: &Path) -> Vec<Entry> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut found: Vec<Entry> = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Some(version) = dir.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !is_valid_version(version) {
            continue;
        }
        // An entry with no engine in it is not a build. That is what a killed
        // install leaves behind, and offering it would be offering a launch
        // that fails on a missing file.
        if !dir.join(crate::engine::LIBRARY).is_file() {
            continue;
        }
        found.push(Entry {
            version: version.to_string(),
            loaded_by: loaded_by(&dir),
            bytes: bytes_in(&dir),
            complete: dir.join(crate::install::BASE_APK).is_file(),
            content_hash: content_hash(&dir),
            dir,
        });
    }
    found.sort_by(|a, b| compare(&b.version, &a.version));
    found
}

pub fn list() -> Vec<Entry> {
    list_in(&root())
}

/// One level, not a walk: an entry holds a handful of files and no
/// subdirectories, and a recursive size of a cache directory is a way to spend
/// a second of somebody's launch on a number shown in a picker.
fn bytes_in(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries.flatten().filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum()
}

/// The Cordial version that last loaded the build in `dir`.
pub fn loaded_by(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join(LOADED_BY)).ok()?;
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// The content hash recorded for the entry in `dir`, if there is one.
pub fn content_hash(dir: &Path) -> Option<Sha256Hash> {
    let text = std::fs::read_to_string(dir.join(CONTENT_SHA256)).ok()?;
    Sha256Hash::parse(text.trim()).ok()
}

/// Record `hash` as `dir`'s content hash.
fn record_content_hash(dir: &Path, hash: &Sha256Hash) -> io::Result<()> {
    std::fs::write(dir.join(CONTENT_SHA256), hash.to_string())
}

/// A streamed SHA-256 of `path`, read a block at a time so a 100+ MB engine is
/// never held whole -- the same reason [`crate::sha256::Hasher`] exists rather
/// than `Sha256Hash::of` being used directly.
fn hash_file(path: &Path) -> io::Result<Sha256Hash> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Hasher::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finish())
}

/// Make sure `entry` has a recorded content hash, computing one only if it
/// does not already have one.
///
/// Called from [`adopt_current`], which is where every install path but
/// [`crate::install::file_into_store`] already converges to key a build; that
/// one calls this itself. See
/// [ADR-037](../../../docs/adr/ADR-037-one-lock-and-a-content-hash-for-the-build-store.md).
///
/// **The cache-hit path is what makes this safe to call wherever a build is
/// keyed**, rather than hashing a 100+ MB file each time. This used to say
/// `adopt_current` reaches it on every launch; it does not, because once the
/// single-slot path is a link `adopt_current` returns before getting here. An
/// entry keyed before this existed therefore has no hash until it is keyed
/// again, which ADR-037 now says too.
///
/// Failure is reported to the caller and is not fatal to keying the build --
/// an entry with no recorded hash is exactly what one predating this feature
/// looks like, and it still launches.
pub fn ensure_content_hash(entry: &Path) -> Option<Sha256Hash> {
    if let Some(existing) = content_hash(entry) {
        return Some(existing);
    }
    let library = entry.join(crate::engine::LIBRARY);
    if !library.is_file() {
        return None;
    }
    let hash = hash_file(&library).ok()?;
    record_content_hash(entry, &hash).ok()?;
    Some(hash)
}

/// The entry under `root` whose recorded content hash is `hash`, if any.
///
/// A lookup primitive and deliberately unwired: nothing here yet uses it to
/// recognise that a fresh download is byte-identical to a build already kept
/// under a different version label and link rather than duplicate it. See
/// ADR-037's "What would change this".
pub fn find_by_content_hash(root: &Path, hash: &Sha256Hash) -> Option<Entry> {
    list_in(root).into_iter().find(|e| e.content_hash.as_ref() == Some(hash))
}

/// An advisory lock over every write to the store at `root`, held for the
/// duration of one mutation.
///
/// Three call sites key or prune a build --
/// [`crate::provider::obtain_and_install`], [`crate::provider::obtain_into_store`]
/// and `cordial-shell`'s extraction of a build Sober or the user already
/// supplied -- and before this existed two of them took different lock files
/// and the third took none. All three reach the store only through
/// [`adopt_current`], [`prune_in`], [`remove_in`] or
/// [`crate::install::file_into_store`], so the lock lives inside those four
/// rather than at each caller. See
/// [ADR-037](../../../docs/adr/ADR-037-one-lock-and-a-content-hash-for-the-build-store.md).
///
/// **Blocking, unlike [`crate::provider::exclusive`].** That lock guards a
/// network fetch a user is watching progress for, so it refuses a second
/// attempt instantly rather than queue it invisibly. This one guards a handful
/// of local renames and, at most, one pass over a 100+ MB file with no network
/// in it -- so a second caller waiting a fraction of a second for the first to
/// finish is the honest behaviour, and refusing an ordinary launch's keying
/// step because a Version-page download happened to be mid-rename would fail
/// for a reason nobody watching it could act on.
pub(crate) fn lock(root: &Path) -> io::Result<std::fs::File> {
    std::fs::create_dir_all(root)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(root.join(".store.lock"))?;
    rustix::fs::flock(&file, rustix::fs::FlockOperation::LockExclusive).map_err(io::Error::from)?;
    Ok(file)
}

/// The store entry `path` resolves to, if it is one.
///
/// Takes the symlink into account, because in the ordinary case the caller has
/// `~/.cache/cordial/lib/<abi>` and the entry is what that points at. A pinned
/// launch passes the entry directly and this returns it unchanged.
///
/// `None` for anything outside the store -- an engine beside the APK, a
/// hand-set `--lib-dir`, a build of unknown version still in the old slot. All
/// ordinary, none of them things to record against.
pub fn entry_at(path: &Path) -> Option<PathBuf> {
    let resolved = std::fs::canonicalize(path).ok()?;
    let root = std::fs::canonicalize(root()).ok()?;
    let name = resolved.file_name()?.to_str()?;
    (resolved.parent() == Some(root.as_path()) && is_valid_version(name)).then_some(resolved)
}

/// Record that this Cordial loaded the build in `dir`.
///
/// Called after a load has succeeded, never before it is attempted: the whole
/// value of the record is that it distinguishes a build somebody has run from
/// one nobody has, and a record written on the way in would say the same thing
/// about both.
pub fn record_loaded_by(dir: &Path, cordial: &str) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join(LOADED_BY), cordial.trim())
}

/// Point `live` at the entry for `version`.
///
/// `live` is the old single-slot path, and after this it is a symlink. The
/// swap is a `symlink` to a temporary name followed by a `rename` over the
/// old one, because `symlink` itself refuses to replace anything: the
/// alternative is unlink-then-symlink, which leaves a window in which there is
/// no engine at all, and a launch in that window fails with a missing file
/// rather than waiting.
///
/// **It refuses when `live` is a real directory**, rather than deleting one.
/// That directory is an engine somebody may be running, and on an install
/// predating the store it is the *only* engine there is. [`adopt_current`] is
/// the way one becomes an entry; this will not do it silently.
pub fn point_current_at(live: &Path, entry: &Path) -> io::Result<()> {
    use std::os::unix::fs::symlink;

    if let Ok(meta) = std::fs::symlink_metadata(live) {
        if meta.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!(
                    "{} is a directory holding an engine, not a link into the store",
                    live.display()
                ),
            ));
        }
    }
    if let Some(parent) = live.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = live.with_file_name(format!(
        ".{}.linking.{}",
        live.file_name().and_then(|n| n.to_str()).unwrap_or("current"),
        std::process::id()
    ));
    let _ = std::fs::remove_file(&temporary);
    symlink(entry, &temporary)?;
    match std::fs::rename(&temporary, live) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&temporary);
            Err(e)
        }
    }
}

/// Which entry `live` currently points at, if it points at one at all.
pub fn current_in(root: &Path, live: &Path) -> Option<String> {
    let target = std::fs::read_link(live).ok()?;
    let resolved = if target.is_absolute() {
        target
    } else {
        live.parent()?.join(target)
    };
    // Compared by name under the store root rather than by canonicalising
    // both: `canonicalize` needs the target to exist, and an entry that has
    // been pruned out from under a stale link is exactly the case worth
    // answering rather than erroring on.
    let name = resolved.file_name()?.to_str()?.to_string();
    (resolved.parent() == Some(root) && is_valid_version(&name)).then_some(name)
}

/// Take an existing single-slot engine into the store, and leave a link behind.
///
/// This is the migration, and it runs at most once per install: after it,
/// `live` is a symlink and the first check here returns.
///
/// **A build whose version is unknown is left exactly where it is.** An APK the
/// user obtained themselves carries no version Cordial can read without parsing
/// Android's binary manifest -- [`crate::cache::recorded_version`] says so at
/// more length -- and an entry keyed on a guess is worse than no entry: the
/// store's whole contract is that a directory name identifies a build. So the
/// return is `Ok(None)`, the old directory keeps working exactly as it did, and
/// the store starts populating from the next build Cordial fetches itself.
pub fn adopt_current(root: &Path, live: &Path) -> io::Result<Option<String>> {
    let Ok(meta) = std::fs::symlink_metadata(live) else {
        return Ok(None);
    };
    if !meta.is_dir() {
        // Already a link, or not there. Either way there is nothing to adopt.
        return Ok(None);
    }
    if !live.join(crate::engine::LIBRARY).is_file() {
        return Ok(None);
    }
    let Some(version) = crate::cache::recorded_version(live).filter(|v| is_valid_version(v)) else {
        return Ok(None);
    };
    let Some(into) = entry_dir_in(root, &version) else {
        return Ok(None);
    };

    std::fs::create_dir_all(root)?;
    // Held for the rest of this function: everything below either renames or
    // deletes something under `root`, and this is one of three places that
    // does so -- see [`lock`].
    let _lock = lock(root)?;
    if into.exists() {
        // The store already has this version. Whatever is in the single slot is
        // a second copy of a build already keyed, so the link replaces it
        // rather than the directory being merged into an entry that is already
        // complete. Removed only after the entry is confirmed to hold an
        // engine, so a half-written entry cannot cost somebody the one they had.
        if into.join(crate::engine::LIBRARY).is_file() {
            // The provenance comes with it, though. The entry's stamp names
            // the archive it was first extracted from; `live`'s names the one
            // that was just extracted and is about to be run, and it is the
            // one the next launch compares against. Keeping the entry's made
            // every launch after this one find the cache stale and extract
            // again. A record `live` lacks is removed from the entry rather
            // than left beside a stamp it does not describe.
            for record in [crate::cache::STAMP, crate::cache::SIGNER] {
                let (fresh, kept) = (live.join(record), into.join(record));
                if fresh.is_file() {
                    std::fs::rename(&fresh, &kept)?;
                } else {
                    let _ = std::fs::remove_file(&kept);
                }
            }
            std::fs::remove_dir_all(live)?;
            point_current_at(live, &into)?;
            ensure_content_hash(&into);
            return Ok(Some(version));
        }
        std::fs::remove_dir_all(&into)?;
    }
    // A rename within one cache root, so this is a metadata change rather than
    // a copy of 115 MB -- and it means there is no moment where both the old
    // path and the new one hold half a build.
    std::fs::rename(live, &into)?;
    point_current_at(live, &into)?;
    ensure_content_hash(&into);
    Ok(Some(version))
}

/// Keep the archives this build was made from, beside the engine.
///
/// **Because the engine alone is not a build.** `libroblox.so` is the engine
/// and the APKs are the assets, and Roblox ships them as one version --
/// [`crate::cache`] exists entirely because pairing a new APK with an old
/// engine is a silent mismatch that presents as anything but. So a store entry
/// that held only the library would offer a rollback that swapped half the
/// build, which is worse than offering none.
///
/// **Hard links, and no copy fallback.** The entry and `build/<abi>` end up as
/// two names for one inode on one filesystem; installing the next build
/// renames over the name under `build/<abi>` and the data stays alive under
/// the entry's. That costs nothing, which is the only reason keeping every
/// build's archives is affordable at all.
///
/// A copy would be 230 MB per entry, and there is one caller for which it is a
/// real possibility rather than a theoretical one: `cordial-shell` links the
/// user's *own* APKs -- Sober's, usually -- and those can sit on another
/// filesystem. Spending 230 MB and a minute of somebody's launch on a rollback
/// they have not asked for is not a decision to make silently, so a link that
/// cannot be made is reported and the entry is simply marked incomplete.
///
/// Linking somebody else's file takes nothing away from them: the inode gains a
/// name, and a program that replaces the file by renaming over it -- which is
/// how a download lands -- leaves this name pointing at what was there. A
/// program that rewrote the file in place would change this copy too, and
/// nothing here can prevent that.
///
/// Failure is reported and not fatal. An entry with no archives beside it is
/// still a real engine that the current build's assets match; it is only a
/// rollback target that cannot be selected, and [`Entry::complete`] is how a
/// picker tells the difference.
pub fn keep_archives(entry: &Path, archives: &[&Path]) -> Vec<String> {
    let mut trouble = Vec::new();
    for archive in archives {
        let Some(name) = archive.file_name() else { continue };
        let target = entry.join(name);
        if target.exists() || !archive.is_file() {
            continue;
        }
        if let Err(e) = std::fs::hard_link(archive, &target) {
            trouble.push(format!("{} could not be linked: {e}", archive.display()));
        }
    }
    trouble
}

/// Break the link at `live` and leave an empty directory in its place.
///
/// Called before an install, and this is the ordering the whole store turns
/// on. Once `live` is a symlink into an entry, *everything that writes to it
/// writes into that entry* -- so an extraction that renames a new
/// `libroblox.so` onto `live/libroblox.so` would land inside the build the
/// user is keeping, overwrite it, and leave the store holding one entry with
/// two versions' worth of claim on it. The first draft did exactly that and
/// the test below is what caught it.
///
/// Breaking the link costs nothing: the entry it pointed at is untouched and
/// stays in the store, which is the build somebody rolls back to.
pub fn detach(live: &Path) -> io::Result<()> {
    if let Ok(meta) = std::fs::symlink_metadata(live) {
        if meta.file_type().is_symlink() {
            std::fs::remove_file(live)?;
        }
    }
    std::fs::create_dir_all(live)
}

/// Drop the oldest entries until at most `keep` remain, never touching one
/// named in `protect`.
///
/// Returns what it removed, so a caller can say so rather than a user finding
/// out by the store being smaller than they left it.
///
/// **`protect` is not optional and is not a convenience.** The current entry is
/// in it, and so is every version any profile has pinned -- pruning a pinned
/// build turns a deliberate choice into a launch failure with no explanation,
/// which is the one outcome that makes the pin worse than not having it.
pub fn prune_in(root: &Path, keep: usize, protect: &[String]) -> Vec<String> {
    // Best-effort, matching the per-entry `is_ok()` below: a store this cannot
    // lock right now is pruned on the next call rather than the caller being
    // handed an error type it would only ever log.
    let Ok(_lock) = lock(root) else {
        return Vec::new();
    };
    let all = list_in(root);
    let mut removed = Vec::new();
    // Newest first, so counting down the list keeps the newest `keep` and the
    // candidates are what is left. A protected entry still occupies one of the
    // kept slots, deliberately: the alternative is that pinning three builds
    // silently raises the bound to six.
    let mut kept = 0usize;
    for entry in all {
        if protect.contains(&entry.version) {
            kept += 1;
            continue;
        }
        if kept < keep {
            kept += 1;
            continue;
        }
        if std::fs::remove_dir_all(&entry.dir).is_ok() {
            removed.push(entry.version);
        }
    }
    removed
}

/// Remove one entry because somebody asked to, from the Version page.
///
/// Refuses the current entry and anything in `protect` for the same reason
/// [`prune_in`] never takes them: the first leaves the slot a dangling link and
/// nothing to launch, the second turns a profile's pin into a launch failure in
/// a profile nobody touched. Removing an entry only drops its names for the
/// archives, so an APK that was linked from Sober's directory stays there.
pub fn remove_in(root: &Path, live: &Path, version: &str, protect: &[String]) -> Result<(), String> {
    let Some(dir) = entry_dir_in(root, version) else {
        return Err(format!("{version:?} is not a Roblox version"));
    };
    let _lock = lock(root).map_err(|e| format!("could not lock the build store: {e}"))?;
    if current_in(root, live).as_deref() == Some(version) {
        return Err(format!("Roblox {version} is the current build, and removing it would leave nothing to launch."));
    }
    if protect.iter().any(|p| p == version) {
        return Err(format!("A profile is pinned to Roblox {version}. Clear that pin first."));
    }
    std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removing_refuses_the_current_build_and_a_pinned_one_and_takes_the_rest() {
        let scratch = Scratch::new("remove");
        let root = scratch.path().join(BUILDS);
        for v in ["1.0", "2.0", "3.0"] {
            std::fs::create_dir_all(root.join(v)).unwrap();
            std::fs::write(root.join(v).join(crate::engine::LIBRARY), v).unwrap();
        }
        let live = scratch.path().join("lib").join("x86_64");
        std::fs::create_dir_all(live.parent().unwrap()).unwrap();
        point_current_at(&live, &root.join("3.0")).unwrap();
        let pins = vec!["2.0".to_string()];

        assert!(remove_in(&root, &live, "3.0", &pins).is_err(), "the current build");
        assert!(remove_in(&root, &live, "2.0", &pins).is_err(), "a pinned build");
        assert!(remove_in(&root, &live, "../lib", &pins).is_err(), "not a version");
        remove_in(&root, &live, "1.0", &pins).unwrap();
        let left: Vec<String> = list_in(&root).into_iter().map(|e| e.version).collect();
        assert_eq!(left, ["3.0", "2.0"]);
    }

    /// A scratch directory that deletes itself, so these tests need no
    /// dependency the workspace does not already have.
    struct Scratch(PathBuf);
    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("cordial-store-test-{}-{}", tag, std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn build(root: &Path, version: &str) -> PathBuf {
        let dir = root.join(version);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(crate::engine::LIBRARY), b"not really an engine").unwrap();
        dir
    }

    #[test]
    fn a_version_is_only_ever_digits_and_dots() {
        assert!(is_valid_version("2.738.0.1393"));
        assert!(is_valid_version("2"));
        assert!(!is_valid_version(""));
        assert!(!is_valid_version(".."));
        assert!(!is_valid_version("../../etc"));
        assert!(!is_valid_version("2.738/0"));
        assert!(!is_valid_version("2.738.0."));
        assert!(!is_valid_version(".2.738"));
        assert!(!is_valid_version("2.738.0-beta"));
        assert!(!is_valid_version(&"1".repeat(33)));
    }

    /// The one this exists for. Sorted as strings, `2.99` beats `2.738`, and a
    /// store that believes that offers the wrong build as newest and prunes the
    /// right one.
    #[test]
    fn versions_compare_numerically_rather_than_as_text() {
        assert_eq!(compare("2.738.0.1393", "2.99.0.1"), Ordering::Greater);
        assert!("2.738.0.1393" < "2.99.0.1", "the string ordering this corrects");
        assert_eq!(compare("2.738.0.1393", "2.738.0.1393"), Ordering::Equal);
        assert_eq!(compare("2.738", "2.738.0"), Ordering::Equal);
        assert_eq!(compare("2.734.0.917", "2.738.0.1393"), Ordering::Less);
    }

    #[test]
    fn the_store_lists_newest_first_and_skips_what_is_not_a_build() {
        let scratch = Scratch::new("list");
        let root = scratch.path();
        build(root, "2.734.0.917");
        build(root, "2.738.0.1393");
        build(root, "2.99.0.1");
        // A directory with no engine in it: what a killed install leaves.
        std::fs::create_dir_all(root.join("2.740.0.1")).unwrap();
        // And something that is not a version at all.
        std::fs::create_dir_all(root.join("scratch")).unwrap();

        let listed: Vec<String> = list_in(root).into_iter().map(|e| e.version).collect();
        assert_eq!(listed, vec!["2.738.0.1393", "2.734.0.917", "2.99.0.1"]);
    }

    #[test]
    fn the_current_link_names_the_entry_it_points_at() {
        let scratch = Scratch::new("link");
        let root = scratch.path().join("builds");
        let live = scratch.path().join("lib/x86_64");
        std::fs::create_dir_all(&root).unwrap();
        let entry = build(&root, "2.738.0.1393");

        point_current_at(&live, &entry).unwrap();
        assert_eq!(current_in(&root, &live).as_deref(), Some("2.738.0.1393"));
        assert!(live.join(crate::engine::LIBRARY).is_file(), "reads through the link");

        // And it re-points rather than refusing, which is what an update does.
        let newer = build(&root, "2.740.0.5");
        point_current_at(&live, &newer).unwrap();
        assert_eq!(current_in(&root, &live).as_deref(), Some("2.740.0.5"));
    }

    /// The engine somebody may be running is not deleted to make room for a
    /// link. `adopt_current` is the only thing that moves one.
    #[test]
    fn a_real_directory_is_never_replaced_by_a_link_behind_your_back() {
        let scratch = Scratch::new("refuse");
        let root = scratch.path().join("builds");
        std::fs::create_dir_all(&root).unwrap();
        let live = scratch.path().join("lib/x86_64");
        std::fs::create_dir_all(&live).unwrap();
        std::fs::write(live.join(crate::engine::LIBRARY), b"the only engine there is").unwrap();
        let entry = build(&root, "2.738.0.1393");

        let refused = point_current_at(&live, &entry).unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::AlreadyExists);
        assert!(live.join(crate::engine::LIBRARY).is_file(), "and it is still there");
    }

    #[test]
    fn an_existing_single_slot_becomes_an_entry_and_a_link() {
        let scratch = Scratch::new("adopt");
        let root = scratch.path().join("builds");
        let live = scratch.path().join("lib/x86_64");
        std::fs::create_dir_all(&live).unwrap();
        std::fs::write(live.join(crate::engine::LIBRARY), b"engine").unwrap();
        crate::cache::record_version(&live, "2.738.0.1393").unwrap();

        let adopted = adopt_current(&root, &live).unwrap();
        assert_eq!(adopted.as_deref(), Some("2.738.0.1393"));
        assert!(std::fs::symlink_metadata(&live).unwrap().file_type().is_symlink());
        assert!(root.join("2.738.0.1393").join(crate::engine::LIBRARY).is_file());
        assert!(live.join(crate::engine::LIBRARY).is_file(), "and still reads through");

        // Idempotent: run again and it is already a link, so there is nothing
        // to adopt and nothing is disturbed.
        assert_eq!(adopt_current(&root, &live).unwrap(), None);
        assert_eq!(current_in(&root, &live).as_deref(), Some("2.738.0.1393"));
    }

    /// An APK the user brought themselves has no version Cordial can read. The
    /// slot keeps working; it just does not become an entry.
    #[test]
    fn a_build_of_unknown_version_is_left_where_it_is() {
        let scratch = Scratch::new("unknown");
        let root = scratch.path().join("builds");
        let live = scratch.path().join("lib/x86_64");
        std::fs::create_dir_all(&live).unwrap();
        std::fs::write(live.join(crate::engine::LIBRARY), b"engine").unwrap();

        assert_eq!(adopt_current(&root, &live).unwrap(), None);
        assert!(live.is_dir() && !live.is_symlink());
        assert!(live.join(crate::engine::LIBRARY).is_file());
    }

    #[test]
    fn pruning_keeps_the_newest_and_never_takes_something_protected() {
        let scratch = Scratch::new("prune");
        let root = scratch.path();
        for v in ["2.730.0.1", "2.734.0.917", "2.738.0.1393", "2.740.0.5", "2.742.0.9"] {
            build(root, v);
        }
        // Two kept by count, plus the pinned one which is older than both.
        let removed = prune_in(root, 2, &["2.730.0.1".to_string()]);
        assert_eq!(removed, vec!["2.738.0.1393", "2.734.0.917"]);

        let left: Vec<String> = list_in(root).into_iter().map(|e| e.version).collect();
        assert_eq!(left, vec!["2.742.0.9", "2.740.0.5", "2.730.0.1"]);
    }

    #[test]
    fn an_entry_says_whether_anything_has_ever_loaded_it() {
        let scratch = Scratch::new("loaded");
        let root = scratch.path();
        let dir = build(root, "2.738.0.1393");
        assert_eq!(list_in(root)[0].loaded_by, None, "fetched and never launched");

        record_loaded_by(&dir, "0.14.0").unwrap();
        assert_eq!(list_in(root)[0].loaded_by.as_deref(), Some("0.14.0"));
    }

    /// The bug this ordering exists for: with `live` a link into an entry, a
    /// write to `live/libroblox.so` lands *inside* the build being kept.
    #[test]
    fn detaching_leaves_the_entry_alone_and_the_slot_writable() {
        let scratch = Scratch::new("detach");
        let root = scratch.path().join("builds");
        let live = scratch.path().join("lib/x86_64");
        std::fs::create_dir_all(&root).unwrap();
        let entry = build(&root, "2.738.0.1393");
        std::fs::write(entry.join(crate::engine::LIBRARY), b"the build being kept").unwrap();
        point_current_at(&live, &entry).unwrap();

        detach(&live).unwrap();
        assert!(live.is_dir() && !live.is_symlink());
        assert!(!live.join(crate::engine::LIBRARY).exists(), "an empty slot to extract into");

        // Writing the next build into the slot does not reach the old entry.
        std::fs::write(live.join(crate::engine::LIBRARY), b"the next build").unwrap();
        assert_eq!(
            std::fs::read(entry.join(crate::engine::LIBRARY)).unwrap(),
            b"the build being kept"
        );
    }

    #[test]
    fn a_content_hash_round_trips_through_its_file() {
        let scratch = Scratch::new("hash-roundtrip");
        let dir = build(scratch.path(), "2.738.0.1393");
        assert_eq!(content_hash(&dir), None, "nothing recorded yet");

        let hash = Sha256Hash::of(b"an engine's bytes, in this test");
        record_content_hash(&dir, &hash).unwrap();
        assert_eq!(content_hash(&dir), Some(hash));
    }

    /// The property `ensure_content_hash` exists for: called on an entry with
    /// no recorded hash it computes and records one from the real file: called
    /// again it must return that *same* value even if the file underneath has
    /// since changed, because the whole point of recording it once is not to
    /// pay for a fresh pass over 100+ MB of engine on every ordinary launch.
    #[test]
    fn ensure_content_hash_computes_once_and_trusts_what_it_recorded() {
        let scratch = Scratch::new("hash-ensure");
        let dir = build(scratch.path(), "2.738.0.1393");
        std::fs::write(dir.join(crate::engine::LIBRARY), b"the real engine bytes").unwrap();

        let first = ensure_content_hash(&dir).expect("a library is there to hash");
        assert_eq!(first, Sha256Hash::of(b"the real engine bytes"));

        // The file changes underneath -- corruption, or a bug elsewhere -- but
        // nothing here re-reads it, because a hash was already recorded.
        std::fs::write(dir.join(crate::engine::LIBRARY), b"different bytes entirely").unwrap();
        let second = ensure_content_hash(&dir).unwrap();
        assert_eq!(second, first, "the stale recorded hash, not a fresh one");
    }

    #[test]
    fn find_by_content_hash_locates_the_matching_entry() {
        let scratch = Scratch::new("hash-find");
        let root = scratch.path();
        let a = build(root, "2.734.0.917");
        let b = build(root, "2.738.0.1393");
        std::fs::write(a.join(crate::engine::LIBRARY), b"engine A").unwrap();
        std::fs::write(b.join(crate::engine::LIBRARY), b"engine B").unwrap();
        ensure_content_hash(&a);
        let hash_b = ensure_content_hash(&b).unwrap();

        let found = find_by_content_hash(root, &hash_b).expect("engine B is in the store");
        assert_eq!(found.version, "2.738.0.1393");
        assert!(find_by_content_hash(root, &Sha256Hash::of(b"nothing kept this")).is_none());
    }

    /// `adopt_current` is what every install path but `file_into_store` already
    /// funnels through, so hooking the hash in there is what makes it apply to
    /// every one of them without touching a call site.
    #[test]
    fn adopting_a_build_records_its_content_hash() {
        let scratch = Scratch::new("hash-adopt");
        let root = scratch.path().join("builds");
        let live = scratch.path().join("lib/x86_64");
        std::fs::create_dir_all(&live).unwrap();
        std::fs::write(live.join(crate::engine::LIBRARY), b"the adopted engine").unwrap();
        crate::cache::record_version(&live, "2.738.0.1393").unwrap();

        adopt_current(&root, &live).unwrap();
        assert_eq!(
            content_hash(&root.join("2.738.0.1393")),
            Some(Sha256Hash::of(b"the adopted engine"))
        );
    }

    /// **A second extraction of a version the store already keeps must leave
    /// the entry stamped for the archive it was just extracted from.** Before
    /// this, the fresh directory -- stamp and all -- was deleted in favour of
    /// the entry, which kept the stamp of the *previous* archive. The next
    /// launch then found the cache stale and extracted 116 MB again, and so on
    /// every launch after. It happens whenever the archive's identity changes
    /// without its version doing so: `stacked update` re-installing the same
    /// build, a build moving from Sober's directory to Stacked's, or Sober
    /// re-downloading one.
    #[test]
    fn re_adopting_a_kept_version_carries_the_new_stamp_and_signer_into_the_entry() {
        let scratch = Scratch::new("readopt-stamp");
        let root = scratch.path().join("builds");
        let live = scratch.path().join("lib/x86_64");
        std::fs::create_dir_all(&live).unwrap();
        std::fs::write(live.join(crate::engine::LIBRARY), b"engine").unwrap();
        crate::cache::record_version(&live, "2.738.0.1393").unwrap();
        std::fs::write(live.join(crate::cache::STAMP), "old archive").unwrap();
        std::fs::write(live.join(crate::cache::SIGNER), "old signer").unwrap();
        adopt_current(&root, &live).unwrap();

        // What `locate` does when the stamp no longer matches: detach, extract
        // afresh, stamp, and key again.
        detach(&live).unwrap();
        std::fs::write(live.join(crate::engine::LIBRARY), b"engine").unwrap();
        crate::cache::record_version(&live, "2.738.0.1393").unwrap();
        std::fs::write(live.join(crate::cache::STAMP), "new archive").unwrap();
        assert_eq!(adopt_current(&root, &live).unwrap().as_deref(), Some("2.738.0.1393"));

        assert_eq!(crate::cache::stamp_of(&live).as_deref(), Some("new archive"));
        assert!(
            !live.join(crate::cache::SIGNER).exists(),
            "a signer recorded for the old archive must not survive beside the new stamp"
        );
    }

    /// The race this store lock exists to close: three code paths mutate one
    /// directory and, before ADR-037, two of them used different lock files
    /// and one used none. Holding the lock externally and measuring how long a
    /// blocked mutator waits is the only way to observe "serialised" rather
    /// than merely "did not corrupt anything on this run".
    #[test]
    fn concurrent_mutations_serialize_on_the_store_lock() {
        let scratch = Scratch::new("concurrent");
        let root = scratch.path().join("builds");
        std::fs::create_dir_all(&root).unwrap();
        build(&root, "1.0");

        let held = lock(&root).unwrap();
        let waited_root = root.clone();
        let start = std::time::Instant::now();
        let handle = std::thread::spawn(move || {
            // Must block until the lock taken above is released below.
            prune_in(&waited_root, 5, &[]);
            start.elapsed()
        });
        std::thread::sleep(std::time::Duration::from_millis(200));
        drop(held);
        let elapsed = handle.join().unwrap();
        assert!(elapsed >= std::time::Duration::from_millis(180), "prune_in ran concurrently: {elapsed:?}");
    }
}
