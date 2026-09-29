//! `notify.send` — a desktop notification through the freedesktop portal.
//!
//! Cordial owns the D-Bus connection; the plugin sends a summary and a body
//! and nothing else. Going through `org.freedesktop.portal.Notification`
//! rather than talking to `org.freedesktop.Notifications` directly matters
//! for the Flatpak build specifically: portal interfaces are reachable from
//! inside the sandbox without any `--talk-name` entry, because the portal is
//! the door Flatpak already leaves open, whereas the notification daemon's
//! own bus name is not. That keeps this capability's cost in
//! `io.github.damnshabu.Stacked.yml` at zero rather than one more narrow-but-present
//! permission — see ADR-007 on keeping the manifest's entries few and
//! specific.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use zbus::blocking::Connection;
use zbus::zvariant::Value;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Send one notification. `summary` becomes the title and must be non-empty
/// — an empty notification is not a meaningful effect and almost always
/// means the caller forgot to fill in a field, which should surface as a
/// refusal rather than a blank toast the user has to puzzle over.
pub fn send(summary: &str, body: &str) -> Result<(), String> {
    if summary.trim().is_empty() {
        return Err("notify.send needs a non-empty summary".into());
    }
    let conn = Connection::session().map_err(|e| format!("could not reach the session bus: {e}"))?;

    // The portal identifies notifications by an id the *sender* picks, so it
    // can later be withdrawn or updated by the same id. Cordial does not
    // expose withdrawal to plugins (that would be a second capability this
    // task does not ask for), so the id only has to be unique per call.
    let id = format!("cordial-{}", NEXT_ID.fetch_add(1, Ordering::Relaxed));

    let mut notification: HashMap<&str, Value> = HashMap::new();
    notification.insert("title", Value::from(summary));
    notification.insert("body", Value::from(body));

    conn.call_method(
        Some("org.freedesktop.portal.Desktop"),
        "/org/freedesktop/portal/desktop",
        Some("org.freedesktop.portal.Notification"),
        "AddNotification",
        &(id.as_str(), notification),
    )
    .map_err(|e| format!("the notification portal refused the call: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whether there is a session bus at all in this environment. CI and
    /// some sandboxes have none, and a notification test has no clean way to
    /// assert the notification was actually seen — the honest thing this
    /// test can check is that the portal call round-trips without error, the
    /// same bar `flag_inspector.rs` sets for "deno is not installed".
    fn have_session_bus() -> bool {
        std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some()
    }

    #[test]
    fn an_empty_summary_is_refused_before_any_bus_call() {
        // Checked first so this assertion holds even in environments with no
        // session bus at all — it is a payload-shape refusal, not a D-Bus
        // outcome.
        assert!(send("", "body").is_err());
        assert!(send("   ", "body").is_err());
    }

    /// `#[ignore]`, for the same reason as `urlopen`'s twin of this test.
    ///
    /// It posts a real notification to the real desktop. `cargo test
    /// --workspace` runs constantly here, so this put a banner on the
    /// developer's screen every time anyone — or any agent — ran the suite.
    /// Its sibling in `urlopen.rs` was opening a browser tab the same way and
    /// managed tens of them before it was caught.
    ///
    /// Run it deliberately when changing the portal call:
    ///
    /// ```text
    /// cargo test -p cordial-plugins -- --ignored notify
    /// ```
    #[test]
    #[ignore = "posts a real desktop notification; run with --ignored"]
    fn a_notification_round_trips_through_the_real_portal_when_one_is_reachable() {
        if !have_session_bus() {
            eprintln!("skipping: no DBUS_SESSION_BUS_ADDRESS in this environment");
            return;
        }
        match send("Cordial", "cargo test exercised notify.send") {
            Ok(()) => {}
            Err(e) => panic!("the portal call failed against a real session bus: {e}"),
        }
    }
}
