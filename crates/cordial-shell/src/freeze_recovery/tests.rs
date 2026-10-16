use super::*;

const FROZEN: &str = include_str!("fixtures/frozen.log");
const HEALTHY: &str = include_str!("fixtures/healthy.log");

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

#[test]
fn a_log_that_has_not_reached_the_finalize_is_undecided() {
    let before: String = FROZEN
        .lines()
        .take_while(|l| !l.contains(FINALIZE))
        .map(|l| format!("{l}\n"))
        .collect();
    assert!(!before.is_empty());
    assert_eq!(scan(&before), Scan::Undecided);
    assert_eq!(scan(""), Scan::Undecided);
}

#[test]
fn the_frozen_shape_is_finalizing_and_only_becomes_frozen_after_five_seconds() {
    assert_eq!(scan(FROZEN), Scan::Finalizing);
    let mut w = Watch::default();
    assert_eq!(w.observe(Scan::Finalizing, secs(10)), Verdict::Undecided);
    assert_eq!(w.observe(Scan::Finalizing, Duration::from_millis(14_900)), Verdict::Undecided);
    assert_eq!(w.observe(Scan::Finalizing, secs(15)), Verdict::Frozen);
}

#[test]
fn the_healthy_shape_is_healthy_at_once() {
    assert_eq!(scan(HEALTHY), Scan::Healthy);
    let mut w = Watch::default();
    assert_eq!(w.observe(scan(HEALTHY), secs(0)), Verdict::Healthy);
}

#[test]
fn a_finalize_that_completes_late_is_healthy_if_it_completes_before_the_deadline() {
    let mut w = Watch::default();
    assert_eq!(w.observe(Scan::Finalizing, secs(1)), Verdict::Undecided);
    let finished = format!("{FROZEN}2026-08-23T10:38:19.000Z,2.0,e24eb6c0,6,Info [FLog::UgcExperienceController] UgcExperienceController::~UgcExperienceController()\n");
    assert_eq!(scan(&finished), Scan::Healthy);
    assert_eq!(w.observe(scan(&finished), secs(3)), Verdict::Healthy);
}

#[test]
fn a_destructor_without_a_released_render_view_still_counts_as_finished() {
    let log = "x [FLog::SingleSurfaceApp] Forcing finalize experience coordinator with state 1\n\
               x [FLog::UgcExperienceController] UgcExperienceController::~UgcExperienceController()\n";
    assert_eq!(scan(log), Scan::Healthy);
}

#[test]
fn a_later_finalize_cannot_rescue_or_condemn_the_first() {
    // A frozen first finalize followed by a second one with a render view
    // released in between: the first decides, because a client that froze never
    // reaches a second, and a log that says otherwise is not the shape measured.
    let log = format!(
        "{FROZEN}x [FLog::Graphics] RenderView destroyed[1]\nx [FLog::SingleSurfaceApp] Forcing finalize experience coordinator with state 1\n"
    );
    assert_eq!(scan(&log), Scan::Finalizing);
    // And the converse: a healthy start whose session later finalises again
    // with no render view stays healthy.
    let later = format!("{HEALTHY}x [FLog::SingleSurfaceApp] Forcing finalize experience coordinator with state 1\n");
    assert_eq!(scan(&later), Scan::Healthy);
}

#[test]
fn a_verdict_once_reached_is_kept() {
    let mut w = Watch::default();
    w.observe(Scan::Finalizing, secs(0));
    assert_eq!(w.observe(Scan::Finalizing, secs(5)), Verdict::Frozen);
    // The destructor line that arrives while the stopped client dies must not
    // turn a client already being stopped back into a healthy one.
    assert_eq!(w.observe(Scan::Healthy, secs(6)), Verdict::Frozen);
}

#[test]
fn a_press_of_play_gets_two_restarts_and_then_the_message() {
    assert_eq!(after_freeze(0), Next::Restart { attempt: 2, of: 3 });
    assert_eq!(after_freeze(1), Next::Restart { attempt: 3, of: 3 });
    assert_eq!(after_freeze(2), Next::GiveUp);
    assert_eq!(after_freeze(7), Next::GiveUp);
}

#[test]
fn the_status_line_is_one_plain_sentence_with_the_attempt_in_it() {
    assert_eq!(status_line(2, 3), "Roblox got stuck starting. Restarting it (2 of 3).");
}

#[test]
fn the_give_up_message_points_at_the_issue_and_names_the_off_switch() {
    let body = give_up_body();
    assert!(body.contains("issue 92"));
    assert!(body.contains("CORDIAL_NO_FREEZE_RESTART=1"));
    assert!(body.contains("3 times"));
}

#[test]
fn only_an_explicit_value_turns_the_recovery_off() {
    assert!(enabled(None));
    assert!(enabled(Some("")));
    assert!(enabled(Some("0")));
    assert!(!enabled(Some("1")));
    assert!(!enabled(Some("true")));
}

#[test]
fn a_relaunch_does_not_read_the_log_of_the_client_it_stopped() {
    let dir = std::env::temp_dir().join(format!("cordial-freeze-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let old = dir.join("old_last.log");
    std::fs::write(&old, FROZEN).unwrap();
    std::fs::write(dir.join("notes.txt"), "not a log").unwrap();

    // Everything in the directory predates the launch.
    let after = SystemTime::now() + Duration::from_secs(60);
    assert_eq!(newest_log_since(&dir, after), None);
    // And before the launch the old one is found, which is why `since` exists.
    assert_eq!(newest_log_since(&dir, SystemTime::UNIX_EPOCH), Some(old));
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Not a test of the code but of the claim: runs the classifier over every
/// engine log under the colon-separated directories in `CORDIAL_FREEZE_CORPUS`
/// and prints how many land where. `cargo test -p cordial-shell --lib
/// freeze_corpus -- --ignored --nocapture`.
#[test]
#[ignore = "reads logs from this machine, which CI does not have"]
fn freeze_corpus() {
    let Ok(roots) = std::env::var("CORDIAL_FREEZE_CORPUS") else { return };
    let mut seen = std::collections::HashSet::new();
    let (mut healthy, mut frozen, mut undecided, mut signed_out_frozen, mut disagree) = (0, 0, 0, 0, 0);
    let mut stack: Vec<PathBuf> = roots.split(':').map(PathBuf::from).collect();
    while let Some(p) = stack.pop() {
        if p.is_dir() {
            if let Ok(rd) = std::fs::read_dir(&p) {
                stack.extend(rd.flatten().map(|e| e.path()));
            }
            continue;
        }
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let engine_log = name.contains("Player_") && name.ends_with("last.log")
            || name == "engine_log_final.txt"
            || name == "engine_log_at_verdict.txt";
        if !engine_log {
            continue;
        }
        let Ok(bytes) = std::fs::read(&p) else { continue };
        let text = String::from_utf8_lossy(&bytes);
        // The same de-duplication the capture's own script used: a log copied
        // into a results directory is one run, not two.
        let key: String = text.chars().take(6000).collect();
        if !seen.insert(key) {
            continue;
        }
        let signed_in = text
            .split("cachedUserId:")
            .nth(1)
            .is_some_and(|r| r.chars().next().is_some_and(|c| c.is_ascii_digit()));
        // The check that does not depend on the rule: a finalize that finished
        // logs the controller's destructor sooner or later, whatever came
        // before it, so the two must agree on every log.
        let verdict = scan(&text);
        if (verdict == Scan::Healthy) != text.contains(CONTROLLER_DESTROYED) && verdict != Scan::Undecided {
            disagree += 1;
        }
        match verdict {
            Scan::Undecided => undecided += 1,
            Scan::Healthy if signed_in => healthy += 1,
            Scan::Finalizing if signed_in => frozen += 1,
            Scan::Finalizing => signed_out_frozen += 1,
            Scan::Healthy => {}
        }
    }
    println!(
        "signed in: healthy {healthy}, frozen {frozen}; signed out stuck {signed_out_frozen}; \
         no finalize at all {undecided}; disagree with the destructor check {disagree}"
    );
    assert_eq!(disagree, 0);
}

/// A stand-in client: `sleep`, copied to a file named `cordial-run` so that
/// [`signal`]'s check on `/proc/<pid>/exe` lets the watch act on it.
fn fake_client(dir: &Path) -> std::process::Child {
    let exe = dir.join("cordial-run");
    std::fs::copy("/bin/sleep", &exe).unwrap();
    std::process::Command::new(&exe).arg("60").spawn().unwrap()
}

/// The command line's watch, end to end against a real child: a healthy log
/// leaves it running and says so, and the frozen shape gets it stopped, which
/// `finish` reports so `stacked play` restarts it rather than calling it a crash.
#[test]
fn the_watch_stops_a_frozen_client_and_leaves_a_healthy_one() {
    use std::os::unix::process::ExitStatusExt;
    let dir = std::env::temp_dir().join(format!("cordial-freeze-watch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    // Control: healthy. The watch gives up on its first decision.
    let logs = dir.join("healthy");
    std::fs::create_dir_all(&logs).unwrap();
    let supervisor = Supervisor::new(logs.clone());
    let mut child = fake_client(&dir);
    std::fs::write(logs.join("a_last.log"), HEALTHY).unwrap();
    let watch = Watchdog::start(child.id(), supervisor);
    std::thread::sleep(Duration::from_millis(2500));
    assert!(child.try_wait().unwrap().is_none(), "a healthy client must be left running");
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(!watch.finish(), "the watch did not stop the healthy client");

    // Frozen: stopped by SIGTERM once the finalize has gone five seconds
    // without its destructor.
    let logs = dir.join("frozen");
    std::fs::create_dir_all(&logs).unwrap();
    let supervisor = Supervisor::new(logs.clone());
    let mut child = fake_client(&dir);
    std::fs::write(logs.join("b_last.log"), FROZEN).unwrap();
    let watch = Watchdog::start(child.id(), supervisor);
    let status = child.wait().unwrap();
    assert_eq!(status.signal(), Some(libc::SIGTERM), "{status:?}");
    assert!(watch.finish(), "the watch must report that it stopped the client");
    std::fs::remove_dir_all(&dir).unwrap();
}
