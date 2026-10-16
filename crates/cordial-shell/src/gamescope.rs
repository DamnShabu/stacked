//! Running inside gamescope.
//!
//! Upstream Cordial's ADR-058 has the measurements (headless gamescope 3.16.29;
//! not repeated here). The short version is that gamescope gives a client two
//! displays and **neither of them is the one the Wayland backend wants**:
//!
//! * Without `--expose-wayland` it sets `WAYLAND_DISPLAY` to the *empty string*,
//!   not to nothing. `android::backend()` asked whether the variable was set,
//!   found it was, chose Wayland, and `cordial-run` stopped at "no window:
//!   libadwaita would not initialise: Failed to initialize GTK" and exited 0.
//! * With `--expose-wayland` the variable names a real socket, and that
//!   compositor advertises no `wl_subcompositor`, which is how the engine's
//!   surface is embedded in the GTK toplevel (ADR-011). The client says "no
//!   window: compositor advertises no wl_subcompositor" and exits 0 again.
//!
//! Both end at the same answer: inside gamescope the game opens through the
//! Xwayland it provides, which `window.rs` already supports (ADR-024). The cost
//! is the one X11 always carries -- no in-app web views, so no sign-in from
//! inside the window -- and it is documented where the reader acts on it.
//!
//! `cage`, which the Roblox manager nests clients in, is a wlroots compositor
//! with a `wl_subcompositor` and sets no gamescope marker, so it is not
//! affected by any of this.
//!
//! This is in the library half, with the rule as a pure function over what it
//! was told: `stacked doctor` and the loader must not disagree about which
//! backend a session gets, and a rule that reads the process environment cannot
//! be tested without interleaving with every other test that does.

/// The variable gamescope exports to its child and nothing else does: the name
/// of gamescope's own Wayland socket, set whether or not `--expose-wayland` was
/// given.
pub const MARKER: &str = "GAMESCOPE_WAYLAND_DISPLAY";

/// Whether this process is a descendant of a gamescope session.
pub fn inside(marker: Option<&str>) -> bool {
    marker.is_some_and(|v| !v.is_empty())
}

/// Whether there is a Wayland compositor the game can embed into.
///
/// An empty `WAYLAND_DISPLAY` is no compositor, which is the whole of the
/// bug above. A gamescope socket is a compositor that cannot host the
/// subsurface, and is refused too -- but only when there is an X display to fall
/// back on, because a gamescope started with no Xwayland has nothing else and
/// the Wayland attempt at least says why it failed.
pub fn wayland_usable(
    wayland_display: Option<&str>,
    display: Option<&str>,
    marker: Option<&str>,
) -> bool {
    if !wayland_display.is_some_and(|v| !v.is_empty()) {
        return false;
    }
    !(inside(marker) && display.is_some_and(|v| !v.is_empty()))
}

/// [`wayland_usable`] for this process.
pub fn wayland_usable_here() -> bool {
    let get = |k: &str| std::env::var(k).ok();
    wayland_usable(
        get("WAYLAND_DISPLAY").as_deref(),
        get("DISPLAY").as_deref(),
        get(MARKER).as_deref(),
    )
}

/// [`inside`] for this process.
pub fn inside_here() -> bool {
    inside(std::env::var(MARKER).ok().as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_wayland_display_is_no_compositor() {
        // What gamescope exports without --expose-wayland, measured 2026-10-15.
        assert!(!wayland_usable(Some(""), Some(":2"), Some("gamescope-0")));
        // And the same for any parent that blanked it rather than removing it.
        assert!(!wayland_usable(Some(""), Some(":0"), None));
    }

    #[test]
    fn a_real_compositor_is_used() {
        assert!(wayland_usable(Some("wayland-0"), Some(":0"), None));
        assert!(wayland_usable(Some("wayland-1"), None, None));
    }

    #[test]
    fn gamescopes_own_wayland_socket_is_not_one_the_game_can_embed_in() {
        // --expose-wayland: WAYLAND_DISPLAY=gamescope-0, no wl_subcompositor.
        assert!(!wayland_usable(Some("gamescope-0"), Some(":2"), Some("gamescope-0")));
    }

    #[test]
    fn with_no_x_display_to_fall_back_on_wayland_is_still_tried() {
        // `gamescope --xwayland-count 0`: the Wayland attempt fails by name,
        // which is better than an X11 attempt that fails with no display at all.
        assert!(wayland_usable(Some("gamescope-0"), None, Some("gamescope-0")));
        assert!(wayland_usable(Some("gamescope-0"), Some(""), Some("gamescope-0")));
    }

    #[test]
    fn nothing_set_is_x11() {
        assert!(!wayland_usable(None, Some(":0"), None));
    }

    #[test]
    fn the_marker_must_name_something() {
        assert!(inside(Some("gamescope-0")));
        assert!(!inside(Some("")));
        assert!(!inside(None));
    }
}
