//! What state a window was left in, kept per profile.
//!
//! Two windows, two files, one shape: `<profile_dir>/window.json` for the
//! launcher and `<profile_dir>/game-window.json` for the one Roblox runs in.
//!
//! ## Why two files and not one
//!
//! A maximised launcher does not mean a maximised game window and never did.
//! They are different sizes for different jobs -- the launcher is a 540x340
//! chooser, the game window is whatever fills a monitor -- and one record would
//! make the last window closed decide how the next one opens. GNOME does not
//! share geometry between an application's windows either.
//!
//! ## What is remembered, and what GNOME says to remember
//!
//! Width, height, maximised, fullscreen. That is the whole of the set the HIG
//! names, and the *unmaximised* width and height at that: restoring a
//! maximised window means restoring the size it should return to when it is
//! unmaximised, not the size of the screen it was covering. GTK does that part
//! -- `gtk_window_get_default_size` reports the unmaximised size -- and the
//! save sites in `window.rs` decline to write at all while the window is
//! maximised or fullscreen, because both states report their own extent through
//! those same properties while a transition is in flight.
//!
//! ## Why per profile, not `shell.json`
//!
//! ADR-013 draws the line between the two config surfaces by what a setting
//! *is*, not by which directory it started in: appearance stays global because
//! it is application chrome, and everything that answers a question about one
//! account moves into the profile. A window preference argues for the profile
//! side of that line for the same reason the task that added this file was
//! given in the first place -- a fullscreen choice made while poking at a
//! throwaway profile must not silently follow the user onto the profile they
//! actually play on. That is exactly the grants-file argument ADR-013 makes at
//! length, applied to one more setting: an approval, or here a preference,
//! given in one profile answering a question about a different one.
//!
//! ## Placement, per ADR-013
//!
//! Same shape as `network.rs`'s own placement note: `window.json` lives beside
//! `flags.json`, `plugin-grants.json` and `network.json` inside the profile
//! directory rather than in `$XDG_CONFIG_HOME/cordial/shell.json`. There is no
//! legacy file to migrate -- this setting did not exist anywhere before this
//! change -- so absence simply means [`WindowState::default`], windowed at
//! whatever size `window.rs` already asked for before this file existed.
//!
//! ## Why there is no saved position
//!
//! `host_window.rs`'s own `smallest_monitor` doc already says it: "Wayland
//! does not let a client choose, or even learn in advance, which output its
//! toplevel will be mapped on -- that is the compositor's decision". The
//! corollary is the same for *placing* a mapped toplevel: there is no
//! `xdg_toplevel` request that moves a window to a coordinate, on any
//! compositor, by design -- ADR-011 is the decision that put Cordial on
//! Wayland at all, and ADR-011 is explicit that the X11-only capabilities it
//! gave up are not coming back. A `WindowState` with an `(x, y)` in it would be
//! two fields this file could write and nothing could ever read back, which is
//! worse than not having them: a settings file that appears to remember
//! something it cannot is the "stub that returns success" AGENTS.md rules out,
//! wearing a config file instead of a function. So this remembers size only.
//!
//! ## The escape hatch
//!
//! The risk named in this feature's task is real and specific: a launch that
//! comes up fullscreen with a broken renderer, or a display that no longer
//! exists, traps the user behind chrome they cannot get to. That risk is about
//! the *engine's* window -- black canvas, no frame, nothing to click -- and
//! the game window now has its own physical F11 handler before engine key
//! delivery. It does not depend on the launcher's accelerator group, so it is
//! available even when the engine canvas has focus or has failed to draw.
//!
//! So both windows restore this state. Size and maximised are applied before
//! the window is shown; a saved fullscreen on the game window is applied after
//! the engine's first present (see [`remember`]). The launcher's
//! `win.fullscreen` action and the game's direct F11 handler are installed
//! before their first frame, leaving no gap in which a restored fullscreen
//! window has no keyboard way out.
//!
//! The game and launcher remain separate records. Restoring either is now safe:
//! the launcher owns `win.fullscreen`, and the game owns the direct F11 path.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One profile's remembered window state. `#[serde(default)]` on the struct
/// and `Option` on the size fields mean a file written before a field existed,
/// or one with only `fullscreen` in it, still loads -- same shape as
/// `ShellConfig` and `NetworkConfig` in this crate.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct WindowState {
    pub fullscreen: bool,
    /// Maximised, which is a different question from fullscreen and has to be
    /// stored separately: a window can be neither, either, or -- as far as GTK
    /// is concerned -- both, and unfullscreening one that was maximised before
    /// must put it back to maximised rather than to its floating size.
    pub maximised: bool,
    /// The windowed size to restore -- meaningless while `fullscreen` is
    /// true, but kept regardless of it, because a window has to have *some*
    /// default size to fall back to the moment it leaves fullscreen. `None`
    /// means "nothing saved yet"; `window.rs` falls back to its own built-in
    /// default in that case, the same 540x340 it used before this file
    /// existed.
    pub width: Option<i32>,
    pub height: Option<i32>,
}

impl WindowState {
    /// Discard a saved size that could not have come from a real window.
    ///
    /// `host_window.rs`'s `fit_within` floors a *requested* size against the
    /// monitor by `min`-ing it, and `i32::min` of a negative number and a
    /// positive one is the negative number -- so an out-of-range value here
    /// would not be floored by that clamp, it would slip straight past it.
    /// Nothing this module writes can produce such a value; this guards
    /// against a file this process did not write, the same posture
    /// `profile::is_valid_name` takes on a name typed into a text field.
    fn sanitised(self) -> Self {
        const MAX_SIDE: i32 = 16384; // Comfortably past any real display; a
                                      // floor against nonsense, not a policy.
        Self {
            fullscreen: self.fullscreen,
            maximised: self.maximised,
            width: self.width.filter(|w| (1..=MAX_SIDE).contains(w)),
            height: self.height.filter(|h| (1..=MAX_SIDE).contains(h)),
        }
    }
}

/// Which window a record is about.
///
/// An enum rather than a filename passed by the caller, so that the two names
/// are decided once here and a typo cannot invent a third window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Which {
    /// The launcher: the chooser, the profile row and the Roblox button.
    Launcher,
    /// The window Roblox itself draws into (`android::wayland`).
    Game,
}

impl Which {
    fn file(self) -> &'static str {
        match self {
            Which::Launcher => "window.json",
            Which::Game => "game-window.json",
        }
    }
}

/// This profile's window-state file for `which`.
///
/// `CORDIAL_WINDOW_STATE` overrides it outright -- the same development-switch
/// shape as `CORDIAL_NETWORK`, `CORDIAL_FLAGS` and `CORDIAL_SHELL_CONFIG`, not
/// a supported per-profile arrangement, because it makes one file serve every
/// profile, which is the thing every one of those files exists to stop.
///
/// **The override applies to the launcher only.** Pointing both windows at one
/// path would make the override do the very thing two files exist to prevent,
/// and silently: a run under it would look like the game window inheriting the
/// launcher's geometry, which is a bug somebody would then go looking for in
/// this file.
pub fn path_in(profile_dir: &Path, which: Which) -> PathBuf {
    if which == Which::Launcher {
        if let Some(override_path) = std::env::var_os("CORDIAL_WINDOW_STATE") {
            return PathBuf::from(override_path);
        }
    }
    profile_dir.join(which.file())
}

/// Read a profile's window state, or the default if there is nothing to read.
///
/// Same default-on-anything-wrong shape as `shell_config::load` and
/// `network::load`: a missing file is the ordinary case -- every profile that
/// existed before this change has none -- and a malformed one is far more
/// likely to be an interrupted write than an attack, so both fall back to
/// [`WindowState::default`] rather than refusing to start. A launcher that
/// would not open because its own window-geometry file was corrupt would be a
/// far worse failure than the one this file exists to fix.
pub fn load(profile_dir: &Path, which: Which) -> WindowState {
    let path = path_in(profile_dir, which);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return WindowState::default();
    };
    match serde_json::from_str::<WindowState>(&text) {
        Ok(state) => state.sanitised(),
        Err(e) => {
            println!(
                "  window: {} is not usable ({e}); starting windowed at the default size",
                path.display()
            );
            WindowState::default()
        }
    }
}

/// Save a profile's window state.
///
/// `create_dir_all` rather than requiring the profile to already exist, the
/// same posture `network::save` takes: this is settings-file plumbing, not the
/// credential path `profile::acquire`'s `restrict_to_owner` exists for, and it
/// must not take that function's lock -- a fullscreen toggle on the launcher's
/// own window must work whether or not a client happens to be running against
/// the profile currently selected in the row above it, and `profile::acquire`
/// would refuse exactly when one is.
pub fn save(profile_dir: &Path, which: Which, state: &WindowState) -> std::io::Result<()> {
    std::fs::create_dir_all(profile_dir)?;
    let text = serde_json::to_string_pretty(state).expect("WindowState always serialises");
    std::fs::write(path_in(profile_dir, which), text)
}

/// Restore `state` onto `window` before it is shown, and keep the file up to
/// date for the rest of its life.
///
/// One call rather than the handful of handlers `window.rs` wires by hand, and
/// the difference between the two is worth naming: the launcher re-reads which
/// profile is selected on every save, because its profile row can change while
/// the window is open. A window that belongs to one running client cannot, so
/// this takes the directory once.
///
/// Returns whether the window was left fullscreen, without fullscreening it.
/// Entering fullscreen before the engine's first frame left clients stuck on
/// start (INFERRED: reported on Niri and in cage, not reproduced here), so the
/// caller defers it until something has been drawn.
pub fn remember(window: &libadwaita::Window, profile_dir: PathBuf, which: Which) -> bool {
    use libadwaita::glib;
    use libadwaita::prelude::*;

    let state = load(&profile_dir, which);
    if let (Some(w), Some(h)) = (state.width, state.height) {
        window.set_default_size(w, h);
    }
    // Before the window is presented, so it maps maximised rather than being
    // seen at its floating size for a frame and then snapping.
    if state.maximised {
        window.maximize();
    }
    // Fullscreen is not applied here. It is handed back for the caller to
    // apply once the engine has drawn: see `wayland::open`.

    let write = {
        let dir = profile_dir.clone();
        move |f: &dyn Fn(&mut WindowState)| {
            let mut state = load(&dir, which);
            f(&mut state);
            if let Err(e) = save(&dir, which, &state) {
                println!("  window: could not save {} state: {e}", path_in(&dir, which).display());
            }
        }
    };

    {
        let write = write.clone();
        window.connect_maximized_notify(move |w| {
            let on = w.is_maximized();
            write(&move |s| s.maximised = on);
        });
    }
    {
        let write = write.clone();
        window.connect_fullscreened_notify(move |w| {
            let on = w.is_fullscreen();
            write(&move |s| s.fullscreen = on);
        });
    }

    // Size, debounced, for the same reason `window.rs` debounces it: GTK fires
    // these on every step of an interactive drag and a write per step would be
    // tens of writes a second into a profile directory that has a live session
    // beside it.
    let pending: std::rc::Rc<std::cell::Cell<Option<glib::SourceId>>> =
        std::rc::Rc::new(std::cell::Cell::new(None));
    let schedule = {
        let window = window.clone();
        let pending = pending.clone();
        let write = write.clone();
        move || {
            if let Some(id) = pending.take() {
                id.remove();
            }
            let window = window.clone();
            let write = write.clone();
            let pending_inner = pending.clone();
            let id = glib::timeout_add_local_once(std::time::Duration::from_millis(400), move || {
                pending_inner.set(None);
                // The unmaximised size is the one worth keeping; both of these
                // states report their own extent through the same properties
                // while a transition is in flight.
                if window.is_fullscreen() || window.is_maximized() {
                    return;
                }
                let (w, h) = (window.default_width(), window.default_height());
                write(&move |s| {
                    s.width = Some(w);
                    s.height = Some(h);
                });
            });
            pending.set(Some(id));
        }
    };
    {
        let schedule = schedule.clone();
        window.connect_default_width_notify(move |_| schedule());
    }
    {
        let schedule = schedule.clone();
        window.connect_default_height_notify(move |_| schedule());
    }
    state.fullscreen
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("cordial-window-state-test-{tag}"));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn a_profile_with_no_file_starts_windowed_at_no_saved_size() {
        let dir = scratch("absent");
        let state = load(&dir, Which::Launcher);
        assert!(!state.fullscreen);
        assert_eq!(state.width, None);
        assert_eq!(state.height, None);
    }

    #[test]
    fn a_malformed_file_falls_back_to_windowed_rather_than_refusing_to_start() {
        let dir = scratch("malformed");
        std::fs::write(path_in(&dir, Which::Launcher), "{not json").unwrap();
        assert!(!load(&dir, Which::Launcher).fullscreen);
    }

    #[test]
    fn a_saved_fullscreen_choice_round_trips() {
        let dir = scratch("fullscreen-roundtrip");
        save(&dir, Which::Launcher, &WindowState { fullscreen: true, maximised: false, width: None, height: None }).unwrap();
        assert!(load(&dir, Which::Launcher).fullscreen);
    }

    #[test]
    fn a_saved_size_round_trips() {
        let dir = scratch("size-roundtrip");
        save(&dir, Which::Launcher, &WindowState { fullscreen: false, maximised: false, width: Some(1024), height: Some(768) }).unwrap();
        let back = load(&dir, Which::Launcher);
        assert_eq!(back.width, Some(1024));
        assert_eq!(back.height, Some(768));
    }

    #[test]
    fn a_config_written_before_the_size_fields_existed_still_loads() {
        // Everyone who saved a window.json before `width`/`height` existed has
        // one holding only `fullscreen`. `#[serde(default)]` is what makes
        // this true; worth a test rather than a note, the same reasoning
        // `shell_config.rs`'s own version of this test gives.
        let dir = scratch("older-schema");
        std::fs::write(path_in(&dir, Which::Launcher), r#"{"fullscreen":true}"#).unwrap();
        let state = load(&dir, Which::Launcher);
        assert!(state.fullscreen);
        assert_eq!(state.width, None);
    }

    #[test]
    fn an_out_of_range_size_is_discarded_rather_than_handed_to_gtk() {
        // The reason `sanitised` exists, spelled out: `fit_within` clamps a
        // requested size by `min`-ing it against the monitor, and `i32::min`
        // of a negative number and a positive one is the negative number --
        // so a tampered or truncated file could otherwise hand GTK a size
        // that clamp does not defend against at all.
        let dir = scratch("out-of-range");
        std::fs::write(path_in(&dir, Which::Launcher), r#"{"fullscreen":false,"width":-1,"height":0}"#).unwrap();
        let state = load(&dir, Which::Launcher);
        assert_eq!(state.width, None, "a negative width must not reach the window");
        assert_eq!(state.height, None, "a zero height must not reach the window");

        std::fs::write(path_in(&dir, Which::Launcher), r#"{"fullscreen":false,"width":99999999,"height":600}"#).unwrap();
        let state = load(&dir, Which::Launcher);
        assert_eq!(state.width, None, "an implausibly large width must not reach the window");
        assert_eq!(state.height, Some(600), "a plausible sibling field must survive the check");
    }

    #[test]
    fn one_profiles_window_state_is_not_anothers() {
        let root = scratch("isolation");
        let testing = root.join("testing");
        let main = root.join("main");
        std::fs::create_dir_all(&testing).unwrap();
        std::fs::create_dir_all(&main).unwrap();

        // The exact scenario the task this file answers named: fullscreen set
        // while poking at a throwaway profile must not follow onto the one
        // somebody actually plays on.
        save(&testing, Which::Launcher, &WindowState { fullscreen: true, maximised: false, width: Some(3440), height: Some(1440) })
            .unwrap();
        assert!(load(&testing, Which::Launcher).fullscreen);
        assert!(!load(&main, Which::Launcher).fullscreen, "a neighbouring profile must not inherit this");
        assert_eq!(load(&main, Which::Launcher).width, None);
    }

    #[test]
    fn the_launcher_and_the_game_window_do_not_share_a_record() {
        // The complaint this answers, in one assertion: a maximised launcher
        // must not decide how the window somebody plays in opens.
        let dir = scratch("two-windows");
        save(
            &dir,
            Which::Launcher,
            &WindowState { fullscreen: false, maximised: true, width: Some(540), height: Some(340) },
        )
        .unwrap();
        save(
            &dir,
            Which::Game,
            &WindowState { fullscreen: true, maximised: false, width: Some(1920), height: Some(1080) },
        )
        .unwrap();

        assert_ne!(path_in(&dir, Which::Launcher), path_in(&dir, Which::Game));
        let launcher = load(&dir, Which::Launcher);
        let game = load(&dir, Which::Game);
        assert!(launcher.maximised && !launcher.fullscreen);
        assert!(game.fullscreen && !game.maximised);
        assert_eq!(launcher.width, Some(540));
        assert_eq!(game.width, Some(1920));
    }

    #[test]
    fn a_maximised_choice_round_trips() {
        let dir = scratch("maximised-roundtrip");
        save(
            &dir,
            Which::Game,
            &WindowState { fullscreen: false, maximised: true, width: Some(800), height: Some(600) },
        )
        .unwrap();
        let back = load(&dir, Which::Game);
        assert!(back.maximised);
        assert_eq!(back.width, Some(800));
    }

    #[test]
    fn a_file_written_before_maximised_existed_still_loads() {
        // Every profile that has a `window.json` today has one without this
        // field. `#[serde(default)]` is what makes that keep working, and this
        // is the assertion that says so.
        let dir = scratch("older-schema-maximised");
        std::fs::write(path_in(&dir, Which::Launcher), r#"{"fullscreen":true,"width":900}"#).unwrap();
        let state = load(&dir, Which::Launcher);
        assert!(state.fullscreen);
        assert!(!state.maximised);
        assert_eq!(state.width, Some(900));
    }

    // No test sets `CORDIAL_WINDOW_STATE` itself: it is process-wide, cargo
    // runs this file's tests in parallel threads of one process, and every
    // other test above calls `load`/`save` without expecting a redirect --
    // `network.rs` makes the same choice for `CORDIAL_NETWORK`, for the same
    // reason. `profile.rs`'s own tests need a shared mutex to test their
    // process-wide override safely at all; this file has nothing that override
    // is worth that machinery for.
}
