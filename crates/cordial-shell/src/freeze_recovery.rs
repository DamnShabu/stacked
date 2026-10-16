//! Recognising the signed-in startup freeze (#92) from the engine's own log,
//! and deciding how many times to restart it.
//!
//! The freeze is a client that starts, logs `Forcing finalize`, and then waits
//! for a task nothing posts. It prints nothing further, burns no CPU and does
//! not exit, so a supervisor that only waits for a process to die waits
//! forever. What does differ from a healthy start is in the log, and the shape
//! was measured rather than guessed, over every engine log on the machine that
//! had a `Forcing finalize` line (`docs/analysis/startup-freeze-capture.md`,
//! 2026-10-04): the healthy runs logged `[Graphics] RenderView destroyed[1]`
//! 10-76 ms before it, and the frozen ones never did. In a healthy run
//! `~UgcExperienceController` follows within about a tenth of a second; in a
//! frozen one it does not follow at all, so five seconds is a wide margin
//! rather than a tuned one.
//!
//! Nothing is injected and nothing is hooked: the engine writes this file into
//! a directory Cordial made, and the shell reads it, which is the argument
//! `cordial_runtime::game_log` already makes for tailing the same directory.
//! That module is in the crate that depends on this one, so the little reader
//! here is a second copy of an idea rather than a call into it.
//!
//! **This restarts a client; it does not fix the race.** The cause is still
//! INFERRED (see the analysis note), and a recovery that works says nothing
//! about whether the race is lost less often.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// The first line of the finalize the engine begins once settings and flags
/// have loaded. Matched on the prefix: the state number after it is the engine's
/// and has been `1` in every log, but nothing here depends on it.
const FINALIZE: &str = "[FLog::SingleSurfaceApp] Forcing finalize";

/// Logged on the finalize thread inside `~SurfaceController[_:1]` when the first
/// app's render view is released. Present before `Forcing finalize` in 22 of 22
/// healthy signed-in logs and absent from all 19 frozen ones.
const RENDER_VIEW_DESTROYED: &str = "[FLog::Graphics] RenderView destroyed[1]";

/// The controller's destructor, which only runs once the finalize has finished.
const CONTROLLER_DESTROYED: &str = "~UgcExperienceController";

/// How long a finalize may go without its destructor before it is called stuck.
pub const FROZEN_AFTER: Duration = Duration::from_secs(5);

/// How many times one `stacked play` is restarted before it gives up.
/// Two, so one launch makes at most three attempts. The freeze was one signed-in
/// start in eight on the day it was measured; if starts were independent, which
/// nobody has checked, three in a row would be about one in five hundred, and a
/// fourth attempt would be a user watching a loop.
pub const MAX_RESTARTS: u32 = 2;

/// How long the shell keeps reading a new client's log before it stops looking.
/// The engine reaches `Forcing finalize` within seconds of its log appearing,
/// so this is a bound on a client that never gets there, not a deadline.
pub const WATCH_FOR: Duration = Duration::from_secs(120);

/// What the log shows so far. Pure over the text, and the whole of the format
/// knowledge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scan {
    /// The first `Forcing finalize` has not been logged yet.
    Undecided,
    /// It has, nothing released the first app's render view before it, and the
    /// controller's destructor has not followed. Frozen if it stays this way.
    Finalizing,
    /// The finalize is the healthy one: either the render view was released
    /// first, or the destructor has run.
    Healthy,
}

/// Read the log text so far.
///
/// Only the first `Forcing finalize` decides. A session that plays a game and
/// leaves logs another one at teardown, with the render view long since
/// released; that must not be able to turn a healthy session frozen, and a
/// frozen one never gets that far.
pub fn scan(log: &str) -> Scan {
    let mut render_view_released = false;
    let mut finalizing = false;
    for line in log.lines() {
        if !finalizing {
            if line.contains(RENDER_VIEW_DESTROYED) {
                render_view_released = true;
            } else if line.contains(FINALIZE) {
                if render_view_released {
                    return Scan::Healthy;
                }
                finalizing = true;
            }
        } else if line.contains(CONTROLLER_DESTROYED) {
            return Scan::Healthy;
        }
    }
    if finalizing {
        Scan::Finalizing
    } else {
        Scan::Undecided
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Undecided,
    Healthy,
    Frozen,
}

/// Turns successive [`Scan`]s into a verdict, because "no destructor yet" only
/// means frozen once it has been true for [`FROZEN_AFTER`].
///
/// `now` is time since the watch began, measured by the caller; this holds no
/// clock so the five seconds can be tested without waiting for them. A
/// decision, once made, stays made: a client declared frozen is being stopped
/// and must not be un-declared by a destructor line that arrives while it dies.
#[derive(Debug, Default)]
pub struct Watch {
    finalizing_since: Option<Duration>,
    decided: Option<Verdict>,
}

impl Watch {
    pub fn observe(&mut self, scan: Scan, now: Duration) -> Verdict {
        if let Some(v) = self.decided {
            return v;
        }
        let verdict = match scan {
            Scan::Undecided => Verdict::Undecided,
            Scan::Healthy => Verdict::Healthy,
            Scan::Finalizing => {
                let since = *self.finalizing_since.get_or_insert(now);
                if now.saturating_sub(since) >= FROZEN_AFTER {
                    Verdict::Frozen
                } else {
                    Verdict::Undecided
                }
            }
        };
        if verdict != Verdict::Undecided {
            self.decided = Some(verdict);
        }
        verdict
    }
}

/// What to do about a client that froze, given how many restarts this
/// `stacked play` has already used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    /// Start it again. `attempt` is the number of the launch about to happen,
    /// counting the first as 1, and `of` is how many there can be in all.
    Restart { attempt: u32, of: u32 },
    GiveUp,
}

pub fn after_freeze(restarts_so_far: u32) -> Next {
    if restarts_so_far >= MAX_RESTARTS {
        Next::GiveUp
    } else {
        Next::Restart { attempt: restarts_so_far + 2, of: MAX_RESTARTS + 1 }
    }
}

/// The one line the user is shown while the client is started again.
pub fn status_line(attempt: u32, of: u32) -> String {
    format!("Roblox got stuck starting. Restarting it ({attempt} of {of}).")
}

pub const GIVE_UP_HEADING: &str = "Roblox keeps getting stuck starting";

/// Said instead of [`status_line`] when the user pressed Ctrl-C while the
/// client was stuck.
pub const INTERRUPTED_WHILE_STUCK: &str =
    "Roblox got stuck starting and has been stopped. Not starting it again, because Ctrl-C was pressed.";

/// What the user is told when every attempt froze.
pub fn give_up_body() -> String {
    format!(
        "Roblox got stuck while starting {} times in a row, and Stacked stopped it each time. \
         This is a known problem on some starts (issue 92 on upstream Cordial's GitHub) and \
         Stacked does not yet know how to prevent it. Starting it again usually works. To stop \
         Stacked restarting a stuck start on its own, set CORDIAL_NO_FREEZE_RESTART=1.",
        MAX_RESTARTS + 1
    )
}

/// `CORDIAL_NO_FREEZE_RESTART=1` turns the recovery off, so a freeze can be
/// measured as a freeze rather than recovered from.
pub fn enabled(no_restart: Option<&str>) -> bool {
    !matches!(no_restart, Some("1") | Some("true") | Some("yes"))
}

/// Set by [`note_interrupt`] and never cleared: once the user has pressed
/// Ctrl-C, nothing this `stacked play` does afterwards should start a client.
static INTERRUPTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

extern "C" fn note_interrupt(_: libc::c_int) {
    // A store to an atomic is all a signal handler may safely do here.
    INTERRUPTED.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// Take Ctrl-C for the rest of this process: note it rather than die of it.
///
/// Ctrl-C reaches the whole foreground process group, so the client hears it
/// and shuts down in order on its own, and this process must outlive it: it
/// holds the read end of the client's output, and a client that loses its
/// stdout mid-shutdown fails there instead of exiting cleanly. That used to be
/// `SIG_IGN`, which threw away the one thing the restart needs to know. A
/// client frozen at startup does not act on Ctrl-C, so the watch is what
/// stops it, and without the note the launcher then started again what the
/// user had just asked to quit, up to [`MAX_RESTARTS`] times.
///
/// A handler rather than `SIG_IGN` also fixes the order of operations that
/// used to need care. An ignored signal survives `exec`, so `SIG_IGN` could
/// only be set after a spawn and had to be put back to `SIG_DFL` before the
/// next one; a caught signal is reset to the default by `exec` itself, so
/// every client, first or restarted, starts able to hear Ctrl-C however many
/// times this is called. A `SIGINT` this process inherited as ignored, from a
/// shell that started it in the background, is left ignored: no Ctrl-C can
/// come from the user then, and the clients inherit it as they always did.
/// `SA_RESTART` so the wait and the output readers carry on through one.
pub fn catch_interrupt() {
    // SAFETY: querying and installing a disposition with a zeroed `sigaction`
    // whose handler only stores to an atomic.
    unsafe {
        let mut old: libc::sigaction = std::mem::zeroed();
        if libc::sigaction(libc::SIGINT, std::ptr::null(), &mut old) != 0 || old.sa_sigaction == libc::SIG_IGN {
            return;
        }
        let mut new: libc::sigaction = std::mem::zeroed();
        new.sa_sigaction = note_interrupt as extern "C" fn(libc::c_int) as libc::sighandler_t;
        new.sa_flags = libc::SA_RESTART;
        libc::sigemptyset(&mut new.sa_mask);
        libc::sigaction(libc::SIGINT, &new, std::ptr::null_mut());
    }
}

/// Whether Ctrl-C has been pressed since [`catch_interrupt`].
pub fn interrupted() -> bool {
    INTERRUPTED.load(std::sync::atomic::Ordering::SeqCst)
}

/// Where the engine writes its logs for this profile.
///
/// The same layout `load.rs` builds from the client's side, including its
/// `CORDIAL_FILES_DIR` override, which the client inherits from the shell's
/// environment.
pub fn logs_dir(profile_dir: &Path) -> PathBuf {
    std::env::var_os("CORDIAL_FILES_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::profile::engine_data_dir(profile_dir))
        .join("files/appData/logs")
}

/// The newest `*.log` in `dir` written at or after `since`.
///
/// The `since` is what stops a relaunch reading the log of the client it just
/// stopped: that file is the newest in the directory and is, by construction,
/// the frozen shape, so without it every restart would be judged frozen on the
/// first poll.
fn newest_log_since(dir: &Path, since: SystemTime) -> Option<PathBuf> {
    let mut best: Option<(SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "log") {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else { continue };
        if modified < since {
            continue;
        }
        if best.as_ref().is_none_or(|(t, _)| modified > *t) {
            best = Some((modified, path));
        }
    }
    best.map(|(_, p)| p)
}

/// Read at most this much of a log. A frozen start is a few hundred lines and a
/// healthy one a few thousand; the cap is for a log that is not either.
const READ_CAP: u64 = 8 * 1024 * 1024;

/// Debug builds only: read this file instead of the engine's log.
///
/// A freeze is one signed-in start in eight and a test of the supervision path
/// cannot wait for one, so a debug shell takes the log from a file that a test
/// writes the frozen shape into. It is compiled out of release builds so a
/// shipped shell has no way to be told a client is frozen when it is not.
#[cfg(debug_assertions)]
fn test_log() -> Option<PathBuf> {
    std::env::var_os("CORDIAL_FREEZE_TEST_LOG").map(PathBuf::from)
}

#[cfg(not(debug_assertions))]
fn test_log() -> Option<PathBuf> {
    None
}

/// Reads one client's log and keeps its [`Watch`].
pub struct Supervisor {
    dir: PathBuf,
    since: SystemTime,
    began: Instant,
    watch: Watch,
}

impl Supervisor {
    /// Begin watching a client that is being started now. Take it *before* the
    /// spawn so a log the new client writes cannot predate `since`.
    ///
    /// Within a millisecond or so it can: a file's mtime comes from the
    /// kernel's coarse clock, and a file written straight after
    /// `SystemTime::now()` was stamped earlier than it in 2,000 writes of
    /// 2,000 here, by up to 0.8 ms. The engine writes its log only once its
    /// library has loaded, far later than that (`INFERRED`: not timed), so it
    /// matters only to a test that writes the log itself.
    pub fn new(dir: PathBuf) -> Self {
        Self { dir, since: SystemTime::now(), began: Instant::now(), watch: Watch::default() }
    }

    pub fn timed_out(&self) -> bool {
        self.began.elapsed() >= WATCH_FOR
    }

    /// Look at the log once. Never blocks for longer than one file read, and
    /// every failure to read is "nothing yet": the log does not exist for the
    /// first seconds of a start.
    pub fn poll(&mut self) -> Verdict {
        let path = match test_log() {
            Some(p) => Some(p),
            None => newest_log_since(&self.dir, self.since),
        };
        let Some(path) = path else { return Verdict::Undecided };
        let Ok(file) = std::fs::File::open(&path) else { return Verdict::Undecided };
        let mut bytes = Vec::new();
        if std::io::Read::read_to_end(&mut std::io::Read::take(file, READ_CAP), &mut bytes).is_err() {
            return Verdict::Undecided;
        }
        self.watch.observe(scan(&String::from_utf8_lossy(&bytes)), self.began.elapsed())
    }
}

/// Ask a client the shell started to exit, `SIGTERM` first.
///
/// Refuses a pid that is not `cordial-run`, next to the `kill` rather than at
/// the call site, for the same reason `profile::Holder::ask_to_stop` does.
pub fn terminate(pid: u32) -> Result<(), String> {
    signal(pid, libc::SIGTERM)
}

/// The escalation, for a frozen engine that ignores `SIGTERM`. A start that
/// froze before it loaded a game has nothing worth a graceful shutdown.
pub fn kill(pid: u32) -> Result<(), String> {
    signal(pid, libc::SIGKILL)
}

fn signal(pid: u32, sig: libc::c_int) -> Result<(), String> {
    let exe = std::fs::read_link(format!("/proc/{pid}/exe")).map_err(|e| format!("process {pid}: {e}"))?;
    if exe.file_name().and_then(|n| n.to_str()) != Some("cordial-run") {
        return Err(format!("process {pid} is not cordial-run ({}), so it was left alone", exe.display()));
    }
    // SAFETY: `kill` with a signal number is safe for any pid; the worst case
    // is ESRCH, the process having exited already.
    if unsafe { libc::kill(pid as libc::pid_t, sig) } != 0 {
        let e = std::io::Error::last_os_error();
        if e.raw_os_error() != Some(libc::ESRCH) {
            return Err(format!("could not signal process {pid}: {e}"));
        }
    }
    Ok(())
}

/// How often a new client's log is read. Once a second is far more often than
/// a five-second verdict needs, and the reading stops at the first decision,
/// which a healthy start reaches in a few seconds.
const POLL: Duration = Duration::from_secs(1);

/// How long a stopped client has to exit on `SIGTERM` before it is killed.
const KILL_AFTER: Duration = Duration::from_secs(5);

/// How finely the watching thread notices that the client has gone. Small
/// enough that a restart is not held up waiting for it to wake.
const TICK: Duration = Duration::from_millis(100);

/// A thread reading one client's log while `stacked play` waits on it.
///
/// Upstream's launcher runs the same [`Supervisor`] from a GLib timeout; the
/// command line has no main loop, and its main thread is in `waitpid`, so the
/// polling has a thread of its own. Only this thread signals the client and
/// only the main thread reaps it, so the exit status the crash report needs
/// is never lost to a second reaper.
pub struct Watchdog {
    exited: std::sync::Arc<std::sync::atomic::AtomicBool>,
    stopped: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Watchdog {
    /// Begin watching `pid`. `supervisor` must have been made before the spawn.
    pub fn start(pid: u32, mut supervisor: Supervisor) -> Self {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        let exited = Arc::new(AtomicBool::new(false));
        let stopped = Arc::new(AtomicBool::new(false));
        let (gone, told) = (exited.clone(), stopped.clone());
        let nap = move |d: Duration| {
            let t0 = Instant::now();
            while t0.elapsed() < d {
                if gone.load(Ordering::Acquire) {
                    return false;
                }
                std::thread::sleep(TICK);
            }
            !gone.load(Ordering::Acquire)
        };
        let thread = std::thread::Builder::new()
            .name("stacked-freeze-watch".into())
            .spawn(move || loop {
                if !nap(POLL) {
                    return;
                }
                if supervisor.timed_out() {
                    println!(
                        "stacked: no finalize in the client's log after {}s; not watching for a freeze (#92)",
                        WATCH_FOR.as_secs()
                    );
                    return;
                }
                match supervisor.poll() {
                    Verdict::Undecided => continue,
                    Verdict::Healthy => {
                        println!("stacked: the client got past its first finalize; not a startup freeze (#92)");
                        return;
                    }
                    Verdict::Frozen => {
                        println!(
                            "stacked: the client (pid {pid}) logged `Forcing finalize` with no render view \
                             released and no controller teardown for {}s: stuck starting (#92); stopping it",
                            FROZEN_AFTER.as_secs()
                        );
                        if let Err(e) = terminate(pid) {
                            println!("stacked: could not stop the stuck client: {e}");
                            return;
                        }
                        told.store(true, Ordering::Release);
                        if nap(KILL_AFTER) {
                            println!("stacked: the stuck client (pid {pid}) ignored SIGTERM; killing it");
                            if let Err(e) = kill(pid) {
                                println!("stacked: could not kill the stuck client: {e}");
                            }
                        }
                        return;
                    }
                }
            })
            .map_err(|e| println!("stacked: could not start the freeze watch (#92): {e}"))
            .ok();
        Watchdog {
            exited,
            stopped,
            thread,
        }
    }

    /// Call once the client has been reaped. Says whether it was this watch
    /// that stopped it, in which case its exit is a restart and not a crash.
    pub fn finish(mut self) -> bool {
        use std::sync::atomic::Ordering;
        self.exited.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            // The thread only panics on a bug in the code above; the verdict
            // it left behind is still the one to act on.
            let _ = t.join();
        }
        self.stopped.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests;
