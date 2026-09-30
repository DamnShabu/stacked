//! Where the Roblox build comes from, as a set of interchangeable sources.
//!
//! [ADR-015](../../../docs/adr/ADR-015-fetching-the-roblox-build.md) permits
//! Cordial to download the official build to the user's own machine and never
//! to ship one, and for a long time this crate could honour the first half only
//! in theory: Roblox publishes no Android download, so
//! [`crate::download::Source::official`] refuses in three plain sentences and
//! the user was left to find an APK themselves. That is the single largest
//! thing standing between somebody and a working client, and it is the reason
//! the quickstart's first instruction is to install a different program.
//!
//! [ADR-025](../../../docs/adr/ADR-025-fetching-from-a-third-party-mirror.md)
//! is what changed. It permits a distributor who is not Roblox, on one
//! condition, and the condition is the whole of it: the archive is worthless
//! until [`crate::apk_signature`] says Roblox's own signing certificate signed
//! those exact bytes. A mirror that alters a byte is caught; a mirror that
//! serves something else entirely is caught; a mirror that is compromised is
//! caught. What a mirror can still do is go down, lie about what versions
//! exist, or watch who asks -- which is why the choice of source is the user's
//! and why the one that needs no network is tried first.
//!
//! ## Two axes of extension, and they are not the same axis
//!
//! Conflating them is easy and it produces the wrong design, so they are named
//! separately here.
//!
//! **Same protocol, different host** is configuration, not code:
//! `CORDIAL_MIRROR_URL` points [`mirror`] at any endpoint that answers in
//! APKPure's shape -- a caching proxy, a copy inside a company network, a
//! different deployment of the same software. Nothing is compiled for it.
//!
//! **A different protocol is a new [`Provider`]**, because APKMirror, F-Droid
//! and Aptoide do not merely live at other addresses: they enumerate versions
//! differently, hand back download URLs differently, and in two of those cases
//! are not machine-readable at all without scraping HTML. No base URL bridges
//! that. A new source is a new file in this directory and a line in [`all`],
//! and the trait exists so that it is only those two things.
//!
//! That is also why this is a trait rather than a function with a `match` in
//! it. [`local`] reads a file already on the disk and touches no network;
//! [`mirror`] speaks an undocumented binary protocol to a third party and
//! validates every URL it is handed. They share an output type and nothing
//! else, and one function covering both would be a long `if` whose branches
//! never meet.
//!
//! ## What a new provider is not allowed to do, and what enforces it
//!
//! **A provider returns bytes. It never returns trust.** It does not verify a
//! signature, does not decide what is installable, and does not put anything
//! anywhere but the directory it was handed. [`obtain`] performs the signature
//! check on whatever comes back, from every source, without asking the source
//! whether it thinks that is necessary.
//!
//! This is the property that makes adding a source cheap. A second mirror buys
//! *availability* and nothing else -- the signature check is what makes any
//! source acceptable at all, so another one adds no trust and removes no risk,
//! and the bar for adding one is correspondingly low. It is also the property
//! most easily lost by accident, by a provider that "helpfully" checks its own
//! download and a later refactor that takes [`obtain`]'s check out as
//! duplicated work.
//!
//! So it is tested against a deliberately hostile provider rather than
//! documented and hoped for: see `a_hostile_provider_cannot_get_anything_past_the_check`.
//!
//! ## The order matters and is not alphabetical
//!
//! [`all`] returns the zero-network source first. On a machine that already has
//! the build -- which is most machines that have ever run Sober, and Sober is
//! what the README tells people to install -- the whole of [`mirror`] is
//! skipped: no request, no third party, no 150 MB, and nothing for a metered
//! connection to object to. Reaching the network to fetch a file that is
//! already present would be the worst version of this feature.

use crate::Unreachable;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Somewhere for a caller to say "stop".
///
/// **A download of a few hundred megabytes has to be abandonable**, and the
/// GNOME interface guidelines say so about any long operation: the control that
/// started it should become the one that stops it. Without this the only way
/// out of a fetch on a slow connection is to kill the client, which loses the
/// profile lock cleanly only by luck.
///
/// Checked between chunks rather than enforced by killing a thread, because the
/// thread owns a partly-written file and a socket, and both want unwinding
/// rather than abandoning. A cancelled fetch removes what it wrote, exactly as
/// a failed one does.
#[derive(Debug, Default)]
pub struct Cancel(AtomicBool);

impl Cancel {
    pub fn new() -> Self {
        Cancel(AtomicBool::new(false))
    }
    pub fn stop(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn stopped(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
    /// `Err` if the caller has asked to stop, for use with `?`.
    pub fn check(&self) -> Result<(), Unreachable> {
        if self.stopped() {
            return Err(Unreachable::Cancelled);
        }
        Ok(())
    }
}

pub mod local;
pub mod mirror;

/// A version a provider says it can supply.
///
/// The code is Android's monotonic `versionCode` and the name is what a person
/// recognises. Both are kept because they answer different questions: the code
/// orders two builds correctly and the name is the only one worth putting in a
/// message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Available {
    pub name: String,
    pub code: u64,
}

/// The archives a build is made of, once they are on disk.
///
/// **`base` and `split` are frequently the same path**, and that is not a bug
/// to tidy away. Roblox's own distribution splits the assets from the engine;
/// mirrors often serve one monolithic APK carrying both. Callers care whether
/// `assets/` and `lib/x86_64/libroblox.so` are reachable, not how many files
/// that took, and [`crate::install`] already checks for the contents rather
/// than the names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Archives {
    pub base: PathBuf,
    pub split: PathBuf,
}

impl Archives {
    /// The distinct files, so a caller verifying signatures does not verify a
    /// monolithic 150 MB archive twice to satisfy the shape of a rule.
    pub fn distinct(&self) -> Vec<&Path> {
        if self.base == self.split {
            vec![self.base.as_path()]
        } else {
            vec![self.base.as_path(), self.split.as_path()]
        }
    }
}

/// How far along a fetch is, for a progress bar that is not a lie.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    /// Asking a source what it has. Cheap, and worth showing because a
    /// provider being down looks like a hang otherwise.
    Asking { provider: &'static str },
    /// Bytes arriving. `total` is absent when the source did not say, which is
    /// common enough that a bar assuming otherwise shows nonsense.
    Fetching { file: String, done: u64, total: Option<u64> },
    /// Checking the signature. On a 150 MB archive this is seconds of full-tilt
    /// hashing with no network traffic, which reads as a stall unless it is
    /// named.
    Verifying { file: String },
}

/// A source of the Roblox Android build.
pub trait Provider {
    /// A short name, for messages. Appears in failures, so it is the word the
    /// user will search for.
    fn name(&self) -> &'static str;

    /// Whether using this source makes network requests.
    ///
    /// [`crate::metered`] asks NetworkManager whether somebody is paying by the
    /// megabyte for this connection, and a source that answers `false` here
    /// does not need to be asked at all.
    fn needs_network(&self) -> bool;

    /// The newest build this source can supply.
    fn newest(&self, progress: &mut dyn FnMut(Progress)) -> Result<Available, Unreachable>;

    /// Put the archives for `version` into `into`, and say where they landed.
    ///
    /// `into` must exist and be empty. **A provider that fails part way leaves
    /// nothing behind**: there is no such thing as most of a build, and a
    /// half-populated directory that a later run mistakes for a complete one is
    /// how a client ends up with assets from one release and an engine from
    /// another.
    fn fetch(
        &self,
        version: &Available,
        cancel: &Cancel,
        into: &Path,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<Archives, Unreachable>;
}

/// Why a build is being obtained, which decides how the sources are ordered.
///
/// **These are not the same question and treating them as one was a bug.**
/// `all()` returns the free source first, and for a first run that is exactly
/// right: most machines that will run Cordial already have this file, and
/// fetching a second copy from a mirror would be slower, more exposed and no
/// more trustworthy. But the same ordering applied to an update means the local
/// copy always wins, so **anybody with Sober installed could never receive a
/// newer build** -- "Download Roblox" would reinstall the build they already
/// had, for ever, and report success.
///
/// So an update asks every source what it has and takes the newest, and only
/// falls back to order when the versions cannot be compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Want {
    /// Any usable build. Cheapest first.
    Any,
    /// The newest available. Compares across sources.
    Newest,
}

/// Every source, in the order they should be tried.
///
/// Zero-network first. See [`Want`] for when that ordering is wrong.
pub fn all() -> Vec<Box<dyn Provider>> {
    vec![Box::new(local::OnThisMachine::default()), Box::new(mirror::ApkPure::default())]
}

/// The source named `name`, for a user who has chosen one.
pub fn named(name: &str) -> Option<Box<dyn Provider>> {
    all().into_iter().find(|p| p.name().eq_ignore_ascii_case(name))
}

/// Sort the sources newest-first, leaving ones that cannot answer at the back.
///
/// A source that fails to answer is not dropped here, only deprioritised: it
/// may still be the only one that can actually deliver, and `obtain` reports
/// what each one said if none of them can.
///
/// **Compared by [`crate::version::major_and_build`], not by raw digits.**
/// This used to rank by `version::numeric`, the plain digit vector, and that
/// is wrong for exactly the reason `version.rs`'s own doc warns about: the
/// sources do not agree on shape. The local provider reads the engine and
/// reports four components, `2.734.0.917`; the mirror reports three,
/// `2.734.917`. Compared element-wise those read as position two being `0`
/// against `917` -- the identical build losing to itself on a padding zero,
/// with the outcome deciding which source `obtain` tries first.
fn order_by_version(
    sources: Vec<Box<dyn Provider>>,
    progress: &mut dyn FnMut(Progress),
    absent: &mut Vec<String>,
) -> Vec<Box<dyn Provider>> {
    let mut scored: Vec<(Option<(u64, u64)>, usize, Box<dyn Provider>)> = Vec::new();
    for (position, source) in sources.into_iter().enumerate() {
        let version = match source.newest(progress) {
            Ok(v) => crate::version::major_and_build(&v.name),
            Err(Unreachable::NoSource { why }) => {
                absent.push(format!("{}: {why}", source.name()));
                None
            }
            // Not fatal here. A mirror being down must not stop a local build
            // from being found, and the real attempt below will report it if
            // it turns out to matter.
            Err(e) => {
                absent.push(format!("{}: {e}", source.name()));
                None
            }
        };
        scored.push((version, position, source));
    }
    // Newest first; ties and unanswerables keep `all()`'s order, which puts the
    // free source ahead of the paid one.
    scored.sort_by(|a, b| match (&a.0, &b.0) {
        (Some(x), Some(y)) => y.cmp(x).then(a.1.cmp(&b.1)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.1.cmp(&b.1),
    });
    scored.into_iter().map(|(_, _, s)| s).collect()
}

/// Free bytes on the filesystem holding `path`, or `None` if it cannot be asked.
///
/// `None` means "unknown", and an unknown must not refuse a download: a
/// pre-flight check that guesses wrong in the refusing direction is worse than
/// no check, because the honest failure it replaces at least happens for a
/// reason the user can see.
fn free_bytes(path: &Path) -> Option<u64> {
    // The nearest existing ancestor: the build directory may not exist yet on
    // a first run, and statvfs on a missing path answers nothing useful.
    let mut probe = path.to_path_buf();
    while !probe.exists() {
        probe = probe.parent()?.to_path_buf();
    }
    let output = std::process::Command::new("df")
        .args(["-Pk", "--output=avail"])
        .arg(&probe)
        .output()
        .ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    text.lines().nth(1)?.trim().parse::<u64>().ok().map(|kb| kb * 1024)
}

/// The newest build any source can actually supply, as a version string.
///
/// **Not the newest Roblox announced.** A release can exist for ARM and not for
/// x86-64 -- 2.735.1138 did, on 2026-08-26 -- and a client that compares
/// against the announcement asks for attention it can never satisfy. Every
/// provider is asked and the highest answer wins; sources that cannot answer
/// are skipped rather than treated as zero.
///
/// One small request per networked source. `None` means nothing answered,
/// which callers must treat as "unknown", never as "nothing newer".
pub fn newest_obtainable() -> Option<String> {
    let mut best: Option<String> = None;
    for source in all() {
        let Ok(available) = source.newest(&mut |_| {}) else { continue };
        let better = match &best {
            None => true,
            Some(have) => crate::version::is_newer(&available.name, have),
        };
        if better {
            best = Some(available.name);
        }
    }
    best
}

/// A build that was obtained and checked.
#[derive(Debug, Clone)]
pub struct Obtained {
    pub archives: Archives,
    pub version: Available,
    /// Which source it came from, for the message that follows.
    pub provider: &'static str,
    /// The certificate that signed it, so a caller can say whose build this is
    /// rather than merely that a check passed.
    pub certificate_sha256: String,
}

/// Get the build, from the first source that has it, and verify it.
///
/// **The signature check is here rather than left to the caller**, and that is
/// the point of the function. [ADR-025](../../../docs/adr/ADR-025-fetching-from-a-third-party-mirror.md)
/// makes a non-Roblox source acceptable only because the archive is verified,
/// and a fetch API that returned unverified paths would make forgetting the
/// check the easy thing to do. There is nothing here that hands back bytes
/// nobody has checked.
///
/// ## Which failures move to the next source and which stop everything
///
/// [`Unreachable::NoSource`] means a source has nothing -- no local build, an
/// empty directory -- and the next one is tried. **Anything else stops.** In
/// particular a signature refusal is never a reason to quietly try somewhere
/// else: an archive that fails verification is the one event this whole design
/// exists to notice, and moving on to the next provider would turn the loudest
/// possible signal into a slightly slower success.
pub fn obtain(
    preferred: Option<&str>,
    want: Want,
    cancel: &Cancel,
    into: &Path,
    progress: &mut dyn FnMut(Progress),
) -> Result<Obtained, Unreachable> {
    cancel.check()?;
    let sources = match preferred {
        Some(name) => vec![named(name).ok_or_else(|| Unreachable::NoSource {
            why: format!(
                "there is no source called {name}. Cordial knows: {}",
                all().iter().map(|p| p.name()).collect::<Vec<_>>().join(", ")
            ),
        })?],
        None => all(),
    };

    obtain_from(sources, want, cancel, &crate::apk_signature::pinned(), into, progress)
}

/// [`obtain`], with the sources and the trusted set handed in.
///
/// Split out so a test can supply a provider of its own. That is not a
/// convenience: the guarantee worth testing is about a source this crate does
/// not contain, since the whole point of the trait is that somebody will add
/// one. A test that can only exercise the two built-in providers proves nothing
/// about the third.
pub(crate) fn obtain_from(
    sources: Vec<Box<dyn Provider>>,
    want: Want,
    cancel: &Cancel,
    trusted: &[String],
    into: &Path,
    progress: &mut dyn FnMut(Progress),
) -> Result<Obtained, Unreachable> {
    let mut absent: Vec<String> = Vec::new();

    // For Newest, ask everybody first and put the winner in front. Asking is
    // one small request per source and no bytes; the alternative is the pin
    // described on `Want`.
    let sources = match want {
        Want::Any => sources,
        Want::Newest => order_by_version(sources, progress, &mut absent),
    };

    for source in sources {
        let version = match source.newest(progress) {
            Ok(v) => v,
            Err(Unreachable::NoSource { why }) => {
                absent.push(format!("{}: {why}", source.name()));
                continue;
            }
            Err(e) => return Err(e),
        };

        cancel.check()?;
        let archives = match source.fetch(&version, cancel, into, progress) {
            Ok(a) => a,
            Err(Unreachable::NoSource { why }) => {
                absent.push(format!("{}: {why}", source.name()));
                continue;
            }
            Err(e) => return Err(e),
        };

        let certificate = verify_archives(&archives, cancel, trusted, progress)?;

        return Ok(Obtained {
            archives,
            version,
            provider: source.name(),
            certificate_sha256: certificate,
        });
    }

    Err(Unreachable::NoSource {
        why: format!("no source had a Roblox build.\n  {}", absent.join("\n  ")),
    })
}

/// The signature check, on every distinct file a source returned, and the
/// certificate they all share.
///
/// One function so that [`obtain_from`] and [`obtain_into_store_from`] run the
/// identical check rather than two copies of it, which is how a copy ends up
/// weaker than the original.
fn verify_archives(
    archives: &Archives,
    cancel: &Cancel,
    trusted: &[String],
    progress: &mut dyn FnMut(Progress),
) -> Result<String, Unreachable> {
    // Every distinct file, and only once each: a monolithic APK is not
    // hashed twice to satisfy the shape of the rule.
    let mut certificate: Option<String> = None;
    for file in archives.distinct() {
        // Verification hashes a few hundred megabytes; a user who has asked
        // to stop should not wait through it.
        cancel.check()?;
        progress(Progress::Verifying {
            file: file.file_name().unwrap_or_default().to_string_lossy().to_string(),
        });
        let signer = crate::apk_signature::verify_signed_by(file, trusted).map_err(|e| {
            Unreachable::Refused {
                what: file.file_name().unwrap_or_default().to_string_lossy().to_string(),
                why: e.to_string(),
            }
        })?;
        // **The two halves must share a certificate.** Each being signed by
        // some pinned key is not enough: two archives signed by different
        // pinned keys would each pass on its own, and pairing them installs
        // an engine from one release beside assets from another. Android's
        // installer enforces the same rule for the same reason.
        match &certificate {
            None => certificate = Some(signer.certificate_sha256),
            Some(first) if *first != signer.certificate_sha256 => {
                return Err(Unreachable::Refused {
                    what: file.file_name().unwrap_or_default().to_string_lossy().to_string(),
                    why: "the two halves of this build were signed by different \
                          certificates, so they are not two halves of one build"
                        .into(),
                })
            }
            Some(_) => {}
        }
    }
    certificate.ok_or_else(|| Unreachable::NoSource {
        why: "the source returned no archives at all".into(),
    })
}

/// Refuse before any network activity when there is no room for a build.
///
/// Disk exhaustion is otherwise caught only as a write error a few hundred
/// megabytes in, which wastes the transfer and reports itself as an IO failure
/// rather than as "you do not have room". This machine reached 353 MB free
/// during a day's work, so it is not a hypothetical.
fn ensure_room(dir: &Path) -> Result<(), Unreachable> {
    if let Some(free) = free_bytes(dir) {
        // The largest archive the mirror is permitted to serve, plus the
        // engine that comes out of it. Deliberately generous: refusing a fetch
        // that would have fitted is worse than starting one that might not,
        // because the second at least fails honestly.
        const NEEDED: u64 = 700 * 1024 * 1024;
        if free < NEEDED {
            return Err(Unreachable::NoSource {
                why: format!(
                    "there is not enough room to install a build: about {} MB free where {} MB \
                     is needed. Nothing was downloaded.",
                    free / 1_048_576,
                    NEEDED / 1_048_576
                ),
            });
        }
    }
    Ok(())
}

/// An advisory lock on `path`, refused at once rather than waited for: a
/// second attempt should say so immediately rather than queue behind a 229 MB
/// download the user cannot see. Released when the file is dropped.
///
/// `rustix::fs::flock` rather than the raw `libc::flock` this used to call
/// directly: same `flock(2)`, but rustix turns the `-1`-on-failure C
/// convention into an `Err`, so there is no unsafe block and no raw fd to
/// mismanage. `std::fs::File::try_lock` would do this with no extra
/// dependency at all, but it only stabilised in Rust 1.89 and this workspace
/// pins `rust-version = "1.75"` for the Flatpak runtime — see
/// [ADR-036](../../../docs/adr/ADR-036-unsafe-is-a-boundary-not-a-convention.md).
fn exclusive(path: &Path, busy: &str) -> Result<std::fs::File, Unreachable> {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .map_err(|e| Unreachable::NoSource { why: e.to_string() })?;
    rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive)
        .map_err(|_| Unreachable::NoSource { why: busy.into() })?;
    Ok(lock)
}

/// The archives under the names a build directory and a store entry use.
///
/// A monolithic archive is one file and must be named once. Handing the same
/// path in twice would have `adopt` copy it to both names and then look for the
/// engine in whichever it found first, which works by accident and doubles the
/// disk it takes.
fn archive_names(archives: &Archives) -> Vec<(&'static str, PathBuf)> {
    let mut named = vec![(crate::install::BASE_APK, archives.base.clone())];
    if archives.split != archives.base {
        named.push((crate::install::SPLIT_APK, archives.split.clone()));
    }
    named
}

/// **A cancel is named, not folded into `NoSource`.** It still has to come back
/// as `Unreachable::Cancelled`, the one string `cordial-shell`'s buttons match
/// on to show "stopped" rather than a red failure.
fn from_install(e: crate::install::Failed) -> Unreachable {
    match e {
        crate::install::Failed::Cancelled => Unreachable::Cancelled,
        other => Unreachable::NoSource { why: other.to_string() },
    }
}

/// Download one named build from the mirror and keep it in the store, leaving
/// the build in use alone.
///
/// The Version page's download ([ADR-033](../../../docs/adr/ADR-033-roblox-versions-are-a-keyed-store.md)).
/// The same signature check as every other fetch, then
/// [`crate::install::file_into_store`] rather than `adopt`, so nothing any
/// profile launches today changes. Returns the version the engine reads as,
/// which is the store's key and what a pin names -- four components, where the
/// mirror's name for the same build has three.
pub fn obtain_into_store(
    version: &Available,
    root: &Path,
    cancel: &Cancel,
    progress: &mut dyn FnMut(Progress),
) -> Result<String, Unreachable> {
    obtain_into_store_from(&mirror::ApkPure, version, root, &crate::apk_signature::pinned(), cancel, progress)
}

pub(crate) fn obtain_into_store_from(
    source: &dyn Provider,
    version: &Available,
    root: &Path,
    trusted: &[String],
    cancel: &Cancel,
    progress: &mut dyn FnMut(Progress),
) -> Result<String, Unreachable> {
    cancel.check()?;
    std::fs::create_dir_all(root)
        .map_err(|e| Unreachable::NoSource { why: format!("{}: {e}", root.display()) })?;
    ensure_room(root)?;
    // Its own lock rather than the install's: this writes only under `root`,
    // and a download for the store should not stop an update from running.
    let _lock = exclusive(
        &root.join(".downloading"),
        "another Cordial window is already downloading a Roblox build. Wait for it to finish.",
    )?;

    let staging = root.join(".fetching");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| Unreachable::NoSource { why: e.to_string() })?;

    let outcome = (|| {
        let archives = source.fetch(version, cancel, &staging, progress)?;
        verify_archives(&archives, cancel, trusted, progress)?;
        crate::install::file_into_store(&archive_names(&archives), root, cancel).map_err(from_install)
    })();

    let _ = std::fs::remove_dir_all(&staging);
    outcome
}

/// Obtain a build and make it the one Cordial launches.
///
/// The whole thing, and the one call the shell needs: choose a source, fetch,
/// verify against the pinned certificates, apply ADR-014's extraction refusals,
/// extract the engine, and swap it in in an order that keeps a working build
/// working if any step fails.
///
/// `staging` is where a downloading source writes. It is emptied first, because
/// anything an interrupted attempt left there was never verified and is exactly
/// the file the ordering exists to keep away from the live build.
pub fn obtain_and_install(
    preferred: Option<&str>,
    want: Want,
    store: Option<&crate::install::Store>,
    cancel: &Cancel,
    progress: &mut dyn FnMut(Progress),
) -> Result<(Obtained, crate::install::Installed), Unreachable> {
    // **Before any network activity.**
    ensure_room(&crate::install::build_dir())?;

    // **One install at a time.** ADR-012's lock covers a profile; nothing
    // covered the build directory, which every profile shares. Two clients --
    // or one client and a second window -- could reach here together, and the
    // loser would find its staging directory emptied, its archives renamed
    // underneath it, or the engine cache stamped for a build it did not
    // install. Held for the whole call; released when this returns, however it
    // returns.
    let _lock = exclusive(
        &crate::install::build_dir().join(".installing"),
        "another Cordial is already installing a Roblox build. Only one install can \
         run at a time, because they share one build directory.",
    )?;

    let staging = crate::install::build_dir().join(".fetching");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)
        .map_err(|e| Unreachable::NoSource { why: e.to_string() })?;

    let outcome = (|| {
        let obtained = obtain(preferred, want, cancel, &staging, progress)?;
        let named = archive_names(&obtained.archives);

        let installed = crate::install::adopt(
            &named,
            &crate::install::build_dir(),
            &crate::install::engine_dir(),
            &crate::install::build_dir().join(crate::install::INCOMING_ENGINE),
            store,
            cancel,
            &mut |_, _, _| {},
        )
        .map_err(from_install)?;
        // `obtain` verified these archives a moment ago, so the launch that
        // follows need not digest them again. Keyed to the installed base by
        // `record_signer` itself, so it vouches for nothing else. Best effort:
        // a record that cannot be written costs one verification, not a build.
        if let Some(base) = crate::install::managed_base() {
            let _ = crate::cache::record_signer(
                &crate::install::engine_dir(),
                &obtained.certificate_sha256,
                &base,
            );
        }
        Ok((obtained, installed))
    })();

    let _ = std::fs::remove_dir_all(&staging);
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_zero_network_source_is_tried_first() {
        let providers = all();
        assert!(
            !providers[0].needs_network(),
            "a source that needs no network must come before one that does, or Cordial \
             downloads a file it already has"
        );
    }

    #[test]
    fn every_source_has_a_distinct_name_and_can_be_asked_for_by_it() {
        let names: Vec<_> = all().iter().map(|p| p.name()).collect();
        for n in &names {
            assert!(named(n).is_some(), "{n} is listed and cannot be looked up");
        }
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "two sources share a name: {names:?}");
    }

    #[test]
    fn an_unknown_source_is_none_rather_than_a_default() {
        assert!(named("whatever-the-user-typed").is_none());
    }

    #[test]
    fn a_monolithic_archive_is_verified_once_and_not_twice() {
        let one = Archives { base: "/x/a.apk".into(), split: "/x/a.apk".into() };
        assert_eq!(one.distinct().len(), 1);
        let two = Archives { base: "/x/a.apk".into(), split: "/x/b.apk".into() };
        assert_eq!(two.distinct().len(), 2);
    }

    /// Naming a source that does not exist must not silently fall back to one
    /// that does. A typo in a setting should say so, not quietly use the
    /// network when the user asked for the local build.
    /// **The comparison a string sort gets wrong, both ways.**
    #[test]
    fn versions_compare_as_numbers_and_not_as_text() {
        use crate::version::numeric;
        assert!(numeric("2.734.917") > numeric("2.734.916"));
        // Lexically "2.734.9" beats "2.734.10". Numerically it does not.
        assert!(numeric("2.734.10") > numeric("2.734.9"));
        // The two sources disagree on shape: the engine reads four components
        // and the mirror says three. Comparing what is there is the honest
        // answer; inventing the missing one is not.
        assert_eq!(numeric("2.734.0.917"), vec![2, 734, 0, 917]);
        assert_eq!(numeric("2.734.917"), vec![2, 734, 917]);
    }

    /// A provider that answers a fixed version and is never asked to fetch —
    /// `order_by_version` only calls `newest`, and a test of it that could
    /// reach `fetch` at all would be testing more than it means to.
    struct Fixed(&'static str, &'static str);
    impl Provider for Fixed {
        fn name(&self) -> &'static str {
            self.0
        }
        fn needs_network(&self) -> bool {
            false
        }
        fn newest(&self, _: &mut dyn FnMut(Progress)) -> Result<Available, Unreachable> {
            Ok(Available { name: self.1.to_string(), code: 0 })
        }
        fn fetch(
            &self,
            _: &Available,
            _: &Cancel,
            _: &Path,
            _: &mut dyn FnMut(Progress),
        ) -> Result<Archives, Unreachable> {
            unreachable!("order_by_version does not fetch")
        }
    }

    /// **The bug, reproduced directly against `order_by_version`.** Two
    /// sources reporting the identical build in the two shapes this crate
    /// actually sees — the engine's four components and the mirror's three —
    /// must tie and keep `all()`'s order, not have the free source demoted
    /// behind the paid one because of a padding zero at the position where
    /// the shapes diverge. Ranking by `version::numeric` did exactly that:
    /// `[2,734,0,917]` compares less than `[2,734,917]` at index 2, so the
    /// identical build in its four-component shape sorted *behind* itself in
    /// its three-component shape.
    #[test]
    fn order_by_version_does_not_let_a_padding_zero_decide() {
        let sources: Vec<Box<dyn Provider>> =
            vec![Box::new(Fixed("local", "2.734.0.917")), Box::new(Fixed("mirror", "2.734.917"))];
        let mut absent = Vec::new();
        let ordered = order_by_version(sources, &mut |_| {}, &mut absent);
        let names: Vec<&str> = ordered.iter().map(|p| p.name()).collect();
        assert_eq!(
            names,
            vec!["local", "mirror"],
            "the same build named in two shapes must not reorder the sources: {names:?}"
        );
    }

    /// **The control for the fix above, and the case `Checked::obtainable`
    /// records.** A comparison that stopped distinguishing versions at all —
    /// to make the tie above hold — would be a different, equally wrong, fix.
    /// Measured 2026-08-26: Roblox announced engine 735 and shipped no
    /// x86-64 build for it, so 2.734.917 was the newest anything could
    /// actually run, and a genuinely newer build still has to win regardless
    /// of which shape either source reports it in.
    #[test]
    fn order_by_version_still_prefers_a_genuinely_newer_build() {
        let sources: Vec<Box<dyn Provider>> =
            vec![Box::new(Fixed("older", "2.734.0.917")), Box::new(Fixed("newer", "2.735.1"))];
        let mut absent = Vec::new();
        let ordered = order_by_version(sources, &mut |_| {}, &mut absent);
        assert_eq!(ordered[0].name(), "newer");
        // And the reverse input order does not launder the answer either.
        let sources: Vec<Box<dyn Provider>> =
            vec![Box::new(Fixed("newer", "2.735.1")), Box::new(Fixed("older", "2.734.0.917"))];
        let ordered = order_by_version(sources, &mut |_| {}, &mut absent);
        assert_eq!(ordered[0].name(), "newer");
    }

    /// A source that behaves as badly as a source can.
    ///
    /// It claims an absurdly high version so it wins any comparison, and it
    /// writes an archive that is a perfectly good ZIP carrying the engine path
    /// -- everything a naive check would look for -- and no signature at all.
    /// This is what a compromised mirror, a hostile fork's extra provider, or
    /// an honest provider pointed at a poisoned CDN all look like from inside
    /// this crate.
    struct Hostile;

    impl Provider for Hostile {
        fn name(&self) -> &'static str {
            "hostile"
        }
        fn needs_network(&self) -> bool {
            false
        }
        fn newest(&self, _: &mut dyn FnMut(Progress)) -> Result<Available, Unreachable> {
            Ok(Available { name: "9999.9.9".into(), code: u64::MAX })
        }
        fn fetch(
            &self,
            _: &Available,
            _: &Cancel,
            into: &Path,
            _: &mut dyn FnMut(Progress),
        ) -> Result<Archives, Unreachable> {
            let path = into.join("base.apk");
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
            w.start_file("lib/x86_64/libroblox.so", zip::write::SimpleFileOptions::default())
                .unwrap();
            use std::io::Write;
            w.write_all(b"\x7fELF this is not Roblox 9999.9.9").unwrap();
            let bytes = w.finish().unwrap().into_inner();
            std::fs::write(&path, bytes).unwrap();
            Ok(Archives { base: path.clone(), split: path })
        }
    }

    /// **The guarantee that makes adding a provider cheap.**
    ///
    /// A second mirror buys availability and nothing else, because the
    /// signature check is what makes any source acceptable. That sentence is
    /// only true while nothing a provider does can reach past it, and this is
    /// the test that says so about a provider this crate does not contain --
    /// which is the only kind worth testing, since the point of the trait is
    /// that somebody will add one.
    ///
    /// It is also the property most easily lost by accident: a provider that
    /// checks its own download, and a later refactor that removes `obtain`'s
    /// check as duplicated work.
    ///
    /// **Controlled, because a refusal test that cannot fail is decoration.**
    /// With the verification taken out of `obtain_from`, this test fails and
    /// prints what got through:
    ///
    /// ```text
    /// Obtained { version: Available { name: "9999.9.9", .. }, provider: "hostile", .. }
    /// ```
    ///
    /// Note what does *not* discriminate: weakening `verify_signed_by` to a
    /// bare `verify` leaves this passing, because an archive with no signing
    /// block is refused either way. The pin is guarded by
    /// `apk_signature::pin_tests` instead. Two checks, two tests, and neither
    /// stands in for the other.
    #[test]
    fn a_hostile_provider_cannot_get_anything_past_the_check() {
        let dir = std::env::temp_dir().join("cordial-hostile-provider");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        let mut noise = |_: Progress| {};

        let trusted =
            vec!["44932ea35a17a267372d71b54d1a0cb3da0dca5113e94406ae2fe18090ba1477".to_string()];
        let e = obtain_from(vec![Box::new(Hostile)], Want::Newest, &Cancel::new(), &trusted, &dir, &mut noise)
            .expect_err("an unsigned archive must never be accepted, whatever produced it");

        // And it fails for the right reason. "No such file" would pass an
        // is_err() assertion while proving nothing about the signature path.
        let said = e.to_string();
        assert!(
            said.contains("signing block") || said.contains("signature") || said.contains("signed"),
            "must be refused over its signature, not incidentally: {said}"
        );

        // Nothing was adopted: the caller gets an error, not a half-install.
        assert!(!dir.join(".cordial-managed").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The Version page's download goes through the same check, and a refusal
    /// leaves the store with nothing in it -- not an entry, not a staging
    /// directory. The control is `the_same_path_accepts_a_genuinely_signed_archive`,
    /// since both paths call one `verify_archives`.
    #[test]
    fn a_hostile_provider_cannot_get_a_build_into_the_store_either() {
        let root = std::env::temp_dir().join(format!("cordial-hostile-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let trusted =
            vec!["44932ea35a17a267372d71b54d1a0cb3da0dca5113e94406ae2fe18090ba1477".to_string()];
        let e = obtain_into_store_from(
            &Hostile,
            &Available { name: "9999.9.9".into(), code: 1 },
            &root,
            &trusted,
            &Cancel::new(),
            &mut |_| {},
        )
        .expect_err("an unsigned archive must never be kept");
        assert!(matches!(e, Unreachable::Refused { .. }), "refused over its signature: {e}");
        assert!(crate::store::list_in(&root).is_empty());
        assert!(!root.join(".fetching").exists(), "staging is not left behind");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The control for the test above. **Without it, that test would pass even
    /// if `obtain_from` refused everything for an unrelated reason** -- a wrong
    /// path, a broken zip reader, a typo in the trusted list -- and would go on
    /// passing after the check it exists to guard had been deleted.
    #[test]
    fn the_same_path_accepts_a_genuinely_signed_archive() {
        let Some(apk) = real_apk() else {
            eprintln!("skipped: no Roblox APK on this machine");
            return;
        };
        let signer = crate::apk_signature::verify(&apk).expect("the shipping APK verifies");
        struct Genuine(std::path::PathBuf);
        impl Provider for Genuine {
            fn name(&self) -> &'static str {
                "genuine"
            }
            fn needs_network(&self) -> bool {
                false
            }
            fn newest(&self, _: &mut dyn FnMut(Progress)) -> Result<Available, Unreachable> {
                Ok(Available { name: "1.0.0".into(), code: 1 })
            }
            fn fetch(
                &self,
                _: &Available,
                _: &Cancel,
                _: &Path,
                _: &mut dyn FnMut(Progress),
            ) -> Result<Archives, Unreachable> {
                Ok(Archives { base: self.0.clone(), split: self.0.clone() })
            }
        }

        let dir = std::env::temp_dir().join("cordial-genuine-provider");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        let mut noise = |_: Progress| {};
        let got = obtain_from(
            vec![Box::new(Genuine(apk))],
            Want::Newest,
            &Cancel::new(),
            &[signer.certificate_sha256.clone()],
            &dir,
            &mut noise,
        )
        .expect("a signed archive from a made-up provider must be accepted");
        assert_eq!(got.certificate_sha256, signer.certificate_sha256);
        assert_eq!(got.provider, "genuine");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn real_apk() -> Option<std::path::PathBuf> {
        let p = std::env::var_os("HOME").map(std::path::PathBuf::from)?.join(
            ".var/app/org.vinegarhq.Sober/data/sober/packages/x86_64/com.roblox.client/base.apk",
        );
        p.is_file().then_some(p)
    }

    #[test]
    fn an_unknown_preferred_source_is_an_error_and_not_a_fallback() {
        let dir = std::env::temp_dir().join("cordial-obtain-unknown");
        std::fs::create_dir_all(&dir).expect("scratch");
        let mut noise = |_: Progress| {};
        let e = obtain(Some("no-such-source"), Want::Any, &Cancel::new(), &dir, &mut noise)
            .unwrap_err();
        assert!(e.to_string().contains("no-such-source"), "{e}");
        assert!(e.to_string().contains("local"), "the error should list what does exist: {e}");
    }
}
