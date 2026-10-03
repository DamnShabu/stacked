//! Hiding the window while the game keeps running, and showing it again.
//!
//! **Asked for by a launcher, not a person at the window.** A launcher running
//! several clients wants the ones nobody is watching out of the way -- off the
//! screen, off the taskbar -- without ending their sessions. `SIGUSR1` hides
//! the window and `SIGUSR2` shows it; both are idempotent, so a launcher that
//! lost track can always say what it wants rather than toggle and hope.
//!
//! Not the compositor's minimise. A client may only *ask* to be minimised
//! (`xdg_toplevel.set_minimized`), niri ignores the request outright, and
//! restoring is the compositor's call, not ours. Unmapping our own toplevel
//! works on every compositor and on X11, and it is ours to undo.
//!
//! On Wayland GTK owns the toplevel, and `gtk_widget_hide` on it keeps the
//! `wl_surface` -- GDK destroys only the xdg role and attaches a null buffer
//! (`gdk_wayland_surface_hide_surface`, GTK 4.22). That matters twice: the
//! engine's canvas is a subsurface of that `wl_surface`, so it is unmapped with
//! its parent and comes back with it; and `wayland::window_closed` reads the
//! surface going away as the window closing, which a hide therefore is not.
//!
//! While hidden, presents are paced to [`HIDDEN_FPS`]. Nothing is drawn to a
//! screen, but the engine would otherwise go on rendering at full rate for a
//! window nobody can see. The visibility the keepalive gate reads also says
//! "not visible", so the engine's own idle throttle is free to go lower still.
//!
//! The state is published as `<profile>/window-state`, `shown` or `hidden`,
//! for as long as the pump runs. A launcher started after a hide can tell which
//! windows it has to show again, and -- because `SIGUSR1`'s default action is
//! to terminate -- a launcher signals only a client that has the file: one
//! from a build before this would be killed outright instead of hidden.

use std::ffi::c_int;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::time::{Duration, Instant};

/// The present rate while hidden. Low enough to cost next to nothing, high
/// enough that the engine's per-frame work (scripts on `RenderStepped`, the
/// network tick it schedules alongside) keeps moving.
pub const HIDDEN_FPS: u64 = 10;

const SIGUSR1: c_int = 10;
const SIGUSR2: c_int = 12;

const NO_REQUEST: u8 = 0;
const HIDE: u8 = 1;
const SHOW: u8 = 2;

/// What the last signal asked for, until the pump acts on it. The only thing
/// the handler touches.
static REQUESTED: AtomicU8 = AtomicU8::new(NO_REQUEST);

/// Whether the window is hidden now, for the render thread's present pacing.
static HIDDEN: AtomicBool = AtomicBool::new(false);

/// When the last paced present went out, in nanoseconds since [`epoch`].
static LAST_PRESENT_NS: AtomicU64 = AtomicU64::new(0);

extern "C" {
    fn signal(signum: c_int, handler: usize) -> usize;
}

extern "C" fn on_hide(_sig: c_int) {
    REQUESTED.store(HIDE, Ordering::Relaxed);
}

extern "C" fn on_show(_sig: c_int) {
    REQUESTED.store(SHOW, Ordering::Relaxed);
}

/// Take `SIGUSR1`/`SIGUSR2`, then say so with a `shown` state -- in that
/// order, so no launcher signals before the handlers are in.
///
/// Installed with the terminating handlers, after engine bring-up, for the
/// same reason they are: whatever the engine installed for these comes first
/// and is replaced here.
pub fn install() {
    // SAFETY: plain `extern "C" fn(c_int)` handlers that store one atomic.
    unsafe {
        signal(SIGUSR1, on_hide as *const () as usize);
        signal(SIGUSR2, on_show as *const () as usize);
    }
    publish(false);
}

/// Act on a pending hide or show. Called from the pump, on the thread that
/// owns the window.
pub fn apply() {
    let wanted = match REQUESTED.swap(NO_REQUEST, Ordering::Relaxed) {
        HIDE => true,
        SHOW => false,
        _ => return,
    };
    if HIDDEN.load(Ordering::Relaxed) == wanted {
        return;
    }
    super::backend_set_hidden(wanted);
    HIDDEN.store(wanted, Ordering::Relaxed);
    publish(wanted);
    println!("[android] window {}", if wanted { "hidden" } else { "shown" });
}

/// Whether the window is hidden by request.
pub fn is_hidden() -> bool {
    HIDDEN.load(Ordering::Relaxed)
}

/// Hold a present back until [`HIDDEN_FPS`] allows it, while hidden. Runs on
/// the engine's render thread, inside `vkQueuePresentKHR`; a shown window
/// pays one relaxed load.
pub fn pace_present() {
    if !HIDDEN.load(Ordering::Relaxed) {
        return;
    }
    let interval = 1_000_000_000 / HIDDEN_FPS;
    let now = epoch().elapsed().as_nanos() as u64;
    let next = LAST_PRESENT_NS.load(Ordering::Relaxed).saturating_add(interval);
    if now < next {
        std::thread::sleep(Duration::from_nanos(next - now));
    }
    LAST_PRESENT_NS.store(epoch().elapsed().as_nanos() as u64, Ordering::Relaxed);
}

/// Remove the state on the way out: from here on the handlers are of no use,
/// and a launcher must not read the client as one it can hide.
pub fn clear_state() {
    let path = state_file();
    match std::fs::remove_file(&path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            eprintln!("[android] could not remove {}: {e}", path.display());
        }
        _ => {}
    }
}

fn publish(hidden: bool) {
    let path = state_file();
    let state: &[u8] = if hidden { b"hidden\n" } else { b"shown\n" };
    if let Err(e) = std::fs::write(&path, state) {
        eprintln!("[android] could not write {}: {e}", path.display());
    }
}

fn state_file() -> std::path::PathBuf {
    crate::profile::active().join("window-state")
}

fn epoch() -> Instant {
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presents_are_paced_only_while_hidden() {
        // The two halves share the statics, so one test drives both in order.
        let t = Instant::now();
        for _ in 0..5 {
            pace_present();
        }
        assert!(t.elapsed() < Duration::from_millis(50), "shown: no pacing");

        HIDDEN.store(true, Ordering::Relaxed);
        let t = Instant::now();
        for _ in 0..4 {
            pace_present();
        }
        HIDDEN.store(false, Ordering::Relaxed);
        // The first may go at once; the next three wait an interval each.
        let floor = Duration::from_nanos(3 * 1_000_000_000 / HIDDEN_FPS);
        assert!(t.elapsed() >= floor, "hidden: {:?} < {floor:?}", t.elapsed());
    }
}
