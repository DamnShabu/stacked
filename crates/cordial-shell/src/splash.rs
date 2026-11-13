//! The startup splash: a small window with the icon and a progress bar, up
//! from just after the profile is claimed until the game window maps.
//!
//! **Why it exists.** Everything `cordial-run` does before the engine asks
//! for its window -- unpacking the APK's assets, resolving the engine's
//! imports, linking `libroblox.so`, `JNI_OnLoad`, the client-settings fetch --
//! happened with nothing on screen, so a launch from the desktop entry looked
//! like a click that did nothing for a second or two, longer on a first run
//! that has to extract.
//!
//! **The bar moves between steps, not during one.** GTK lives on the main
//! thread (see `host_window`'s note on why it cannot have one of its own),
//! and the main thread is the one doing the loading, so nothing repaints while
//! a step runs. [`step`] repaints by iterating the main context itself before
//! the next step starts. A step that blocks -- the settings fetch on a slow
//! network, the first extraction -- shows as the bar holding at that step's
//! label, which is at least the truth about where the time is going.
//!
//! **Wayland only.** On X11 GTK is not brought up until the text editor needs
//! it, with its own backend and renderer settings (`x11_editor`), and starting
//! it earlier for a splash would decide those for it.
//!
//! **Two things have to happen before the splash can exist, and both are
//! about it being the first GTK toplevel in the process.** GTK chooses its
//! renderer once, from `GSK_RENDERER`, when the first surface is realised, so
//! [`crate::gtk_renderer::settle`] runs first -- otherwise the splash would
//! fix the renderer the probe exists to choose (issue #53). And `gtk_init`
//! calls `setlocale(LC_ALL, "")`, which used to happen just before the
//! engine's native code was initialised and now would happen before
//! `libroblox.so` is even linked; the locale is put back to what it was here
//! and [`apply_deferred_locale`] applies GTK's at the point it used to happen,
//! so the engine's static initialisers and `JNI_OnLoad` run under the same
//! locale as before. INFERRED that anything in them is locale-sensitive; the
//! restore is there so that nobody has to find out.
//!
//! `CORDIAL_SPLASH=off` skips it, which is also the control for any claim
//! that it changed how long a launch takes.

use gtk::prelude::*;
use libadwaita as adw;
use libadwaita::gtk;

/// Turns the splash off. Read once, when [`show`] is called.
pub const ENV: &str = "CORDIAL_SPLASH";

struct Splash {
    window: adw::Window,
    bar: gtk::ProgressBar,
    status: gtk::Label,
}

thread_local! {
    static SPLASH: std::cell::RefCell<Option<Splash>> = const { std::cell::RefCell::new(None) };
    static DEFERRED_LOCALE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether the environment asks for no splash.
pub fn disabled_by(value: Option<&str>) -> bool {
    matches!(
        value.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("off" | "0" | "false" | "no")
    )
}

/// Show the splash, if it is wanted and GTK can come up. Never an error: a
/// splash that cannot open is a launch without a splash, not a failed launch.
///
/// Call on the thread that will later open the game window, after
/// [`crate::gtk_renderer::begin`].
pub fn show() {
    if disabled_by(std::env::var(ENV).ok().as_deref()) {
        return;
    }
    crate::gtk_renderer::settle();
    let before = current_locale();
    if let Err(e) = crate::host_window::init_wayland() {
        println!("  splash: skipped, no display ({e})");
        return;
    }
    if let Some(locale) = before {
        restore_locale(&locale);
        DEFERRED_LOCALE.with(|d| d.set(true));
    }
    let splash = build();
    splash.window.present();
    SPLASH.with(|s| *s.borrow_mut() = Some(splash));
    pump();
}

/// Move the bar to `fraction` (0 to 1) and say what is being done now.
/// Does nothing when there is no splash.
pub fn step(fraction: f64, what: &str) {
    let shown = SPLASH.with(|s| {
        let s = s.borrow();
        let Some(splash) = s.as_ref() else {
            return false;
        };
        splash.bar.set_fraction(fraction.clamp(0.0, 1.0));
        splash.status.set_label(what);
        true
    });
    if shown {
        pump();
    }
}

/// Close the splash. Called once the game window has mapped, so the desktop
/// is never left with nothing of Stacked's showing between the two.
pub fn finish() {
    if let Some(splash) = SPLASH.with(|s| s.borrow_mut().take()) {
        splash.bar.set_fraction(1.0);
        splash.window.close();
        pump();
    }
}

/// Apply the locale `gtk_init` would have set, if [`show`] held it back.
/// Called where GTK used to be initialised, before the engine's native code
/// starts. See the module doc.
pub fn apply_deferred_locale() {
    if DEFERRED_LOCALE.with(|d| d.replace(false)) {
        // SAFETY: on the main thread, at a point where GTK itself would have
        // made this exact call before the splash existed.
        unsafe { libc::setlocale(libc::LC_ALL, c"".as_ptr()) };
    }
}

fn current_locale() -> Option<std::ffi::CString> {
    // SAFETY: a query, with a null locale. The returned string is owned by
    // libc and may be overwritten by the next call, so it is copied at once.
    let p = unsafe { libc::setlocale(libc::LC_ALL, std::ptr::null()) };
    if p.is_null() {
        return None;
    }
    // SAFETY: non-null, and libc's locale strings are NUL-terminated.
    Some(unsafe { std::ffi::CStr::from_ptr(p) }.to_owned())
}

fn restore_locale(locale: &std::ffi::CStr) {
    // SAFETY: on the thread that just ran `gtk_init`, before anything else
    // has read the locale it set.
    unsafe { libc::setlocale(libc::LC_ALL, locale.as_ptr()) };
}

/// Let GTK draw what [`step`] changed. Bounded by both a count and a time,
/// because this is called on the launch's critical path and must never turn
/// into waiting.
fn pump() {
    let ctx = gtk::glib::MainContext::default();
    let until = std::time::Instant::now() + std::time::Duration::from_millis(16);
    for _ in 0..32 {
        if !ctx.iteration(false) || std::time::Instant::now() > until {
            break;
        }
    }
}

/// The icon from the theme when it is installed there, and otherwise from the
/// copy compiled into this binary. A hand-run `cordial-run` and an AppImage
/// that has not installed its desktop entry have no icon in the theme, and
/// GTK's stand-in for a missing one is a broken-image glyph in the middle of
/// the splash.
fn icon() -> gtk::Image {
    const SVG: &[u8] = include_bytes!(
        "../../../packaging/icons/hicolor/scalable/apps/io.github.damnshabu.Stacked.svg"
    );
    let installed = gtk::gdk::Display::default()
        .is_some_and(|d| gtk::IconTheme::for_display(&d).has_icon(crate::branding::ICON));
    if installed {
        return gtk::Image::from_icon_name(crate::branding::ICON);
    }
    match gtk::gdk::Texture::from_bytes(&gtk::glib::Bytes::from_static(SVG)) {
        Ok(texture) => gtk::Image::from_paintable(Some(&texture)),
        Err(e) => {
            println!("  splash: no icon ({e})");
            gtk::Image::new()
        }
    }
}

fn build() -> Splash {
    let icon = icon();
    icon.set_pixel_size(88);
    icon.add_css_class("stacked-splash-icon");

    let name = gtk::Label::new(Some(crate::branding::NAME));
    name.add_css_class("stacked-splash-name");

    let status = gtk::Label::new(Some("Starting"));
    status.add_css_class("stacked-splash-status");
    status.set_ellipsize(gtk::pango::EllipsizeMode::End);

    let bar = gtk::ProgressBar::new();
    bar.add_css_class("stacked-splash-bar");
    bar.set_hexpand(true);

    let version = gtk::Label::new(Some(&crate::version::full()));
    version.add_css_class("stacked-splash-version");

    let column = gtk::Box::new(gtk::Orientation::Vertical, 10);
    column.append(&icon);
    column.append(&name);
    column.append(&bar);
    column.append(&status);
    column.append(&version);
    column.add_css_class("stacked-splash-card");

    let window = adw::Window::builder()
        .title(crate::branding::NAME)
        .default_width(380)
        .resizable(false)
        .decorated(false)
        .content(&column)
        .build();
    window.add_css_class("stacked-splash");

    let css = gtk::CssProvider::new();
    css.load_from_string(&sheet(crate::theme::Theme::from_env()));
    gtk::style_context_add_provider_for_display(
        &WidgetExt::display(&window),
        &css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    Splash {
        window,
        bar,
        status,
    }
}

/// The splash's own sheet: its layout always, the brand's colours only under
/// the Stacked theme, so `theme = system` gets a splash in the desktop's
/// colours like the rest of the window.
fn sheet(theme: crate::theme::Theme) -> String {
    let mut s = String::from(
        // The window itself is transparent and the card inside it is
        // rounded: an undecorated window loses libadwaita's own rounding, and
        // a rounded window background left square black corners behind it.
        ".stacked-splash { background: none; } \
         .stacked-splash-card { background-color: @window_bg_color; border-radius: 12px; \
             padding: 28px 32px 20px; } \
         .stacked-splash-name { font-size: 1.5em; font-weight: 700; } \
         .stacked-splash-status { font-size: 0.9em; opacity: 0.8; } \
         .stacked-splash-version { font-size: 0.75em; opacity: 0.5; } \
         .stacked-splash-bar trough, .stacked-splash-bar progress { min-height: 6px; border-radius: 3px; }",
    );
    if theme == crate::theme::Theme::Stacked {
        use crate::theme::palette::*;
        s.push_str(&format!(
            " .stacked-splash-card {{ background-color: {BASE}; color: {TEXT}; \
                 box-shadow: inset 0 0 0 1px {MUTED}; }} \
             .stacked-splash-bar trough {{ background-color: alpha({MUTED}, 0.45); }} \
             .stacked-splash-bar progress {{ background-color: {ACCENT}; }}"
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_explicit_off_skips_the_splash() {
        for off in ["off", "0", "false", "no", " OFF "] {
            assert!(disabled_by(Some(off)), "{off:?} should turn it off");
        }
        for on in [None, Some(""), Some("on"), Some("1"), Some("of")] {
            assert!(!disabled_by(on), "{on:?} should leave it on");
        }
    }

    #[test]
    fn the_system_theme_gets_the_layout_without_the_brand() {
        let system = sheet(crate::theme::Theme::System);
        let stacked = sheet(crate::theme::Theme::Stacked);
        assert!(!system.contains(crate::theme::palette::ACCENT));
        assert!(stacked.contains(crate::theme::palette::ACCENT));
        assert!(stacked.starts_with(&system));
    }
}
