//! The X11 backend's text editor: the runtime half.
//!
//! [`cordial_shell::x11_editor::X11Editor`] is the widget and the window; this
//! is what drives it, the counterpart of `wayland.rs`'s `sync_text_overlay`,
//! `update_text_overlay` and the two `connect_editor_*` closures. Before it,
//! typed text on X11 reached the engine through the buffer path in `window.rs`
//! and was never drawn, because the engine does not draw a focused TextBox's own
//! contents (ADR-024 has the reasoning; #86 the report).
//!
//! The decision and what it rules out is
//! [ADR-062](../../../../docs/adr/ADR-062-the-x11-editor-is-an-override-redirect-window.md).
//!
//! **Everything here runs on the pump thread**, which is the thread that
//! initialised GTK, so the editor lives in a `thread_local` and a call from any
//! other thread finds nothing and falls back to the buffer path. The GTK
//! signals fire from inside [`pump`], with no borrow held.

use super::text_geometry::{Placed, Resolver};
use cordial_linker_sys::game_activity::RawTextBoxInfo;
use cordial_shell::x11_editor::X11Editor;
use std::cell::{Cell, RefCell};
use std::ffi::{c_int, c_ulong};
use std::rc::Rc;
use std::sync::atomic::{AtomicI64, Ordering};

/// The GameActivity handle the pump was last called with, for the closures GTK
/// calls back into, which have no argument to carry it.
static ACTIVE_HANDLE: AtomicI64 = AtomicI64::new(0);

type Cache = Option<(u32, u64, String, i32, RawTextBoxInfo)>;

struct Driver {
    editor: X11Editor,
    resolver: Resolver,
    cache: RefCell<Cache>,
    visible: Cell<bool>,
}

thread_local! {
    static DRIVER: RefCell<Option<Rc<Driver>>> = const { RefCell::new(None) };
}

fn driver() -> Option<Rc<Driver>> {
    DRIVER.with(|d| d.borrow().clone())
}

/// `CORDIAL_X11_EDITOR=off` runs the backend as it was before the editor
/// existed, which is how the difference is shown in one session rather than
/// across two builds.
fn disabled() -> bool {
    std::env::var("CORDIAL_X11_EDITOR").is_ok_and(|v| v == "off" || v == "0")
}

/// Bring up the editor for the engine's X window `engine`. A failure is logged
/// and leaves the backend on its old buffer path: an editor that cannot start
/// is not a reason to refuse to run the game.
pub fn init(engine: c_ulong) {
    if disabled() {
        println!("[android] X11 text editor: off (CORDIAL_X11_EDITOR); typed text is sent and not drawn");
        return;
    }
    let built = cordial_shell::x11_editor::init().and_then(|()| X11Editor::new(engine));
    let editor = match built {
        Ok(e) => e,
        Err(e) => {
            eprintln!(
                "[android] X11 text editor unavailable ({e}); typed text is sent to the game but \
                 will not be drawn until the box loses focus"
            );
            return;
        }
    };
    // Roblox's own font, out of the APK, as on Wayland; `None` is not an error.
    editor.set_font_family(super::editor_font::install().map(str::to_owned));

    // What the user typed, back to the engine. The widget owns the text, so this
    // is the only way a keystroke reaches Roblox while a box is focused.
    editor.connect_changed(|text, caret| {
        let Some(which) = cordial_linker_sys::game_activity::focused_textbox() else { return };
        super::input::adopt_editor_text(text, caret);
        let handle = ACTIVE_HANDLE.load(Ordering::Relaxed);
        if handle != 0 {
            let _ = cordial_linker_sys::game_activity::text_input(handle, text, caret, caret);
        }
        send_current_text(which);
        if handle != 0 {
            super::input::deliver_surface_redraw(handle);
        }
    });
    // Enter on a single-line box is an editor action: the final text, then the
    // engine is told Enter was pressed, and what follows is the engine's.
    editor.connect_activate(|| {
        let Some(which) = cordial_linker_sys::game_activity::focused_textbox() else { return };
        send_current_text(which);
        super::input::return_pressed(which);
    });
    // Keys the engine must hear although the editor has the keyboard: Escape
    // (the engine decides whether a box blurs), F11 and the recording hotkey.
    editor.connect_key_forward(Box::new(|keyval, keycode, state, down| {
        super::window::current().is_some_and(|w| w.forward_editor_key(keyval, keycode, state, down))
    }));

    DRIVER.with(|d| {
        *d.borrow_mut() = Some(Rc::new(Driver {
            editor,
            resolver: Resolver::default(),
            cache: RefCell::new(None),
            visible: Cell::new(false),
        }))
    });
    println!("[android] X11 text editor: a GTK window over the focused box owns the typed text (ADR-062)");
}

/// The GameActivity handle the last pump carried, for callers GTK reaches
/// without one (the editor's key forwarding).
pub fn active_handle() -> i64 {
    ACTIVE_HANDLE.load(Ordering::Relaxed)
}

/// The descriptor GTK reads its X events from, for the looper to wake on.
pub fn connection_fd() -> Option<c_int> {
    driver().map(|d| d.editor.connection_fd())
}

/// Whether the editor holds the keyboard, which is when the engine's own key
/// path must not also edit the text.
pub fn owns_keyboard() -> bool {
    driver().is_some_and(|d| d.visible.get() && d.editor.owns_keyboard())
}

/// Whether there is an editor at all on this thread.
pub fn available() -> bool {
    driver().is_some()
}

/// The development control surface's `paste`/`settext`, into the widget.
/// `None` when there is no editor here, in which case the caller uses the
/// buffer path.
pub fn replace_text(text: &str, replace_all: bool) -> Option<usize> {
    let d = driver()?;
    Some(if replace_all { d.editor.replace_text(text) } else { d.editor.paste_at_caret(text) })
}

/// Hide the editor; the window is closing.
pub fn close() {
    if let Some(d) = driver() {
        d.editor.set_text_overlay(None);
        d.editor.tick();
    }
}

/// One pump: let GTK run (which is where a keystroke becomes an edit), then put
/// the editor where the focused box is.
pub fn pump(handle: i64) {
    let Some(d) = driver() else { return };
    ACTIVE_HANDLE.store(handle, Ordering::Relaxed);
    d.editor.tick();
    sync_text_overlay(&d);
}

fn canvas() -> (i32, i32) {
    super::window::current().map(|w| w.geometry()).map_or((0, 0), |(w, h, _)| (w, h))
}

fn sync_text_overlay(d: &Driver) {
    let Some(_which) = cordial_linker_sys::game_activity::focused_textbox() else {
        if d.visible.replace(false) {
            *d.cache.borrow_mut() = None;
            *super::wayland::LAST_EDITOR_RECT.lock().unwrap_or_else(|e| e.into_inner()) = None;
            d.resolver.reset();
            d.editor.set_text_overlay(None);
        }
        return;
    };
    // Before the revision is read: applying the engine's text is what moves it,
    // and an engine-side clear would otherwise show a tick late.
    super::input::apply_engine_text();
    let generation = cordial_linker_sys::game_activity::textbox_generation();
    let revision = super::input::text_buffer_revision();
    let (info, placed) = d.resolver.resolve(generation, canvas());
    let unchanged = d
        .cache
        .borrow()
        .as_ref()
        .is_some_and(|(g, r, _, _, old)| *g == generation && *r == revision && *old == info);
    if unchanged {
        return;
    }
    let (text, caret) = super::input::text_buffer_snapshot();
    update_overlay(d, generation, super::input::text_buffer_revision(), &text, caret, info, placed);
}

/// What the engine was last told, mirrored back into the overlay so the next
/// tick does not redraw what the keystroke already did.
fn send_current_text(which: i64) {
    let Some(d) = driver() else { return };
    let (text, caret) = super::input::text_buffer_snapshot();
    super::input::pass_text(which, &text, caret);
    let generation = cordial_linker_sys::game_activity::textbox_generation();
    let (info, placed) = d.resolver.resolve(generation, canvas());
    update_overlay(&d, generation, super::input::text_buffer_revision(), &text, caret, info, placed);
}

fn update_overlay(d: &Driver, generation: u32, revision: u64, text: &str, caret: i32, info: RawTextBoxInfo, placed: Placed) {
    {
        let mut cache = d.cache.borrow_mut();
        let unchanged = cache
            .as_ref()
            .is_some_and(|(g, r, old, old_caret, old_info)| *g == generation && *r == revision && old == text && *old_caret == caret && *old_info == info);
        if unchanged {
            return;
        }
        *cache = Some((generation, revision, text.to_owned(), caret, info));
    }
    *super::wayland::LAST_EDITOR_RECT.lock().unwrap_or_else(|e| e.into_inner()) =
        Some((info.x, info.y, info.width, info.height, placed.token()));

    // Which of Roblox's fonts this box is in, if the engine named one. A
    // synthesised spec carries zeroes and zero is a value the table has no row
    // for, so it is skipped rather than reported as an unresolvable id.
    let face = match (placed, super::editor_font::font_id(&info)) {
        (Placed::Fallback, _) | (_, None) => None,
        (_, Some(id)) => match super::editor_font::face_for_id(id) {
            Some(face) => Some((id, face)),
            None => {
                let fallback = super::editor_font::default_face();
                super::editor_font::log_unresolved(id, fallback);
                fallback.map(|face| (id, face))
            }
        },
    };
    if super::input::trace_text() {
        eprintln!(
            "[cordial] text editor placed from {} x={} y={} w={} h={} xAlign={} yAlign={} \
             multiline={} textInputType={} fontSize={} (x11)",
            placed.source(),
            info.x, info.y, info.width, info.height, info.x_alignment, info.y_alignment,
            info.multiline, info.text_input_type, info.font_size,
        );
    }
    let font_size = match face.as_ref() {
        Some((_, f)) => info.font_size * f.from_rbx_font_ratio,
        None => info.font_size,
    };
    d.editor.set_text_overlay(Some(cordial_shell::host_window::TextOverlay {
        text,
        caret_chars: caret,
        x: info.x,
        y: info.y,
        width: info.width,
        height: info.height,
        font_size,
        text_color: info.text_color as u32,
        font_family: face.as_ref().map(|(_, f)| f.family.as_str()),
        font_weight: face.as_ref().map_or(400, |(_, f)| f.weight),
        font_italic: face.as_ref().is_some_and(|(_, f)| f.italic),
        password: matches!(info.text_input_type, 5 | 9 | 10),
        multiline: info.multiline != 0,
        text_wrapped: info.text_wrapped != 0,
        fallback: placed == Placed::Fallback,
        x_alignment: info.x_alignment,
        y_alignment: info.y_alignment,
    }));
    d.visible.set(true);
}
