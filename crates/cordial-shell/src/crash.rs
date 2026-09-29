//! How the launcher tells a crash from an ordinary exit, and what it says
//! about one.
//!
//! The GTK launcher this replaced put the answer on an `AdwStatusPage` with the
//! client's last output behind an expander. `stacked` has a terminal instead,
//! and the output has been on it the whole time, so what is left is the
//! verdict and one sentence: see `main.rs`, which prints [`describe`] and
//! points at `stacked diagnostics`.
//!
//! ## The output is not written anywhere new
//!
//! `launch::pump` echoes the client's streams to the launcher's own stdout and
//! stderr, exactly as inheriting them did, and keeps a redacted copy in memory
//! for the crash summary. Nothing is added to a log file, and `[cookies]` and
//! `[identity]` lines are replaced before they reach the buffer -- see
//! `launch::redact` for why that happens at capture.

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
}
