//! Where the focused Roblox TextBox is, from the best source that will say.
//!
//! This is the resolution `wayland.rs` does in `resolve_textbox_geometry`,
//! `polled_textbox_info`, `fallback_textbox_info` and `info_fits_canvas`, taken
//! out of that file so the X11 backend's editor ([`super::x11_text`]) places
//! itself by the same rules. ADR-024 committed to exactly that ("two backends
//! may not mean two editors"), and listed these functions by name as the ones
//! with no Wayland call in them. **`wayland.rs` still has its own copy**: it
//! belongs to other work in flight and the move is a deletion there, not a
//! change here -- see ADR-062. Until it is done a fix to one copy has to be
//! made in the other. The history behind each rule (the search modal's
//! zero-height first answer, its opening animation, the stale rectangle that
//! survives a fullscreen change) is in `wayland.rs`'s comments on the originals
//! and is not repeated at length here.

use cordial_linker_sys::game_activity::RawTextBoxInfo;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Which source put the editor where it is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Placed {
    /// The engine said where this box is.
    Engine,
    /// The engine has not said yet, so the editor is held where the last box
    /// was until it does.
    Carried,
    /// Nothing has ever said, so the editor is in a bar Cordial positions.
    Fallback,
}

impl Placed {
    pub(crate) fn source(self) -> &'static str {
        match self {
            Placed::Engine => "engine geometry",
            Placed::Carried => "the previous box, while this one lays out",
            Placed::Fallback => "fallback bar",
        }
    }

    /// For the control socket, whose replies are whitespace-separated fields.
    pub(crate) fn token(self) -> &'static str {
        match self {
            Placed::Engine => "engine",
            Placed::Carried => "carried",
            Placed::Fallback => "fallback",
        }
    }
}

struct LastPlacement {
    info: RawTextBoxInfo,
    at: Instant,
    /// The canvas the rectangle was accepted under, so the same numbers
    /// surviving a *different* canvas can be told from a box that did not move.
    canvas: (i32, i32),
}

struct Polled {
    generation: u32,
    property_generation: u32,
    asked: Instant,
    usable: Option<RawTextBoxInfo>,
    /// A rectangle seen once and not yet repeated. Only one that repeats is
    /// trusted, because the search modal answers every frame of its opening
    /// animation with a different one.
    pending: Option<RawTextBoxInfo>,
}

/// The state [`Resolver::resolve`] keeps between pumps.
#[derive(Default)]
pub(crate) struct Resolver {
    polled: Mutex<Option<Polled>>,
    last: Mutex<Option<LastPlacement>>,
}

/// Half the pump rate: the answer moves (the modal's field measured `w=592`
/// and then `w=564` once it had text), but a JNI call that constructs a Java
/// object on every tick is a cost on the one thread this project defends.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How long the editor may be held at the previous box's place while the
/// engine works out the new one: long enough for the search modal, short enough
/// that a box which will never report geometry still reaches the fallback bar.
const CARRY_OVER: Duration = Duration::from_millis(1_500);

/// Whether a rectangle could belong to the window as it is *now*. The engine
/// answers from its own idea of where the box is and that does not update on a
/// mere resize; a rectangle that does not fit the canvas at all cannot be this
/// window's honest answer. A margin rather than an exact bound: the sign being
/// caught is grossly outside, not a few pixels of rounding. No canvas yet
/// passes everything.
pub(crate) fn info_fits_canvas(info: &RawTextBoxInfo, (cw, ch): (i32, i32)) -> bool {
    if cw <= 0 || ch <= 0 {
        return true;
    }
    const MARGIN: f32 = 8.0;
    info.x >= -MARGIN
        && info.y >= -MARGIN
        && info.x + info.width <= cw as f32 + MARGIN
        && info.y + info.height <= ch as f32 + MARGIN
}

/// A bar across the lower third of the canvas, where an on-screen keyboard's
/// own text would sit on the device this engine was built for. Deliberately
/// not the exact box -- that is unknown, and a correctly styled editor in the
/// wrong place reads as a rendering bug where a bar that is obviously a bar
/// reads as what it is.
pub(crate) fn fallback_info((cw, ch): (i32, i32)) -> RawTextBoxInfo {
    let (w, h) = if cw > 0 && ch > 0 { (cw as f32, ch as f32) } else { (1280.0, 720.0) };
    let width = (w * 0.6).max(240.0);
    let height = 44.0;
    RawTextBoxInfo {
        x: (w - width) / 2.0,
        y: (h * 0.72).min(h - height - 8.0).max(0.0),
        width,
        height,
        font_size: 18.0,
        text_color: 0x00FF_FFFFu32 as i32,
        // Centre. `Default`'s zero is `Top`, which would shrink this bar to one
        // line's height and anchor it at the top.
        y_alignment: 1,
        ..Default::default()
    }
}

impl Resolver {
    /// Where the focused box is: the spec `showKeyboard` volunteered if usable,
    /// else what `nativeGetTextBoxInfo` answers, else the previous box's place
    /// for [`CARRY_OVER`], else the fallback bar.
    pub(crate) fn resolve(&self, generation: u32, canvas: (i32, i32)) -> (RawTextBoxInfo, Placed) {
        let found = match cordial_linker_sys::game_activity::focused_textbox_info() {
            Some(info) if info.width > 0.0 && info.height > 0.0 && info_fits_canvas(&info, canvas) => Some(info),
            _ => self.polled(generation, canvas),
        };
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        // A repeat under a different canvas is the same staleness
        // `info_fits_canvas` cannot see: going *into* fullscreen, a windowed
        // rectangle still fits easily and is just as wrong.
        let found = match (found, last.as_ref()) {
            (Some(info), Some(p)) if info == p.info && p.canvas != canvas => None,
            (found, _) => found,
        };
        if let Some(info) = found {
            *last = Some(LastPlacement { info, at: Instant::now(), canvas });
            return (info, Placed::Engine);
        }
        if let Some(p) = last.as_ref() {
            if p.at.elapsed() < CARRY_OVER {
                return (p.info, Placed::Carried);
            }
        }
        (fallback_info(canvas), Placed::Fallback)
    }

    /// Forget everything: the box went away.
    pub(crate) fn reset(&self) {
        *self.polled.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    fn polled(&self, generation: u32, canvas: (i32, i32)) -> Option<RawTextBoxInfo> {
        let native = super::input::textbox_info_native();
        if native.is_null() {
            return None;
        }
        let mut state = self.polled.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let property_generation = cordial_linker_sys::game_activity::textbox_property_generation();
        match state.as_ref() {
            Some(p) if p.generation == generation && p.property_generation == property_generation => {
                if now.duration_since(p.asked) < POLL_INTERVAL {
                    return p.usable;
                }
            }
            // A new box, or the same box restyled: nothing carries over.
            _ => {
                *state = Some(Polled { generation, property_generation, asked: now, usable: None, pending: None });
            }
        }
        let carried = state.as_ref().and_then(|p| p.usable);
        let pending = state.as_ref().and_then(|p| p.pending);
        // SAFETY: `native` was resolved against the loaded libroblox.so, which is never unloaded.
        let fresh = match unsafe { cordial_linker_sys::game_activity::textbox_info_now(native) } {
            // A zero height is the trap: the modal, caught mid-animation,
            // answers `x=596 y=10 w=42 h=0` -- an answer-shaped value that is
            // invisible.
            Ok(Some(i)) if i.width > 0.0 && i.height > 0.0 && info_fits_canvas(&i, canvas) => Some(i),
            Ok(_) => None,
            Err(e) => {
                if super::input::trace_text() {
                    eprintln!("[cordial] nativeGetTextBoxInfo failed: {e}");
                }
                None
            }
        };
        let (answer, new_pending) = match fresh {
            Some(i) if pending == Some(i) => (Some(i), Some(i)),
            Some(i) => (carried, Some(i)),
            // Null is ordinary (the whole sign-in page answers it) and neither
            // confirms nor restarts a pending rectangle.
            None => (carried, pending),
        };
        *state = Some(Polled { generation, property_generation, asked: now, usable: answer, pending: new_pending });
        answer
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(x: f32, y: f32, w: f32, h: f32) -> RawTextBoxInfo {
        RawTextBoxInfo { x, y, width: w, height: h, ..Default::default() }
    }

    /// Roblox's search modal answers a box on a canvas it no longer fits after a
    /// window shrinks out of fullscreen; that rectangle must be refused.
    #[test]
    fn a_rectangle_beyond_the_canvas_is_not_believed() {
        assert!(info_fits_canvas(&info(470.0, 264.0, 340.0, 22.0), (1280, 800)));
        assert!(!info_fits_canvas(&info(1550.0, 264.0, 340.0, 22.0), (1280, 800)));
        assert!(info_fits_canvas(&info(1550.0, 264.0, 340.0, 22.0), (0, 0)), "no canvas yet passes");
        assert!(info_fits_canvas(&info(-4.0, 0.0, 100.0, 22.0), (1280, 800)), "rounding overhang is tolerated");
    }

    /// The fallback is a bar inside the canvas, centred vertically in itself.
    #[test]
    fn the_fallback_bar_sits_inside_the_canvas_with_a_centre_alignment() {
        let f = fallback_info((1280, 800));
        assert!(info_fits_canvas(&f, (1280, 800)));
        assert_eq!(f.y_alignment, 1);
        assert_eq!(f.width, 768.0);
        let small = fallback_info((300, 200));
        assert!(small.y >= 0.0 && small.y + small.height <= 200.0, "{small:?}");
    }

    #[test]
    fn placed_tokens_have_no_spaces() {
        for p in [Placed::Engine, Placed::Carried, Placed::Fallback] {
            assert!(!p.token().contains(' '));
        }
    }
}
