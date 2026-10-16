//! The late post-settings call waits for the engine's first render step (#92).
//!
//! On a launch with no usable flag cache the engine answers Cordial's bootstrap
//! with `onFlagsFailed`, waits for `nativePostClientSettingsLoadedInitialization3`
//! and only then fetches its flags and starts its Lua app. The startup freeze is
//! that call landing *before* the render job's first step
//! (`stepDataModelJob: No DM yet`) and the first app's teardown still being
//! under way when the step arrives; a call that lands after the step has never
//! frozen (docs/NEXT.md, ADR-065).
//!
//! Which side of the step the call falls on used to be an accident of the
//! clock: Home arrives at a different time every launch and the call is made a
//! fixed 250 ms after the handoff. This waits on the engine instead of on the
//! clock.
//!
//! **The marker is read from the engine's own log file, not from liblog.**
//! FLog lines reach `appData/logs/*_Player_*.log` and not `__android_log_print`:
//! of 395 lines of Cordial's stderr in a signed-in launch, none was a render
//! step, and the only engine tags that passed through liblog were
//! `rbx.JNIRobloxSettings`, `GameActivity` and three others. The file is the
//! channel `game_log` already follows, for the same reason it is not a hook:
//! the engine writes a file into a directory Cordial made and Cordial reads it.
//!
//! The gate runs on a thread of its own and never sleeps the pump: a pump held
//! for 330 ms or more stalls the window for good (docs/NEXT.md).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime};

/// Either line is the render job's first step. They carry the same millisecond
/// in every one of the 30 engine logs of 2026-10-16 that were read for this;
/// "No DM yet" is the earlier of the two by 40 to 100 microseconds.
pub const MARKERS: [&str; 2] = ["stepDataModelJob: No DM yet", "Bind workspace now"];

/// How long to wait for the step before posting anyway.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(3);

/// How long to look for the engine's log before giving up on reading one.
///
/// The engine has been logging for a quarter of a second by the time the gate
/// starts, so a log that is not there after this is not coming, and waiting out
/// the whole timeout for a marker nobody can see would only delay the post.
const FIND_LOG_FOR: Duration = Duration::from_millis(1000);

const TICK: Duration = Duration::from_millis(1);

/// How long to let the step get under way before the post follows it.
///
/// The marker is the *start* of the step: it is logged before the workspace is
/// bound, 40 to 100 microseconds ahead of "Bind workspace now". Every healthy
/// first-regime launch measured had the post 24 to 46 ms behind it, so this
/// reproduces that margin, 10 ms of it, rather than posting in the same
/// millisecond. `CORDIAL_POST_GATE_SETTLE_MS` overrides it (0 for none).
const SETTLE: Duration = Duration::from_millis(10);

/// What an unset `CORDIAL_POST_GATE` means. The variable is parsed by the loader's
/// `switch_on`, so `0`, `off`, `false` and `no` turn the gate off.
pub const DEFAULT_ON: bool = true;

static STARTED: OnceLock<SystemTime> = OnceLock::new();

/// Called once, first thing in `main`: the log of an earlier launch must never
/// be mistaken for this one's, and the only thing that tells them apart is that
/// this one's was not modified before the process began.
pub fn note_start() {
    let _ = STARTED.set(SystemTime::now());
}

/// Finds a marker in a stream of byte chunks, including one cut in two.
#[derive(Default)]
pub struct MarkerScan {
    carry: Vec<u8>,
}

impl MarkerScan {
    pub fn feed(&mut self, chunk: &[u8]) -> bool {
        self.carry.extend_from_slice(chunk);
        let hit = MARKERS.iter().any(|m| find(&self.carry, m.as_bytes()));
        if !hit {
            // Keep only what could be the front half of a marker.
            let keep = MARKERS.iter().map(|m| m.len()).max().unwrap_or(0).saturating_sub(1);
            if self.carry.len() > keep {
                let cut = self.carry.len() - keep;
                self.carry.drain(..cut);
            }
        }
        hit
    }
}

fn find(hay: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle)
}

/// One look at the engine's log.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Probe {
    /// The first render step is in the log.
    Seen,
    /// A log is being read and the step is not in it yet.
    NotYet,
    /// There is no log of this launch to read.
    NoLog,
}

/// How the wait ended.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Outcome {
    /// The step was in the log the first time it was looked at, so the post is
    /// already on the safe side of it.
    AlreadyPast,
    /// The step arrived after `waited`.
    Seen { waited: Duration },
    /// No step in `waited`; post anyway.
    TimedOut { waited: Duration },
    /// No log to read after `waited`; post anyway.
    NoLog { waited: Duration },
}

/// Wait for `probe` to say [`Probe::Seen`].
///
/// The first probe is made before any sleep, which is what makes "the step
/// came before the gate started" cost nothing. `nap` is how the caller sleeps,
/// so the logic can be exercised without a clock.
pub fn wait(
    mut probe: impl FnMut() -> Probe,
    timeout: Duration,
    find_log_for: Duration,
    mut nap: impl FnMut(),
) -> Outcome {
    let t0 = Instant::now();
    let mut first = true;
    loop {
        let p = probe();
        let waited = t0.elapsed();
        match p {
            Probe::Seen if first => return Outcome::AlreadyPast,
            Probe::Seen => return Outcome::Seen { waited },
            Probe::NoLog if waited >= find_log_for => return Outcome::NoLog { waited },
            _ if waited >= timeout => return Outcome::TimedOut { waited },
            _ => {}
        }
        first = false;
        nap();
    }
}

/// Reads this launch's engine log from its start, a chunk per probe.
pub struct LogProbe {
    dir: PathBuf,
    since: SystemTime,
    file: Option<std::fs::File>,
    scan: MarkerScan,
    last_scan: Option<Instant>,
}

impl LogProbe {
    pub fn new(dir: impl Into<PathBuf>, since: SystemTime) -> Self {
        Self { dir: dir.into(), since, file: None, scan: MarkerScan::default(), last_scan: None }
    }

    pub fn probe(&mut self) -> Probe {
        if self.file.is_none() {
            // A directory listing a millisecond is more than a log that
            // appears once needs.
            if self.last_scan.is_some_and(|t| t.elapsed() < Duration::from_millis(20)) {
                return Probe::NoLog;
            }
            self.last_scan = Some(Instant::now());
            match newest_since(&self.dir, self.since).and_then(|p| std::fs::File::open(p).ok()) {
                Some(f) => self.file = Some(f),
                None => return Probe::NoLog,
            }
        }
        let Some(file) = self.file.as_mut() else { return Probe::NoLog };
        let mut buf = [0u8; 64 * 1024];
        loop {
            match file.read(&mut buf) {
                Ok(0) => return Probe::NotYet,
                Ok(n) => {
                    if self.scan.feed(&buf[..n]) {
                        return Probe::Seen;
                    }
                }
                Err(_) => return Probe::NotYet,
            }
        }
    }
}

/// The most recently modified `*.log` in `dir` that was touched at or after `since`.
fn newest_since(dir: &Path, since: SystemTime) -> Option<PathBuf> {
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

/// Hold the calling thread until the engine's first render step, or until it is
/// clear it will not be seen. Prints one line saying which.
///
/// Call it from a thread that is not the pump.
pub fn hold(logs_dir: &Path) -> Outcome {
    let since = STARTED.get().copied().unwrap_or_else(|| SystemTime::now() - Duration::from_secs(120));
    let mut p = LogProbe::new(logs_dir, since);
    let outcome = wait(|| p.probe(), DEFAULT_TIMEOUT, FIND_LOG_FOR, || std::thread::sleep(TICK));
    println!("  post gate: {}", describe(outcome));
    if matches!(outcome, Outcome::Seen { .. }) {
        std::thread::sleep(settle());
    }
    outcome
}

fn settle() -> Duration {
    std::env::var("CORDIAL_POST_GATE_SETTLE_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map_or(SETTLE, Duration::from_millis)
}

pub fn describe(o: Outcome) -> String {
    match o {
        Outcome::AlreadyPast => "the first render step was already in the engine's log; posting".into(),
        Outcome::Seen { waited } => {
            format!("first render step seen after {} ms; posting", waited.as_millis())
        }
        Outcome::TimedOut { waited } => {
            format!("no render step in {} ms; posting anyway", waited.as_millis())
        }
        Outcome::NoLog { waited } => {
            format!("no engine log to read after {} ms; posting anyway", waited.as_millis())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn a_marker_cut_across_two_chunks_is_still_found() {
        let line = b"... [FLog::SurfaceController] SurfaceController::RenderJob::stepDataModelJob: No DM yet. Continue...";
        for cut in 1..line.len() {
            let mut s = MarkerScan::default();
            let a = s.feed(&line[..cut]);
            let b = s.feed(&line[cut..]);
            assert!(a || b, "cut at {cut} lost the marker");
        }
    }

    #[test]
    fn the_other_marker_works_too_and_noise_does_not() {
        let mut s = MarkerScan::default();
        assert!(!s.feed(b"onDataModelNotification: Received type(APP_READY, 10), data(Home)\n"));
        assert!(s.feed(b"scheduleRender: No data model. Bind workspace now...\n"));
    }

    #[test]
    fn a_step_already_in_the_log_posts_at_once_without_sleeping() {
        let naps = Cell::new(0);
        let out = wait(|| Probe::Seen, Duration::from_secs(3), Duration::from_secs(1), || naps.set(naps.get() + 1));
        assert_eq!(out, Outcome::AlreadyPast);
        assert_eq!(naps.get(), 0);
    }

    #[test]
    fn a_step_that_arrives_later_releases_the_post() {
        let n = Cell::new(0);
        let out = wait(
            || {
                n.set(n.get() + 1);
                if n.get() >= 4 { Probe::Seen } else { Probe::NotYet }
            },
            Duration::from_secs(3),
            Duration::from_secs(1),
            || {},
        );
        assert!(matches!(out, Outcome::Seen { .. }), "{out:?}");
        assert_eq!(n.get(), 4);
    }

    #[test]
    fn no_step_posts_anyway_after_the_timeout() {
        let out = wait(|| Probe::NotYet, Duration::from_millis(30), Duration::from_secs(1), || {
            std::thread::sleep(Duration::from_millis(2))
        });
        match out {
            Outcome::TimedOut { waited } => assert!(waited >= Duration::from_millis(30), "{waited:?}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn no_log_gives_up_early_rather_than_waiting_out_the_timeout() {
        let out = wait(|| Probe::NoLog, Duration::from_secs(60), Duration::from_millis(20), || {
            std::thread::sleep(Duration::from_millis(2))
        });
        assert!(matches!(out, Outcome::NoLog { .. }), "{out:?}");
    }

    #[test]
    fn the_probe_follows_a_real_file_and_ignores_an_older_one() {
        let dir = std::env::temp_dir().join(format!("cordial-post-gate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // An earlier launch's log, which holds the marker and must not count.
        let old = dir.join("1_Player_old_last.log");
        std::fs::write(&old, "x No DM yet stepDataModelJob: No DM yet. Continue\n").unwrap();
        std::thread::sleep(Duration::from_millis(30));
        let since = SystemTime::now();
        let mut p = LogProbe::new(&dir, since);
        assert_eq!(p.probe(), Probe::NoLog);

        std::thread::sleep(Duration::from_millis(30));
        let new = dir.join("2_Player_new_last.log");
        std::fs::write(&new, "Info [FLog::DataModelBindings] APP_READY Home\n").unwrap();
        std::thread::sleep(Duration::from_millis(25));
        assert_eq!(p.probe(), Probe::NotYet);

        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(&new).unwrap();
        f.write_all(b"[FLog::SurfaceController] SurfaceController::RenderJob::stepDataModelJob: No DM yet. Continue...\n")
            .unwrap();
        assert_eq!(p.probe(), Probe::Seen);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
