//! How the launcher tells a crash from an ordinary exit, and what it says
//! about one.
//!
//! The GTK launcher this replaced put the answer on an `AdwStatusPage` with the
//! client's last output behind an expander. `stacked` has a terminal instead,
//! and the output has been on it the whole time, so what is left is the
//! verdict and one sentence: see `main.rs`, which prints [`describe`] and
//! points at `stacked diagnostics`.
//!
//! ## A crash is written to the profile, and nothing else is
//!
//! `launch::pump` echoes the client's streams to the launcher's own stdout and
//! stderr, exactly as inheriting them did, and keeps a redacted copy in memory
//! for the crash summary. `[cookies]` and `[identity]` lines are replaced
//! before they reach the buffer -- see `launch::redact` for why that happens at
//! capture.
//!
//! That copy used to die with the launcher. Started from the desktop, or by a
//! manager that does not keep its child's output, a `SIGSEGV` left nothing on
//! disk at all: no core dump, nothing in Roblox's own log, which the engine
//! stops writing before the signal. "It crashes at random" could not be
//! investigated from such a report, because there was nothing to look at. So on
//! a crash, and only then, [`write_report`] puts the same redacted tail in
//! `<profile>/crashes/`, and keeps the newest [`KEPT_REPORTS`].

use std::path::{Path, PathBuf};

/// How many crash reports a profile keeps. Enough to see whether crashes share
/// a last line, few enough that a client crashing on every launch cannot fill
/// a disk.
pub const KEPT_REPORTS: usize = 20;

/// Write a crash report into `<profile_dir>/crashes`, then delete all but the
/// newest [`KEPT_REPORTS`]. Returns the report's path.
///
/// The name is a UTC timestamp in the shape Roblox's own logs use
/// (`20261001T003500Z`), so the files sort by time and line up with the
/// engine's log from the same session.
pub fn write_report(
    profile_dir: &Path,
    status: &std::process::ExitStatus,
    diagnostics: &str,
    output: &str,
) -> std::io::Result<PathBuf> {
    let dir = profile_dir.join("crashes");
    std::fs::create_dir_all(&dir)?;
    let stamp = utc_stamp(std::time::SystemTime::now());
    // Two crashes within a second -- a client that dies at load, relaunched by
    // a script -- must not overwrite each other.
    let mut path = dir.join(format!("crash-{stamp}.log"));
    let mut n = 1;
    while path.exists() {
        n += 1;
        path = dir.join(format!("crash-{stamp}-{n}.log"));
    }
    let text = format!(
        "Stacked crash report, {stamp}\n{}\n\n{}\n\n\
         The client's last output, oldest first (lines about your saved session are left out):\n\n{}\n",
        describe(status),
        diagnostics.trim_end(),
        output,
    );
    std::fs::write(&path, text)?;
    prune(&dir, KEPT_REPORTS);
    Ok(path)
}

/// Delete all but the newest `keep` crash reports in `dir`.
///
/// By modification time, with the name only breaking ties. The names alone do
/// not sort: ten crashes in one second were measured naming themselves
/// `...Z.log`, `...Z-2.log` up to `...Z-10.log`, which order as `-10`, `-2`,
/// ..., `-9`, then the first, so a sort by name would delete the wrong ones.
fn prune(dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut reports: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| n.starts_with("crash-") && n.ends_with(".log"))
        })
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    reports.sort();
    let excess = reports.len().saturating_sub(keep);
    for (_, old) in &reports[..excess] {
        let _ = std::fs::remove_file(old);
    }
}

/// `20261016T143005Z` for a moment in time.
fn utc_stamp(when: std::time::SystemTime) -> String {
    let secs = when.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as libc::time_t;
    // SAFETY: `gmtime_r` writes only into the `tm` it is handed, and reads
    // only `secs`; neither outlives the call.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        libc::gmtime_r(&secs, &mut tm);
        tm
    };
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

/// Whether an exit means something went wrong.
///
/// **Only a failing status counts.** The three ordinary ways a session ends --
/// the user closing Roblox's window, `SIGTERM`/`SIGINT`, and `--run` expiring
/// -- all leave `cordial-run` exiting zero, because it has one shutdown path
/// and three entry points into it (see `launch::DEFAULT_RUN_SECONDS`). A signal
/// death gives no code at all and `success()` is false for it, which is the
/// case this page most needs to catch: a `SIGSEGV` in the engine is the crash
/// nobody currently gets told about.
///
/// An X11 connection loss is a shutdown caused by the display server, not an
/// engine crash. Xlib has two messages for this depending on where the
/// connection is noticed: `XIO: fatal IO error` or `X connection to :1 broken
/// (explicit kill or server shutdown).` Both leave status 1. Treating every
/// non-zero status as a crash made the launcher show an alarming page for that
/// ordinary external teardown.
pub fn is_crash(status: &std::process::ExitStatus, output: &str) -> bool {
    if status.success() {
        return false;
    }

    // A signal is never a clean X11 teardown. The display may print a
    // connection-loss line while the process is already dying, but SIGSEGV,
    // SIGABRT and friends still describe a real process failure.
    if status.code().is_none() {
        return true;
    }

    // The exact spacing in Xlib's `XIO` line varies between versions, while
    // the connection-loss wording is stable. Keep this as a marker check
    // rather than special-casing exit code 1: a loader failure also returns 1
    // and must still reach the crash page.
    let display_shutdown = (output.contains("XIO:") && output.contains("fatal IO error"))
        || (output.contains("X connection to ") && output.contains(" broken"));
    !display_shutdown
}

/// How the exit is described in one line, without making the user read it as a
/// number.
///
/// `ExitStatus`'s own `Display` already spells out both shapes -- "exit status:
/// 1" and "signal: 11 (SIGSEGV)" -- so this adds the sentence around it rather
/// than reimplementing the formatting and getting the signal names wrong.
pub fn describe(status: &std::process::ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("Roblox stopped on its own, with exit code {code}."),
        None => format!("Roblox was stopped by the system ({status})."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;

    #[test]
    fn a_clean_exit_is_not_a_crash() {
        // The requirement that keeps this page out of the way of ordinary use:
        // closing Roblox, or `--run` expiring, must not raise an error window.
        assert!(!is_crash(&ExitStatus::from_raw(0), ""));
    }

    #[test]
    fn a_failing_exit_code_is_a_crash() {
        // `wait`-style encoding: the low byte is the signal, the next is the
        // code. 1 << 8 is "exited with 1".
        let status = ExitStatus::from_raw(1 << 8);
        assert!(is_crash(&status, ""));
        assert!(describe(&status).contains("exit code 1"), "{}", describe(&status));
    }

    #[test]
    fn a_signal_death_is_a_crash_and_is_not_described_as_an_exit_code() {
        // The case the old three-second alert could never catch: an engine
        // `SIGSEGV` twenty minutes into a session. There is no exit code here,
        // and saying "exit code 0" -- which `code()` returning `None` would
        // become under a careless `unwrap_or_default` -- would describe a crash
        // as a clean shutdown.
        let status = ExitStatus::from_raw(11); // SIGSEGV, no core flag
        assert!(is_crash(&status, ""));
        let line = describe(&status);
        assert!(!line.contains("exit code"), "{line}");
        assert!(line.contains("stopped by the system"), "{line}");
    }

    #[test]
    fn an_x11_connection_loss_is_not_an_engine_crash() {
        let status = ExitStatus::from_raw(1 << 8);
        assert!(!is_crash(
            &status,
            "XIO:  fatal IO error 0 (Success) on X server \":1\""
        ));
    }

    #[test]
    fn an_x11_connection_break_is_not_an_engine_crash() {
        let status = ExitStatus::from_raw(1 << 8);
        assert!(!is_crash(
            &status,
            "X connection to :1 broken (explicit kill or server shutdown)."
        ));
    }

    #[test]
    fn a_signal_with_an_x11_line_is_still_a_crash() {
        let status = ExitStatus::from_raw(libc::SIGSEGV);
        assert!(is_crash(
            &status,
            "X connection to :1 broken (explicit kill or server shutdown)."
        ));
    }

    #[test]
    fn a_report_carries_the_verdict_the_diagnostics_and_the_tail() {
        let profile = tempfile::tempdir().unwrap();
        let path = write_report(
            profile.path(),
            &ExitStatus::from_raw(libc::SIGSEGV),
            "Stacked   0.21.11 (abc)\n",
            "first line\n    (a line about your saved session was left out)\nlast line",
        )
        .unwrap();
        assert_eq!(path.parent().unwrap(), profile.path().join("crashes"));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("SIGSEGV"), "{text}");
        assert!(text.contains("Stacked   0.21.11 (abc)"), "{text}");
        assert!(text.trim_end().ends_with("last line"), "{text}");
    }

    #[test]
    fn two_crashes_in_one_second_are_both_kept() {
        let profile = tempfile::tempdir().unwrap();
        let status = ExitStatus::from_raw(libc::SIGABRT);
        let a = write_report(profile.path(), &status, "", "a").unwrap();
        let b = write_report(profile.path(), &status, "", "b").unwrap();
        assert_ne!(a, b);
        assert!(a.exists() && b.exists());
    }

    #[test]
    fn only_the_newest_reports_are_kept_and_other_files_are_left_alone() {
        // Twenty-five crashes in one second, named the way `write_report`
        // names them: the first bare, the rest `-2` to `-25`.
        let dir = tempfile::tempdir().unwrap();
        let base = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_791_895_805);
        for i in 1..=25u64 {
            let name = if i == 1 { "crash-20261013T125005Z.log".to_string() } else { format!("crash-20261013T125005Z-{i}.log") };
            let file = std::fs::File::create(dir.path().join(name)).unwrap();
            file.set_modified(base + std::time::Duration::from_millis(i)).unwrap();
        }
        std::fs::write(dir.path().join("notes.txt"), "").unwrap();
        prune(dir.path(), 20);
        let left: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(left.len(), 21, "{left:?}");
        assert!(left.contains(&"notes.txt".to_string()));
        // The first five are gone, the last twenty kept -- including `-10`
        // to `-25`, which a sort by name would have put first.
        assert!(!left.contains(&"crash-20261013T125005Z.log".to_string()), "{left:?}");
        assert!(!left.contains(&"crash-20261013T125005Z-5.log".to_string()), "{left:?}");
        assert!(left.contains(&"crash-20261013T125005Z-6.log".to_string()), "{left:?}");
        assert!(left.contains(&"crash-20261013T125005Z-25.log".to_string()), "{left:?}");
    }

    #[test]
    fn the_stamp_is_utc_in_robloxs_log_shape() {
        let when = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_791_895_805);
        assert_eq!(utc_stamp(when), "20261013T125005Z");
    }
}
