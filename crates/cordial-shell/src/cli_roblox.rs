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
    if let Ok(build) = crate::install::locate(&config.roblox) {
        println!("Roblox is already here: {}", build.apk.display());
        println!("`stacked update` fetches a newer build if there is one.");
        return 0;
    }
    obtain(Want::Any)
}

pub fn update() -> u8 {
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
    obtain(Want::Newest)
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
            let Some(version) = offered.into_iter().find(|v| &v.name == wanted) else {
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
            let Some(version) = args.get(1) else {
                eprintln!("stacked: versions remove needs a VERSION");
                return 2;
            };
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
    let Some(version) = rest.first() else {
        eprintln!("stacked: pin needs a VERSION; `stacked versions` lists the kept ones");
        return 2;
    };
    // Refused here rather than at the next launch: a pin to a build that is
    // not on disk stops the profile launching at all.
    if !store::list().iter().any(|e| &e.version == version) {
        eprintln!("stacked: {version} is not kept. `stacked versions get {version}` fetches it first.");
        return 1;
    }
    set_pin(crate::chosen_profile(named), Some(version))
}

pub fn unpin(args: &[String]) -> u8 {
    let (named, _) = match crate::take_profile(args) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("stacked: {e}");
            return 2;
        }
    };
    set_pin(crate::chosen_profile(named), None)
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
