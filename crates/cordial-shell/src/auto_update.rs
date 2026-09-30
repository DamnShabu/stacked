//! Keeping the Roblox build current at the moment somebody asks to play.
//!
//! Roblox enforces a minimum client version on its servers and moves it
//! roughly weekly, so a build that launched fine last week can be refused at
//! the join screen this week. The GTK launcher had a background timer for
//! this; ADR-043 removed it with the window, and for the release that followed,
//! staying current meant remembering to run `stacked update`. Sober updates on
//! its own, and "it stopped letting me join" is the report a manual step
//! produces. [ADR-044](../../../docs/adr/ADR-044-updates-happen-at-launch.md)
//! records the decision; this module is the whole of it.
//!
//! **A check never stops a launch.** Offline, a mirror that is down, a
//! connection NetworkManager calls metered, a download that fails part way:
//! each is said in one line and the build already on disk starts. The only
//! case that must fetch before it can play is a machine with no build at all,
//! and that is the first-run install `stacked install` already did.
//!
//! **Bounded, and memoised.** The check is one small request to each
//! networked source, run on its own thread with [`CHECK_DEADLINE`], so a stalled
//! DNS lookup costs five seconds rather than the launch. A check that found
//! nothing newer is trusted for [`RECHECK_AFTER`], so relaunching after a crash
//! or rejoining a server does not pay for it again.

use crate::install::{self, Build, Origin};
use cordial_update::provider::{self, Cancel, Want};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// How long the version check may hold up a launch before the build on disk
/// starts anyway.
const CHECK_DEADLINE: Duration = Duration::from_secs(5);

/// How long a check that found nothing newer is trusted.
const RECHECK_AFTER: Duration = Duration::from_secs(10 * 60);

/// What to do about the build before this launch.
#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    /// Not Stacked's build to replace: an APK somebody named, by setting or by
    /// `CORDIAL_APK`. Updating would download a build the launcher then
    /// declines to use, because a chosen APK outranks a managed one.
    NotOurs,
    /// This profile runs a pinned version, so the current build is not what
    /// is about to start and fetching a newer one now buys nothing.
    Pinned,
    /// The installed build's version could not be read, so nothing can be
    /// established as newer than it. Downloading anyway would fetch 150 MB on
    /// every launch of a build this cannot identify.
    UnknownInstalled,
    /// No networked source answered in time.
    Unanswered,
    Current,
    Newer { installed: String, newest: String },
}

/// The decision, with the network asked only if it can change the answer.
///
/// `newest` is a closure so that a build that is not ours, or a pinned
/// profile, costs no request at all -- and so the tests can prove it.
pub fn decide(
    origin: Origin,
    pinned: bool,
    installed: Option<&str>,
    newest: impl FnOnce() -> Option<String>,
) -> Decision {
    if matches!(origin, Origin::Environment | Origin::Chosen) {
        return Decision::NotOurs;
    }
    if pinned {
        return Decision::Pinned;
    }
    let Some(installed) = installed else {
        return Decision::UnknownInstalled;
    };
    match newest() {
        None => Decision::Unanswered,
        Some(newest) if cordial_update::version::is_newer(&newest, installed) => {
            Decision::Newer { installed: installed.to_string(), newest }
        }
        Some(_) => Decision::Current,
    }
}

/// The newest version any networked source offers, or `None` if none answered
/// within `deadline`.
///
/// **Networked sources only.** The local provider answers by decompressing and
/// scanning the engine inside Sober's APK, which is the most expensive thing a
/// version question can cost; the build on disk has already been identified by
/// [`installed_version`] by the time this runs, so asking it again would pay
/// that cost to learn nothing.
pub(crate) fn newest_online(deadline: Duration) -> Option<String> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut best: Option<String> = None;
        for source in provider::all().into_iter().filter(|p| p.needs_network()) {
            let Ok(offered) = source.newest(&mut |_| {}) else { continue };
            if best.as_deref().is_none_or(|have| cordial_update::version::is_newer(&offered.name, have)) {
                best = Some(offered.name);
            }
        }
        let _ = tx.send(best);
    });
    // A thread still waiting on a stalled connection is left to finish on its
    // own: it holds nothing the launch needs, and this process outlives it by
    // exactly as long as the game runs.
    rx.recv_timeout(deadline).ok().flatten()
}

/// The version of the build that would start, from the engine itself.
///
/// `cordial_update::engine::installed_version` memoises the scan beside the
/// library, keyed on its length and modification time, and the client calls
/// the same function on every start -- so on any launch after the first this
/// reads a short file rather than 118 MB.
fn installed_version(build: &Build) -> Option<String> {
    cordial_update::engine::installed_version(&build.lib_dir)
}

fn memo_path() -> PathBuf {
    cordial_update::install::cache_root().join("update-checked")
}

/// Whether a check that found nothing newer ran recently enough to trust.
fn checked_recently(memo: &Path, installed: &str, now: SystemTime) -> bool {
    let Ok(text) = std::fs::read_to_string(memo) else { return false };
    let mut lines = text.lines();
    let (Some(when), Some(version)) = (lines.next(), lines.next()) else { return false };
    let Ok(secs) = when.trim().parse::<u64>() else { return false };
    let then = SystemTime::UNIX_EPOCH + Duration::from_secs(secs);
    // Keyed on the version too: a build that changed since the check -- a
    // `stacked versions` switch, Sober updating its own copy -- is a new
    // question, not a remembered answer. `same_build` because an update is
    // remembered in the mirror's spelling and read back in the engine's.
    let version = version.trim();
    (version == installed || cordial_update::version::same_build(version, installed))
        && now.duration_since(then).is_ok_and(|age| age < RECHECK_AFTER)
}

fn remember_check(memo: &Path, installed: &str, now: SystemTime) {
    let secs = now.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    if let Some(parent) = memo.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let tmp = memo.with_extension("new");
    if std::fs::write(&tmp, format!("{secs}\n{installed}\n")).is_ok() {
        let _ = std::fs::rename(&tmp, memo);
    }
}

/// Whether NetworkManager says, or guesses, that this connection is paid for
/// by the megabyte.
///
/// **Narrower than [`cordial_update::metered::Metered::is_metered`] on
/// purpose.** That reading treats "unknown" as metered, which is right for a
/// download nobody asked for and wrong here: somebody pressed Play, the
/// download is announced with its size and can be stopped with Ctrl-C, and a
/// machine without NetworkManager -- most of systemd-networkd and iwd -- would
/// otherwise never be updated at all. Positive evidence of a metered
/// connection still holds the download back.
fn metered() -> bool {
    use cordial_update::metered::Metered;
    matches!(cordial_update::metered::query(), Ok(Metered::Yes | Metered::GuessYes))
}

/// Say something where the person who started this will see it.
///
/// A terminal gets it on stdout. A launch from the desktop entry or a browser
/// has no terminal, and a 150 MB download that nothing announces reads as a
/// Play button that did nothing for a minute, so it also becomes a desktop
/// notification there.
fn announce(summary: &str, body: &str) {
    println!("stacked: {body}");
    // SAFETY: `isatty` reads nothing but its argument.
    if unsafe { libc::isatty(1) } != 1 {
        let _ = cordial_plugins::notify::send(summary, body);
    }
}

/// Install a build on a machine with none, the way `stacked install` does.
///
/// `Want::Newest` rather than `Any`: `Any` takes the cheapest build, which on
/// a machine with Sober is Sober's copy whatever its age, and the check on the
/// next launch would then download the newest one anyway. `Newest` still
/// prefers Sober's copy when it *is* the newest, since the sources are ordered
/// free-first on a tie.
pub fn first_install() -> Result<(), String> {
    // The same rule as an update, and more so: this is the whole build.
    // `stacked install` is the explicit way to fetch it anyway.
    if metered() {
        return Err(METERED.into());
    }
    announce(
        "Stacked is downloading Roblox",
        "no Roblox build yet; getting one now. This happens once, and the game starts when it is done.",
    );
    obtain()
}

/// What [`first_install`] says when it would not download.
pub const METERED: &str = "there is no Roblox build yet, and this connection is metered, so it \
                           was not downloaded. `stacked install` downloads it when you choose.";

fn obtain() -> Result<(), String> {
    let cancel = Cancel::new();
    let mut progress = crate::cli_roblox::progress_printer();
    let result = provider::obtain_and_install(
        None,
        Want::Newest,
        Some(&cordial_update::install::Store::live(cordial_shell::profile::all_pinned_versions())),
        &cancel,
        &mut progress,
    );
    crate::cli_roblox::end_progress_line();
    match result {
        Ok((got, installed)) => {
            println!(
                "stacked: installed Roblox {} from {}",
                installed.version.unwrap_or(got.version.name),
                got.provider
            );
            Ok(())
        }
        Err(e) => Err(e.to_string()),
    }
}

/// Bring the build up to date if it is Stacked's to update and a newer one is
/// offered. Returns whether a new build was installed, so the caller knows to
/// locate it again.
pub fn before_launch(build: &Build, origin: Origin, pinned: bool) -> bool {
    let installed = installed_version(build);
    let memo = memo_path();
    let now = SystemTime::now();
    if let Some(v) = installed.as_deref() {
        if checked_recently(&memo, v, now) {
            return false;
        }
    }
    let decision = decide(origin, pinned, installed.as_deref(), || newest_online(CHECK_DEADLINE));
    match decision {
        Decision::NotOurs | Decision::Pinned | Decision::UnknownInstalled => false,
        Decision::Unanswered => {
            println!("stacked: could not check for a newer Roblox; starting the build you have");
            false
        }
        Decision::Current => {
            if let Some(v) = installed.as_deref() {
                remember_check(&memo, v, now);
            }
            false
        }
        Decision::Newer { installed, newest } => {
            if metered() {
                println!(
                    "stacked: Roblox {newest} is out (you have {installed}). This connection is \
                     metered, so it was not downloaded; `stacked update` gets it when you choose."
                );
                return false;
            }
            announce(
                "Stacked is updating Roblox",
                &format!("updating Roblox {installed} to {newest}. The game starts when it is done."),
            );
            match obtain() {
                Ok(()) => {
                    remember_check(&memo, &newest, now);
                    true
                }
                // Remembered like a check that found nothing, so a failure that
                // will recur -- a refused signature, a mirror that breaks part
                // way -- costs one attempt every ten minutes rather than a
                // download on every launch.
                Err(e) => {
                    eprintln!(
                        "stacked: the update failed ({e}); starting Roblox {installed}. \
                         It is tried again in ten minutes, or now with `stacked update`."
                    );
                    remember_check(&memo, &installed, now);
                    false
                }
            }
        }
    }
}

/// Whether `origin` is a build this module would ever replace, for callers
/// that want to say so without running a check.
pub fn manages(origin: Option<Origin>) -> bool {
    matches!(origin, None | Some(Origin::Managed | Origin::Sober))
}

/// The origin of the build `locate` would find, if any, as far as updating
/// it is concerned.
///
/// **An engine named with `roblox.lib_dir` makes the build the user's**,
/// whatever the APK's origin: `locate` runs that engine against whichever APK
/// it finds, so installing a newer APK would pair new assets with the old
/// engine -- the mismatch `cordial_update::cache` exists to prevent -- and the
/// engine's version would never change, so the check would download again on
/// every launch. Found by review, and reproduced through `locate_with`.
pub fn origin_of(config: &crate::shell_config::ShellConfig) -> Option<Origin> {
    let origin = install::effective_apk(&config.roblox).map(|(_, origin)| origin);
    match origin {
        Some(Origin::Managed | Origin::Sober) if config.roblox.lib_dir.is_some() => Some(Origin::Chosen),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn never() -> Option<String> {
        panic!("the network must not be asked for this decision")
    }

    #[test]
    fn a_build_somebody_chose_is_never_checked() {
        assert_eq!(decide(Origin::Chosen, false, Some("2.700.1"), never), Decision::NotOurs);
        assert_eq!(decide(Origin::Environment, false, Some("2.700.1"), never), Decision::NotOurs);
    }

    #[test]
    fn a_pinned_profile_is_not_checked() {
        assert_eq!(decide(Origin::Managed, true, Some("2.700.1"), never), Decision::Pinned);
    }

    #[test]
    fn an_unreadable_installed_version_is_not_treated_as_old() {
        assert_eq!(decide(Origin::Managed, false, None, never), Decision::UnknownInstalled);
    }

    #[test]
    fn newer_is_decided_across_the_two_version_shapes() {
        // The engine records four components and the mirror three, for the
        // same build; `version::is_newer` is what makes those compare equal.
        assert_eq!(
            decide(Origin::Managed, false, Some("2.734.0.917"), || Some("2.734.917".into())),
            Decision::Current
        );
        assert_eq!(
            decide(Origin::Sober, false, Some("2.734.0.917"), || Some("2.735.1002".into())),
            Decision::Newer { installed: "2.734.0.917".into(), newest: "2.735.1002".into() }
        );
        assert_eq!(
            decide(Origin::Managed, false, Some("2.735.0.1002"), || Some("2.734.917".into())),
            Decision::Current,
            "an older build on the mirror is not an update"
        );
    }

    #[test]
    fn no_answer_is_unanswered_and_not_current() {
        assert_eq!(decide(Origin::Managed, false, Some("2.734.0.917"), || None), Decision::Unanswered);
    }

    #[test]
    fn a_stalled_check_gives_up_at_the_deadline() {
        // The real sources are unreachable from a test, and whether they fail
        // fast or hang depends on the machine, so this measures the one thing
        // that is promised: the call returns by the deadline whatever the
        // network does.
        let started = std::time::Instant::now();
        let _ = newest_online(Duration::from_millis(300));
        assert!(started.elapsed() < Duration::from_secs(2), "took {:?}", started.elapsed());
    }

    #[test]
    fn the_memo_expires_and_is_keyed_on_the_version() {
        let dir = tempfile::tempdir().unwrap();
        let memo = dir.path().join("update-checked");
        let now = SystemTime::now();
        assert!(!checked_recently(&memo, "2.734.0.917", now), "no memo, no trust");
        remember_check(&memo, "2.734.0.917", now);
        assert!(checked_recently(&memo, "2.734.0.917", now));
        assert!(!checked_recently(&memo, "2.735.0.1", now), "a different build is a new question");
        remember_check(&memo, "2.735.1002", now);
        assert!(checked_recently(&memo, "2.735.0.1002", now), "the mirror's spelling of the same build");
        assert!(!checked_recently(&memo, "2.734.0.917", now + RECHECK_AFTER + Duration::from_secs(1)));
        std::fs::write(&memo, "garbage").unwrap();
        assert!(!checked_recently(&memo, "2.734.0.917", now));
    }

    #[test]
    fn only_stackeds_own_and_sobers_builds_are_managed() {
        assert!(manages(None));
        assert!(manages(Some(Origin::Managed)));
        assert!(manages(Some(Origin::Sober)));
        assert!(!manages(Some(Origin::Chosen)));
        assert!(!manages(Some(Origin::Environment)));
    }
}
