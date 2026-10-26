use super::*;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

fn scratch(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("cordial-secrets-test-{tag}"));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// What the probe says, for a test that must not depend on the machine it
/// runs on.
fn present() -> Result<(), String> {
    Ok(())
}

fn locked() -> Result<(), String> {
    Err("the desktop keyring is locked".to_string())
}

#[test]
fn the_setting_decides_and_says_which() {
    // The three answers a user can give, and the one they can give by
    // mistake. `keyring` is the only one that is allowed to end in nothing
    // being saved, and even that says so rather than failing.
    assert_eq!(
        decide("", Path::new("/profiles/default/cookies"), present).0,
        Store::Keyring
    );
    assert_eq!(
        decide("auto", Path::new("/profiles/default/cookies"), present).0,
        Store::Keyring
    );
    // And `file` never asks the bus at all: a user who has said they do not
    // want a keyring should not pay a round trip to be told so.
    assert_eq!(
        decide(
            "file",
            Path::new("/profiles/default/cookies"),
            || unreachable!("the file store must not touch the bus")
        )
        .0,
        Store::File
    );
    assert!(
        decide("file", Path::new("/profiles/default/cookies"), present)
            .1
            .contains("plaintext")
    );
    assert_eq!(
        decide("nonsense", Path::new("/profiles/default/cookies"), present).0,
        Store::File
    );
    assert!(
        decide("nonsense", Path::new("/profiles/default/cookies"), present)
            .1
            .contains("not one of")
    );
}

#[test]
fn a_locked_keyring_is_the_ordinary_case_and_never_an_error() {
    // **This is the branch that matters most and the one that cannot be
    // measured here.** With auto-login the password that would unlock the
    // login keyring is never typed, so `Locked = true` is the normal state
    // on those machines rather than an edge case — and the developer
    // machine this was written on has both collections unlocked, which was
    // read off the bus and could not be changed to find out without locking
    // somebody's real keyring. So the *consequence* of a locked keyring is
    // pinned here instead: it degrades, it warns, and under no setting does
    // it become a failure or a prompt.
    let (store, line) = decide("auto", Path::new("/profiles/default/cookies"), locked);
    assert_eq!(
        store,
        Store::File,
        "a locked keyring must not cost the user their session"
    );
    assert!(line.contains("locked"), "and must say why: {line}");
    assert!(
        line.contains("0600"),
        "and where the session went instead: {line}"
    );

    // Only somebody who explicitly asked for keyring-or-nothing gets
    // nothing, and even they are told rather than left to notice.
    let (store, line) = decide("keyring", Path::new("/profiles/default/cookies"), locked);
    assert_eq!(store, Store::None);
    assert!(line.contains("will not be saved"), "{line}");
}

#[test]
fn nothing_here_can_stop_a_client_starting() {
    // The rule the whole module is built to, as an assertion: every
    // combination of setting and machine resolves to a store, and no
    // combination resolves to an error a caller could propagate.
    for requested in ["", "auto", "keyring", "file", "nonsense"] {
        for probe in [present as fn() -> Result<(), String>, locked] {
            let (_store, line) = decide(requested, Path::new("/profiles/default/cookies"), probe);
            assert!(!line.is_empty(), "every outcome is announced");
        }
    }
}

#[test]
fn a_stored_body_is_not_readable_by_other_users() {
    // The profile directory is already 0700, so this is the second lock on
    // the same door. It is worth having because a profile directory can be
    // copied, archived or synced by something that does not preserve the
    // directory's mode, and the file's own mode travels with it.
    let dir = scratch("perms");
    save(Store::File, &dir, Kind::Cookies, "a=b\n").unwrap();
    let mode = std::fs::metadata(dir.join("cookies"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(
        mode & 0o177,
        0,
        "a session must not be readable by anyone else"
    );
}

#[test]
fn an_interrupted_write_cannot_leave_half_a_session() {
    // Temp file plus rename, observed rather than assumed: after a save the
    // temp name must not exist, so nothing can later be mistaken for a
    // store.
    let dir = scratch("atomic");
    save(Store::File, &dir, Kind::Cookies, "a=1\n").unwrap();
    save(Store::File, &dir, Kind::Cookies, "a=2\n").unwrap();
    assert!(
        !dir.join("cookies.new").exists(),
        "the temp file must not survive"
    );
    assert_eq!(
        load(Store::File, &dir, Kind::Cookies).as_deref(),
        Some("a=2\n")
    );
}

#[test]
fn nothing_saved_is_not_a_failure() {
    // Every absence has to read as an ordinary signed-out launch. Turning
    // "you were logged out" into "the client would not start" is strictly
    // worse, and is the rule this module is built to.
    let dir = scratch("absent");
    assert!(load(Store::File, &dir, Kind::Cookies).is_none());
    assert!(load(Store::None, &dir, Kind::Cookies).is_none());
    assert!(save(Store::None, &dir, Kind::Cookies, "a=b").is_ok());
    assert!(erase(Store::File, &dir, Kind::Identity).is_ok());
}

#[test]
fn erasing_overwrites_before_unlinking() {
    // `remove_file` unlinks and does not erase. The point of the shred is
    // that the bytes are gone from the blocks as well as from the
    // directory entry; what can be asserted from here is the part that is
    // deterministic — the file is written over its whole length and then
    // removed, and no `.new` temp is left holding a copy.
    let dir = scratch("shred");
    save(Store::File, &dir, Kind::Cookies, ".ROBLOSECURITY=fake\n").unwrap();
    erase(Store::File, &dir, Kind::Cookies).unwrap();
    assert!(!dir.join("cookies").exists());
    assert!(!dir.join("cookies.new").exists());
}

#[test]
fn two_profiles_cannot_read_each_others_session() {
    // The attribute set is the whole of the isolation between profiles, and
    // between an agent's scratch profile and the one somebody plays on.
    // Both are called `default`; only the path tells them apart.
    let a = attributes(
        Path::new("/home/someone/.local/share/cordial/profiles/default"),
        Kind::Cookies,
    );
    let b = attributes(
        Path::new("/home/someone/.cache/scratch/cordial/profiles/default"),
        Kind::Cookies,
    );
    assert_ne!(
        a, b,
        "two roots with the same profile name must not share an item"
    );
    let cookies = attributes(Path::new("/p/default"), Kind::Cookies);
    let identity = attributes(Path::new("/p/default"), Kind::Identity);
    assert_ne!(cookies, identity, "the two stores must be separate items");
}

struct KeyringCleanup<'a> {
    dir: &'a Path,
    kind: Kind,
}

impl Drop for KeyringCleanup<'_> {
    fn drop(&mut self) {
        let _ = erase(Store::Keyring, self.dir, self.kind);
    }
}

/// The live tests take turns with the real service.
///
/// Every call goes through one worker thread, and the first one also pays for
/// connecting to D-Bus. Run side by side on a loaded machine, one test's
/// probe could queue behind the other's calls past its two-second timeout.
/// That marks the service wedged for the rest of the process, so the other
/// test's migration was refused and its assertion failed: a flake that looked
/// like a lost session. Seen twice in full workspace runs, each taking about
/// 3 s against the usual 0.4 s.
fn live_service() -> std::sync::MutexGuard<'static, ()> {
    static TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());
    TURN.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A timeout part-way through is the machine being slow, not a wrong
/// answer, and is reported as a skip rather than passed off as either.
fn service_timed_out() -> bool {
    let wedged = keyring::wedged();
    if wedged {
        println!("skipped: the secret service stopped answering part-way through");
    }
    wedged
}

/// The real thing, against the real service, skipped rather than failed
/// where there is none.
///
/// A skip is printed rather than passed silently, because a test that
/// quietly does nothing on the machine that matters is how this project
/// ends up believing something it never measured.
#[test]
fn a_session_survives_the_round_trip_through_the_service() {
    let _turn = live_service();
    let scratch = tempfile::tempdir().unwrap();
    let dir = scratch.path();
    if let Err(why) = usable() {
        println!("skipped: {why}");
        return;
    }
    let _cleanup = KeyringCleanup {
        dir,
        kind: Kind::Cookies,
    };
    // Obviously fake, and short. No test in this repository holds a real
    // token, at any verbosity.
    let body = "# cordial test\nroblox.com\tCORDIALTEST=not-a-session\n";
    let saved = save(Store::Keyring, dir, Kind::Cookies, body);
    let back = load(Store::Keyring, dir, Kind::Cookies);
    let seen = snapshot(SnapshotRequest {
        store: Store::Keyring,
        profile_dir: dir,
        kind: Kind::Cookies,
        max_bytes: 1024 * 1024,
    });
    let erased = erase(Store::Keyring, dir, Kind::Cookies);
    let gone = load(Store::Keyring, dir, Kind::Cookies);
    if service_timed_out() {
        return;
    }
    saved.unwrap();
    assert_eq!(
        back.as_deref(),
        Some(body),
        "what was stored must come back byte for byte"
    );
    assert_eq!(
        seen.as_deref(),
        Some(body.as_bytes()),
        "the launcher's read-only path must decode the same service value"
    );
    erased.unwrap();
    assert!(
        gone.is_none(),
        "and a removed item must not linger in somebody's keyring"
    );
}

#[test]
fn a_plaintext_store_is_adopted_and_destroyed() {
    // The migration, end to end: the owner has a live session in a file
    // right now, and the launch after this change has to take it in
    // *without* signing them out. The body is returned as well as stored,
    // which is the half that is easy to leave out and expensive to notice.
    let _turn = live_service();
    let scratch = tempfile::tempdir().unwrap();
    let dir = scratch.path();
    if let Err(why) = usable() {
        println!("skipped: {why}");
        return;
    }
    let _cleanup = KeyringCleanup {
        dir,
        kind: Kind::Cookies,
    };
    let body = "# cordial test\nroblox.com\tCORDIALTEST=adopt-me\n";
    save(Store::File, dir, Kind::Cookies, body).unwrap();
    let migrated = load(Store::Keyring, dir, Kind::Cookies);
    let file_left = dir.join("cookies").exists();
    let next = load(Store::Keyring, dir, Kind::Cookies);
    if service_timed_out() {
        return;
    }
    assert_eq!(
        migrated.as_deref(),
        Some(body),
        "the migrating launch must still be signed in"
    );
    assert!(!file_left, "and the plaintext file must be gone");
    assert_eq!(
        next.as_deref(),
        Some(body),
        "the next launch must read it from the service"
    );
    erase(Store::Keyring, dir, Kind::Cookies).unwrap();
}
