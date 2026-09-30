//! `stacked install`, `update`, `versions`, `pin` and `unpin`: the Roblox
//! build, from the command line.
//!
//! Everything here is `cordial_update`'s, called the way the GTK launcher's
//! first-run screen, update dialog and Version page called it. What those
//! added was a progress bar and a Cancel button; a terminal has a line that
//! rewrites itself and Ctrl-C, and Ctrl-C is safe mid-download for the reason
//! [`cordial_update::provider::Cancel`] gives -- a fetch that stops leaves the
//! previous build in place and its staging directory is swept on the next one.

use cordial_shell::profile;
use cordial_update::provider::{self, Cancel, Progress, Want};
use cordial_update::store;
use std::io::Write;

/// One line of progress, overwritten in place when stdout is a terminal and
/// printed plainly otherwise, so a log captured from a pipe is readable.
pub(crate) fn progress_printer() -> impl FnMut(Progress) {
    let tty = rustix_isatty();
    let mut last_percent = None;
    move |p: Progress| {
        let line = match &p {
            Progress::Asking { provider } => format!("asking {provider}"),
            Progress::Fetching { file, done, total: Some(total) } if *total > 0 => {
                let percent = done.saturating_mul(100) / total;
                if !tty && last_percent == Some(percent / 10) {
                    return;
                }
                last_percent = Some(percent / 10);
                format!("downloading {file}: {percent}% of {} MB", total / 1_000_000)
            }
            Progress::Fetching { file, done, .. } => {
                format!("downloading {file}: {} MB", done / 1_000_000)
            }
            Progress::Verifying { file } => format!("checking {file} is signed by Roblox"),
        };
        if tty {
            print!("\r\x1b[2K{line}");
            let _ = std::io::stdout().flush();
        } else {
            println!("{line}");
        }
    }
}

fn rustix_isatty() -> bool {
    // SAFETY: `isatty` reads nothing but its argument and cannot fail in a way
    // that matters here; a false negative only means plain lines.
    unsafe { libc::isatty(1) == 1 }
}

pub(crate) fn end_progress_line() {
    if rustix_isatty() {
        println!();
    }
}

fn obtain(want: Want) -> u8 {
    let cancel = Cancel::new();
    let mut progress = progress_printer();
    let result = provider::obtain_and_install(
        None,
        want,
        Some(&cordial_update::install::Store::live(profile::all_pinned_versions())),
        &cancel,
        &mut progress,
    );
    end_progress_line();
    match result {
        Ok((got, installed)) => {
            println!(
                "installed Roblox {} from {}, signed by certificate {}",
                installed.version.unwrap_or(got.version.name),
                got.provider,
                &got.certificate_sha256[..got.certificate_sha256.len().min(16)],
            );
            println!("`stacked` starts it.");
            0
        }
        Err(e) => {
            eprintln!("stacked: {e}");
            1
        }
    }
}

/// `Any`: the cheapest usable build, which on a machine with Sober is a copy
/// already on disk and costs no request at all.
pub fn install() -> u8 {
    let config = crate::shell_config::load(&crate::shell_config::path());
    match crate::install::locate(&config.roblox) {
        Ok(build) => {
            println!("Roblox is already here: {}", build.apk.display());
            println!("`stacked update` fetches a newer build if there is one.");
            return 0;
        }
        // A build somebody named that cannot be used is not fixed by
        // downloading another: the named one still outranks it, so the
        // download would be 150 MB the next launch ignores, after a message
        // saying it was ready.
        Err(crate::install::NotFound::Unusable(why)) => {
            if let Some((_, origin)) = crate::install::effective_apk(&config.roblox) {
                if !crate::auto_update::manages(Some(origin)) {
                    eprintln!("stacked: {why}");
                    return 1;
                }
            }
        }
        Err(crate::install::NotFound::NoBuild) => {}
    }
    obtain(Want::Any)
}

pub fn update(args: &[String]) -> u8 {
    let force = match args {
        [] => false,
        [flag] if flag == "--force" => true,
        _ => {
            let other = args.iter().find(|a| *a != "--force").unwrap_or(&args[0]);
            eprintln!("stacked: unexpected argument {other:?}; update takes only --force");
            return 2;
        }
    };
    let config = crate::shell_config::load(&crate::shell_config::path());
    // A build the user pointed at, or Sober's, is theirs and not this
    // launcher's to replace: installing a newer one beside it would not change
    // what runs. Said rather than silently downloading 150 MB that is then
    // ignored.
    // Sober's copy is the exception: a managed build outranks it in
    // `effective_apk`, so downloading one is exactly how to stop using it.
    if let Some((_, origin)) = crate::install::effective_apk(&config.roblox) {
        if !origin.updatable() && origin != crate::install::Origin::Sober {
            eprintln!("stacked: {}", origin.why_not_updatable().unwrap_or_default());
            return 1;
        }
    }
    // Asked before downloading, because `Want::Newest` does not compare with
    // what is installed: it fetched and re-installed the same 150 MB whenever
    // nothing was newer, and said "installed" as though something had changed.
    if !force {
        if let Ok(build) = crate::install::locate(&config.roblox) {
            if let Some(installed) = cordial_update::engine::installed_version(&build.lib_dir) {
                println!("checking for a build newer than {installed}");
                match crate::auto_update::newest_online(std::time::Duration::from_secs(30)) {
                    Some(newest) if !cordial_update::version::is_newer(&newest, &installed) => {
                        println!("Roblox {installed} is the newest build on offer; nothing to download.");
                        return 0;
                    }
                    Some(_) => {}
                    None => println!("no source answered; trying to download anyway"),
                }
            }
        }
    }
    obtain(Want::Newest)
}

/// The store's own name for a build, if it keeps one matching `wanted`.
///
/// Matched by `version::same_build` as well as exactly, because the mirror and
/// the store spell one build two ways -- `2.734.917` and `2.734.0.917` -- and
/// `versions available` prints the first while `versions` prints the second.
/// Exact names alone meant `stacked versions get 2.734.917` followed by
/// `stacked pin 2.734.917` was refused as "not kept", with advice to download
/// the same build again.
fn kept_as(wanted: &str) -> Option<String> {
    kept_entry(wanted).map(|e| e.version)
}

fn kept_entry(wanted: &str) -> Option<store::Entry> {
    let entries = store::list();
    let exact = entries.iter().position(|e| e.version == wanted);
    let index = exact.or_else(|| entries.iter().position(|e| cordial_update::version::same_build(&e.version, wanted)))?;
    entries.into_iter().nth(index)
}

pub fn versions(args: &[String]) -> u8 {
    match args.first().map(String::as_str) {
        None | Some("list") => {
            let root = store::root();
            let current = store::current_in(&root, &crate::install::engine_cache());
            let pinned = profile::all_pinned_versions();
            let entries = store::list();
            if entries.is_empty() {
                println!("no builds kept. `stacked install` or `stacked versions get VERSION`.");
                return 0;
            }
            for entry in entries {
                let mut notes = Vec::new();
                if current.as_deref() == Some(entry.version.as_str()) {
                    notes.push("current".to_string());
                }
                if pinned.contains(&entry.version) {
                    notes.push("pinned".to_string());
                }
                if !entry.complete {
                    notes.push("incomplete".to_string());
                }
                if let Some(by) = &entry.loaded_by {
                    notes.push(format!("last run by {by}"));
                }
                println!(
                    "{:<16} {:>5} MB  {}",
                    entry.version,
                    entry.bytes / 1_000_000,
                    notes.join(", ")
                );
            }
            0
        }
        Some("available") => match provider::mirror::offered() {
            Ok(offered) => {
                for v in offered {
                    println!("{}", v.name);
                }
                0
            }
            Err(e) => {
                eprintln!("stacked: {e}");
                1
            }
        },
        Some("get") => {
            let Some(wanted) = args.get(1) else {
                eprintln!("stacked: versions get needs a VERSION; `stacked versions available` lists them");
                return 2;
            };
            let offered = match provider::mirror::offered() {
                Ok(offered) => offered,
                Err(e) => {
                    eprintln!("stacked: {e}");
                    return 1;
                }
            };
            // Only a complete entry counts as kept. One without its APK --
            // keyed before archives were kept, or linked from another
            // filesystem -- cannot run, and fetching it again is how it is
            // repaired, so it falls through to the download.
            if let Some(kept) = kept_entry(wanted).filter(|e| e.complete) {
                let kept = kept.version;
                println!("Roblox {kept} is already kept. `stacked pin {kept}` runs it on the current profile.");
                return 0;
            }
            let Some(version) = offered
                .into_iter()
                .find(|v| &v.name == wanted || cordial_update::version::same_build(&v.name, wanted))
            else {
                eprintln!("stacked: {wanted} is not offered; `stacked versions available` lists what is");
                return 1;
            };
            let cancel = Cancel::new();
            let mut progress = progress_printer();
            let result = provider::obtain_into_store(&version, &store::root(), &cancel, &mut progress);
            end_progress_line();
            match result {
                Ok(kept) => {
                    println!("kept Roblox {kept}. `stacked pin {kept}` runs it on the current profile.");
                    0
                }
                Err(e) => {
                    eprintln!("stacked: {e}");
                    1
                }
            }
        }
        Some("remove" | "rm" | "delete") => {
            let Some(wanted) = args.get(1) else {
                eprintln!("stacked: versions remove needs a VERSION");
                return 2;
            };
            let version = &kept_as(wanted).unwrap_or_else(|| wanted.clone());
            match store::remove_in(
                &store::root(),
                &crate::install::engine_cache(),
                version,
                &profile::all_pinned_versions(),
            ) {
                Ok(()) => {
                    println!("removed Roblox {version}");
                    0
                }
                Err(e) => {
                    eprintln!("stacked: {e}");
                    1
                }
            }
        }
        Some(other) => {
            eprintln!("stacked: unknown versions command {other:?}. Try `stacked help`.");
            2
        }
    }
}

pub fn pin(args: &[String]) -> u8 {
    let (named, rest) = match crate::take_profile(args) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("stacked: {e}");
            return 2;
        }
    };
    let Some(wanted) = rest.first() else {
        eprintln!("stacked: pin needs a VERSION; `stacked versions` lists the kept ones");
        return 2;
    };
    // Refused here rather than at the next launch: a pin to a build that is
    // not on disk stops the profile launching at all.
    let Some(entry) = kept_entry(wanted) else {
        eprintln!("stacked: {wanted} is not kept. `stacked versions get {wanted}` fetches it first.");
        return 1;
    };
    // `install::apply_pin` refuses an entry without its APK at launch, so a
    // pin to one would only move the failure to the next Play.
    if !entry.complete {
        eprintln!(
            "stacked: {} is kept without its APK, so it cannot run. \
             `stacked versions get {}` fetches it again.",
            entry.version, entry.version
        );
        return 1;
    }
    let version = entry.version;
    match crate::existing_profile(named) {
        Ok(name) => set_pin(name, Some(&version)),
        Err(e) => {
            eprintln!("stacked: {e}");
            1
        }
    }
}

pub fn unpin(args: &[String]) -> u8 {
    let (named, _) = match crate::take_profile(args) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("stacked: {e}");
            return 2;
        }
    };
    match crate::existing_profile(named) {
        Ok(name) => set_pin(name, None),
        Err(e) => {
            eprintln!("stacked: {e}");
            1
        }
    }
}

fn set_pin(name: String, version: Option<&str>) -> u8 {
    let result = profile::dir(&name).and_then(|dir| profile::set_pinned_version(&dir, version));
    match (result, version) {
        (Ok(()), Some(v)) => {
            println!("profile {name:?} now runs Roblox {v}");
            0
        }
        (Ok(()), None) => {
            println!("profile {name:?} runs the current build");
            0
        }
        (Err(e), _) => {
            eprintln!("stacked: {e}");
            1
        }
    }
}
