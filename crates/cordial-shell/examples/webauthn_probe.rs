//! Open one web window through [`cordial_shell::webview::open`] and let it
//! report what the page got.
//!
//! Why this exists rather than a note in a document: the question "does this
//! machine's WebKitGTK do WebAuthn" is only answerable by asking a live page,
//! and the only web window Cordial ever opens is the one the engine asks for
//! through `openNativeOverlay` — which needs a signed-in client and a click,
//! neither of which an agent may produce here (AGENTS.md's caution on
//! synthesised input, and `docs/analysis/webview-surface.md` §6's "nobody has
//! observed a web-view request under Cordial"). So the capability line printed
//! by `open` was unobservable, on the exact code path where it matters most.
//!
//! This drives that same `open` — same `NetworkSession`, same
//! `UserContentManager`, same policy — with a URL given on the command line,
//! and quits. It signs in to nothing, types nothing, and stores nothing.
//!
//!     cargo run --release --features webview --example webauthn_probe \
//!         -- https://www.roblox.com/login
//!
//! The URL must pass `webview_policy::evaluate`, which means HTTPS on the
//! default port with no userinfo; anything else is refused before a view is
//! built, which is the policy working rather than the example failing.

use libadwaita::prelude::*;

/// Long enough for a real page to finish loading and short enough that a
/// forgotten run does not sit on the profile lock. `open` prints its answer on
/// the first `LoadEvent::Finished`, so this only has to outlast one load.
const SECONDS_BEFORE_QUIT: u32 = 25;

fn main() -> libadwaita::glib::ExitCode {
    let url = std::env::args().nth(1).unwrap_or_else(|| "https://www.roblox.com/login".to_string());

    // A distinct application id from the shell's. Sharing `io.github.damnshabu.Stacked`
    // would make this a second invocation of a single-instance application —
    // it would hand its arguments to a running Cordial and exit without ever
    // opening a view, which reads as the probe silently doing nothing.
    let app = libadwaita::Application::builder()
        .application_id("io.github.damnshabu.Stacked.WebAuthnProbe")
        .build();

    app.connect_activate(move |app| {
        let window = libadwaita::ApplicationWindow::builder()
            .application(app)
            .default_width(900)
            .default_height(700)
            .build();
        window.set_content(Some(&libadwaita::StatusPage::builder().title("WebAuthn probe").build()));
        window.present();

        // The dialog is deliberately dropped: `open` returns a handle so a
        // later `closeWindow` can find it, and this example has no protocol to
        // serve. The dialog stays presented on its parent regardless.
        if cordial_shell::webview::open(&window, &cordial_shell::webview::WindowRequest {
            url: url.clone(),
            show_domain_as_title: true,
            ..Default::default()
        })
        .is_none()
        {
            eprintln!("[probe] the policy refused that URL; nothing was opened");
        }

        let app = app.clone();
        libadwaita::glib::timeout_add_seconds_local_once(SECONDS_BEFORE_QUIT, move || app.quit());
    });

    // No arguments handed to GTK: the URL above is ours, and GApplication would
    // otherwise try to parse it as an option and fail.
    app.run_with_args::<&str>(&[])
}
