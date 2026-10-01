//! The in-experience web window, as a dialog attached to the client.
//!
//! Roblox keeps a browser inside itself. Account settings, buying Robux, the
//! sign-in flow and anything else the client does not render natively all open
//! one of these. Without it those buttons do nothing at all — no window, no
//! error, no log line.
//!
//! Informed by mocktail's implementation, Copyright 2026 komaruworld,
//! Apache-2.0 — see `NOTICE` and `third_party/mocktail-webview/`. The security
//! rules are theirs and live in [`crate::webview_policy`]; this file is Cordial's
//! own window and does not share their structure. Apache-2.0 section 4(b): a
//! derived work, saying so.
//!
//! ## Why a dialog rather than a window
//!
//! Because the protocol says so, once you read what it asks for. The engine
//! exports `getHideHeaderKey`, `getShowDomainAsTitleKey` and
//! `getBackButtonVisibleKey` — that is chrome for a view embedded in an app with
//! a header bar that can be suppressed, not for a browser window. On Android
//! these are in-app overlays and never separate tasks.
//!
//! An `AdwDialog` is the same thing on this desktop: attached to the client
//! window, adaptive, and dismissed by the gesture every other GNOME sheet uses.
//! A second top-level would land wherever the compositor decided, which this
//! project has already fought once over the engine canvas.
//!
//! **This choice held up, but needed a partner fix to actually be visible.**
//! An `AdwDialog` "draws inside its parent's own surface" — true, and still
//! true below — but that surface is the same `wl_surface` the engine's own
//! canvas is a `wl_subsurface` of, per ADR-011, and a subsurface's default
//! stacking is *above* its parent. So a dialog opened here rendered correctly
//! from the moment this file was written, and was invisible from the same
//! moment, painted into a surface the engine was compositing over every
//! frame — reported as "the whole window goes white/blank", which reads as
//! this window failing to open at all rather than as a stacking order one
//! layer up from it. `crates/cordial-runtime/src/android/wayland.rs`'s
//! `WaylandWindow::webview_dialog_opened`/`webview_dialog_closed` is the fix:
//! it lowers the engine's subsurface behind `parent_surface` for as long as a
//! dialog from [`open`] is up. Nothing in *this* file changed for it, because
//! the stacking is entirely a property of the engine's own subsurface, which
//! this crate has no handle to — see that module's own doc for the mechanism
//! and why a nested-compositor screenshot missed it the first time.
//!
//! ## Why in-process, when mocktail's helper is not
//!
//! Checked directly against `webview_helper_launcher.cc` before writing this,
//! because the filename is a real architectural claim and deserved reading
//! rather than assuming. What it shows: mocktail's helper is a second
//! `posix_spawn`ed binary talking to the first over a `SOCK_SEQPACKET`
//! control channel it hand-rolls (`MOCKTAIL-WEBVIEW 1`, `MWVC`/`MWVE` framed
//! packets) — real process isolation, built because mocktail's engine process
//! is C++ over SDL3 and carries **no GTK at all**. Adding a browser to that
//! process would mean linking GTK, WebKitGTK and libadwaita into a process
//! that currently has none of them, for a feature the engine process itself
//! never needs to touch.
//!
//! That reasoning does not transfer. [ADR-011](../../../docs/adr/ADR-011-wayland-and-libadwaita.md)
//! already put GTK4 and libadwaita in Cordial's engine process, unconditionally,
//! for a reason that has nothing to do with web views: the engine's `wl_surface`
//! is a `wl_subsurface` of the GTK toplevel, and a Wayland subsurface cannot
//! parent across a process boundary, so "one connection, therefore one
//! process" was decided before this module existed. Cordial does not choose
//! between one GTK process and none — it is already one GTK process, and a
//! `WebKitWebView` in that process is a widget, not a second toolkit.
//! `docs/analysis/webview-surface.md` §5 reaches the same conclusion by
//! reading the WebKitGTK API surface rather than mocktail's launcher, and is
//! the fuller writeup; this file's summary and that document should not be
//! allowed to drift apart.
//!
//! What actually isolates page content — the process boundary mocktail's
//! helper buys by being a separate binary — Cordial gets for free from
//! **WebKitGTK's own multi-process model**: a `WebKitWebView` is a widget, and
//! the page itself runs in `WebKitWebProcess` and `WebKitNetworkProcess` under
//! WebKit's own sandbox, maintained by people who do it full time. A
//! hand-rolled helper would add a second IPC layer on top of that for
//! isolation already present, and it would cost the attached dialog, because a
//! separate process cannot present one — `AdwDialog` is not a second
//! `xdg_toplevel` to begin with; it is drawn inside its parent's own surface
//! (libadwaita's dialog host, not a second `GdkSurface`), so there is no
//! second Wayland connection here for a helper process to avoid fighting over.
//!
//! **What would change this**, stated the way ADR-011 states its own
//! reversal condition: if WebKit's own process crashes started taking Cordial
//! down with them, that would be a measurement, not a prediction, and nothing
//! here has measured it — recorded in `docs/analysis/webview-surface.md` §5
//! and repeated here so it is not lost if this file is read on its own.
//!
//! What is *not* skipped, and is the one thing mocktail's shape and this one
//! agree on without qualification: **every bridge message is origin-checked**.
//! This window is where a user signs in and where payment happens, so a page
//! able to post arbitrary commands at the engine is the whole security
//! boundary. Every navigation goes through [`crate::webview_policy::evaluate`]
//! before it is allowed, and — see [`open`]'s bridge handler below — every
//! bridge message is checked again, against the page's *current* address, not
//! the one it was granted the bridge for; a page can navigate itself
//! unprivileged after loading privileged, and mocktail's own
//! `IsPrivilegedBridgeAllowed` re-reads `webkit_web_view_get_uri` for exactly
//! that reason. This file does the same, at the same point.
//!
//! **A popup is not exempt from any of this.** Until this file also answered
//! WebKit's `create` signal (see [`install_popup_handling`]), a page that
//! opened a second window with `window.open()` got one with no bridge, no
//! navigation policy and no log line -- which is this module's own opening
//! sentence, describing a gap in this file rather than in the engine. Every
//! popup now gets the identical registration, policy check and origin
//! re-check a top-level window does, via the same functions [`open`] itself
//! calls.

use adw::prelude::*;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::glib;
use webkit6::prelude::*;

use crate::webview_policy;

/// The two message-handler names mocktail's bridge registers, kept identical
/// here so a page written against either app's bridge finds the same handler
/// name present (whether it gets an answer is the origin check, not this).
/// Named in `third_party/mocktail-webview/webview_helper_policy.h`, itself
/// citing `kExecuteRobloxHandler`/`kRobloxWkHybridHandler`.
const BRIDGE_EXECUTE_ROBLOX: &str = "executeRoblox";
const BRIDGE_ROBLOX_WK_HYBRID: &str = "RobloxWKHybrid";

/// A third handler name, ours alone -- never claimed to be part of Roblox's
/// own contract. Registered and wired only when [`bridge_probe_enabled`], so
/// [`bridge_shim`]'s probe instrumentation has somewhere to report what the
/// page actually touches.
const BRIDGE_PROBE: &str = "cordialBridgeProbe";

/// `CORDIAL_WEBVIEW_BRIDGE_PROBE=1` -- instrument the page's own JS instead of
/// guessing at its shape from outside.
///
/// Written for issue #40's open question after `CORDIAL_TRACE_BRIDGE`
/// confirmed the Servers-list Join button posts to neither registered
/// handler at all: something upstream of the bridge decides not to call it,
/// and nothing so far said what. This does three things, all reporting
/// through [`BRIDGE_PROBE`] rather than through
/// `enable-write-console-messages-to-stdout`, because
/// `CORDIAL_WEBVIEW_CONSOLE_LOG` was tried first and produced nothing at all
/// on this build, even on page load -- which the postMessage channel is
/// already proven to survive (`Load More`, scroll and close all round-trip
/// through it in the same sessions):
///
/// 1. `window.onerror`/`unhandledrejection` listeners, so a thrown handler is
///    visible instead of read as "the button did nothing".
/// 2. `window.webkit.messageHandlers` wrapped in a `Proxy` whose `get`/`has`
///    traps report every property name the page asks for -- including one
///    that comes back `undefined`, which names a handler this file never
///    registered exactly as precisely as one that did.
/// 3. `window.__globalRobloxAndroidBridge__`'s own object wrapped the same
///    way, so a probe for `.getVersion` or similar sibling method reports
///    itself instead of silently returning `undefined`.
///
/// Off by default: this reports the page's own property accesses, which is
/// exactly the kind of behavioural detail `CORDIAL_TRACE_BRIDGE`'s doc already
/// argues should not be on unless asked for.
fn bridge_probe_enabled() -> bool {
    std::env::var_os("CORDIAL_WEBVIEW_BRIDGE_PROBE").is_some()
}

/// A fourth handler name, for [`hybrid_launch_enabled`]'s own status
/// reporting -- independent of [`BRIDGE_PROBE`] because this ships on by
/// default and must report even with every diagnostic switch off.
const HYBRID_LAUNCH_LOG: &str = "cordialHybridLaunchLog";

/// The escape hatch. On by default since issue #40/#34's fix (below); set
/// this to turn it back off rather than the other way around, because the
/// default is now the behaviour, not the diagnostic.
const HYBRID_LAUNCH_DISABLE_VAR: &str = "CORDIAL_WEBVIEW_DISABLE_HYBRID_LAUNCH";

/// The default behaviour for issue #40/#34: a game's Servers-list Join button
/// did nothing, because it calls `Roblox.Hybrid.Game.launchGame(payloadJson,
/// callback)` -- a real, page-defined contract with a structured payload
/// (`placeId`, `instanceId`, `joinAttemptId`, `joinAttemptOrigin`,
/// `browserTrackerId`, `requestType`) -- and nothing native ever received it,
/// because `Roblox.Hybrid` is the *page's own* object; grepping this whole
/// file confirms nothing here ever creates or seeds `window.Roblox` in any
/// form, so there was no native layer underneath it for the call to reach.
/// This installs one: [`bridge_shim`]'s injected script wraps `launchGame` in
/// place, so a call still runs the page's own original implementation (its
/// callback, its analytics, its promise resolution all unchanged) and
/// *additionally* posts the same JSON payload through `executeRoblox` -- the
/// bridge this file already owns and already wires end-to-end -- where
/// `cordial_runtime::webview::forward_bridge_message` recognises the shape
/// and hands it to `cordial_runtime::deeplink`'s live publish path instead of
/// the engine's own `signalJavascriptCallback` (see that function's own doc
/// for why: this payload does not match the "Hybrid Module" JSON-RPC shape
/// that channel expects, so forwarding it there too would not do anything).
///
/// `StartGameParams` (`docs/analysis/app-bridge.md`) was **not** needed:
/// `deeplink.rs`'s existing `Linking.detectURL` publish already carries a
/// place id into a real join, and it needed only the instance and a few more
/// query fields added to carry a specific server too. See
/// `deeplink::publish_hybrid_game_launch` for what is carried and what is
/// dropped.
///
/// Installed defensively, not assumed present at document-start: the website
/// defines `Roblox`, and `Roblox.Hybrid`, and `Roblox.Hybrid.Game`, none of
/// which exist yet when this shim runs. So this watches for each to appear
/// (a `get`/`set` pair) and wraps `launchGame` the moment `Game` is real. If
/// it never appears at all, nothing is wrapped and [`HYBRID_LAUNCH_LOG`]
/// says so -- there is no second guess here, only the one call shape
/// measured against a real, signed-in Join click.
///
/// Verified 5/5 on the diagnosed build (behind the switch this default
/// replaces) and 3/3 again after the probes around it were trimmed, on two
/// public games from the Servers list, each landing in the exact requested
/// `instanceId` per the engine's own join log -- not just the right place.
/// Play from home and Play from a game page were re-checked both times and
/// are unaffected, because neither ever goes through a `WebView`. Private
/// servers remain untested: none reachable from the test account without a
/// purchase or subscription, both of which are out of scope here.
fn hybrid_launch_enabled() -> bool {
    std::env::var_os(HYBRID_LAUNCH_DISABLE_VAR).is_none()
}

/// Where a policy-approved bridge message goes once it has passed
/// [`webview_policy::bridge_message_acceptable`] against the page's *current*
/// address.
///
/// This crate cannot call into `cordial_runtime` directly to reach the
/// engine's JNI natives — `cordial-runtime` depends on `cordial-shell` for
/// `host_window`, not the reverse, and adding the opposite edge would be a
/// cycle. The presenter side of this same problem (`cordial_runtime::webview
/// ::set_presenter`) solved it with a callback installed once at startup;
/// this is that shape, in the other direction. `load.rs`'s
/// `install_webview_presenter` installs a sink that forwards straight to
/// `cordial_runtime::webview::forward_bridge_message`, which is where the
/// actual `WebViewProtocol.signalJavascriptCallback` call lives — see that
/// function's doc for the native, its declared signature, and what remains
/// unestablished about what happens on the engine's side of it.
type BridgeSink = dyn Fn(&str) + Send + Sync;
static BRIDGE_SINK: std::sync::OnceLock<std::sync::Arc<BridgeSink>> = std::sync::OnceLock::new();

/// The dialog most recently presented by [`open`], so a close that did not
/// originate from the user's own click on it -- the engine asking on
/// `close-window`, or `cordial_runtime::webview` deciding a join has actually
/// started -- has something to call `.close()` on. `None` once that dialog's
/// own `closed` signal has fired, by whichever path.
///
/// GTK objects are `Rc`-refcounted, not atomically, so this may only be
/// touched from the thread that calls [`open`] -- the GTK main thread, the
/// same one `install_webview_presenter` already re-enters via
/// `glib::MainContext::default().invoke` before doing anything else with a
/// dialog. That single-thread invariant is the whole justification for the
/// `unsafe impl` below, the same shape
/// `cordial_runtime::android::wayland::HostWindowCell` already uses for the
/// identical reason.
struct CurrentDialogCell(std::cell::RefCell<Option<adw::Dialog>>);
// SAFETY: see the doc comment above -- GTK-main-thread-only by construction.
unsafe impl Send for CurrentDialogCell {}
unsafe impl Sync for CurrentDialogCell {}
static CURRENT_DIALOG: CurrentDialogCell = CurrentDialogCell(std::cell::RefCell::new(None));

/// Close the currently-open web window, the same way a user dismissing it
/// would, so everything already wired to `AdwDialog`'s own `closed` signal --
/// `load.rs`'s canvas-raise and its `report_window_closed` call to the engine
/// -- still runs. Callers do not need a second, parallel "tell the engine it
/// closed" step; that is the entire point of going through the same signal
/// rather than reaching for a lower-level teardown.
///
/// Must be called from the GTK main thread, same as [`open`] itself --
/// callers re-enter it via `glib::MainContext::default().invoke` first.
///
/// Returns `false`, and says why, both when there was nothing open to close
/// and when `AdwDialog::close` itself declined the request -- either is worth
/// seeing rather than folding into a single silent no-op, the discipline this
/// whole module holds itself to for every other refusal.
pub fn close_current() -> bool {
    let dialog = CURRENT_DIALOG.0.borrow().clone();
    match dialog {
        Some(dialog) => {
            let accepted = dialog.close();
            if !accepted {
                eprintln!("[webview] close_current: the open dialog did not accept the close request");
            }
            accepted
        }
        None => {
            eprintln!("[webview] close_current: no web window is currently open");
            false
        }
    }
}

#[derive(Clone, Copy)]
enum BridgeHandler {
    ExecuteRoblox,
    RobloxWkHybrid,
}

impl BridgeHandler {
    /// The registered handler name this variant answers for, so a rejection
    /// log can say which of the two contracts a malformed message arrived
    /// on without printing the message itself.
    fn name(self) -> &'static str {
        match self {
            BridgeHandler::ExecuteRoblox => BRIDGE_EXECUTE_ROBLOX,
            BridgeHandler::RobloxWkHybrid => BRIDGE_ROBLOX_WK_HYBRID,
        }
    }
}

/// A one-word JS type name for `value`, for the rejection log below -- never
/// the value itself, which may be whatever the page and the engine are
/// mid-conversation about. `object` covers the case a page posted an object
/// with no `command` string property, which reads identically to an
/// `is_object()` failure in the log unless something distinguishes them; this
/// does not distinguish that further because doing so would mean walking the
/// object's own properties, and this function's whole job is to say what
/// arrived without looking at what it says.
fn describe_value_shape(value: &webkit6::javascriptcore::Value) -> &'static str {
    if value.is_undefined() {
        "undefined"
    } else if value.is_null() {
        "null"
    } else if value.is_boolean() {
        "a boolean"
    } else if value.is_number() {
        "a number"
    } else if value.is_string() {
        "a string"
    } else if value.is_array() {
        "an array"
    } else if value.is_function() {
        "a function"
    } else if value.is_object() {
        "an object"
    } else {
        "an unrecognised JS value"
    }
}

fn forward_script_message(
    view: &webkit6::WebView,
    value: &webkit6::javascriptcore::Value,
    handler: BridgeHandler,
) {
    // Match mocktail's two distinct handler contracts. executeRoblox posts an
    // object whose complete JSON representation is the command; RobloxWKHybrid
    // posts an envelope and the native receives only its string `command`.
    // Treating both signals as the former was subtle: both handlers existed,
    // the callback ran, and the engine still received the wrong bytes for Join.
    let command = match handler {
        BridgeHandler::ExecuteRoblox if value.is_object() => {
            value.to_json(0).map(|s| s.to_string())
        }
        BridgeHandler::RobloxWkHybrid if value.is_object() => value
            .object_get_property("command")
            .filter(|property| property.is_string())
            .and_then(|property| property.to_string_as_bytes())
            .and_then(|bytes| std::str::from_utf8(bytes.as_ref()).ok().map(str::to_owned)),
        _ => None,
    };
    // Named by handler and shaped by JS type, rather than a bare "malformed":
    // this is the one place that distinguishes "a message arrived on this
    // handler but was not the shape expected" from the silence a page
    // opening a *second* window produces instead (see
    // `install_popup_handling`'s doc) -- two failure modes that otherwise
    // both read as "pressing the button did nothing". The type name is
    // logged, never the value -- a JS type says nothing about what the page
    // and the engine were mid-conversation about.
    let Some(command) = command.filter(|command| !command.is_empty()) else {
        eprintln!(
            "[webview] rejected a malformed {} bridge command (arrived as {})",
            handler.name(),
            describe_value_shape(value),
        );
        return;
    };

    let current_uri = view.uri().map(|u| u.to_string()).unwrap_or_default();
    let verdict = webview_policy::evaluate(&current_uri);
    if let Err(reason) = webview_policy::bridge_message_acceptable(&verdict, command.len()) {
        eprintln!("[webview] rejected a bridge message (host {}): {reason}", verdict.host);
        return;
    }
    match BRIDGE_SINK.get() {
        Some(sink) => sink(&command),
        None => eprintln!(
            "[webview] a bridge message passed policy (host {}, {} bytes), but no sink is \
             installed to forward it -- see set_bridge_sink's doc",
            verdict.host,
            command.len()
        ),
    }
}

/// Install the sink [`open`]'s bridge handler forwards an approved message
/// to. Only the first call takes effect — see `cordial_runtime::webview::
/// set_presenter`'s doc for why two installations racing is a bug worth
/// seeing rather than one that resolves itself silently.
pub fn set_bridge_sink(f: impl Fn(&str) + Send + Sync + 'static) {
    if BRIDGE_SINK.set(std::sync::Arc::new(f)).is_err() {
        eprintln!("[webview] set_bridge_sink called twice; keeping the first sink installed");
    }
}

/// How the engine asked for the window to look.
///
/// Field names deliberately match the protocol's own keys — `hideHeader`,
/// `showDomainAsTitle` — because the engine's `getHideHeaderKey()` and friends
/// are what fill them, and a rename here would make the correspondence something
/// a reader has to work out.
#[derive(Debug, Clone, Default)]
pub struct WindowRequest {
    pub url: String,
    pub title: Option<String>,
    pub hide_header: bool,
    pub show_domain_as_title: bool,
    pub back_button_visible: bool,
    /// A validated `.ROBLOSECURITY` value to seed this window's cookie jar
    /// with before the first load, or `None` to open signed out.
    ///
    /// Deliberately not something this module fetches for itself. The runtime
    /// owns the live cookie jar; the shared secret backend only knows its saved
    /// form. `cordial_runtime::webview::extract_roblosecurity` does
    /// the validation (RFC 6265 `cookie-octet`, a length bound) that has to
    /// happen before a value reaches a `Set-Cookie` header; this field only
    /// carries the already-checked result.
    pub roblox_session_cookie: Option<String>,
    /// The `User-Agent` this window's `WebKitWebView` should send, or `None`
    /// to leave WebKitGTK's own default in place.
    ///
    /// Why this exists at all: with no User-Agent set, WebKitGTK sends its
    /// own, and roblox.com — reading a browser UA rather than the app's own
    /// — serves the full desktop site, complete with the site's own
    /// navigation bar (Search, Charts, Marketplace, Create, Robux, avatar,
    /// notifications) stacked above whatever page the engine asked this
    /// window to open. On Android the same URL renders the embedded in-app
    /// layout, because the app sends its own User-Agent and the site branches
    /// on it. Cordial already computes that string for the engine's own HTTP
    /// client — see `InitParams.userAgent`, built by `native/init_params.cpp`'s
    /// `build_user_agent` — and `cordial_runtime::webview::user_agent` hands
    /// the identical bytes back out, so this field is never a second,
    /// independently-typed copy of that string. Whether the site's own
    /// navigation bar actually disappears once this is set is the thing this
    /// change expects but a maintainer with a live window needs to confirm —
    /// see this crate's caller for what was and was not run.
    pub user_agent: Option<String>,
}

/// Open a web window for `request`, attached to `parent`.
///
/// Returns the dialog so a later `closeWindow` or `mutateWindow` can find it
/// again; the protocol has both and neither can be served by a window that was
/// opened and forgotten.
///
/// A request whose URL the policy refuses opens nothing and says why. Refusing
/// loudly matters more here than elsewhere: silence is the failure mode this
/// whole module exists to end, and "the window did not open" must never again be
/// indistinguishable from "nobody was listening".
pub fn open(parent: &(impl IsA<gtk4::Widget> + Clone), request: &WindowRequest) -> Option<adw::Dialog> {
    let policy = webview_policy::evaluate(&request.url);
    if !policy.allowed {
        eprintln!(
            "[webview] refused to open {} (scheme {}, host {})",
            if request.url.len() > 120 { "a very long address" } else { &request.url },
            policy.scheme,
            policy.host,
        );
        return None;
    }

    // Before anything WebKit can spawn from. WebKitGTK answers a helper it
    // cannot start with `g_error`, which took the whole client down in v0.21.3
    // when the AppImage's helpers were not where its library looked -- see
    // `webkit_helpers`. Run on every open rather than once, because a /tmp
    // cleaner can remove the staged copies under a client that has been up for
    // days, and putting them back costs a few small file reads.
    if let Err(reason) = helpers_ready() {
        eprintln!("[webview] cannot open a web window: {reason}");
        return None;
    }

    // Ephemeral, deliberately: a `NetworkSession` built without a data
    // directory keeps its cookie jar in memory and writes nothing to disk.
    // ADR-012 already made Cordial's session store the desktop secret
    // service rather than a file; a `WebKitWebsiteDataManager`'s own cookie
    // jar on top of that would be a second copy of the same secret, on disk,
    // that this project spent a whole ADR getting *out* of a file. A fresh
    // session per `open()` call also settles the per-profile requirement
    // without any extra bookkeeping: Cordial runs one profile per process
    // (ADR-012's lock), so there is never a second window in this process for
    // one session to leak into.
    let network_session = webkit6::NetworkSession::new_ephemeral();
    if let Some(cookie_value) = &request.roblox_session_cookie {
        if let Some(cookie_manager) = network_session.cookie_manager() {
            let mut cookie =
                webkit6::soup::Cookie::new(".ROBLOSECURITY", cookie_value, ".roblox.com", "/", -1);
            cookie.set_secure(true);
            cookie.set_http_only(true);
            // The callback reports failure only, and reports it as a fact
            // with no value in it -- the one thing worth knowing here is
            // whether the session made it into the view, never what it was.
            cookie_manager.add_cookie(&cookie, gtk4::gio::Cancellable::NONE, |result| {
                if let Err(e) = result {
                    eprintln!("[webview] could not seed the session cookie: {e}");
                }
            });
        }
    }

    // Registered unconditionally, whatever `policy` said about the opening
    // url. What matters for the bridge is not the address this window was
    // asked to open with, but the address a message actually arrived from --
    // see the module doc's closing paragraph and mocktail's
    // `IsPrivilegedBridgeAllowed`, which this mirrors. A page that starts
    // privileged and navigates itself away must lose the bridge; a page that
    // starts unprivileged and is somehow later on a Roblox host still may not
    // get it retroactively without this being checked again at the point of
    // use, which is exactly what happens below.
    //
    // Done through [`register_bridge`] rather than inline, because a page
    // that opens a second window from inside this one -- `window.open()`, or
    // a `target="_blank"` link -- needs the identical registration on that
    // window too. See [`install_popup_handling`]'s doc for the gap that used
    // to leave a popup with none of this at all.
    let user_content = webkit6::UserContentManager::new();
    register_bridge(&user_content);

    // With no User-Agent set, WebKitGTK sends its own, and roblox.com reads
    // that as an ordinary desktop browser rather than the app it is -- and
    // serves the full desktop site, complete with the site's own navigation
    // bar (Search, Charts, Marketplace, Create, Robux, avatar, notifications)
    // stacked above whatever page the engine actually asked this window to
    // open. See `WindowRequest::user_agent`'s own doc for where the string
    // comes from; `webkit_settings_set_user_agent` (via `Settings::builder`)
    // is the API this crate has for the equivalent of Android's
    // `WebSettings.setUserAgentString`. Built as its own `Settings` and
    // handed to the builder below, rather than fetched from the view and
    // mutated afterwards, so the very first navigation -- `load_uri`, at the
    // end of this function -- already carries it; nothing here has
    // established whether an in-flight load's headers can be changed after
    // the fact, and there is no reason to find out when the ordering below
    // never needs to.
    let settings = webkit6::Settings::new();
    if let Some(ua) = &request.user_agent {
        settings.set_user_agent(Some(ua));
    }
    if console_log_enabled() {
        settings.set_enable_write_console_messages_to_stdout(true);
    }

    let view = webkit6::WebView::builder()
        .network_session(&network_session)
        .user_content_manager(&user_content)
        .settings(&settings)
        .hexpand(true)
        .vexpand(true)
        .build();

    // **Corrected**: this used to say nothing here forwards to the engine,
    // with the missing receiver left as follow-up work. That silence is what
    // made "pressing Join navigates to the game's detail page instead of
    // joining" look like a WebKit bug rather than what it was -- the page's
    // JS calling `executeRoblox.postMessage`/`RobloxWKHybrid`, getting no
    // answer, and falling back to plain navigation. [`set_bridge_sink`]
    // above is the receiver; `load.rs`'s `install_webview_presenter` wires it
    // to `cordial_runtime::webview::forward_bridge_message`, which calls
    // `WebViewProtocol.signalJavascriptCallback` -- see that function's own
    // doc for the native and what calling it does and does not establish.
    // AGENTS.md's rule against a stub that lies is still the reason a
    // message that fails policy, or arrives with no sink installed, is
    // reported rather than silently dropped: a page that believes its
    // command was delivered when nothing on this end received it is exactly
    // the lie that rule exists to rule out.
    wire_bridge_messages(&user_content, &view);

    // The policy is applied again on every navigation, not just the first. A
    // page that is allowed to load may redirect, and the address that matters
    // for the bridge is wherever it ended up rather than where it started.
    install_navigation_policy(&view);
    install_nav_probe(&view);

    // **A page opening a second window got none of the above at all, until
    // now.** `window.open()` and a `target="_blank"` link both fire WebKit's
    // `create` signal, and an unanswered `create` returns nothing by
    // WebKitGTK's own default -- no view, no error, nothing reaching this
    // process to log. See [`install_popup_handling`]'s doc for why this was
    // never exercised: the only window this module used to build was the one
    // `openWindow` itself asked for, and nothing inside that page had ever
    // been observed to open a second one.
    // `Clone::clone(parent)`, not `parent.clone()` -- `parent` is already a
    // reference, and `&T` is unconditionally `Clone` in its own right (a
    // trivial pointer copy), so `.clone()` on it resolves to *that* impl
    // before ever considering `T`'s. The explicit trait form pins `Self = T`
    // from the argument type instead, giving the owned widget `upcast` needs.
    install_popup_handling(&view, Clone::clone(parent).upcast(), request.user_agent.clone());

    // Whether this WebKitGTK build has WebAuthn at all, asked of a live page,
    // once per process. A Roblox account with a passkey enrolled cannot finish
    // sign-in without `navigator.credentials.get`, and when the binding is
    // absent the site's passkey button throws `undefined is not an object`
    // somewhere inside minified JS, in a window with no console and no error
    // path back to Cordial -- the silent shape this whole module exists to end.
    //
    // Asked rather than read off the library, because `ENABLE_WEB_AUTHN` is a
    // property of whichever libwebkitgtk the machine happens to carry and
    // upstream's default answers only for upstream. What was measured, and the
    // reason it is not a passing detail, is in
    // `docs/analysis/webview-surface.md` section 9.
    //
    // Nothing is polyfilled here and nothing should be. A shim standing in for
    // `navigator.credentials.get` would be a stub that lies: the page would
    // carry on believing an authenticator had been asked and had refused,
    // when nothing was asked at all. Reporting the gap leaves it where
    // somebody can find it, which is the whole of `native/opensles.cpp`'s
    // reasoning applied one layer up.
    view.connect_load_changed(|v, event| {
        if event != webkit6::LoadEvent::Finished {
            return;
        }
        static PROBED: std::sync::Once = std::sync::Once::new();
        PROBED.call_once(|| {
            // Two type names and a boolean, and deliberately nothing else. A
            // bare `typeof` cannot carry a session cookie, a one-time ticket
            // or an address, which matters because the page this runs on is
            // usually the sign-in page.
            const PROBE: &str = "[typeof PublicKeyCredential, typeof navigator.credentials, \
                String(window.isSecureContext)].join(' ')";
            // Run in a named script world rather than the page's own. A world
            // sees the real IDL bindings but not properties the page defined,
            // so `window.PublicKeyCredential = function () {}` in the page
            // cannot talk this diagnostic into reporting a capability the
            // build does not have. The distinction costs one argument and the
            // page in question is a sign-in page.
            const WORLD: &str = "cordial-capability-probe";
            v.evaluate_javascript(PROBE, Some(WORLD), None, gtk4::gio::Cancellable::NONE, |r| {
                match r {
                    Ok(value) => {
                        let answer = value.to_str();
                        if answer.starts_with("undefined") {
                            eprintln!(
                                "[webview] this WebKitGTK build has no WebAuthn \
                                 (PublicKeyCredential/navigator.credentials/isSecureContext = \
                                 {answer}) -- a passkey sign-in cannot complete in this window; \
                                 see docs/analysis/webview-surface.md section 9"
                            );
                        } else {
                            eprintln!("[webview] WebAuthn is present in this build ({answer})");
                        }
                    }
                    Err(e) => eprintln!("[webview] could not ask the page about WebAuthn: {e}"),
                }
            });

            // **`CORDIAL_WEBVIEW_BRIDGE_TEST=1`: make the page call the bridge,
            // so the JS-to-engine direction can be seen working.**
            //
            // Everything else about this window is observable from outside --
            // the dialog opens or it does not, the page renders or it does not.
            // The bridge is the one part that is invisible until a real Roblox
            // page decides to use it, which needs signed-in UI and a click
            // AGENTS.md's rule against synthesising input rules out. So there
            // was no way to tell a working bridge from a broken one except by
            // waiting for somebody to press Join and report back.
            //
            // This skips the page's own decision to call the bridge and skips
            // nothing else: the message goes through the same handler, the same
            // origin and size policy, the same sink and the same
            // `signalJavascriptCallback`. A run with this set prints
            // `forwarded a bridge message` from `cordial_runtime::webview` if
            // and only if the whole chain works.
            //
            // The page's own world, deliberately, unlike the probe above: the
            // bridge global is injected there and a named world would not see
            // it -- which would make this report "absent" on a perfectly
            // healthy build.
            if std::env::var_os("CORDIAL_WEBVIEW_BRIDGE_TEST").is_some() {
                const SELF_TEST: &str = r#"(() => {
  const bridge = window.__globalRobloxAndroidBridge__;
  if (!bridge || typeof bridge.executeRoblox !== "function") { return "no bridge on this page"; }
  bridge.executeRoblox(JSON.stringify({ cordial: "bridge-self-test" }));
  return "called";
})()"#;
                v.evaluate_javascript(
                    SELF_TEST,
                    None,
                    None,
                    gtk4::gio::Cancellable::NONE,
                    |r| match r {
                        Ok(value) => eprintln!(
                            "[webview] bridge self-test: {} (a `forwarded a bridge message` line \
                             below is the round trip; its absence is the finding)",
                            value.to_str()
                        ),
                        Err(e) => eprintln!("[webview] bridge self-test could not run: {e}"),
                    },
                );
            }

            // **`CORDIAL_WEBVIEW_CLICK_PROBE=1`: fire a script-driven `.click()`
            // on the Servers list's own Join control, for issue #40.**
            //
            // `CORDIAL_TRACE_BRIDGE` and `CORDIAL_WEBVIEW_BRIDGE_PROBE` together
            // established, on a real signed-in click, that Join produces no
            // bridge message, no probe of `window.webkit.messageHandlers` or
            // `window.__globalRobloxAndroidBridge__` under any property name,
            // and no thrown error -- the page's own handler, whatever it is,
            // never touches any of the three things this file can see. That
            // is consistent with either "the handler is bound to a touch event
            // our synthetic pointer never sends" or "Join was never going to
            // call this bridge at all". This distinguishes the first case from
            // the second, one specific way: a real DOM `.click()` fires
            // `click` listeners but not `touch*` ones, so if THIS produces a
            // bridge message where a hardware-driven pointer click never did,
            // the gap is event-binding, not the page's own intent.
            //
            // Scoped to a leaf node whose own text is exactly "Join" AND whose
            // ancestry (within 6 levels) also carries "of N people max" --
            // text that only appears on a server-row card. A bare `/join/i`
            // search would also match the home page's own promotional "Join"
            // button on an unrelated experience banner and a paid
            // subscription's own "Join" control on a monetised page; both are
            // web content this same code path renders, and clicking either
            // for a diagnostic would be the exact unwanted side effect this
            // scoping exists to rule out. Off by default, and only ever
            // dispatches a `click`, never a purchase or a form submission.
            // Runs on the WebView's own `load-changed == Finished`, which is
            // the initial document load -- but the Servers list's own rows
            // arrive later, from an async fetch the popup makes after that
            // (measured directly: the popup shows its own "Loading" text at
            // `Finished` time, and the rows are not there yet). A first
            // version of this returned a Promise from an `async` IIFE meaning
            // to have `evaluate_javascript` await it; measured directly, this
            // WebKitGTK binding does not support that and reports "Unsupported
            // result type" instead of waiting. So the wait is a plain
            // Rust-side retry instead: five attempts, 1.5s apart, each a
            // synchronous, fire-and-forget find-and-click. `attempts_left`
            // only gates how many *more* ticks fire after one that found
            // nothing; a click, once sent, is not retried.
            if std::env::var_os("CORDIAL_WEBVIEW_CLICK_PROBE").is_some() {
                const CLICK_PROBE: &str = r#"(() => {
  const candidates = Array.from(document.querySelectorAll("*")).filter((el) => {
    return el.children.length === 0 && (el.textContent || "").trim() === "Join";
  });
  for (const el of candidates) {
    let node = el, onCard = false;
    for (let i = 0; i < 6 && node; i++, node = node.parentElement) {
      if (/of\s+\d+\s+people max/i.test(node.textContent || "")) { onCard = true; break; }
    }
    if (onCard) {
      el.click();
      return "clicked a server-row Join element";
    }
  }
  return `no server-row Join element found yet (${candidates.length} bare "Join" text node(s) on the page)`;
})()"#;
                // Retries are scheduled from *inside* the completion callback,
                // once this attempt's result is known -- scheduling them
                // right after firing `evaluate_javascript` (which is
                // fire-and-forget) would race its own async result and could
                // schedule a follow-up attempt that clicks Join a second time
                // after the first attempt already succeeded.
                fn tick(v: webkit6::WebView, attempts_left: u32) {
                    v.evaluate_javascript(CLICK_PROBE, None, None, gtk4::gio::Cancellable::NONE, {
                        let v = v.clone();
                        move |r| {
                            let found = matches!(&r, Ok(value) if value.to_str().starts_with("clicked"));
                            match &r {
                                Ok(value) => eprintln!(
                                    "[webview] click probe (CORDIAL_WEBVIEW_CLICK_PROBE=1): {}",
                                    value.to_str()
                                ),
                                Err(e) => eprintln!("[webview] click probe could not run: {e}"),
                            }
                            if found || r.is_err() {
                                return;
                            }
                            if attempts_left == 0 {
                                eprintln!(
                                    "[webview] click probe: giving up after 5 attempts over ~7.5s"
                                );
                                return;
                            }
                            glib::timeout_add_local_once(
                                std::time::Duration::from_millis(1500),
                                move || tick(v, attempts_left - 1),
                            );
                        }
                    });
                }
                tick(v.clone(), 4);
            }
        });
    });

    let header = adw::HeaderBar::new();
    // `showDomainAsTitle` exists because a user in a payment flow needs to be
    // able to see who they are actually talking to. When the engine asks for it,
    // the host wins over whatever title the page would like to call itself.
    let title = if request.show_domain_as_title {
        policy.host.clone()
    } else {
        request.title.clone().unwrap_or_else(|| policy.host.clone())
    };
    header.set_title_widget(Some(&adw::WindowTitle::new(&title, "")));
    header.set_visible(!request.hide_header);
    header.set_show_start_title_buttons(request.back_button_visible);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&view));

    let dialog = adw::Dialog::new();
    dialog.set_child(Some(&toolbar));
    dialog.set_content_width(900);
    dialog.set_content_height(700);
    dialog.set_title(&title);

    view.load_uri(&request.url);
    dialog.present(Some(parent));

    // Tracked for [`close_current`], and untracked again the moment this
    // dialog's own `closed` signal fires -- by any path, not only that
    // function's -- so a stale handle here never outlives the window it
    // names. Guarded by identity, not just by clearing unconditionally: a
    // second `open()` call that presented a newer dialog before this one
    // got around to closing must not have its entry erased by the older
    // dialog's own belated signal.
    *CURRENT_DIALOG.0.borrow_mut() = Some(dialog.clone());
    let tracked = dialog.clone();
    dialog.connect_closed(move |_| {
        let mut slot = CURRENT_DIALOG.0.borrow_mut();
        if slot.as_ref() == Some(&tracked) {
            *slot = None;
        }
    });

    Some(dialog)
}

/// The script every view this module builds needs, so `window.__globalRobloxAndroidBridge__`
/// is present in a popup exactly as it is in [`open`]'s own top-level window.
///
/// **The global the page actually looks for is `__globalRobloxAndroidBridge__`.**
///
/// `register_script_message_handler` (in [`register_bridge`]) exposes the handler as
/// `window.webkit.messageHandlers.executeRoblox.postMessage(...)`, which is
/// WebKitGTK's only shape. The page never touches that directly. It looks
/// for a global object holding an `executeRoblox` **function that takes a
/// JSON string**, and calls it as
///
/// ```text
/// window.__globalRobloxAndroidBridge__.executeRoblox(jsonString)
/// ```
///
/// Two earlier attempts missed for different reasons and both are worth
/// recording. The first shipped the message handler alone and nothing ever
/// called it, which read as a bridge failing to deliver when in fact the
/// page had concluded it was not inside an app and used an ordinary link --
/// reported as "pressing Join opens the game's detail page instead of
/// joining". The second guessed the Android `addJavascriptInterface` shape,
/// `window.executeRoblox.someMethod(args)`, and was wrong in both the name
/// and the calling convention: it is one function taking a string, not an
/// object with methods.
///
/// The shape is adapted from mocktail (`src/webview/webview_helper_policy.cc`,
/// Apache-2.0), which is the third implementation this project has learned
/// the platform's own vocabulary from rather than inferring it. Guessing at
/// a JS calling convention is exactly as unreliable as guessing at a JNI
/// descriptor, and this file now has two instances of each.
///
/// `writable: false, configurable: false` so a page-defined object can never
/// replace the native bridge, and the whole thing is a no-op when the
/// handler is absent rather than installing a global that silently swallows
/// calls. Policy is still re-checked against the live uri on every message,
/// because this necessarily runs in the page's own world.
///
/// **Never log the payload** -- it carries whatever the page and the engine
/// are mid-conversation about.
/// The early return below (`if (!handler) return;`) never retries, which
/// looks like a race waiting to happen: if `window.webkit.messageHandlers`
/// were not populated when a document-start script runs, the bridge would
/// silently never install.
///
/// **It cannot happen, and the reason is structural rather than lucky.**
/// `WebKitWebView`'s `user-content-manager` property is construct-only, so
/// a manager cannot be attached after the view exists -- handlers and
/// scripts are necessarily registered before there is any document to run
/// them in. Checked in `WebKit-6.0.gir`; this file's own ordering
/// (handlers, then script, then `WebView::builder()`, then `load_uri`)
/// satisfies it regardless.
///
/// Recorded because it is an appealing theory that costs a day: it explains
/// an intermittent symptom neatly and is wrong.
fn bridge_shim() -> String {
    // Assembled and injected **before** `probe_prelude` below. The two used
    // to fight over `window.Roblox` when both were on -- `BRIDGE_PROBE` once
    // installed its own `get`/`set` pair on the same property -- but that
    // wrapping is gone (it found the `Roblox.Hybrid.Game.launchGame` contract
    // this file now implements directly, and keeping two independent
    // watchers on one page object after the contract was known was a trap
    // for the next person to touch this file, not a diagnostic worth
    // shipping). `probe_prelude` no longer touches `window.Roblox` at all,
    // so the ordering here is no longer load-bearing between the two; kept
    // as the injection order regardless, since there is no reason to change
    // it now that it does not matter.
    let hybrid_launch_prelude = if hybrid_launch_enabled() {
        format!(
            r#"
  const __cordialHybridLog = (text) => {{
    try {{
      const h = window.webkit && window.webkit.messageHandlers && window.webkit.messageHandlers.{log};
      if (h) h.postMessage(text);
    }} catch (_) {{ /* the fix must never be the reason the page breaks */ }}
  }};
  let __cordialHybridArmed = false;
  const __cordialTryIntercept = (hybrid) => {{
    try {{
      if (!hybrid || typeof hybrid !== "object") {{ return false; }}
      const game = hybrid.Game;
      if (!game || typeof game.launchGame !== "function") {{ return false; }}
      if (game.launchGame.__cordialIntercepted) {{ __cordialHybridArmed = true; return true; }}
      const original = game.launchGame;
      // Runs the page's own `launchGame` exactly as before -- its callback,
      // its analytics, its own promise resolution -- and additionally posts
      // the identical JSON payload through `executeRoblox`, the bridge this
      // file already owns end-to-end. This is Cordial implementing the layer
      // the page is already calling, not a patch to the page's own code: the
      // original function is never removed, only wrapped.
      const intercepted = function(payload, callback) {{
        try {{
          const bridge = window.__globalRobloxAndroidBridge__;
          // **Measured, not assumed:** round 6's own capture logged this
          // argument as a JSON-quoted string, which looked like proof it
          // always arrives pre-stringified -- but that capture went through
          // the diagnostic probe's own `__cordialDescribeArg`, which calls
          // `JSON.stringify` on anything that is not *already* a string, so
          // an object argument and a string argument were indistinguishable
          // in that log. A live call here, checked with a direct `typeof`,
          // arrived as a plain object, not a string. `executeRoblox` needs a
          // string (its own implementation calls `JSON.parse` on it), so
          // both shapes are handled rather than assuming either.
          let json = null;
          if (typeof payload === "string") {{
            json = payload;
          }} else if (payload && typeof payload === "object") {{
            try {{ json = JSON.stringify(payload); }} catch (_) {{ json = null; }}
          }}
          if (bridge && typeof bridge.executeRoblox === "function" && json !== null) {{
            bridge.executeRoblox(json);
          }} else {{
            __cordialHybridLog(
              "launchGame called but the bridge was not callable (bridge=" + typeof bridge +
              ", executeRoblox=" + (bridge && typeof bridge.executeRoblox) +
              ", payload=" + typeof payload + ")"
            );
          }}
        }} catch (e) {{
          // Logged, not swallowed silently -- AGENTS.md's rule against a stub
          // that lies applies here exactly: a call that looks like it
          // reached the bridge but actually threw must not read the same as
          // one that succeeded.
          __cordialHybridLog("launchGame's own bridge post threw: " + String(e && e.message));
        }}
        return original.apply(this, arguments);
      }};
      intercepted.__cordialIntercepted = true;
      game.launchGame = intercepted;
      __cordialHybridArmed = true;
      __cordialHybridLog("intercepted Roblox.Hybrid.Game.launchGame");
      return true;
    }} catch (e) {{
      __cordialHybridLog("could not intercept Roblox.Hybrid.Game.launchGame: " + String(e && e.message));
      return false;
    }}
  }};
  // `Hybrid` on the `Roblox` object gets the identical present-or-watch
  // treatment `Roblox` itself gets below, one level in: the website assigns
  // `Roblox` as one object literal with `Hybrid` already inside it in every
  // run measured so far, so the immediate check almost always succeeds --
  // but "almost always" is exactly the case this defends against, not the
  // common one.
  //
  // **Every backing store below is a closure-captured variable, never
  // `this.<something>`.** Measured directly on 2026-09-18: a first version
  // stored the watched value on `this` inside the getter (`this.
  // __cordialRobloxValue`), and `bundleVerifier.js` -- the same script round
  // 5 already found sensitive to exactly this kind of accessor -- calls the
  // installed getter with a receiver that is not `window`, or with none at
  // all, throwing `TypeError: undefined is not an object (evaluating
  // 'this.__cordialRobloxValue')` from inside the getter every time it did.
  // The diagnostic probe's own equivalent code never had this bug because it
  // takes a different shape on write (it replaces the accessor with a plain
  // data property immediately, so there is no getter left for anything to
  // call oddly); this needs the accessor to persist so a *later* read still
  // reports through it, so the fix is to stop depending on `this` at all.
  const __cordialArmHybrid = (roblox) => {{
    if (!roblox || typeof roblox !== "object") {{ return; }}
    if (__cordialTryIntercept(roblox.Hybrid)) {{ return; }}
    try {{
      const hdesc = Object.getOwnPropertyDescriptor(roblox, "Hybrid");
      if (!hdesc || hdesc.configurable) {{
        let hybridValue = hdesc ? hdesc.value : undefined;
        Object.defineProperty(roblox, "Hybrid", {{
          configurable: true,
          get() {{ return hybridValue; }},
          set(v) {{ hybridValue = v; __cordialTryIntercept(v); }},
        }});
      }}
    }} catch (e) {{
      __cordialHybridLog("could not watch Roblox.Hybrid for later assignment: " + String(e && e.message));
    }}
  }};
  try {{
    const rdesc = Object.getOwnPropertyDescriptor(window, "Roblox");
    if (rdesc && "value" in rdesc && rdesc.value) {{
      __cordialArmHybrid(rdesc.value);
    }} else if (!rdesc) {{
      let robloxValue;
      Object.defineProperty(window, "Roblox", {{
        configurable: true,
        get() {{ return robloxValue; }},
        set(v) {{ robloxValue = v; __cordialArmHybrid(v); }},
      }});
    }} else {{
      __cordialHybridLog("window.Roblox already has its own accessor; not watching it");
    }}
  }} catch (e) {{
    __cordialHybridLog("could not watch window.Roblox: " + String(e && e.message));
  }}
  setTimeout(() => {{
    if (!__cordialHybridArmed) {{
      __cordialHybridLog("Roblox.Hybrid.Game.launchGame never appeared within 5s; not installed");
    }}
  }}, 5000);
"#,
            log = HYBRID_LAUNCH_LOG,
        )
    } else {
        String::new()
    };
    // The probe adds a `post` helper and wraps `window.webkit.messageHandlers`
    // itself, so it has to run before the plain lookup below, and both live in
    // one script -- two separate `UserScript`s at the same injection point run
    // in registration order, but keeping the read-then-wrap sequencing in one
    // string is the only way to be sure the wrap happens first without relying
    // on that ordering guarantee. See [`bridge_probe_enabled`]'s doc for why
    // this exists and what it reports.
    let probe_prelude = if bridge_probe_enabled() {
        format!(
            r#"
  const __cordialProbePost = (kind, detail) => {{
    try {{
      const h = window.webkit && window.webkit.messageHandlers && window.webkit.messageHandlers.{probe};
      if (h) h.postMessage(JSON.stringify({{kind, detail}}));
    }} catch (_) {{ /* the probe must never be the thing that breaks the page */ }}
  }};
  window.addEventListener("error", (e) => {{
    __cordialProbePost("window.onerror", {{
      message: String(e.message), filename: String(e.filename),
      lineno: e.lineno, colno: e.colno,
      stack: (e.error && e.error.stack) ? String(e.error.stack) : null,
    }});
  }});
  window.addEventListener("unhandledrejection", (e) => {{
    __cordialProbePost("unhandledrejection", {{
      reason: (e.reason && e.reason.stack) ? String(e.reason.stack) : String(e.reason),
    }});
  }});
  // `window.webkit.messageHandlers` is WebKit's own readonly IDL attribute,
  // not a plain data property -- reassigning it while "use strict" is active
  // (the whole shim runs strict, from the IIFE's own top line) throws a
  // TypeError, uncaught, right here, before a single line below this block
  // ever runs. Measured directly on 2026-09-18: with no try/catch, the very
  // first probe message on every popup was a same-origin-scrubbed
  // `{{"message":"Script error.","filename":"","lineno":0,"colno":0}}` --
  // the signature WebKit gives an exception thrown inside a UserScript's own
  // isolated world -- and the shim's remaining lines, including the bridge
  // install below, never executed at all. A follow-up Join click on that same
  // popup then produced `free(): corrupted unsorted chunks` in Cordial's own
  // log and the client stopped answering its devctl socket. Whether that
  // second failure is this throw or WebKitGTK's own handling of a Proxy over
  // a native host object is not established -- but an uncaught throw here is
  // reason enough on its own to guard it, since it was silently disabling the
  // bridge this switch exists to diagnose. `write` on the descriptor is
  // checked first so the common case (a build where this rejects) never
  // pays for a throw/catch at all.
  if (window.webkit && window.webkit.messageHandlers) {{
    const desc = Object.getOwnPropertyDescriptor(window.webkit, "messageHandlers");
    const replaceable = !desc || desc.writable || typeof desc.set === "function";
    if (replaceable) {{
      try {{
        window.webkit.messageHandlers = new Proxy(window.webkit.messageHandlers, {{
          get(target, prop, receiver) {{
            const value = Reflect.get(target, prop, receiver);
            if (prop !== "{probe}") {{
              __cordialProbePost("messageHandlers.get", {{ prop: String(prop), present: value !== undefined }});
            }}
            return value;
          }},
          has(target, prop) {{
            __cordialProbePost("messageHandlers.has", {{ prop: String(prop) }});
            return Reflect.has(target, prop);
          }},
        }});
      }} catch (e) {{
        __cordialProbePost("messageHandlers.wrap-failed", {{ message: String(e && e.message) }});
      }}
    }} else {{
      __cordialProbePost("messageHandlers.wrap-skipped", {{ reason: "not configurable/writable on this build" }});
    }}
  }}
  // Widened for issue #40 after `executeRoblox`/`RobloxWKHybrid` and
  // `__globalRobloxAndroidBridge__` all came back silent on a real Join
  // click: the analytics beacon Join fires
  // (`evt=playGameClicked`/`gamePlayIntent`, then
  // `privateServerJoin_Success`) proves the page believes it handed the join
  // off *successfully*, which rules out a dropped call and points at a
  // contract this file had never named at the time. That contract is now
  // known and implemented directly by [`hybrid_launch_enabled`]
  // (`Roblox.Hybrid.Game.launchGame`) -- found by watching `window.Roblox`
  // itself here, one level deep, until the page's own call to it was
  // visible. That `Roblox`/`Hybrid`/`Game`-specific wrapping is gone from
  // this probe now that the contract it found is shipped as the fix: keeping
  // two independent watchers on one page object, with the silent-conflict
  // wart the two used to have (see [`bridge_shim`]'s own doc), would be a
  // trap for the next person to read this file rather than a diagnostic
  // worth keeping. What is still here is the generic candidate-name sweep
  // below, which answers a different, still-open question -- whether the
  // page ever reaches for some *other* native-sounding global this file has
  // not yet named.
  //
  // Every name below is watched two ways: a name currently absent gets a
  // logging accessor installed in its place (a read is reported instead of
  // silently coming back `undefined`, which is the whole point -- a page
  // probing for a name Cordial never registered would otherwise look
  // identical to one not probing at all); a name already present gets
  // wrapped in a `Proxy` the same guarded way `messageHandlers` is above,
  // so reads of *its* properties are visible too.
  {{
    const CANDIDATES = [
      "RobloxWKHybrid", "AndroidBridge", "JSBridge", "ReactNativeWebView",
      "AndroidInterface", "NativeInterface", "WebViewJavascriptBridge",
      "Bridge", "external",
    ];
    // **The getter below must carry a setter, and this is not optional.**
    // Measured directly on 2026-09-18: a first version installed a bare
    // `get()` with no `set()` on every currently-absent name, including
    // `window.Roblox` below. `window.Roblox` is not a native candidate at
    // all -- it is the page's *own* namespace object, initialised by its own
    // bootstrap with something in the shape of `Roblox = Roblox || {{}}`. A
    // getter with no setter makes that assignment a silent no-op in
    // non-strict code (the property has no `[[Set]]`, so nothing is stored,
    // and the getter goes on returning `undefined` forever after). Every
    // later reference the page's own bootstrap makes to `Roblox.<anything>`
    // then throws `TypeError: undefined is not an object`, and the page dies
    // rendering a blank popup -- which round 4 first read as "the site
    // serves a broken bundle under an Android UA" before noticing the same
    // crash also now hit the default Windows identity, which round 3 had
    // exercised repeatedly with no such failure. The break was never the
    // page or the UA; it was this probe's own missing setter, installed one
    // round earlier. Every name below gets a real setter now: it replaces
    // the accessor with a plain, writable, configurable data property
    // holding whatever the page assigned, so a legitimate write behaves
    // exactly as it would with no probe installed at all, and is logged
    // rather than silently allowed to look identical to one that was not.
    for (const name of CANDIDATES) {{
      try {{
        const desc = Object.getOwnPropertyDescriptor(window, name);
        if (!desc) {{
          Object.defineProperty(window, name, {{
            configurable: true,
            get() {{
              __cordialProbePost("global.read", {{ name, present: false }});
              return undefined;
            }},
            set(v) {{
              __cordialProbePost("global.write", {{ name, type: typeof v }});
              Object.defineProperty(window, name, {{
                value: v, writable: true, configurable: true, enumerable: true,
              }});
            }},
          }});
        }} else {{
          __cordialProbePost("global.present", {{ name, type: typeof desc.value }});
        }}
      }} catch (e) {{
        __cordialProbePost("global.probe-failed", {{ name, message: String(e && e.message) }});
      }}
    }}
    // Free, per the switch's own doc: settles whether a user-agent gate
    // could be why Join never reaches for any of the above, without needing
    // to catch a read of it in the act.
    __cordialProbePost("navigator.userAgent", {{ value: String(navigator.userAgent) }});
  }}
"#,
            probe = BRIDGE_PROBE,
        )
    } else {
        String::new()
    };
    let bridge_object = if bridge_probe_enabled() {
        format!(
            r#"const bridgeImpl = {{}};
  Object.defineProperty(bridgeImpl, "{a}", {{
    value: (query) => handler.postMessage(JSON.parse(query)),
    enumerable: true, writable: false, configurable: false
  }});
  const bridge = new Proxy(bridgeImpl, {{
    get(target, prop, receiver) {{
      const value = Reflect.get(target, prop, receiver);
      __cordialProbePost("globalRobloxAndroidBridge.get", {{ prop: String(prop), present: value !== undefined }});
      return value;
    }},
    has(target, prop) {{
      __cordialProbePost("globalRobloxAndroidBridge.has", {{ prop: String(prop) }});
      return Reflect.has(target, prop);
    }},
  }});"#,
            a = BRIDGE_EXECUTE_ROBLOX,
        )
    } else {
        format!(
            r#"const bridge = {{}};
  Object.defineProperty(bridge, "{a}", {{
    value: (query) => handler.postMessage(JSON.parse(query)),
    enumerable: true, writable: false, configurable: false
  }});"#,
            a = BRIDGE_EXECUTE_ROBLOX,
        )
    };
    format!(
        r#"(() => {{
  "use strict";
{hybrid_launch_prelude}
{probe_prelude}
  const handlers = window.webkit && window.webkit.messageHandlers;
  const handler = handlers && handlers.{a};
  if (!handler) {{ return; }}
  {bridge_object}
  try {{
    Object.defineProperty(window, "__globalRobloxAndroidBridge__", {{
      value: bridge, enumerable: true, writable: false, configurable: false
    }});
  }} catch (_) {{ /* a page may not replace the native bridge */ }}
}})();"#,
        a = BRIDGE_EXECUTE_ROBLOX,
    )
}

/// [`crate::webkit_helpers::prepare`], with the staging directory said once per
/// process: it is the line that tells a reader of a log which WebKit helpers
/// this client was going to run.
fn helpers_ready() -> Result<(), String> {
    static SAID: std::sync::Once = std::sync::Once::new();
    match crate::webkit_helpers::prepare()? {
        crate::webkit_helpers::Helpers::Staged(dir) => SAID.call_once(|| {
            eprintln!("[webview] WebKit helper processes staged at {}", dir.display());
        }),
        crate::webkit_helpers::Helpers::System => {}
    }
    Ok(())
}

/// `CORDIAL_WEBVIEW_CONSOLE_LOG=1` -- ask WebKitGTK to write the page's own
/// `console.log`/`warn`/`error` to Cordial's stdout
/// (`enable-write-console-messages-to-stdout`).
///
/// Diagnostic only, for issue #40: if the Join button's own handler throws --
/// a missing global, a rejected promise -- the page's own console is the
/// fastest way to see that, faster than guessing at the bridge shape from
/// outside. Off by default because a page's console output is exactly the
/// kind of thing that can carry whatever the user was doing, unfiltered.
fn console_log_enabled() -> bool {
    std::env::var_os("CORDIAL_WEBVIEW_CONSOLE_LOG").is_some()
}

/// Register the two bridge handler names and inject [`bridge_shim`] into
/// `user_content`. Shared between [`open`]'s own top-level view and
/// [`create_popup_view`], because a popup with no bridge handlers registered
/// is a popup a Roblox sign-in or payment flow cannot talk to -- the same
/// silent gap this whole module exists to end, one level down.
fn register_bridge(user_content: &webkit6::UserContentManager) {
    for handler in [BRIDGE_EXECUTE_ROBLOX, BRIDGE_ROBLOX_WK_HYBRID] {
        if !user_content.register_script_message_handler(handler, None) {
            eprintln!("[webview] could not register the {handler} bridge handler");
        }
    }
    // Registered only under the probe switch: a handler nothing ever asks for
    // is harmless to leave registered, but there is no reason to hand a page
    // an extra name to enumerate when [`bridge_probe_enabled`] is off.
    if bridge_probe_enabled() && !user_content.register_script_message_handler(BRIDGE_PROBE, None) {
        eprintln!("[webview] could not register the {BRIDGE_PROBE} diagnostic handler");
    }
    // Independent of `bridge_probe_enabled()` -- see [`hybrid_launch_enabled`]'s
    // own doc for why the fix's own status reporting cannot depend on the
    // diagnostic probe being on.
    if hybrid_launch_enabled() && !user_content.register_script_message_handler(HYBRID_LAUNCH_LOG, None)
    {
        eprintln!("[webview] could not register the {HYBRID_LAUNCH_LOG} handler");
    }

    // **`TopFrame` is load-bearing, not a leftover.** It pairs with the origin
    // check in `forward_script_message`, and changing one without the other
    // opens a hole. Written down because the pairing is invisible from either
    // end and the obvious "fix" for a bridge-in-an-iframe bug is to flip this
    // constant.
    //
    // Measured on this machine with `cargo run -p cordial-shell --example
    // frame_scope_probe`, against a page with one same-origin `srcdoc` child:
    //
    //     TopFrame   -> ["top"]
    //     AllFrames  -> ["top", "nested"]
    //
    // So a nested frame does receive the script under `AllFrames`, and it can
    // reach `window.webkit.messageHandlers` -- the handler is registered per
    // script *world*, not per frame. `AllFrames` is in fact WebKitGTK's
    // documented default; this is a deliberate narrowing away from it.
    //
    // The narrowing is what makes the policy check correct. Messages arrive
    // through `script-message-received`, which does not say which frame sent
    // them, and `forward_script_message` therefore evaluates
    // `webview_policy::evaluate(view.uri())` -- **the top-level document's
    // address**. With `TopFrame` that is the sender's address by construction.
    // With `AllFrames` it would not be: any nested frame, including a
    // third-party one embedded in a roblox.com page, could post a bridge
    // command and have it judged against roblox.com rather than against
    // itself.
    //
    // So if the Join control ever does turn out to live in an iframe, the fix
    // is **not** one constant. It is `AllFrames` plus per-frame origin
    // attribution, and this API does not obviously offer the latter.
    //
    // **Ruled out, not just less likely.** `hybrid_launch_enabled`'s fix
    // watches `window.Roblox.Hybrid.Game.launchGame` in the top-level
    // document and it is that call the Join button reaches, directly, with
    // no bridge post and no message handler involved at all -- confirmed on
    // a real signed-in Join click, repeatedly. The Join control was never in
    // an iframe; it calls a plain JS method on the page's own top-level
    // object, which this `TopFrame` scoping was always going to see. Left in
    // place because the security reasoning above (a nested frame must never
    // be judged against the top document's address) holds regardless of this
    // one button, and `CORDIAL_WEBVIEW_BRIDGE_ALL_FRAMES` remains available
    // for the next `broken_feature` that does turn out to live in a frame.
    // `CORDIAL_WEBVIEW_BRIDGE_ALL_FRAMES=1` -- diagnostic only, per the doc
    // above: this widens injection to every frame with NO per-frame origin
    // attribution added, so a message from an iframe would still be judged
    // against the top-level address. That is not a fix to ship; it exists so
    // issue #40/#34's "does Join live in an iframe" question can be answered
    // by measurement rather than left as the less-favoured of two guesses.
    // If this makes Join's bridge message appear, the real fix is `AllFrames`
    // plus the origin attribution this API does not obviously offer -- not
    // this switch left on.
    let frames = if std::env::var_os("CORDIAL_WEBVIEW_BRIDGE_ALL_FRAMES").is_some() {
        eprintln!(
            "[webview] CORDIAL_WEBVIEW_BRIDGE_ALL_FRAMES=1: injecting the bridge shim into every \
             frame, diagnostic-only -- see register_bridge's doc before treating this as a fix"
        );
        webkit6::UserContentInjectedFrames::AllFrames
    } else {
        webkit6::UserContentInjectedFrames::TopFrame
    };
    user_content.add_script(&webkit6::UserScript::new(
        &bridge_shim(),
        frames,
        webkit6::UserScriptInjectionTime::Start,
        &[],
        &[],
    ));
}

/// Wire `script-message-received` on `user_content` to [`forward_script_message`]
/// for both handler names, reporting bridge messages that arrive on `view`.
/// Split out from [`register_bridge`] because this half needs `view` itself
/// (to know which page a message came from), which does not exist yet when
/// `register_bridge` has to run -- `user-content-manager` is a construct-only
/// property, so the manager and its handlers must be finished before
/// `WebView::builder().build()`, and this half can only run after it.
fn wire_bridge_messages(user_content: &webkit6::UserContentManager, view: &webkit6::WebView) {
    {
        let bridge_view = view.clone();
        user_content.connect_script_message_received(Some(BRIDGE_EXECUTE_ROBLOX), move |_manager, value| {
            forward_script_message(&bridge_view, value, BridgeHandler::ExecuteRoblox);
        });
    }
    {
        let bridge_view = view.clone();
        user_content.connect_script_message_received(Some(BRIDGE_ROBLOX_WK_HYBRID), move |_manager, value| {
            forward_script_message(&bridge_view, value, BridgeHandler::RobloxWkHybrid);
        });
    }
    // The probe's own channel. Printed directly and unconditionally once a
    // message arrives -- [`bridge_probe_enabled`] is already the opt-in, the
    // same way `CORDIAL_TRACE_BRIDGE` and `CORDIAL_WEBVIEW_CONSOLE_LOG` are
    // their own gates, so there is no second flag to check here.
    if bridge_probe_enabled() {
        user_content.connect_script_message_received(Some(BRIDGE_PROBE), move |_manager, value| {
            let text = value
                .is_string()
                .then(|| value.to_string_as_bytes())
                .flatten()
                .and_then(|bytes| std::str::from_utf8(bytes.as_ref()).ok().map(str::to_owned));
            match text {
                Some(text) => println!("[webview] bridge probe (CORDIAL_WEBVIEW_BRIDGE_PROBE=1): {text}"),
                None => eprintln!("[webview] bridge probe message arrived in an unexpected shape"),
            }
        });
    }
    // The fix's own status channel -- install succeeded, or `Roblox.Hybrid.Game`
    // never appeared at all. Independent of `bridge_probe_enabled()`, same
    // reasoning as its registration above.
    if hybrid_launch_enabled() {
        user_content.connect_script_message_received(Some(HYBRID_LAUNCH_LOG), move |_manager, value| {
            let text = value
                .is_string()
                .then(|| value.to_string_as_bytes())
                .flatten()
                .and_then(|bytes| std::str::from_utf8(bytes.as_ref()).ok().map(str::to_owned));
            match text {
                Some(text) => println!("[webview] hybrid launch: {text}"),
                None => eprintln!("[webview] hybrid launch status message arrived in an unexpected shape"),
            }
        });
    }
}

/// Approve or refuse a navigation, or a request to open a new window, against
/// [`webview_policy::evaluate`]. Wired onto every view this module builds --
/// [`open`]'s own top-level view and every popup [`create_popup_view`] makes
/// in answer to WebKit's `create` signal -- because a page that redirects
/// itself, or opens `window.open()` at an address the policy would have
/// refused as a first load, must be judged exactly as strictly the second
/// time.
///
/// **`NewWindowAction` used to go unanswered here.** `INFERRED`: WebKitGTK's
/// own documentation for `WebKitWebView::decide-policy` describes an
/// unhandled decision as defaulting to `webkit_policy_decision_use()`, which
/// would mean a blocked-scheme popup was already reaching `create` regardless
/// of what this function did with it -- but nothing in this session ran a
/// build to confirm that default rather than reading it off webkitgtk.org, so
/// treat it as documented rather than measured here. What the checked-in
/// change to this function does not depend on that either way:
/// [`install_popup_handling`] refuses inside `create` itself regardless, and
/// checking the type here too, with its own log line, is the same
/// belt-and-braces this file already applies to bridge messages -- re-checked
/// at the point of use rather than trusted from an earlier decision, because
/// "the window did not open" must never again be indistinguishable from
/// "nobody was listening".
fn install_navigation_policy(view: &webkit6::WebView) {
    view.connect_decide_policy(|_, decision, kind| {
        // `Response` decisions (whether to render a fetched response inline
        // or hand it off, e.g. as a download) were never even logged before
        // this branch -- the function fell straight through to `return
        // false` below, which is why a join implemented as "navigate to a
        // launcher URL and inspect what comes back" would have left no trace
        // at all. Reported under [`nav_probe_enabled`] only; the decision
        // itself (`false`, WebKitGTK's own documented default) is
        // unchanged -- this is visibility, not a new policy.
        if nav_probe_enabled() && kind == webkit6::PolicyDecisionType::Response {
            if let Some(resp) = decision.downcast_ref::<webkit6::ResponsePolicyDecision>() {
                let uri = resp
                    .response()
                    .and_then(|r| r.uri())
                    .map(|u| u.to_string())
                    .unwrap_or_default();
                let status = resp.response().map(|r| r.status_code());
                println!(
                    "[webview] nav probe (CORDIAL_WEBVIEW_NAV_PROBE=1): decide-policy Response \
                     uri={uri} status={status:?}"
                );
            }
            return false;
        }
        if kind != webkit6::PolicyDecisionType::NavigationAction
            && kind != webkit6::PolicyDecisionType::NewWindowAction
        {
            return false;
        }
        let Some(nav) = decision.downcast_ref::<webkit6::NavigationPolicyDecision>() else {
            return false;
        };
        let uri = nav
            .navigation_action()
            .and_then(|a| a.request())
            .and_then(|r| r.uri())
            .map(|u| u.to_string())
            .unwrap_or_default();
        let verdict = webview_policy::evaluate(&uri);
        if verdict.allowed {
            // The refused branch below has always logged unconditionally --
            // AGENTS.md's rule that a refusal must never look like silence.
            // An *allowed* navigation had no equivalent, so a join that
            // navigates somewhere Cordial's own policy happily accepts (its
            // own `roblox.com` origin, say, via a URL this file never
            // thought to name) would still look exactly like nothing
            // happened. Gated because most navigations are unremarkable and
            // logging all of them unconditionally would just be noise on
            // every ordinary page load.
            if nav_probe_enabled() {
                println!(
                    "[webview] nav probe (CORDIAL_WEBVIEW_NAV_PROBE=1): decide-policy {kind:?} \
                     ALLOWED uri={uri}"
                );
            }
            decision.use_();
        } else {
            let what = if kind == webkit6::PolicyDecisionType::NewWindowAction {
                "opening a new window"
            } else {
                "navigation"
            };
            eprintln!(
                "[webview] blocked {what} to scheme {} host {}",
                verdict.scheme, verdict.host
            );
            decision.ignore();
        }
        true
    });
}

/// `CORDIAL_WEBVIEW_NAV_PROBE=1` -- off by [`nav_probe_enabled`], its own
/// gate the same shape as [`bridge_probe_enabled`]'s.
///
/// Written for issue #40 after `CORDIAL_WEBVIEW_BRIDGE_PROBE` established, on
/// a real signed-in Join click, that the page reads no property on
/// `window.webkit.messageHandlers` or `window.__globalRobloxAndroidBridge__`
/// at all, and throws nothing. That rules out a bridge call under a
/// misspelled or unregistered handler name; it does not say where the click
/// goes instead. Two candidates remain, both invisible to the bridge probe by
/// construction because neither touches the bridge:
///
/// - **A navigation**, whether `window.location`, a `target="_blank"` link, or
///   a custom scheme such as `roblox://` or `roblox-player://` carrying the
///   launcher's query. `crates/cordial-runtime/src/deeplink.rs` already
///   refuses a deep link carrying `accessCode`, `linkCode`,
///   `reservedServerAccessCode`, `gameId` or `jobId`
///   (`docs/analysis/deep-links.md`) -- but that refusal is downstream of a
///   scheme actually reaching Cordial's own handling. If WebKit's own
///   `decide-policy` never fires for whatever scheme this is, or fires and
///   is refused by [`webview_policy::evaluate`] (which knows nothing about a
///   `roblox://`-shaped scheme and would refuse it exactly the way it
///   refuses anything non-`https`), the refusal is either invisible or is
///   the bug, and either way it explains total silence at the reporter's end
///   -- a refusal this file already logs unconditionally, so this switch is
///   what tells the two apart: no line at all (WebKit dropped it before
///   `decide-policy`) versus a `blocked navigation` line that was there all
///   along and nobody had a reason to grep for.
/// - **A network request** -- a join or launcher API call whose response
///   this file has never once inspected.
///
/// This does not change what Cordial *does* with any of the above, only what
/// it prints: every `load-changed` transition with the view's current URI,
/// and every subresource request's URI plus how it ended
/// (`finished`/`failed`, and the response status where WebKit hands one
/// back). [`install_navigation_policy`] carries the `decide-policy` half of
/// this, gated the same way, because the two signals are two branches of one
/// question and belong next to each other rather than duplicated here.
fn nav_probe_enabled() -> bool {
    std::env::var_os("CORDIAL_WEBVIEW_NAV_PROBE").is_some()
}

fn install_nav_probe(view: &webkit6::WebView) {
    if !nav_probe_enabled() {
        return;
    }
    // Confirms the premise [`install_scheme_probe`] relies on: that *this*
    // view answers to the same `WebContext` the scheme handlers are
    // registered on, so "no scheme request arrived" means the page never
    // tried one rather than that this file registered a handler on a
    // context nothing here actually uses. Compared by pointer identity
    // (`ToGlibPtr`) rather than trusting a language-level `==`, since
    // nothing in this file had previously checked whether `WebContext`
    // implements that meaningfully.
    match (view.web_context(), webkit6::WebContext::default()) {
        (Some(a), Some(b)) => {
            use glib::translate::ToGlibPtr;
            let same = ToGlibPtr::<*mut webkit6::ffi::WebKitWebContext>::to_glib_none(&a).0
                == ToGlibPtr::<*mut webkit6::ffi::WebKitWebContext>::to_glib_none(&b).0;
            println!(
                "[webview] nav probe (CORDIAL_WEBVIEW_NAV_PROBE=1): this view's WebContext is \
                 {} the default one the scheme handlers are registered on",
                if same { "the same as" } else { "DIFFERENT FROM" }
            );
        }
        (view_ctx, default_ctx) => println!(
            "[webview] nav probe (CORDIAL_WEBVIEW_NAV_PROBE=1): WebContext check inconclusive \
             (view has one: {}, default exists: {})",
            view_ctx.is_some(),
            default_ctx.is_some()
        ),
    }
    view.connect_load_changed(|v, event| {
        let uri = v.uri().map(|u| u.to_string()).unwrap_or_default();
        println!(
            "[webview] nav probe (CORDIAL_WEBVIEW_NAV_PROBE=1): load-changed {event:?} uri={uri}"
        );
    });
    // One subresource can fire many of these (a redirect chain, a retried
    // fetch), so each request gets its own `uri` captured into its own
    // `finished`/`failed` closures rather than re-reading `resource.uri()`
    // later -- WebKit reuses the same `WebResource` object across a
    // redirect and updates its `uri` property in place, which would report
    // the *final* address for a request that actually started somewhere
    // this file needed to see.
    view.connect_resource_load_started(|_v, resource, request| {
        let uri = request.uri().map(|u| u.to_string()).unwrap_or_default();
        println!(
            "[webview] nav probe (CORDIAL_WEBVIEW_NAV_PROBE=1): resource-load-started uri={uri}"
        );
        resource.connect_finished({
            let uri = uri.clone();
            move |r| {
                let status = r.response().map(|resp| resp.status_code());
                println!(
                    "[webview] nav probe (CORDIAL_WEBVIEW_NAV_PROBE=1): resource finished \
                     uri={uri} status={status:?}"
                );
            }
        });
        resource.connect_failed(move |_r, err| {
            println!(
                "[webview] nav probe (CORDIAL_WEBVIEW_NAV_PROBE=1): resource FAILED uri={uri} \
                 error={err}"
            );
        });
    });
    install_scheme_probe();
}

/// Register `roblox`/`roblox-player` as handled URI schemes on the process's
/// **default** `WebContext` -- every `WebView` this module builds uses it,
/// since none of them is constructed with a `web-context` of its own -- so a
/// scheme WebKit would otherwise refuse before it ever reaches a signal this
/// file can see instead arrives here, in full, as a
/// `WebKitURISchemeRequest`.
///
/// Written for the coordinator's own reading of round 4: `navigator.userAgent`
/// came back as the **desktop** string (`ROBLOX Windows App ... Desktop`),
/// because `native/init_params.cpp`'s `device_identity()` defaults to
/// `PcWindows11` and nothing in this session had set
/// `CORDIAL_DEVICE_PROFILE`. The desktop roblox.com does not call a
/// JavaScript bridge to join at all -- it hands off to the installed Roblox
/// Player through a protocol-handler URL, historically `roblox-player://`
/// carrying `placeLauncherUrl`, and reports success to its own analytics
/// immediately, which is exactly the `privateServerJoin_Success` beacon round
/// 3 measured with no bridge read anywhere near it. `decide-policy` on the
/// main frame saw nothing because the attempt, if there is one, plausibly
/// never becomes a main-frame `NavigationAction` at all -- a hidden iframe's
/// `src`, or a scheme WebKit does not recognise being resolved before
/// `create`/`decide-policy` ever fire for it. Registering the scheme
/// ourselves is what makes either case arrive somewhere loggable regardless.
///
/// `finish_error` on every request, immediately: there is nothing to load --
/// this scheme has no real content on Cordial, and the request exists only
/// to be observed. Leaving it unfinished would leave whatever made the
/// request (an iframe, most likely) waiting indefinitely.
///
/// One-time per process, not per-view: `register_uri_scheme` operates on the
/// `WebContext`, which is shared, and registering the same scheme twice is
/// at best redundant and at worst WebKitGTK's own guess at what "the second
/// registration wins" means -- untested here, so avoided outright.
fn install_scheme_probe() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let Some(ctx) = webkit6::WebContext::default() else {
            eprintln!(
                "[webview] nav probe (CORDIAL_WEBVIEW_NAV_PROBE=1): no default WebContext, \
                 cannot register roblox/roblox-player scheme handlers"
            );
            return;
        };
        for scheme in ["roblox", "roblox-player"] {
            ctx.register_uri_scheme(scheme, |request| {
                let uri = request.uri().map(|u| u.to_string()).unwrap_or_default();
                println!(
                    "[webview] nav probe (CORDIAL_WEBVIEW_NAV_PROBE=1): scheme request uri={uri}"
                );
                let mut err = glib::Error::new(
                    gtk4::gio::IOErrorEnum::Failed,
                    "Cordial's nav probe observed this request; it is not actually handled",
                );
                request.finish_error(&mut err);
            });
        }
        println!(
            "[webview] nav probe (CORDIAL_WEBVIEW_NAV_PROBE=1): registered scheme handlers for \
             roblox, roblox-player"
        );
    });
}

/// Answer WebKit's `create` signal on `view`, fired when a page inside it
/// calls `window.open()` or follows a `target="_blank"` link.
///
/// **Nothing answered this before, and that is a real gap rather than a
/// theory.** This module's own opening line says what an unanswered Android
/// expectation looks like: "no window, no error, no log line". An unanswered
/// `create` is WebKitGTK's own equivalent -- the default return is nothing,
/// so a page that opens a window this way gets exactly that: no view is
/// built, nothing is logged, and the click that triggered it is
/// indistinguishable from one that did nothing at all. That was a plausible
/// account of issue #40 ("pressing Join in the Servers list does nothing")
/// once, weighed against the iframe hypothesis [`register_bridge`] records --
/// **both are now ruled out.** `hybrid_launch_enabled`'s fix established
/// what Join actually does: it calls `Roblox.Hybrid.Game.launchGame` as a
/// plain method on the page's own top-level object, no second window and no
/// nested frame involved. This `create`-signal gap is still worth having
/// fixed -- a page opening a real popup (sign-in, a payment step) needs one
/// with the same bridge and policy a top-level window gets, which is what
/// this function is for -- it is simply no longer issue #40/#34's own cause.
///
/// Modelled on mocktail's `OnCreatePopup`/`CreateSurface`
/// (`third_party/mocktail-webview/mocktail_webview_helper.cc`, Apache-2.0),
/// which wires `create`, `ready-to-show` and `close` together and reuses its
/// own view-construction function for a popup rather than building one with
/// less on it than a top-level window gets -- the same reuse
/// [`create_popup_view`] does here via [`register_bridge`],
/// [`wire_bridge_messages`] and [`install_navigation_policy`].
///
/// `chrome_parent` is the widget [`open`] itself presents its dialog
/// against, threaded through so a popup presents alongside it. `AdwDialog`
/// has no transient-for requirement of its own -- libadwaita already stacks
/// multiple dialogs presented against the same parent correctly, which is
/// simpler than mocktail's own `gtk_window_set_transient_for` bookkeeping and
/// needs none of it because Cordial's dialogs were never separate
/// `GtkWindow`s to begin with (see this module's own doc on why).
fn install_popup_handling(view: &webkit6::WebView, chrome_parent: gtk4::Widget, user_agent: Option<String>) {
    view.connect_create(move |parent_view, action| {
        let uri = action
            .request()
            .and_then(|r| r.uri())
            .map(|u| u.to_string())
            .unwrap_or_default();
        let verdict = webview_policy::evaluate(&uri);
        eprintln!(
            "[webview] the page asked to open a new window (host {}, scheme {}); {}",
            verdict.host,
            verdict.scheme,
            if verdict.allowed { "creating one" } else { "refusing" },
        );
        if !verdict.allowed {
            return None;
        }
        Some(create_popup_view(parent_view, chrome_parent.clone(), user_agent.clone()).upcast())
    });
}

/// Build the WebKitWebView, dialog chrome and bridge for one popup a page
/// opened with `window.open()`, and present it once WebKit says it is ready.
///
/// `related_view` rather than a fresh `NetworkSession` -- unlike [`open`]'s
/// own ephemeral session, a popup must share the parent's cookie jar to be
/// usable for anything Roblox would open one for (a sign-in step, a payment
/// confirmation). mocktail's `CreateSurface` sets a new view up the same way:
/// `related-view` when a related view exists, `network-session` only when it
/// does not, never both, and asserts afterwards that the result carries the
/// same `NetworkSession` as the app's one shared view in either case -- which
/// is this crate's evidence that `related-view` inherits the session rather
/// than a WebKitGTK API guarantee read off documentation. This crate's own
/// `open()` creates a fresh ephemeral session specifically because Cordial
/// only ever has one web window in flight at a time (see
/// `cordial_runtime::webview::report_window_closed`'s doc); a popup is that
/// same window's child, not a second unrelated one, so it inherits rather
/// than repeating that choice.
///
/// The user agent is still applied explicitly, to the view's own live
/// `Settings` fetched after construction, rather than assumed to be
/// inherited through `related-view` -- mocktail's `CreateSurface` re-applies
/// its settings to every view it builds regardless of relation, which reads
/// as the same caution rather than an oversight, and nothing here has
/// measured whether WebKitGTK actually would inherit them if left unset.
///
/// Not presented immediately: WebKit hands this view back from `create`
/// before it has navigated anywhere or been given a size, and presenting an
/// empty, zero-sized dialog would be worse than the wait. `ready-to-show` is
/// WebKit's own signal that it is safe to, the same point mocktail's
/// `OnReadyToShow` waits for before its equivalent of `present`.
fn create_popup_view(
    parent_view: &webkit6::WebView,
    chrome_parent: gtk4::Widget,
    user_agent: Option<String>,
) -> webkit6::WebView {
    let user_content = webkit6::UserContentManager::new();
    register_bridge(&user_content);

    // No `.settings(&settings)` construct property here, unlike [`open`]'s own
    // view -- mocktail's `CreateSurface` never passes one at construction
    // either, for a related or an unrelated view; it applies its equivalent
    // (`ConfigureWebView`) to `webkit_web_view_get_settings()` afterwards in
    // both cases. Matched here rather than risk an untested interaction
    // between `related-view` and a `settings` construct property this crate
    // never had reason to try before.
    let view = webkit6::WebView::builder()
        .related_view(parent_view)
        .user_content_manager(&user_content)
        .hexpand(true)
        .vexpand(true)
        .build();
    // Fully qualified, because `settings()` is ambiguous on a `WebView`: GTK's
    // `WidgetExt` offers one returning `gtk::Settings` and WebKit's `WebViewExt`
    // offers this one returning `Option<webkit6::Settings>`. Left bare it fails
    // to compile with E0034, and the `Some(..)` pattern below only makes sense
    // for the WebKit one.
    if let Some(settings) = webkit6::prelude::WebViewExt::settings(&view) {
        if let Some(ua) = &user_agent {
            settings.set_user_agent(Some(ua));
        }
        if console_log_enabled() {
            settings.set_enable_write_console_messages_to_stdout(true);
        }
    }

    wire_bridge_messages(&user_content, &view);
    install_navigation_policy(&view);
    install_nav_probe(&view);
    install_popup_handling(&view, chrome_parent.clone(), user_agent);

    let header = adw::HeaderBar::new();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&view));

    let dialog = adw::Dialog::new();
    dialog.set_child(Some(&toolbar));
    dialog.set_content_width(900);
    dialog.set_content_height(700);

    {
        let dialog = dialog.clone();
        let shown_view = view.clone();
        view.connect_ready_to_show(move |_| {
            let host =
                webview_policy::evaluate(&shown_view.uri().map(|u| u.to_string()).unwrap_or_default())
                    .host;
            header.set_title_widget(Some(&adw::WindowTitle::new(&host, "")));
            dialog.set_title(&host);
            dialog.present(Some(&chrome_parent));
        });
    }
    // The counterpart to `create`: a page calling `window.close()` on itself.
    // Closing the dialog rather than leaving an unresponsive window up with
    // nothing left in it to show anything.
    {
        let dialog = dialog.clone();
        // `AdwDialog::close` returns whether it accepted the request, which
        // `connect_close`'s own signature has no room for -- WebKit's `close`
        // signal wants a plain reaction, not a verdict.
        view.connect_close(move |_| {
            let _ = dialog.close();
        });
    }

    // **Untested: whether this leaks a closed popup.** `dialog` holds `view`
    // (through `toolbar`), and `view`'s own signal connections above hold
    // `dialog` back -- a reference cycle, if GTK's own teardown of a closed
    // dialog does not break it by dropping the content widget (and with it
    // the signal closures) before the cycle matters. This crate could not
    // build to check, so it is written down rather than assumed away; a weak
    // reference via `glib::clone!` would be the fix if a live run ever shows
    // popup dialogs surviving their own closure.
    view
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The struct is filled from the engine's own keys, so a field quietly
    /// changing meaning would be a real bug. This only pins the defaults, which
    /// are the conservative ones: no chrome hidden, no domain forced, no back
    /// button, which is what an empty request should produce.
    #[test]
    fn a_default_request_hides_nothing_and_forces_nothing() {
        let r = WindowRequest::default();
        assert!(!r.hide_header);
        assert!(!r.show_domain_as_title);
        assert!(!r.back_button_visible);
        assert!(r.title.is_none());
        // The conservative default for a session, too: open signed out
        // rather than guess at a cookie nobody supplied.
        assert!(r.roblox_session_cookie.is_none());
        // Same reasoning: no invented User-Agent, only one actually computed
        // and threaded through by the caller.
        assert!(r.user_agent.is_none());
    }

    /// `open()` itself needs a live GTK/Wayland display to construct a
    /// `WebKitWebView` and is not exercised here for the same reason no test
    /// in this file has ever called it: `cargo test` runs headless, with no
    /// compositor for `gtk::init` to attach to. The two bridge handler names
    /// are pinned on their own, because a typo here silently produces a page
    /// that can call `window.webkit.messageHandlers.executeRoblox` and get
    /// nothing back with no error anywhere -- the exact failure mode this
    /// whole module exists to end.
    #[test]
    fn the_bridge_handler_names_match_mocktails() {
        assert_eq!(BRIDGE_EXECUTE_ROBLOX, "executeRoblox");
        assert_eq!(BRIDGE_ROBLOX_WK_HYBRID, "RobloxWKHybrid");
    }

    /// `forward_script_message`'s malformed-command log names a handler by
    /// this, so a typo here would make the log lie about which contract a
    /// bad message arrived on -- pinned the same way the constants above are.
    #[test]
    fn bridge_handler_name_matches_its_registered_handler() {
        assert_eq!(BridgeHandler::ExecuteRoblox.name(), BRIDGE_EXECUTE_ROBLOX);
        assert_eq!(BridgeHandler::RobloxWkHybrid.name(), BRIDGE_ROBLOX_WK_HYBRID);
    }

    /// Pure-function coverage for the one part of [`register_bridge`] that
    /// does not need a live display: the shim text itself still installs the
    /// global the page actually calls, under the handler name WebKitGTK's
    /// `postMessage` is registered against.
    #[test]
    fn the_bridge_shim_installs_the_android_bridge_global() {
        let shim = bridge_shim();
        assert!(shim.contains("__globalRobloxAndroidBridge__"));
        assert!(shim.contains(BRIDGE_EXECUTE_ROBLOX));
    }
}
