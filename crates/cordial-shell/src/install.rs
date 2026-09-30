//! Where the Roblox build is, and how Cordial goes looking for one.
//!
//! Cordial ships no Roblox code and never will, so every launch depends on a
//! build the user already has. Nothing in this project used to record where
//! that was: the only way to run the client was a hand-typed `cordial-run`
//! command line carrying `--lib-dir` and `--apk`, which is why the chooser's
//! activate handler had nothing to call.
//!
//! The resolution order here is the same one `justfile`'s `dev` recipe uses,
//! deliberately and to the letter, because that recipe has been run end to end
//! and this had not. Two things in it are not guessable and both cost somebody
//! an afternoon to establish:
//!
//! The engine is **not in `base.apk`** on a split build. `libroblox.so` lives
//! in `split_config.x86_64.apk` beside it, so anything that assumes the APK it
//! was given contains the engine fails on the ordinary case. Each candidate is
//! tried in turn instead of one being asserted.
//!
//! And the extracted engine belongs in the cache rather than beside the APK,
//! because the APK is usually inside another application's data directory,
//! which Cordial has no business writing into.
//!
//! **Detection is a filesystem check every time, never a remembered answer.**
//! A stored "yes, it is installed" goes stale the moment the user deletes the
//! build or Sober replaces it, and a launcher that then fails with a path
//! error is worse than one that simply looks again.
//!
//! And the extracted engine is **stamped with the APK it came from**. Presence
//! alone was the whole test until now, which meant a new Roblox build left the
//! *old* engine in the cache and Cordial ran it against the new APK's assets —
//! a silent version mismatch, and worse than the cold start the cache exists to
//! avoid, because nothing about it presents as a caching problem. `justfile`'s
//! `client` recipe had the same bug and had it fixed; this had not.
//! [`cordial_update::cache`] owns the stamp and writes the same string that
//! recipe writes, so the two never make each other re-extract 115 MB.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The engine object. `--lib-dir` names the directory holding it.
pub const LIBRARY: &str = "libroblox.so";

/// Points this run at one APK without touching the saved settings. Set by
/// `just dev --apk <path>`.
pub const APK_OVERRIDE: &str = "CORDIAL_APK";

/// Its path inside whichever APK carries it.
///
/// Taken from `cordial-update` rather than declared again here. It was declared
/// twice, privately, and two copies of one ABI string is how a port ends up
/// half done: the crate that extracts the engine and the crate that looks for
/// it would disagree, and nothing would say so.
use cordial_update::apk::LIBRARY_IN_APK;

/// What the user has pinned by hand, if anything.
///
/// Both are `None` on a fresh install and stay that way for anyone who lets
/// detection do its job — which is the intended case, not a degraded one. A
/// value here is an override, and [`locate`] honours it over anything it would
/// otherwise find, because a user who went to Settings and chose a file meant
/// that file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RobloxInstall {
    pub apk: Option<PathBuf>,
    pub lib_dir: Option<PathBuf>,
}

/// A build that has been found and checked: both of these exist right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Build {
    pub apk: PathBuf,
    pub lib_dir: PathBuf,
}


/// Why a launch cannot proceed, split by what the user can do about it.
#[derive(Debug)]
pub enum NotFound {
    /// No APK anywhere. The answer is the Sober instructions, not an error
    /// dialog — this is a first-run state rather than a fault, and it is the
    /// only one with a scripted way out.
    NoBuild,
    /// An APK was found or configured, and getting the engine out of it did not
    /// work. Carries something specific enough to act on.
    Unusable(String),
}

/// Sober's copy of the official Android build.
///
/// Named rather than searched for, because the point is to be able to tell the
/// user exactly where Cordial looked. Sober downloads the same official
/// x86-64 Android build this runtime loads and leaves it unpacked, which makes
/// it far and away the least painful way for someone to obtain one — but it is
/// another application's private directory, so Cordial *offers* what it finds
/// there and records the path only once the user has launched with it. It never
/// silently depends on Sober being installed.
pub fn sober_apk() -> PathBuf {
    sober_apk_under(&std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(std::env::temp_dir))
}

/// Split out from [`sober_apk`] so the path itself can be pinned by a test
/// without a test having to write to `HOME`, which is process-wide and would
/// interleave with every other test in this crate that reads it.
fn sober_apk_under(home: &Path) -> PathBuf {
    // Sober names this directory segment after the same Android ABI string
    // `cordial_update::apk::HOST_ABI` spells on x86_64: "x86_64". Not verified
    // for aarch64 -- Sober is a project this codebase may observe running but
    // never inspect (AGENTS.md) -- so this is INFERRED from the x86_64 naming
    // pattern, not confirmed against a real Sober install on ARM. See the same
    // caveat in `cordial_update::provider::local`.
    home.join(format!(
        ".var/app/org.vinegarhq.Sober/data/sober/packages/{}/com.roblox.client/base.apk",
        cordial_update::apk::HOST_ABI
    ))
}

/// Where Cordial keeps the engine it extracted. Same path `just dev` uses, so
/// the two never make each other re-extract 115 MB.
pub fn engine_cache() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
        // Must stay in step with `cordial_update::install::engine_dir`, which
        // is ABI-named for the same reason: two builds for two architectures
        // sharing one home directory must not share one engine cache.
        .join("cordial/lib")
        .join(cordial_update::apk::HOST_ABI)
}

/// The APK Cordial would use, and whether the user picked it or Cordial found
/// it. `None` means there is nothing to launch and the instructions are the
/// answer.
pub fn effective_apk(configured: &RobloxInstall) -> Option<(PathBuf, Origin)> {
    // `CORDIAL_APK` overrides the saved setting outright, the same override
    // pattern `CORDIAL_SHELL_CONFIG`, `CORDIAL_FLAGS` and `CORDIAL_PROFILE_ROOT`
    // already use. `just dev --apk <path>` is what sets it: that recipe now
    // starts the shell rather than the engine, so the one path that genuinely
    // varies between contributors has to reach the shell somehow, and pointing
    // it at a build for one run must not overwrite what the user chose in
    // Settings.
    if let Some(apk) = std::env::var_os(APK_OVERRIDE) {
        return Some((PathBuf::from(apk), Origin::Environment));
    }
    if let Some(apk) = &configured.apk {
        return Some((apk.clone(), Origin::Chosen));
    }
    // **Cordial's own download, which this did not look at until it was caught
    // by running it.** The Download button installs into
    // `cordial_update::install::build_dir()`, and every lookup here went from
    // the environment, to the setting, to Sober -- so a user who pressed
    // Download watched it verify and install a build, and then got "No Roblox
    // build found" on the same screen. The feature installed to a directory
    // the launcher did not know about.
    //
    // Ahead of Sober because it is the more deliberate of the two: somebody
    // who pressed Download asked for this build, where Sober's is a file that
    // happened to be on the disk. Behind the setting and the environment
    // because both of those are somebody saying which build they want, and
    // this must not override that.
    if let Some(managed) = cordial_update::install::managed_base() {
        return Some((managed, Origin::Managed));
    }

    let sober = sober_apk();
    sober.is_file().then_some((sober, Origin::Sober))
}

/// Where a path came from, so the UI can say. A detected path that presents
/// itself as configuration is how a user ends up not knowing that deleting
/// another application will break this one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Environment,
    Chosen,
    /// Cordial downloaded it and put it there. See [`effective_apk`].
    Managed,
    Sober,
}

impl Origin {
    /// Whether Cordial may replace this build with a newer one.
    ///
    /// **Only its own.** Everything else here is either a file somebody named
    /// deliberately or a file belonging to another program, and Cordial writes
    /// neither. That is one half of the rule; the other half is
    /// `cordial_update::install::ours_to_write`, which stops it writing over a
    /// build it did not install even inside its own directory.
    ///
    /// It also decides whether an update is worth *fetching*, which is the part
    /// that is easy to miss. `effective_apk` prefers a chosen APK over a
    /// downloaded one, so downloading a newer build while the user has chosen
    /// their own would spend a few hundred megabytes on a file the launcher
    /// would then decline to use. Silently. The Updates page says which case
    /// the user is in rather than leaving them to notice.
    pub fn updatable(self) -> bool {
        matches!(self, Origin::Managed)
    }

    /// Why not, for the one line the Updates page shows.
    pub fn why_not_updatable(self) -> Option<&'static str> {
        match self {
            Origin::Managed => None,
            Origin::Environment => {
                Some("CORDIAL_APK names the build for this run, so Stacked will not replace it.")
            }
            Origin::Chosen => Some(
                "You chose this APK, so it will not be replaced. `stacked config unset roblox.apk` \
                 lets Stacked manage a build instead.",
            ),
            Origin::Sober => Some(
                "This build belongs to Sober and will not be written to. Download one and \
                 Stacked will manage its own copy.",
            ),
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Origin::Environment => "Set by CORDIAL_APK for this run only",
            Origin::Chosen => "Chosen with `stacked config set roblox.apk`",
            Origin::Managed => "Downloaded by Stacked",
            Origin::Sober => "Found in Sober's download (org.vinegarhq.Sober), which Stacked does not manage",
        }
    }
}

/// Swap in the build a profile has pinned, if it has pinned one.
///
/// **Both halves or neither.** A pin that moved only `lib_dir` would run an old
/// engine against the current build's assets, which is the silent version
/// mismatch `cordial_update::cache` exists to prevent and which presents as
/// anything but a version problem. So an entry that does not hold its own
/// `base.apk` is refused as a pin rather than half-applied -- that is what
/// `store::Entry::complete` is, and every entry keyed before the archives were
/// kept beside them is one.
///
/// **A missing entry is refused, not silently ignored.** Falling back to the
/// current build would run the very version the user pinned away from, and say
/// nothing. The message names the version and says what to do, because the two
/// ways to get here -- a store pruned past it, or a profile copied to another
/// machine -- both leave the user looking at a build they did not choose.
///
/// Roblox enforces a minimum client version server-side and will refuse an old
/// build whenever it decides to. Nothing here can prevent that, and the
/// settings page says so beside the picker rather than letting somebody
/// conclude Cordial broke.
pub fn apply_pin(build: Build, profile_dir: &Path) -> Result<Build, NotFound> {
    let Some(version) = cordial_shell::profile::pinned_version(profile_dir) else {
        return Ok(build);
    };
    let entries = cordial_update::store::list();
    let Some(entry) = entries.iter().find(|e| e.version == version) else {
        return Err(NotFound::Unusable(format!(
            "This profile is pinned to Roblox {version}, and that build is not in the \
             store. `stacked versions get {version}` fetches it, or `stacked unpin` clears \
             the pin to follow whichever build is current."
        )));
    };
    let Some(apk) = entry.base_apk() else {
        return Err(NotFound::Unusable(format!(
            "This profile is pinned to Roblox {version}, and Cordial kept that build's engine \
             without the APK it came from -- so its assets are gone and it cannot be run on its \
             own. Clear the pin, or pin a build Cordial has downloaded since."
        )));
    };
    Ok(Build { apk, lib_dir: entry.dir.clone() })
}

/// Find a usable build, extracting the engine from the APK if that is what it
/// takes.
///
/// The extraction runs on the calling thread and the calling thread is the one
/// GTK draws on, so the window stops responding while it happens. Measured at
/// 0.6 s for the 115 MB object on this machine, once, on the first launch after
/// an install — which is a worse trade than a progress bar and a better one
/// than the two states and a worker thread that a progress bar costs. If that
/// stops being true, this is the place to move.
/// Establish that Roblox signed this archive, at most once per build.
///
/// **The check belongs here because this is the only place every launch passes
/// through.** `cordial-update` verifies what it downloads, but the launcher
/// reaches a build four other ways -- `CORDIAL_APK`, the APK chosen in
/// Settings, an engine directory beside it, or Sober's package directory -- and
/// none of those went through the downloader. Nothing in this crate called
/// [`cordial_update::apk_signature`] at all, so a substituted archive launched
/// exactly like a genuine one and was then keyed into the same store the
/// Version page lists. Reported by @kanqz; issue #51.
///
/// **Recorded rather than repeated**, because verifying digests the whole
/// archive and `cordial_update::cache`'s own header argues against paying that
/// on every launch. The first launch after this lands verifies once, the same
/// one-off cost an unstamped cache already pays to re-extract; every launch
/// after reads a fingerprint.
///
/// A refusal names which of the two failures happened, because
/// [`cordial_update::apk_signature::Refusal`] distinguishes "somebody changed
/// this file" from "this is intact and is not Roblox's", and collapsing them
/// into one shrug is what that type exists to prevent.
///
/// Returns the fingerprint when it had to verify, for [`locate_with`] to
/// record once the engine directory has settled. Recording it here, before an
/// extraction replaces that directory, wrote it into the build being replaced
/// and cost a second verification on the next launch.
///
/// The fingerprint comes back with the archive's stamp as it was *before* the
/// digest, so what is recorded describes the bytes that were checked.
fn verified_once(apk: &Path, cache: &Path) -> Result<Option<(String, String)>, NotFound> {
    let pinned = cordial_update::apk_signature::pinned();
    if let Some(known) = cordial_update::cache::recorded_signer(cache, apk) {
        if pinned.iter().any(|p| p.eq_ignore_ascii_case(&known)) {
            return Ok(None);
        }
    }
    let stamp = cordial_update::cache::stamp_for(apk);
    match cordial_update::apk_signature::verify_signed_by(apk, &pinned) {
        Ok(signer) => Ok(stamp.map(|s| (signer.certificate_sha256, s))),
        Err(e) => Err(NotFound::Unusable(format!(
            "Stacked will not run {}: {e}.\n\nThis is the archive Stacked was pointed at, not \
             one it downloaded. `stacked config unset roblox.apk` lets Stacked find or fetch \
             a build it can check.",
            apk.display()
        ))),
    }
}

pub fn locate(configured: &RobloxInstall) -> Result<Build, NotFound> {
    locate_with(configured, verified_once)
}

/// [`locate`], with the signature check injected.
///
/// **The seam exists so the extraction tests remain testable.** They drive real
/// behaviour worth keeping -- that a new Roblox build at the same path
/// re-extracts, and that an unchanged one does not -- against archives built by
/// `apk_holding`, which are plain zips. Gating `locate` unconditionally would
/// make every one of them unrunnable, and the way out is not to fabricate a
/// signed archive: `cordial_update::apk_signature`'s own tests explain that one
/// signed by an invented key exercises the parser and proves nothing about
/// whether Roblox's real build is accepted.
///
/// So tests inject a verifier that accepts, and a separate test drives the real
/// entry point above to prove an unsigned archive is refused. The same shape as
/// `browser_account::profile::matching_profile_with`, for the same reason.
fn locate_with(
    configured: &RobloxInstall,
    verify: impl FnOnce(&Path, &Path) -> Result<Option<(String, String)>, NotFound>,
) -> Result<Build, NotFound> {
    let Some((apk, _)) = effective_apk(configured) else {
        return Err(NotFound::NoBuild);
    };
    if !apk.is_file() {
        return Err(NotFound::Unusable(format!(
            "No APK at {}. `stacked config set roblox.apk PATH` chooses another, or \
             `stacked config unset roblox.apk` lets Stacked look again.",
            apk.display()
        )));
    }
    // Before any of the four paths below returns a `Build`, so that none of
    // them can hand the loader an archive nobody established the origin of.
    let fresh_signer = verify(&apk, &engine_cache())?;
    let build = locate_verified(configured, apk)?;
    // Not fatal if it cannot be written: the cost is verifying again next
    // launch, which is slow rather than wrong. The same shape as the version
    // and stamp writes in `locate_verified`.
    if let Some((fingerprint, stamp)) = fresh_signer {
        if let Err(e) = cordial_update::cache::record_signer_stamped(&engine_cache(), &fingerprint, &stamp) {
            println!("  shell: verified {} but could not record it: {e}", build.apk.display());
        }
    }
    Ok(build)
}

/// The rest of [`locate_with`], once the archive's signature is established.
fn locate_verified(configured: &RobloxInstall, apk: PathBuf) -> Result<Build, NotFound> {

    // An explicit --lib-dir wins and is not second-guessed: someone who set it
    // has a reason, and quietly extracting over the top of it would hide a
    // mismatch between the engine they meant to test and the one they got.
    if let Some(lib_dir) = &configured.lib_dir {
        return if lib_dir.join(LIBRARY).is_file() {
            Ok(Build { apk, lib_dir: lib_dir.clone() })
        } else {
            Err(NotFound::Unusable(format!(
                "No {LIBRARY} in {}. `stacked config unset roblox.lib_dir` lets Stacked extract one.",
                lib_dir.display()
            )))
        };
    }

    // Beside the APK, which is where it lands if you unzip in place.
    if let Some(beside) = apk.parent().map(|d| d.join("lib").join(cordial_update::apk::HOST_ABI)) {
        if beside.join(LIBRARY).is_file() {
            return Ok(Build { apk, lib_dir: beside });
        }
    }

    // The cache is the only location here whose contents Cordial put there, so
    // it is the only one it can vouch for — and it only vouches for it against
    // the APK it was extracted from. An unstamped cache counts as stale, which
    // re-extracts once for everyone upgrading past this change: 0.6 s, once,
    // and the right answer for a directory nobody can attribute.
    //
    // The stale engine is deliberately *not* deleted first. Extraction writes a
    // temporary and renames over it, so there is nothing to clear, and deleting
    // up front would leave a user with no engine at all if the extraction then
    // failed.
    let cache = engine_cache();
    let stale = cache.join(LIBRARY).is_file() && !cordial_update::cache::is_current(&cache, &apk);
    if stale {
        println!(
            "  shell: {} was extracted from a different {}; re-extracting",
            cache.display(),
            apk.display()
        );
    } else if cache.join(LIBRARY).is_file() {
        // Keyed here as well, or an install that is already up to date never
        // reaches the store: this branch is every launch after the first, and
        // the extraction below -- the only other place that keys -- runs only
        // when Roblox changes. A no-op once the cache is a link.
        // `cordial_update::install::SPLIT_APK`, not a filename rebuilt from
        // `HOST_ABI` here: Play spells the split with underscores
        // (`split_config.arm64_v8a.apk`) while `HOST_ABI` keeps the hyphen the
        // APK's own `lib/arm64-v8a/` directory uses (see that constant's own
        // comment). The two spellings are identical for x86_64, which is why
        // rebuilding it from `HOST_ABI` here compiled and passed on that
        // architecture while quietly constructing the wrong filename
        // (`split_config.arm64-v8a.apk`) for aarch64.
        let split = apk.parent().map(|d| d.join(cordial_update::install::SPLIT_APK));
        let mut archives: Vec<&Path> = vec![apk.as_path()];
        if let Some(split) = split.as_deref().filter(|s| s.is_file()) {
            archives.push(split);
        }
        key_into_store(&cordial_update::store::root(), &cache, &archives);
        return Ok(Build { apk, lib_dir: cache });
    }

    // **Before the extraction, and for the same reason `install::adopt` does
    // it.** Once the cache path is a symlink into the keyed store, extracting
    // "into the cache" writes *inside* whichever build is current -- so a Sober
    // update, whose whole symptom is a stale cache, would overwrite the entry
    // the user might want to go back to. `adopt_current` files the outgoing
    // build away first; `detach` then leaves a real, empty directory to
    // extract into. Both are no-ops on a build whose version was never known,
    // which is exactly today's behaviour for an APK the user brought.
    let store_root = cordial_update::store::root();
    match cordial_update::store::adopt_current(&store_root, &cache) {
        Ok(Some(kept)) => println!("  shell: kept the previous build as {kept}"),
        Ok(None) => {}
        Err(e) => println!("  shell: could not keep the previous build: {e}"),
    }
    if let Err(e) = cordial_update::store::detach(&cache) {
        return Err(NotFound::Unusable(format!("{}: {e}", cache.display())));
    }

    match extract_engine(&apk, &cache) {
        Ok(from) => {
            // Stamped only once the engine is on disk. A stamp written first
            // and an extraction that then failed would claim a cache that is
            // not there, which is the same class of lie in the other direction.
            if let Err(e) = cordial_update::cache::write_stamp(&cache, &apk) {
                // Not fatal: an unstamped cache re-extracts next launch, which
                // is slow rather than wrong. Said out loud so a cache that
                // re-extracts every time has an explanation somewhere.
                println!("  shell: extracted {LIBRARY} but could not stamp the cache: {e}");
            }
            // **And the version, or this build can never be updated.**
            //
            // The store keys entries on `cache::recorded_version`, and the
            // launch-time update check (`auto_update.rs`) is deliberately
            // both-or-nothing: an unknown installed version is not an old one.
            // So an engine with no readable version is never offered an update.
            // (This named the GTK launcher's `Checked::installed` and
            // `update_available` until both were deleted with it.)
            //
            // Until now the only writer was `cordial_update::install::adopt`,
            // which runs when *Cordial* downloaded the build. Everyone whose
            // build came from Sober or from an APK they chose themselves --
            // which `provider::local` calls the source most users should end up
            // on -- extracted their engine through this path instead, and could
            // not update through the interface at all.
            //
            // **The extracted engine, not the archive `from` names.** This
            // read `from` until 2026-09-13, and `from` is the *APK*:
            // `extract_engine` returns the candidate it found the library in,
            // not the library. Scanning the archive finds nothing -- measured
            // on this host, `split_config.x86_64.apk` gives `None` where the
            // engine out of it gives `2.738.0.1397`, and
            // `engine::scan_the_real_build` keeps that as a guard. So the
            // paragraph below, about a build that can never be updated, was
            // describing a bug this code still had: every build that came from
            // Sober or from a user's own APK went unrecorded, and the update
            // check answered no for ever.
            //
            // It is also what the keyed store keys on, so an unrecorded version
            // now means a build that cannot be kept or rolled back to either.
            match cordial_update::engine::version_of(&cache.join(LIBRARY)) {
                Some(version) => {
                    if let Err(e) = cordial_update::cache::record_version(&cache, &version) {
                        println!("  shell: extracted {LIBRARY} but could not record its version: {e}");
                    }
                }
                // Not fatal, and worth saying: an engine whose version cannot
                // be read leaves the update check unable to compare, which is
                // the honest state rather than a guessed one.
                None => println!("  shell: could not read a version out of the extracted {LIBRARY}"),
            }
            println!("  shell: extracted {LIBRARY} from {} into {}", from.display(), cache.display());

            // And key what was just extracted, by the same route the outgoing
            // build took above -- it is now a real directory with a version
            // recorded in it, which is all `adopt_current` needs. `lib_dir`
            // stays the same path either way; after this it reads through a
            // link. Pruning protects every profile's pin, which is why it is
            // asked for here rather than assumed empty.
            let mut archives: Vec<&Path> = vec![apk.as_path()];
            if from != apk {
                archives.push(from.as_path());
            }
            key_into_store(&store_root, &cache, &archives);
            Ok(Build { apk, lib_dir: cache })
        }
        Err(e) => Err(NotFound::Unusable(e)),
    }
}

/// Move a cache with a recorded version into the keyed store (ADR-033), keep
/// the archives it came from beside it, and prune.
///
/// The archives are the user's own files -- Sober's, usually -- and
/// hard-linking them costs no disk and takes nothing away from whoever else is
/// using them. A link cannot be made across a filesystem boundary, and
/// `store::keep_archives` says so rather than copying 230 MB onto somebody's
/// launch without asking. Pruning protects every profile's pin.
fn key_into_store(store_root: &Path, cache: &Path, archives: &[&Path]) {
    match cordial_update::store::adopt_current(store_root, cache) {
        Ok(Some(keyed)) => {
            println!("  shell: kept {} as Roblox {keyed}", cache.display());
            let entry = store_root.join(&keyed);
            for trouble in cordial_update::store::keep_archives(&entry, archives) {
                println!("  shell: {keyed} is kept without its archives: {trouble}");
            }
            // The build just keyed is the one about to run: pruning protected
            // pins and not it, so keying a build older than the newest few
            // deleted it at once, and every launch after extracted it again.
            // Found by review.
            let mut protect = cordial_shell::profile::all_pinned_versions();
            protect.push(keyed.clone());
            let dropped = cordial_update::store::prune_in(store_root, cordial_update::store::KEEP, &protect);
            if !dropped.is_empty() {
                println!("  shell: removed older builds: {}", dropped.join(", "));
            }
        }
        Ok(None) => {}
        Err(e) => println!("  shell: could not key {} into the store: {e}", cache.display()),
    }
}

/// Candidate archives, in the order the justfile tries them: the APK named
/// first, then its `split_config*` siblings.
fn engine_candidates(apk: &Path) -> Vec<PathBuf> {
    let mut candidates = vec![apk.to_path_buf()];
    if let Some(dir) = apk.parent() {
        if let Ok(entries) = std::fs::read_dir(dir) {
            let mut splits: Vec<PathBuf> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("split_config") && n.ends_with(".apk"))
                })
                .collect();
            splits.sort();
            candidates.extend(splits);
        }
    }
    candidates
}

/// Pull [`LIBRARY_IN_APK`] (`lib/x86_64/libroblox.so` on x86-64,
/// `lib/arm64-v8a/libroblox.so` on aarch64) out of the first archive that has
/// it.
///
/// Written to a temporary name and renamed into place, because a launch
/// interrupted halfway leaves a 40 MB file that looks exactly like a complete
/// one to the `is_file` check above, and the next launch would then hand the
/// loader a truncated engine. `rename` within one directory is atomic.
fn extract_engine(apk: &Path, into: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(into).map_err(|e| format!("{}: {e}", into.display()))?;

    let mut tried = Vec::new();
    for candidate in engine_candidates(apk) {
        let Ok(file) = std::fs::File::open(&candidate) else { continue };
        let Ok(mut archive) = zip::ZipArchive::new(std::io::BufReader::new(file)) else {
            continue;
        };
        let Ok(mut entry) = archive.by_name(LIBRARY_IN_APK) else {
            tried.push(candidate);
            continue;
        };

        let partial = into.join(format!("{LIBRARY}.partial"));
        let mut out = std::fs::File::create(&partial).map_err(|e| format!("{}: {e}", partial.display()))?;
        std::io::copy(&mut entry, &mut out).map_err(|e| format!("{}: {e}", partial.display()))?;
        drop(out);
        std::fs::rename(&partial, into.join(LIBRARY)).map_err(|e| format!("{}: {e}", into.display()))?;
        return Ok(candidate);
    }

    Err(format!(
        "No {LIBRARY_IN_APK} in {} or its split_config siblings ({} tried). \
         On a split build the engine is in {}, not base.apk — \
         if it is somewhere else, `stacked config set roblox.lib_dir PATH`.",
        apk.display(),
        tried.len(),
        cordial_update::install::SPLIT_APK
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("cordial-shell-install-test-{tag}"));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// `CORDIAL_APK` is process-wide, so the two tests that care about it have
    /// to be kept apart from each other. Same reasoning as `profile`'s own ENV
    /// mutex, and the same reason: cargo runs these as threads of one process.
    static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn a_chosen_apk_wins_over_anything_detected() {
        // Someone who went to Settings and picked a file meant that file, even
        // if Sober's copy is sitting right there.
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(APK_OVERRIDE);
        let install = RobloxInstall { apk: Some(PathBuf::from("/somewhere/base.apk")), lib_dir: None };
        let (apk, origin) = effective_apk(&install).unwrap();
        assert_eq!(apk, PathBuf::from("/somewhere/base.apk"));
        assert_eq!(origin, Origin::Chosen);
    }

    #[test]
    fn the_environment_override_wins_over_the_saved_setting() {
        // `just dev --apk <path>` has to be able to point one run at a build
        // without silently rewriting what the user chose in Settings.
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var(APK_OVERRIDE, "/from/env/base.apk");
        let install = RobloxInstall { apk: Some(PathBuf::from("/from/settings/base.apk")), lib_dir: None };
        let (apk, origin) = effective_apk(&install).unwrap();
        std::env::remove_var(APK_OVERRIDE);
        assert_eq!(apk, PathBuf::from("/from/env/base.apk"));
        assert_eq!(origin, Origin::Environment);
    }

    /// **Cordial updates its own build and nothing else.**
    ///
    /// The rule is not only about not overwriting somebody's file. It also
    /// stops Cordial spending a few hundred megabytes fetching a build the
    /// launcher would then decline to use, because `effective_apk` prefers a
    /// chosen APK over a downloaded one -- a download that succeeds and changes
    /// nothing, silently, which is worse than one that fails.
    #[test]
    fn only_a_build_cordial_installed_is_one_it_will_replace() {
        assert!(Origin::Managed.updatable());
        assert!(!Origin::Chosen.updatable());
        assert!(!Origin::Environment.updatable());
        assert!(!Origin::Sober.updatable());

        // And every case that is not updatable says why, because "no updates
        // for you" with no reason is the silent failure with a label on it.
        for origin in [Origin::Chosen, Origin::Environment, Origin::Sober] {
            let why = origin.why_not_updatable().expect("must say why");
            assert!(!why.is_empty(), "{origin:?}");
        }
        assert!(Origin::Managed.why_not_updatable().is_none());
    }

    /// **The bug this test exists for was found by pressing the button.**
    ///
    /// The Download button installs into `cordial_update::install::build_dir()`,
    /// and the lookup went environment -> setting -> Sober and stopped. So a
    /// user could watch Cordial fetch a build, verify its signature and install
    /// it, and then be told on the same screen that no Roblox build was found.
    /// Every unit test passed throughout: each half was correct and nothing
    /// asserted that they met.
    #[test]
    fn a_build_cordial_downloaded_itself_is_one_the_launcher_can_find() {
        let order = [Origin::Environment, Origin::Chosen, Origin::Managed, Origin::Sober];
        assert_eq!(order.len(), 4, "a new origin needs a place in this order");

        // Managed comes after the two that are somebody stating a preference,
        // and before the one that is a file which merely happens to be there.
        let managed_at = order.iter().position(|o| *o == Origin::Managed).unwrap();
        let sober_at = order.iter().position(|o| *o == Origin::Sober).unwrap();
        let chosen_at = order.iter().position(|o| *o == Origin::Chosen).unwrap();
        assert!(managed_at < sober_at, "a deliberate download must beat a file that was lying about");
        assert!(chosen_at < managed_at, "an explicit choice must beat a download");

        // And it says where it came from, because a detected path presenting
        // itself as configuration is how somebody ends up not knowing that
        // deleting another application will break this one.
        assert!(Origin::Managed.describe().contains("Stacked"));
        assert!(Origin::Sober.describe().contains("Sober"));
    }

    #[test]
    fn the_split_apk_is_tried_after_the_one_it_was_given() {
        // The engine is not in base.apk on a split build. Asserting otherwise
        // is the mistake this ordering exists to stop, so the order is pinned.
        let dir = scratch("candidates");
        let split_name = cordial_update::install::SPLIT_APK;
        for name in ["base.apk", split_name, "split_config.en.apk"] {
            std::fs::write(dir.join(name), b"not really a zip").unwrap();
        }
        let candidates = engine_candidates(&dir.join("base.apk"));
        assert_eq!(candidates[0], dir.join("base.apk"));
        assert!(candidates.contains(&dir.join(split_name)));
    }

    #[test]
    fn the_detected_location_is_the_one_the_justfile_documents() {
        // `just dev` prints this path to anyone who has no build, and the two
        // must not drift: a user told to look in one place while the shell
        // looks in another has no way to tell which is wrong.
        let p = sober_apk_under(Path::new("/home/someone"));
        assert_eq!(
            p,
            Path::new(&format!(
                "/home/someone/.var/app/org.vinegarhq.Sober/data/sober/packages/{}/com.roblox.client/base.apk",
                cordial_update::apk::HOST_ABI
            ))
        );
    }

    #[test]
    fn a_configured_apk_that_has_gone_away_is_reported_rather_than_ignored() {
        // The stored path going stale is the ordinary way this breaks — the
        // user moves or deletes the build. Falling back to detection silently
        // would launch something other than what they chose.
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(APK_OVERRIDE);
        let dir = scratch("stale");
        let install = RobloxInstall { apk: Some(dir.join("gone.apk")), lib_dir: None };
        match locate(&install) {
            Err(NotFound::Unusable(msg)) => assert!(msg.contains("stacked config"), "{msg}"),
            other => panic!("expected a usable message, got {other:?}"),
        }
    }

    /// An APK-shaped zip whose engine is `engine`.
    fn apk_holding(engine: &[u8]) -> Vec<u8> {
        use std::io::Write;
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        w.start_file(LIBRARY_IN_APK, zip::write::SimpleFileOptions::default()).unwrap();
        w.write_all(engine).unwrap();
        w.finish().unwrap().into_inner()
    }

    #[test]
    fn a_new_roblox_build_re_extracts_rather_than_running_the_old_engine() {
        // The defect this fixes, end to end. Presence alone was the whole test,
        // so a new build left the OLD engine in the cache and Cordial ran it
        // against the new APK's assets — a version mismatch with nothing in it
        // that looks like a caching problem. Delete the `is_current` call in
        // `locate` and the second assertion below fails.
        //
        // `XDG_CACHE_HOME` is process-wide, hence the same guard the two
        // `CORDIAL_APK` tests take.
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(APK_OVERRIDE);
        let dir = scratch("restamp");
        let cache_home = dir.join("cache");
        let previous = std::env::var_os("XDG_CACHE_HOME");
        std::env::set_var("XDG_CACHE_HOME", &cache_home);

        let apk = dir.join("base.apk");
        std::fs::write(&apk, apk_holding(b"the old engine")).unwrap();
        let install = RobloxInstall { apk: Some(apk.clone()), lib_dir: None };

        let first = locate_with(&install, |_, _| Ok(None)).unwrap();
        assert_eq!(std::fs::read(first.lib_dir.join(LIBRARY)).unwrap(), b"the old engine");

        // A new Roblox build lands at the same path, which is exactly what
        // Sober updating does.
        std::fs::write(&apk, apk_holding(b"the new engine, which is longer")).unwrap();
        let second = locate_with(&install, |_, _| Ok(None)).unwrap();
        let got = std::fs::read(second.lib_dir.join(LIBRARY)).unwrap();

        match previous {
            Some(v) => std::env::set_var("XDG_CACHE_HOME", v),
            None => std::env::remove_var("XDG_CACHE_HOME"),
        }
        assert_eq!(
            got, b"the new engine, which is longer",
            "the cache must follow the APK it was extracted from"
        );
    }

    #[test]
    fn an_unchanged_apk_does_not_re_extract() {
        // The control for the test above. Re-extracting every launch would be
        // 115 MB of pointless work and would make the fix indistinguishable
        // from having no cache at all.
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(APK_OVERRIDE);
        let dir = scratch("unchanged");
        let cache_home = dir.join("cache");
        let previous = std::env::var_os("XDG_CACHE_HOME");
        std::env::set_var("XDG_CACHE_HOME", &cache_home);

        let apk = dir.join("base.apk");
        std::fs::write(&apk, apk_holding(b"the engine")).unwrap();
        let install = RobloxInstall { apk: Some(apk.clone()), lib_dir: None };

        let build = locate_with(&install, |_, _| Ok(None)).unwrap();
        // Something no extraction would ever produce, so its survival is proof
        // the second call did not extract.
        std::fs::write(build.lib_dir.join(LIBRARY), b"left alone").unwrap();
        let again = locate_with(&install, |_, _| Ok(None)).unwrap();
        let got = std::fs::read(again.lib_dir.join(LIBRARY)).unwrap();

        match previous {
            Some(v) => std::env::set_var("XDG_CACHE_HOME", v),
            None => std::env::remove_var("XDG_CACHE_HOME"),
        }
        assert_eq!(got, b"left alone", "an unchanged APK must not re-extract");
    }

    #[test]
    fn an_archive_without_the_engine_says_where_it_looked() {
        let dir = scratch("noengine");
        std::fs::write(dir.join("base.apk"), b"not really a zip").unwrap();
        let err = extract_engine(&dir.join("base.apk"), &dir.join("cache")).unwrap_err();
        assert!(err.contains("split_config"), "{err}");
    }

    /// **The regression guard for issue #51**, reported by @kanqz: a build the
    /// launcher was merely pointed at used to reach the loader without anything
    /// asking whose signature was on it.
    ///
    /// This drives the real entry point rather than the seam, so deleting the
    /// check in `locate` fails here. It asserts a *refusal*, which is what
    /// makes it writable at all -- `apk_holding` produces a plain zip with no
    /// signing block, exactly the shape of a substituted APK, and no genuine
    /// signed archive is needed to prove that one is turned away.
    #[test]
    fn an_unsigned_archive_is_refused_by_the_real_entry_point() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(APK_OVERRIDE);
        let dir = scratch("unsigned");
        let cache_home = dir.join("cache");
        let previous = std::env::var_os("XDG_CACHE_HOME");
        std::env::set_var("XDG_CACHE_HOME", &cache_home);

        let apk = dir.join("base.apk");
        std::fs::write(&apk, apk_holding(b"an engine nobody signed")).unwrap();
        let install = RobloxInstall { apk: Some(apk.clone()), lib_dir: None };
        let refused = locate(&install);

        match previous {
            Some(v) => std::env::set_var("XDG_CACHE_HOME", v),
            None => std::env::remove_var("XDG_CACHE_HOME"),
        }
        match refused {
            Err(NotFound::Unusable(msg)) => {
                assert!(msg.contains("no APK signing block"), "{msg}")
            }
            other => panic!("an unsigned archive must not reach the loader, got {other:?}"),
        }
    }




}
