//! The editor widgets: a real `gtk::Text` (or a `gtk::TextView` for a
//! multi-line box) that owns the text of the Roblox TextBox in focus.
//!
//! **This is [`crate::host_window::HostWindow`]'s editor, lifted out so a
//! second window can hold it.** On Wayland the editor is a child of the one
//! toplevel, in a `GtkFixed` over the engine's subsurface. On X11 the engine's
//! window is a bare Xlib one and the editor lives in a small window of its own
//! ([`crate::x11_editor`]); both want the same widgets, the same Roblox-to-GTK
//! styling and the same "the widget owns the text, Cordial mirrors it" rule
//! (docs/adr/ADR-024), and ADR-024's own commitment was that two backends may
//! not mean two editors.
//!
//! **`HostWindow` does not use this yet, and that is deliberate for now.** The
//! Wayland path was last changed by other work that was in flight when this
//! was written (the multi-line colour fix, fullscreen, stacking), and moving
//! ~600 lines out of a file several people were editing would have been a
//! merge conflict with no behavioural gain. The placement helpers below are
//! therefore copies of `host_window.rs`'s private ones, with the same tests;
//! the follow-up is for `HostWindow` to hold an `EditorCore` and delete its
//! copy. Until then a change to how a box is styled has to be made in both
//! places, and `docs/adr/ADR-062` says so.

use crate::host_window::TextOverlay;
use libadwaita::gtk;
use libadwaita::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

type ChangedSink = Rc<RefCell<Option<Box<dyn Fn(&str, i32)>>>>;
type ActivateSink = Rc<RefCell<Option<Box<dyn Fn()>>>>;

/// The stylesheet that strips the widgets' own chrome. Loaded once per display
/// by the host; the box's colours are a separate, reloaded sheet
/// ([`EditorCore::colour_provider`]).
pub const EDITOR_SHEET: &str = ".cordial-editor { \
         background: none; background-image: none; border: none; box-shadow: none; \
         outline: none; padding: 0; margin: 0; min-height: 0; min-width: 0; } \
     .cordial-editor-multiline, .cordial-editor-multiline text { \
         background: none; background-image: none; border: none; box-shadow: none; \
         outline: none; padding: 0; margin: 0; min-height: 0; min-width: 0; } \
     .cordial-editor-multiline-scroll { \
         background: none; background-image: none; border: none; box-shadow: none; } \
     .cordial-text-fallback { \
         background-color: rgba(28, 28, 30, 0.94); color: #ffffff; \
         border: 1px solid rgba(255, 255, 255, 0.25); border-radius: 8px; \
         padding: 6px 10px; }";

/// The editor widgets and everything about how a Roblox box styles them.
pub struct EditorCore {
    /// What the host window holds. Both widgets sit at its origin: the host
    /// positions the *window* on the box, so nothing here needs to know where
    /// on the screen it is.
    root: gtk::Fixed,
    text: gtk::Text,
    view: gtk::TextView,
    scroll: gtk::ScrolledWindow,
    tag: gtk::TextTag,
    colours: gtk::CssProvider,
    seeding: Rc<Cell<bool>>,
    view_seeding: Rc<Cell<bool>>,
    changed: ChangedSink,
    activated: ActivateSink,
    font_family: RefCell<Option<String>>,
    active_multiline: Cell<bool>,
    /// GDK's integer scale: widget sizes are logical pixels and the box is in
    /// the engine's physical ones.
    scale: Cell<f32>,
}

impl EditorCore {
    pub fn new() -> Self {
        let root = gtk::Fixed::new();
        root.set_hexpand(true);
        root.set_vexpand(true);

        let text = gtk::Text::new();
        text.set_visible(false);
        text.add_css_class("cordial-editor");
        text.set_overflow(gtk::Overflow::Hidden);
        // The engine draws its own pointer into the frame, so the system one
        // must not appear over a box. `none` is the CSS keyword for "draw
        // nothing"; `None` would inherit the window's and show an I-beam.
        text.set_cursor_from_name(cursor());

        let seeding = Rc::new(Cell::new(false));
        let changed: ChangedSink = Rc::new(RefCell::new(None));
        let activated: ActivateSink = Rc::new(RefCell::new(None));
        {
            let sink = activated.clone();
            text.connect_activate(move |_| {
                if let Some(cb) = sink.borrow().as_ref() {
                    cb();
                }
            });
        }
        {
            // One closure for both signals: the text and the caret are one fact
            // to the engine, and reporting them separately would send a caret for
            // text it had not been told about yet.
            let seeding = seeding.clone();
            let sink = changed.clone();
            let widget = text.clone();
            let notify: Rc<dyn Fn()> = Rc::new(move || {
                if seeding.get() {
                    return;
                }
                if let Some(cb) = sink.borrow().as_ref() {
                    cb(widget.text().as_str(), widget.position());
                }
            });
            let on_text = notify.clone();
            text.connect_changed(move |_| on_text());
            text.connect_notify_local(Some("cursor-position"), move |_, _| notify());
        }
        root.put(&text, 0.0, 0.0);

        let view = gtk::TextView::new();
        view.add_css_class("cordial-editor-multiline");
        view.set_overflow(gtk::Overflow::Hidden);
        view.set_cursor_from_name(cursor());
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_child(Some(&view));
        scroll.set_has_frame(false);
        scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroll.add_css_class("cordial-editor-multiline-scroll");
        scroll.set_visible(false);

        let tag = gtk::TextTag::new(Some("cordial-editor-style"));
        let buffer = view.buffer();
        buffer.tag_table().add(&tag);
        let view_seeding = Rc::new(Cell::new(false));
        {
            let seeding = view_seeding.clone();
            let sink = changed.clone();
            let buf = buffer.clone();
            let notify: Rc<dyn Fn()> = Rc::new(move || {
                if seeding.get() {
                    return;
                }
                if let Some(cb) = sink.borrow().as_ref() {
                    let (start, end) = buf.bounds();
                    let caret = buf.iter_at_mark(&buf.get_insert()).offset();
                    cb(buf.text(&start, &end, true).as_str(), caret);
                }
            });
            let on_text = notify.clone();
            buffer.connect_changed(move |_| on_text());
            buffer.connect_notify_local(Some("cursor-position"), move |_, _| notify());
        }
        {
            // A GtkTextBuffer does not extend a tag to text typed at its edges,
            // so the one style tag is reapplied across the whole buffer on every
            // change -- the equivalent of `gtk::Text`'s single attribute list.
            let tag = tag.clone();
            buffer.connect_changed(move |buf| {
                let (start, end) = buf.bounds();
                buf.apply_tag(&tag, &start, &end);
            });
        }
        root.put(&scroll, 0.0, 0.0);

        Self {
            root,
            text,
            view,
            scroll,
            tag,
            colours: gtk::CssProvider::new(),
            seeding,
            view_seeding,
            changed,
            activated,
            font_family: RefCell::new(None),
            active_multiline: Cell::new(false),
            scale: Cell::new(1.0),
        }
    }

    /// The widget a host window puts in itself.
    pub fn widget(&self) -> &gtk::Fixed {
        &self.root
    }

    /// The per-box colour sheet. The host adds it to its display; it is
    /// reloaded whenever a box takes focus.
    pub fn colour_provider(&self) -> &gtk::CssProvider {
        &self.colours
    }

    /// Tell the widgets how many device pixels make one of GTK's. On a plain X
    /// server this is 1; under `GDK_SCALE=2` the engine's rectangle is twice
    /// what GTK would draw it as.
    pub fn set_scale(&self, scale: i32) {
        self.scale.set(scale.max(1) as f32);
    }

    pub fn set_font_family(&self, family: Option<String>) {
        *self.font_family.borrow_mut() = family;
    }

    pub fn connect_changed<F: Fn(&str, i32) + 'static>(&self, f: F) {
        *self.changed.borrow_mut() = Some(Box::new(f));
    }

    pub fn connect_activate<F: Fn() + 'static>(&self, f: F) {
        *self.activated.borrow_mut() = Some(Box::new(f));
    }

    /// Which widget [`Self::apply`] last showed: `true` for the multi-line one.
    pub fn multiline_active(&self) -> bool {
        self.active_multiline.get()
    }

    /// Whether either widget currently holds GTK's keyboard focus.
    pub fn has_focus(&self) -> bool {
        self.text.has_focus() || self.view.has_focus()
    }

    /// Insert at the caret, replacing a selection first, as a real paste does.
    /// Same pair of `GtkEditable` calls GTK's own handler makes, so the
    /// control surface and a Ctrl+V share an implementation.
    pub fn paste_at_caret(&self, text: &str) -> usize {
        let text = text.replace('\n', " ");
        self.text.delete_selection();
        let mut pos = self.text.position();
        self.text.insert_text(&text, &mut pos);
        self.text.set_position(pos);
        self.text.text().chars().count()
    }

    pub fn replace_text(&self, text: &str) -> usize {
        let text = text.replace('\n', " ");
        self.text.set_text(&text);
        self.text.set_position(text.chars().count() as i32);
        self.text.text().chars().count()
    }

    pub fn multiline_paste_at_caret(&self, text: &str) -> usize {
        let buffer = self.view.buffer();
        buffer.delete_selection(false, true);
        buffer.insert_at_cursor(text);
        buffer.char_count() as usize
    }

    pub fn multiline_replace_text(&self, text: &str) -> usize {
        let buffer = self.view.buffer();
        buffer.set_text(text);
        let end = buffer.end_iter();
        buffer.place_cursor(&end);
        buffer.char_count() as usize
    }

    /// Hide both widgets and empty them. Seeded, not typed: clearing on blur
    /// must not reach the engine as an edit that empties the box the user just
    /// finished filling in.
    pub fn clear(&self) {
        self.text.set_visible(false);
        self.scroll.set_visible(false);
        self.active_multiline.set(false);
        self.seeding.set(true);
        self.text.set_text("");
        self.seeding.set(false);
        self.view_seeding.set(true);
        self.view.buffer().set_text("");
        self.view_seeding.set(false);
    }

    /// Style, seed and size the widget for `overlay` and give it keyboard
    /// focus. Returns the rectangle it occupies in the overlay's own
    /// coordinates, which is *not* always the box's: a `Top`- or
    /// `Bottom`-aligned single-line box shrinks to one line (see
    /// [`vertical_placement`]).
    ///
    /// The widget is left at this root's origin; the caller moves the window it
    /// is in onto the returned rectangle. The text is written only when it
    /// differs from what the widget holds -- a blind `set_text` every tick
    /// would reset the selection and fight the caret.
    pub fn apply(&self, overlay: &TextOverlay<'_>) -> (i32, i32, i32, i32) {
        let (x, y) = (overlay.x.round() as i32, overlay.y.round() as i32);
        let (w, h) = (overlay.width.max(1.0).round() as i32, overlay.height.max(1.0).round() as i32);
        // Colours as CSS because the caret takes `caret-color`, not a Pango
        // foreground; an attribute-styled editor on a light field drew dark
        // text with the theme's white caret.
        let rgb = format!(
            "#{:02x}{:02x}{:02x}",
            (overlay.text_color >> 16) & 0xff,
            (overlay.text_color >> 8) & 0xff,
            overlay.text_color & 0xff
        );
        self.colours.load_from_string(&editor_colour_sheet(&rgb));
        self.active_multiline.set(overlay.multiline);
        if overlay.multiline {
            self.text.set_visible(false);
            self.apply_multiline(overlay, x, y, w, h)
        } else {
            self.scroll.set_visible(false);
            self.apply_singleline(overlay, x, y, w, h)
        }
    }

    fn logical(&self, px: i32) -> i32 {
        ((px as f32 / self.scale.get()).round() as i32).max(1)
    }

    fn apply_singleline(&self, overlay: &TextOverlay<'_>, x: i32, y: i32, w: i32, h: i32) -> (i32, i32, i32, i32) {
        // Absolute size, in the same device units as the rectangle beside it in
        // the spec; a CSS font-size would be scaled by the desktop's text
        // scaling and draw larger than the text Roblox draws unfocused.
        let attrs = gtk::pango::AttrList::new();
        attrs.insert(gtk::pango::AttrSize::new_size_absolute(
            (overlay.font_size.max(1.0) / self.scale.get() * gtk::pango::SCALE as f32).round() as i32,
        ));
        // The box's own family wins over the process-wide one; weight and style
        // ride with the family only, since a weight over Pango's own choice of
        // face would restyle the desktop font.
        let per_box = overlay.font_family;
        let process_wide = self.font_family.borrow();
        if let Some(family) = per_box.or(process_wide.as_deref()) {
            attrs.insert(gtk::pango::AttrString::new_family(family));
        }
        if per_box.is_some() {
            attrs.insert(gtk::pango::AttrInt::new_weight(pango_weight(overlay.font_weight)));
            attrs.insert(gtk::pango::AttrInt::new_style(if overlay.font_italic {
                gtk::pango::Style::Italic
            } else {
                gtk::pango::Style::Normal
            }));
        }
        drop(process_wide);
        self.text.set_attributes(Some(&attrs));
        self.text.set_alignment(gtk_xalign(overlay.x_alignment));
        // GTK's own masking, so the widget holds the real text; and the purpose
        // as well, because an input method decides what to offer from the
        // purpose and not from whether glyphs are drawn.
        self.text.set_visibility(!overlay.password);
        self.text.set_input_purpose(if overlay.password {
            gtk::InputPurpose::Password
        } else {
            gtk::InputPurpose::FreeForm
        });

        if self.text.text() != overlay.text {
            self.seeding.set(true);
            self.text.set_text(overlay.text);
            self.text.set_position(overlay.caret_chars);
            self.seeding.set(false);
        }

        let (_, natural_h, _, _) = self.text.measure(gtk::Orientation::Vertical, -1);
        let natural_px = (natural_h as f32 * self.scale.get()).round() as i32;
        let (y, h) = vertical_placement(overlay.y_alignment, y, h, natural_px);
        self.text.set_size_request(self.logical(w), self.logical(h));
        self.root.move_(&self.text, 0.0, 0.0);
        if overlay.fallback {
            self.text.add_css_class("cordial-text-fallback");
        } else {
            self.text.remove_css_class("cordial-text-fallback");
        }
        self.text.set_visible(true);
        if !self.text.has_focus() {
            // Not `grab_focus`: `gtk-entry-select-on-focus` would select the
            // whole field and the next keystroke would replace it.
            self.text.grab_focus_without_selecting();
        }
        (x, y, w, h)
    }

    fn apply_multiline(&self, overlay: &TextOverlay<'_>, x: i32, y: i32, w: i32, h: i32) -> (i32, i32, i32, i32) {
        let tag = &self.tag;
        tag.set_size((overlay.font_size.max(1.0) / self.scale.get() * gtk::pango::SCALE as f32).round() as i32);
        let per_box = overlay.font_family;
        let process_wide = self.font_family.borrow();
        tag.set_family(per_box.or(process_wide.as_deref()));
        drop(process_wide);
        if per_box.is_some() {
            tag.set_weight(overlay.font_weight);
            tag.set_style(if overlay.font_italic { gtk::pango::Style::Italic } else { gtk::pango::Style::Normal });
        } else {
            tag.set_weight(400);
            tag.set_style(gtk::pango::Style::Normal);
        }
        self.view.set_justification(gtk_justification(overlay.x_alignment));
        self.view.set_wrap_mode(if overlay.text_wrapped { gtk::WrapMode::WordChar } else { gtk::WrapMode::None });

        let buffer = self.view.buffer();
        let (start, end) = buffer.bounds();
        if buffer.text(&start, &end, true).as_str() != overlay.text {
            self.view_seeding.set(true);
            buffer.set_text(overlay.text);
            let at = buffer.iter_at_offset(overlay.caret_chars.max(0));
            buffer.place_cursor(&at);
            self.view_seeding.set(false);
        }
        let (start, end) = buffer.bounds();
        buffer.apply_tag(tag, &start, &end);
        let target = buffer.iter_at_offset(overlay.caret_chars.max(0));
        if buffer.iter_at_mark(&buffer.get_insert()).offset() != target.offset() {
            buffer.place_cursor(&target);
        }

        let (y, h) = multiline_placement(y, h);
        self.scroll.set_size_request(self.logical(w), self.logical(h));
        self.root.move_(&self.scroll, 0.0, 0.0);
        if overlay.fallback {
            self.scroll.add_css_class("cordial-text-fallback");
        } else {
            self.scroll.remove_css_class("cordial-text-fallback");
        }
        self.scroll.set_visible(true);
        if !self.view.has_focus() {
            self.view.grab_focus();
        }
        (x, y, w, h)
    }
}

impl Default for EditorCore {
    fn default() -> Self {
        Self::new()
    }
}

/// `none` unless `CORDIAL_SHOW_CURSOR` is set, which is the debugging switch
/// the engine window has always had.
fn cursor() -> Option<&'static str> {
    if std::env::var_os("CORDIAL_SHOW_CURSOR").is_some() {
        None
    } else {
        Some("none")
    }
}

/// GTK's `Editable::set_alignment` fraction for Roblox's `Enum.TextXAlignment`:
/// `Left` 0, `Right` 1, `Center` 2 (the published order, which is not
/// alphabetical). Copy of `host_window::gtk_xalign`.
fn gtk_xalign(x_alignment: i32) -> f32 {
    match x_alignment {
        1 => 1.0,
        2 => 0.5,
        _ => 0.0,
    }
}

/// The `gtk::TextView` counterpart of [`gtk_xalign`], same ordinals.
fn gtk_justification(x_alignment: i32) -> gtk::Justification {
    match x_alignment {
        1 => gtk::Justification::Right,
        2 => gtk::Justification::Center,
        _ => gtk::Justification::Left,
    }
}

/// Where to draw a box's one line vertically for Roblox's `yAlignment`:
/// `Center` is the box unchanged (the only value ever measured), `Top` and
/// `Bottom` shrink to the line's natural height at that edge. `UNVERIFIED` for
/// the latter two, as in `host_window.rs`.
fn vertical_placement(y_alignment: i32, box_y: i32, box_h: i32, natural_h: i32) -> (i32, i32) {
    let h = natural_h.clamp(1, box_h.max(1));
    match y_alignment {
        0 => (box_y, h),
        2 => (box_y + (box_h - h).max(0), h),
        _ => (box_y, box_h),
    }
}

/// A multi-line box always fills its rectangle.
fn multiline_placement(box_y: i32, box_h: i32) -> (i32, i32) {
    (box_y, box_h)
}

/// The colour of both widgets. A `GtkTextView` takes its colour from its own
/// node and not from its `text` child, which is the multi-line half of #86.
fn editor_colour_sheet(rgb: &str) -> String {
    format!(
        ".cordial-editor, .cordial-editor-multiline, .cordial-editor-multiline text \
         {{ color: {rgb}; caret-color: {rgb}; }}"
    )
}

fn pango_weight(open_type: i32) -> gtk::pango::Weight {
    use gtk::pango::Weight;
    match open_type {
        ..=149 => Weight::Thin,
        150..=249 => Weight::Ultralight,
        250..=324 => Weight::Light,
        325..=364 => Weight::Semilight,
        365..=389 => Weight::Book,
        390..=449 => Weight::Normal,
        450..=549 => Weight::Medium,
        550..=649 => Weight::Semibold,
        650..=749 => Weight::Bold,
        750..=849 => Weight::Ultrabold,
        850..=949 => Weight::Heavy,
        _ => Weight::Ultraheavy,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xalign_follows_roblox_enum_order_and_defaults_left() {
        assert_eq!(gtk_xalign(0), 0.0);
        assert_eq!(gtk_xalign(1), 1.0);
        assert_eq!(gtk_xalign(2), 0.5);
        assert_eq!(gtk_xalign(99), 0.0);
        assert_eq!(gtk_justification(1), gtk::Justification::Right);
        assert_eq!(gtk_justification(2), gtk::Justification::Center);
    }

    #[test]
    fn centre_is_untouched_and_top_bottom_anchor() {
        assert_eq!(vertical_placement(1, 10, 22, 13), (10, 22));
        assert_eq!(vertical_placement(0, 10, 22, 13), (10, 13));
        assert_eq!(vertical_placement(2, 10, 22, 13), (19, 13));
        assert_eq!(vertical_placement(0, 10, 22, 40), (10, 22), "clamped to the box");
        assert_eq!(multiline_placement(10, 88), (10, 88));
    }

    /// The multi-line half of #86: the sheet must name the view's own node, or
    /// a light theme draws near-black text on a dark game.
    #[test]
    fn the_colour_sheet_names_the_multiline_widget_itself() {
        let sheet = editor_colour_sheet("#d5d5dd");
        assert!(sheet.contains(".cordial-editor-multiline,"), "{sheet}");
        assert!(sheet.contains("caret-color: #d5d5dd"), "{sheet}");
    }

    #[test]
    fn weights_land_on_the_face_they_name() {
        assert_eq!(pango_weight(400), gtk::pango::Weight::Normal);
        assert_eq!(pango_weight(700), gtk::pango::Weight::Bold);
        assert_eq!(pango_weight(900), gtk::pango::Weight::Heavy);
    }
}
