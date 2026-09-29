//! Turning GDK's view of the monitors into a list shaped exactly like
//! `cordial_runtime::refresh::Output`, and keeping that list current as the
//! desktop changes underneath it.
//!
//! **This module cannot import `cordial_runtime::refresh`, and that is not a
//! style choice.** `cordial-runtime` already depends on `cordial-shell` for
//! `host_window` (see `lib.rs` and ADR-011), so the reverse edge this task's
//! brief describes -- `use cordial_runtime::refresh::{Output, ...}` from
//! here -- is a Cargo dependency cycle, not merely an unwanted one:
//!
//! ```text
//! error: cyclic package dependency: package `cordial-runtime` depends on itself.
//! ```
//!
//! Measured by actually adding the dependency line and running `cargo check
//! -p cordial-shell`, which is what printed the above, rather than assumed
//! from reading `Cargo.toml`. `profile.rs`'s header already says the same
//! thing about why *that* module cannot depend on `cordial-runtime` either --
//! this is the same wall, hit from a different room.
//!
//! So [`Output`] here is a second, identically-shaped type, not the one
//! `refresh.rs` defines, and this file does not call `supported_from`,
//! `current_for`, `hz_from_millihertz` or `worth_announcing` -- there is no
//! path from this crate to the functions that decide those questions, and
//! AGENTS.md's rule against reimplementing already-decided logic is the
//! reason this file does not attempt a second copy of what they compute,
//! rather than papering over the wall with one. What crosses the wall is
//! data: a `Vec<Output>` whose fields are named and typed to match, so that
//! whoever wires [`watch`]'s callback into `cordial-runtime` -- which *can*
//! reach `refresh.rs`, being on the correct side of the edge -- can turn each
//! one into the real type with a field-for-field copy and then hand the list
//! straight to `supported_from`/`current_for`, no translation logic in
//! between beyond that copy.
//!
//! ## Why two listeners and not one
//!
//! A rate the engine should be told about can change for two different
//! reasons, and GDK reports them through two different objects.
//!
//! **A monitor appears or disappears.** That is the `GListModel` behind
//! [`gdk::Display::monitors`] and its `items-changed` signal -- plugging in a
//! second screen, or unplugging one, changes what the list contains even if
//! the window never moves.
//!
//! **The window ends up on a different monitor.** Nothing about the monitor
//! list changes when that happens; what changes is which one
//! `gdk::Display::monitor_at_surface` names for this window's surface. GDK
//! does not hand that out as its own signal -- there is no `notify::monitor`
//! -- so this listens broadly, to every `notify::` the surface fires, and
//! relies on the recheck being cheap and idempotent rather than trying to
//! guess in advance which property name means "you moved". A caller with
//! access to `refresh::worth_announcing` is where the resulting noise
//! (scale-factor and geometry notifications that leave the rate untouched)
//! should be filtered -- see the note on [`watch`].
//!
//! ## What this does not do
//!
//! It does not call the engine. `pass_current_refresh_rate` and
//! `pass_supported_refresh_rates` live behind a library handle this crate
//! does not hold, in `crates/cordial-runtime/src/bin/load.rs`. [`watch`]
//! takes a callback instead, so wiring those calls in -- and running the
//! result through `refresh.rs`'s real decision functions, which `load.rs` can
//! reach and this file cannot -- is meant to be the one thing left to do.
//!
//! ## What is and is not exercised
//!
//! This developer's machine has one monitor. The hotplug branch and the
//! window-crosses-a-boundary branch are therefore **INFERRED**: they are
//! wired up the way GDK's own API says to wire them, and every line of Rust
//! in this file compiles and runs against a real `gdk::Display`, but nothing
//! here has been watched fire from an actual hotplug or an actual drag across
//! two outputs. The single-monitor path -- enumerate, compute `current`, log,
//! react to the list changing size at all -- has been, and the log line this
//! file's task report quotes is what that looked like.

use libadwaita::gtk;
use libadwaita::gtk::gdk;
use libadwaita::gtk::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

/// One output, shaped to match `cordial_runtime::refresh::Output` field for
/// field -- see this file's header for why it cannot be that type.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Output {
    /// Refresh in hertz. GDK reports millihertz; converted on the way in.
    pub hz: f32,
    /// Whether the window is mostly on this one, per
    /// `gdk::Display::monitor_at_surface`.
    pub current: bool,
}

/// Every monitor GDK currently lists, each carrying whether this window is
/// mostly on it.
///
/// Empty rather than an error when there is no default display or the window
/// has no surface yet -- both are ordinary before the window is mapped, and
/// `refresh.rs`'s own functions already treat an empty list as "nothing
/// plausible is known" rather than a fault.
pub fn outputs(window: &impl IsA<gtk::Window>) -> Vec<Output> {
    let window = window.as_ref();
    let Some(display) = gdk::Display::default() else { return Vec::new() };

    // `monitor_at_surface` is what ADR-011's refresh.rs calls out by name as
    // the answer to "mostly on", for the window straddling two outputs case
    // its own header describes. `window.surface()` is `None` before the
    // window is realised, which this treats the same as "no current
    // monitor" rather than as a fault.
    let current_monitor = window.surface().and_then(|s| display.monitor_at_surface(&s));

    let monitors = display.monitors();
    (0..monitors.n_items())
        .filter_map(|i| monitors.item(i))
        .filter_map(|obj| obj.downcast::<gdk::Monitor>().ok())
        .map(|m| {
            // GDK's own millihertz-to-hertz conversion is
            // `refresh::hz_from_millihertz`, which this cannot call -- see
            // this file's header. Its whole body is `mhz as f32 / 1000.0`;
            // reproducing that division here is not a second copy of a
            // *decision*, there is none to make, only a unit to convert on
            // the way into a `Vec<Output>` this file is allowed to build.
            let hz = m.refresh_rate() as f32 / 1000.0;
            // `gdk::Monitor` is a GObject wrapper; `PartialEq` on it compares
            // the underlying pointer, which is what "the same monitor"
            // actually means here rather than, say, equal geometry -- two
            // identical monitors side by side must not be confused for one.
            let current = current_monitor.as_ref() == Some(&m);
            Output { hz, current }
        })
        .collect()
}

/// What [`watch`] remembers between rechecks, so a `notify::` storm on the
/// surface does not recompute and log on every single property.
struct State {
    window: gtk::Window,
    /// The list last reported, compared whole so a recheck that changed
    /// nothing -- the common case, since GDK's `notify::` is not scoped to
    /// properties this file cares about -- neither logs nor calls back.
    /// `refresh::worth_announcing`'s hundredth-hertz tolerance and its
    /// "supported can shrink, current losing its answer is not news" rules
    /// are `cordial_runtime`'s to apply once this list reaches a caller that
    /// can reach them; this equality is only "did anything at all change",
    /// deliberately coarser and not a substitute for that comparison.
    last: Rc<Cell<Option<Vec<Output>>>>,
    on_change: Rc<dyn Fn(&[Output])>,
}

impl State {
    fn recheck(&self) {
        let outs = outputs(&self.window);
        let previous = self.last.take();
        let changed = previous.as_deref() != Some(outs.as_slice());
        if changed {
            log(&outs);
            (self.on_change)(&outs);
        }
        self.last.set(Some(outs));
    }
}

/// Log what was just observed, in the shell's own `"  shell: "` voice --
/// `main.rs` and `window.rs` both write it that way for anything worth seeing
/// in the terminal a launch was started from.
fn log(outputs: &[Output]) {
    println!("  shell: refresh -- {} monitor(s): {outputs:?}", outputs.len());
}

/// Start watching this window's monitors.
///
/// `on_change` is called with the fresh `Output` list whenever it differs
/// from what was last computed -- a hotplug that changes the monitor count, a
/// rate change, or the window landing on a different monitor. It also fires
/// once immediately with whatever is known right away (typically an empty
/// list, before the window is mapped), so a caller does not have to separately
/// seed itself with the starting state.
///
/// **This is not the point at which to decide whether the engine should be
/// told.** That needs `refresh::supported_from`, `refresh::current_for` and
/// `refresh::worth_announcing`, none of which this crate can reach -- see
/// this file's header. The intended shape, once wired into
/// `crates/cordial-runtime/src/bin/load.rs`, which sits on the correct side
/// of that edge:
///
/// ```ignore
/// let previous_current = Rc::new(Cell::new(None));
/// cordial_shell::refresh_watch::watch(&window, move |outs| {
///     let real: Vec<cordial_runtime::refresh::Output> = outs
///         .iter()
///         .map(|o| cordial_runtime::refresh::Output { hz: o.hz, current: o.current })
///         .collect();
///     let now = cordial_runtime::refresh::current_for(&real);
///     if cordial_runtime::refresh::worth_announcing(previous_current.get(), now) {
///         if let Some(hz) = now {
///             engine.pass_current_refresh_rate(hz);
///         }
///         engine.pass_supported_refresh_rates(&cordial_runtime::refresh::supported_from(&real));
///     }
///     previous_current.set(now);
/// });
/// ```
///
/// `cordial_shell::refresh_watch::watch` is in the library half, so
/// `cordial-runtime` can reach it; see `lib.rs`.
pub fn watch(window: &impl IsA<gtk::Window>, on_change: impl Fn(&[Output]) + 'static) {
    let window = window.as_ref().clone();
    let state = Rc::new(State {
        window: window.clone(),
        last: Rc::new(Cell::new(None)),
        on_change: Rc::new(on_change),
    });

    // Fires once up front, on whatever the window already knows -- usually
    // "no surface yet", reported as an empty list rather than skipped. The
    // real answer follows moments later once GTK has mapped the window and
    // the hooks below fire.
    state.recheck();

    // Hotplug: a monitor being added or removed changes the list regardless
    // of where the window is.
    if let Some(display) = gdk::Display::default() {
        let s = state.clone();
        display.monitors().connect_items_changed(move |_, _, _, _| s.recheck());
    }

    // The window landing on a different monitor changes which one is
    // current. There is no dedicated signal for that -- see this file's
    // header -- so this connects broadly and relies on `State::recheck`'s
    // whole-list comparison to drop the notifications that were about
    // something else.
    hook_surface_notify(&window, &state);

    // The surface GDK hands back is a new object each time the window is
    // realised, so the hook above has to be reinstalled whenever that
    // happens rather than assumed to survive it. In the ordinary case -- one
    // present, one surface, for the life of the window -- this fires zero
    // further times and costs nothing.
    let s = state.clone();
    window.connect_realize(move |w| hook_surface_notify(w, &s));
}

/// Connect to every property-change notification the window's current
/// surface fires, if it has one yet.
///
/// A no-op before the window is realised, which is why [`watch`] also hooks
/// `realize`: calling this from both places means neither ordering --
/// watching a window that is already on screen, or watching one that is
/// about to be built -- leaves the surface unwatched.
fn hook_surface_notify(window: &gtk::Window, state: &Rc<State>) {
    let Some(surface) = window.surface() else { return };
    let s = state.clone();
    surface.connect_notify_local(None, move |_, _| s.recheck());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_shape_matches_the_field_names_refresh_rs_uses() {
        // Pins the two fields by name and type. This is the whole contract
        // between this file and `cordial_runtime::refresh::Output` -- there
        // is no shared type to pin it with a real equality check, so the
        // nearest thing available is asserting the fields construct and read
        // back the way `refresh.rs`'s own tests expect of its `Output`.
        let o = Output { hz: 59.94, current: true };
        assert_eq!(o.hz, 59.94);
        assert!(o.current);
    }
}
