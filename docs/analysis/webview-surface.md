# The WebView surface — what the engine actually asks the platform for

Marketplace, Profile, Friends, Communities, Create, Blog, Learn, gift cards, Help
& Safety and most link-opening in Roblox's Android client are web content rather
than engine-rendered UI. None of it works under Cordial. This is the map of what
would have to exist for it to.

**The headline is a correction.** The obvious plan — implement
`android.webkit.WebView` and its client classes in the framework layer, backed by
WebKitGTK — is aimed at the wrong boundary. The engine never calls
`android.webkit` at all. What it calls is a much smaller Roblox-owned surface,
and that is what this document inventories.

## How this was established

Four sources, in descending order of authority.

A live run of the current build, today, reaching the landing page:

```bash
CORDIAL_LOG_LEVEL=v CORDIAL_STUB_QUIET=1 ./target/release/cordial-run \
    --lib-dir ~/.cache/cordial-trace/lib/x86_64 --apk <base.apk> \
    --host-libc --game-activity --dump-classes classes.cpp --run 45
```

`just client --run 30` does the same thing and finds the APK itself.

The binary was `cordial-load` until 312451e renamed it to `cordial-run`, and a
stale `target/release/cordial-load` survives the rename and still executes. Two
runs of this investigation were spent measuring code that was never in the
binary under test. `native/CMakeLists.txt`'s build script warns about the
neighbouring version of this trap; the check that settles it is
`grep -ac '<a string you just added>' target/release/cordial-run`.

`--dump-classes` is `libjnivm`'s `GenerateClassDump`: every class and member
Roblox reached for through JNI during that run, whether or not Cordial answered
it. Note that "Constructed Unresolved symbol" lines do **not** appear at runtime
in an ordinary build — the `LOG` call sites in `libjnivm` are behind a
compile-time `JNI_TRACE`, which `native/CMakeLists.txt` leaves off. The class
dump carries the same information without a rebuild and is what was used here.

The engine's own log from the project owner's signed-in session,
`~/.local/share/cordial/instances/default/data/files/appData/logs/*_Player_*.log`,
which reached `APP_READY (Home)`.

`docs/traces/waydroid-roblox-startup.log.gz`, the logcat capture of the same APK
on real Android. It is startup-only and stops at the landing page, so it says
nothing about opening a web view. It does carry two things that matter: the
`EnableAndroidWebViewService4` flag value, and the cookie handler firing.

Declared prototypes read out of the shipping dex's `method_ids`/`proto_ids`
tables, and exported symbol names read out of `libroblox.so`'s dynamic symbol
table. Both are observation of a shipped binary, which AGENTS.md permits
explicitly. No decompilation was read.

## 1. The engine does not use `android.webkit`

Verified two ways, and worth stating plainly because it changes the design.

`libroblox.so` contains eleven `android/…` class descriptors as strings:
`android/app/ActivityThread`, `android/content/Context`,
`android/content/res/Configuration`, `android/os/Build`, `android/os/Build$VERSION`,
`android/os/Debug`, `android/os/LocaleList`, `android/view/KeyEvent`,
`android/view/MotionEvent`, plus two prefixes. None is `android/webkit`.

The class dump from the run above lists **64** classes Roblox reached for. None
is under `android/webkit` either, and none matches `cookie`, `credential`,
`token`, `keystore`, `sharedpref` or `account` in any case.

**64, not 2501.** That number is circulating and it is the generated file's line
count, not a class count — the dump is C++ source, and each class costs roughly
nine lines of namespace declaration, class body and hook registration. Measured
across three runs of the same build: 2501 lines / 64 classes, 2510 lines / 65
classes, 2501 lines / 64 classes. The two move together, nine lines to the
class. Count with

```bash
grep -cE '^class jnivm::' classes.cpp
```

The one class that varied between those runs was
`com/roblox/engine/jni/NativeInputInterface`, so the set is not quite fixed
run to run even at the same point in startup.

The distinction matters for sizing this work: 2501 makes the Java surface look
like a rewrite of Android, and 64 is what it actually is at this point in
startup. The conclusion drawn from the larger number — that no cookie or
credential class is requested during bring-up — is unaffected and holds on
this dump too.

`android/webkit/WebView`, `WebViewClient` and friends *do* appear in
`docs/analysis/framework-classes.txt`, and that is not a contradiction: that file
is the dex's referenced-type table, meaning the APK's **Java** code uses them.
Roblox's Java code is what drives a `WebView`, and Cordial does not run Roblox's
Java code — it stands in for it. So `WebView`, `WebViewClient`,
`WebChromeClient`, `WebSettings`, `CookieManager` and `ValueCallback` are not on
Cordial's implementation list at all. The distinction is the one
`observed-java-surface.md` opens with: which APIs the app references versus which
ones the engine reaches for.

What Cordial has to implement instead is the Roblox-owned interface that sits
between the engine and that Java code — the same relationship as
`NativeGLJavaInterface`, which Cordial already implements most of.

## 2. Two transports exist; the older one is the live one

**The legacy transport** is static methods on
`com.roblox.engine.jni.NativeGLJavaInterface`, called from the engine's
`DataModelBindings`. `native/android_classes.cpp` already implements part of this
class.

**The newer transport** is the universal-app message bus:
`com.roblox.protocols.webview.WebViewProtocol` subscribes to protocol messages
and the engine publishes open/close/mutate requests as JSON. Its Java side is
almost entirely a shell over native methods `libroblox.so` exports —
`Java_com_roblox_protocols_webview_WebViewProtocol_initializeAndroidWebViewProtocol`,
`…_getProtocolName`, `…_getOpenWindowId`, and so on — so a host that wanted to
speak it would call those exports itself rather than reimplement them.

The waydroid capture settles which is live:

```
07-31 22:50:57.420 D rbx.JNIRobloxSettings: nativeInitializeNativeFlags: ... 8: EnableAndroidWebViewService4 = false
```

The message-bus path is off. `EnableWebViewTelemetry`, `AndroidWebViewFocusFix`
and `EnableWebViewCameraPermission` are all true in the same capture, which is
what you would expect of a shipping legacy path with telemetry around it. So the
legacy `NativeGLJavaInterface` route is the one to implement, and the message-bus
protocol is where this will move when Roblox flips that flag — not something to
build first.

That flag value is a server-controlled rollout, not a constant. Re-read it from a
fresh capture before relying on it.

## 3. The call inventory

Signatures are the dex's own declarations. "Answered" is what
`native/android_classes.cpp` does today.

### 3a. Engine to platform, on `com/roblox/engine/jni/NativeGLJavaInterface`

| Method | Signature | Answered today |
|---|---|---|
| `openNativeOverlay` | `(Ljava/lang/String;Ljava/lang/String;)V` | hooked, silently discarded |
| `onAppBridgeNotification` | `(Ljava/lang/String;Ljava/lang/String;)V` | unresolved |
| `onDataModelNotificationCallback` | `(Ljava/lang/String;Ljava/lang/String;)V` | unresolved |
| `getWebViewUserAgent` | `()V` | unresolved |

`openNativeOverlay` is the direct one: url and title, and it corresponds to the
engine's `BrowserService::openNativeOverlay`, whose own diagnostic strings say it
refuses non-Roblox urls. `libroblox.so` also carries
`[FLog::DataModelBindings] openWebView_: Url:{}, Title:{}.`, which is the same
two arguments on the C++ side of the same boundary.

`getWebViewUserAgent()V` returns void because it is a *request*, not a getter.
The platform is expected to answer asynchronously by calling back into the engine
through `com.roblox.engine.jni.NativeGLInterface.setWebviewUserAgent(Ljava/lang/String;)V`,
whose implementation `libroblox.so` exports as
`Java_com_roblox_engine_jni_NativeGLInterface_setWebviewUserAgent`. Answering it
truthfully means having a web view whose user agent you can ask for; until then
the honest state is unanswered, and the engine falls back to whatever
`InitParams.userAgent` gave it.

`onAppBridgeNotification` and `onDataModelNotificationCallback` are both
`(String type, String data)`. The owner's signed-in session shows the C++ side of
the second one firing with exactly the payloads that drive app navigation:

```
onDataModelNotification: Received type(APP_READY, 10), data(Home)
onDataModelNotification: Received type(APP_READY, 10), data(More)
onDataModelNotification: Received type(PURCHASE_ROBUX, 8), data({"animated":true})
```

and the waydroid capture shows the Java side receiving the same thing on real
Android (`rbx.datamodel: onDataModelNotification() type:APP_READY data:Landing`),
which is what establishes that this callback is where those events cross the
boundary rather than dying inside the engine.

`APP_READY` is not itself a web-view request. `PURCHASE_ROBUX` is closer to one:
Buy Robux is web content on Android. The precise mapping from notification type
to "open this url" lives in Roblox's Java and was not read.

### 3b. Adjacent, unresolved, and on the same class

These are pre-cached by the engine at `JNI_OnLoad` and were unresolved in the run
above. None is WebView, but they sit in the same table and the same argument
applies to each — resolving matters for its own sake.

| Method | Signature | Honest completion |
|---|---|---|
| `gameLoadedCallback` | `(J)V` | no reply expected; a no-op is honest |
| `saveImageToAlbum` | `(Ljava/lang/String;)V` | reply failure via `NativeGLInterface.nativeImageSavedToAlbumFinished(Ljava/lang/String;ZLjava/lang/String;)V` |
| `getMobileAdvertisingId` | `()V` | reply path is `NativeGLInterface.setMobileAdvertisingId(Ljava/lang/String;)V`; Cordial has no advertising id and must not invent one |
| `onExtendedAnalyticsRecvCallback` | `([BI)V` | no reply expected |
| `onVrSessionStateUpdate` | `(I)V` | no reply expected |
| `promptNativePurchase` | `(JLjava/lang/String;)V` | Play Billing does not exist here |
| `promptNativePurchaseWithPaymentSessionId` | `(JLjava/lang/String;Ljava/lang/String;)V` and a 3-string overload | as above |

### 3c. The message-bus route, for when the flag flips

Exports on `libroblox.so`, all called *from* Java on Android, so a host speaking
this protocol calls them directly:

```
Java_com_roblox_protocols_webview_WebViewProtocol_initializeAndroidWebViewProtocol
Java_com_roblox_protocols_webview_WebViewProtocol_signalJavascriptCallback
Java_com_roblox_protocols_webview_WebViewProtocol_getProtocolName
Java_com_roblox_protocols_webview_WebViewProtocol_get{Open,Close,Mutate}WindowId
Java_com_roblox_protocols_webview_WebViewProtocol_getIsAvailableId
Java_com_roblox_protocols_webview_WebViewProtocol_getHandleWindowCloseId
Java_com_roblox_protocols_webview_WebViewProtocol_get{Url,Title,WindowType,IsVisible,
    HideHeader,BackButtonVisible,ShowDomainAsTitle,SearchType,SearchParams,Available}Key
Java_com_roblox_protocols_webview_DomainAllowListChecker_{checkDomainAllowList,
    enableDomainAllowListChecker,isKnownTrustedDomain}
```

The window is described by a JSON object whose keys those `get…Key` calls name.
`libroblox.so`'s `FLog::WebViewConfigLogs` strings confirm the shape without any
guessing: url (required), title, animated, visible, windowType, searchType,
searchParams. Delivery is `Java_com_roblox_universalapp_messagebus_MessageBus_*`
— `doSubscribeProtocolMethodRequestRaw`, `publishProtocolMethodResponseRaw`,
`subscribe`, `setRequestHandlerRaw` — returning a
`com.roblox.universalapp.messagebus.Connection`, which is the one class of this
group Cordial's run already reaches (it is a return type, so the engine caches
its method ids up front).

`DomainAllowListChecker` is worth noting for the security shape: the engine, not
the host, decides whether a url may be opened, and it exposes that decision to
the platform rather than trusting it. A Cordial implementation should ask it and
honour the answer, not open whatever it is handed.

## 4. Cookies — the part that carries the session

A web view that opens `roblox.com` and is not signed in is useless, so the
platform's cookie jar and the engine's have to be shared. On Android that is
`android.webkit.CookieManager`; on the engine side it is:

| Export | Declared as |
|---|---|
| `Java_com_roblox_universalapp_cookie_JNICookieManager_getCookie` | — |
| `…_JNICookieManager_setCookie` | — |
| `…_JNICookieManager_setCookiesFromDisk` | — |
| `…_JNICookieManager_convertCookiesToNetscape` | — |
| `…_JNICookieProtocol_updateOnSetCookieHandler` | `updateOnSetCookieHandler(Lcom/roblox/universalapp/cookie/JNICookieProtocol$OnSetCookieHandler;)V` |
| `Java_com_roblox_engine_jni_NativeSettingsInterface_nativeGetCookiesForDomain` | `(Ljava/lang/String;)Ljava/lang/String;` |
| `…_nativeGetCookiesInNetscapeFormat` | `(Ljava/lang/String;)Ljava/lang/String;` |
| `…_nativeSetMultipleCookies` | `(Ljava/lang/String;Ljava/lang/String;)V` |

The push direction is `CookieProtocol$OnSetCookieHandlerImpl.onSetCookie([Ljava/lang/String;Ljava/lang/String;)V`
— an array of `Set-Cookie` headers and the url they came from — and the waydroid
capture shows it firing during ordinary startup:

```
I OnSetCookieHandlerImpl: Updated WebViewCookieHandler with Cookies from URL https://apis.roblox.com/browser-tracker-api/device/initialize
```

**This is the authentication cookie.** `.ROBLOSECURITY` flows across this
boundary, and so does `RBXEventTrackerV2` (`libroblox.so` carries
`BrowserTrackerIdRequest: No RBXEventTrackerV2 in cookie.`). Two consequences
that are not optional:

Nothing on this path may be logged with its value. Cordial's own diagnostics
must print at most a name and a count. The same applies to urls crossing
`openNativeOverlay`, which can carry a one-time auth ticket in a query string —
log origin and path, never the query.

WebKitGTK's cookie jar must be non-persistent, or persisted no less carefully
than the engine's own. `WebKitWebsiteDataManager` defaults to ephemeral when
constructed without a base directory, which is the right default here.

## 5. What WebKitGTK answers directly, and what it does not

Assessed against the WebKitGTK 6.0 API. Marked **INFERRED** throughout: none of
this has been run, because the development headers are not installed (§7).

| Needed | WebKitGTK 6.0 |
|---|---|
| Show a url in a window | `webkit_web_view_new` + `webkit_web_view_load_uri`. Direct. |
| Title for the header bar | `webkit:title` property, `notify::title`. Direct. |
| Report a closed window back to the engine | GTK window close signal. Direct. |
| Seed the session cookie | `webkit_website_data_manager_get_cookie_manager` + `webkit_cookie_manager_add_cookie`, fed from `nativeGetCookiesForDomain`. Direct. |
| Receive cookies the web view sets | no add-cookie signal; requires `WebKitWebsiteDataManager` inspection or a `SoupSession` feature. Awkward. |
| Run JavaScript in the view (`BrowserService.ExecuteJavaScript`) | `webkit_web_view_evaluate_javascript`. Direct. |
| Page-to-host messages (`BrowserService.SendCommand`, and the `bs:open` / `bs:execJs` / `lp:openUrl` message types in `DFLog::StratusWebView`) | `webkit_user_content_manager_register_script_message_handler`. Direct. |
| A user agent matching what Roblox expects | `webkit:settings user-agent`, settable. What value it should be is an open question, not an API one. |
| Sit inside the engine's window | it cannot. See below. |

The last row is the architectural constraint and it is worth being explicit
about. [ADR-011](../adr/ADR-011-wayland-and-libadwaita.md) makes the engine's
`wl_surface` a `wl_subsurface` of a GTK toplevel, in one process, because a
subsurface cannot parent across connections. A `WebKitWebView` is a GTK widget in
that same toplevel's widget tree, which means the natural implementation is a
widget in the content slot *beside* the engine's canvas — not a second process.

That differs from Sober, which ships `sober_services` as a separate binary
against libwebkitgtk-6.0/libgtk-4/libadwaita-1/libsoup-3/libjavascriptcoregtk-6.0
and keeps its engine process toolkit-free. Both arrangements work; they are not
interchangeable. Sober's buys crash isolation for the browser and pays for it
with a separate window, and it is available to Sober precisely because its engine
process has no GTK in it. Cordial's engine process already links GTK4 and
libadwaita — ADR-011 §"One connection, therefore one process" — so an in-window
web view is available to Cordial and is the better answer to
"the communities view opens as a separate window", which
[ADR-001](../adr/ADR-001-in-process-hooking.md) lists as a motivating gap.

A separate process becomes the right answer if WebKit's own crashes start taking
the engine down with them. That is a measurement, not a prediction, and nothing
here has measured it.

## 6. Verified, inferred, not established

**Verified, this session:**

- No `android/webkit` class descriptor in `libroblox.so`, and no `android/webkit`
  class in a full startup-to-landing `--dump-classes` capture.
- The 64 classes the engine reached for in that run — and that `--dump-classes`
  output is 2501 *lines*, which is where the figure of 2501 classes came from.
- Which members of `NativeGLJavaInterface` Cordial answers versus leaves
  unresolved. `openNativeOverlay`, `onAppBridgeNotification`,
  `onDataModelNotificationCallback` and `getWebViewUserAgent` are all reached
  for; before this change the first was hooked to an empty body and the other
  three were unresolved.
- `EnableAndroidWebViewService4 = false` in the waydroid capture.
- The dex-declared signatures in §3 and §4, and the exported native names in §3c
  and §4.
- `webkitgtk-6.0.pc` is absent; `gtk4` 4.22.4 and `libadwaita-1` 1.9.1 are present.
- The four hooks in §8 register, and `onDataModelNotificationCallback` fires
  three times per run with `APP_READY` carrying `PlatformAccountRouter`,
  `Startup` and `Landing` — identical on three consecutive 40-second runs. The
  control is the same instrument on the previous binary, where all three
  appeared unhooked in the class dump and printed nothing;
  `gameLoadedCallback` is still unhooked and still shows the unhooked shape in
  the same dump, which is what makes that reading a measurement rather than an
  assumption.

**Inferred, not proven:**

- That `openNativeOverlay(url, title)` is the call that opens Marketplace,
  Profile and the rest. It is the only engine-to-platform call on the live
  transport that carries a url, and the engine logs `openWebView_: Url, Title`
  on its own side of the boundary — but nobody has watched it fire, because
  doing so needs a click in a signed-in client (§6, below).
- That the legacy transport is the live one for *this* build. The flag value
  comes from a 2026-07-31 capture on real Android; Cordial's own run is not
  signed in and never gets far enough to exercise either path.
- Everything in §5, which is API reading rather than running.

**Not established, and the honest reason:**

- **Nobody has observed a web-view request under Cordial.** Reproducing one needs
  a signed-in client and a click on Marketplace or Buy Robux. Cordial does not
  persist a session across runs — a fresh run stops at `APP_READY (Landing)` — so
  reaching the signed-in app means a human typing credentials, and driving the
  click is not something an agent may do here (AGENTS.md's caution on synthesised
  input). The `openNativeOverlay` stub in `native/android_classes.cpp` now reports
  when it fires, so the next person to run a signed-in session and click one of
  those entries will see it without instrumenting anything.
- Which notification types on `onDataModelNotificationCallback` correspond to
  which web destinations. That mapping is in Roblox's Java, which was not read.
- What user agent Roblox's web endpoints expect, and whether they behave
  differently for one that does not look like Android's WebView.
- Whether `DomainAllowListChecker` must be initialised before
  `openNativeOverlay` will fire at all.

## 7. Build dependency

**Superseded.** The container `just build toolbox` uses carries
`webkitgtk6.0-devel-2.52.5-1.fc44`, the `webview` feature builds, and §9's
probe runs against a live `WebKitWebView`. What follows is the state that held
when this section was written, kept because the pkg-config output below is
still the check to run on a machine where the feature will not build.

Stage 3 of this work — an actual `WebKitWebView` — was **blocked on a package
that was not installed**:

```
$ pkg-config --modversion webkitgtk-6.0
Package webkitgtk-6.0 was not found in the pkg-config search path.
$ pkg-config --modversion gtk4 libadwaita-1
4.22.4
1.9.1
```

The runtime libraries are present (`libwebkitgtk-6.0.so.4`,
`libjavascriptcoregtk-6.0.so.1`, `libsoup-3.0.so.0` all resolve via `ldconfig`,
from `webkitgtk6.0-2.52.5`), so only the development package is missing. On
Fedora that is `webkitgtk6.0-devel`. Installing it is the maintainer's call and
was not done here; `CONTRIBUTING.md` records it beside the other optional
dependencies.

## 8. What exists in the tree now

Four hooks on `NativeGLJavaInterface` in `native/android_classes.cpp`, and
nothing else. They report; none of them acts.

`openNativeOverlay` was already hooked, with an empty body. It now prints the
request. That is the whole of the change to it and it is the point of the
change: a request to show a web page used to vanish leaving no trace anywhere,
so "Marketplace does nothing" and "Marketplace was never asked for" produced
byte-identical output. `onAppBridgeNotification`,
`onDataModelNotificationCallback` and `getWebViewUserAgent` were unresolved and
are now hooked, which matters beyond the reporting: an unresolved JNI call
leaves a pending exception, and the next JNI call on that thread trips over it
somewhere unrelated.

`getWebViewUserAgent` is deliberately left unanswered rather than given a
plausible string, which is the one place in this change where AGENTS.md's rule
about stubs that lie has teeth. The truthful answer is the user agent of the web
view that will render the page; there is no web view, so any value here is a
claim about a browser that does not exist, and it would reach Roblox's servers
as a header describing something that never made the request.

Urls are printed with their query string elided. They can carry a single-use
authentication ticket, and the cookie path beside them carries `.ROBLOSECURITY`
(§4). A truncation rather than an elision would still print the front of a
token, which is why the helper cuts at the `?` rather than at a length.

## 9. WebAuthn — measured absent, and not a build flag away

**A Roblox account with a passkey enrolled cannot finish sign-in in this
window, on any WebKitGTK build available today.** Not "does not yet"; the API
the flow needs is absent from the browser engine, not merely disabled in it.

### What was measured

`crates/cordial-shell/examples/webauthn_probe.rs` drives the same
`webview::open` the engine's `openWindow` request takes — same ephemeral
`NetworkSession`, same `UserContentManager`, same policy — and `open` asks the
loaded page three questions. Against `https://www.roblox.com/login`, six
runs on 2026-08-20 with byte-identical output — three on a tree at
`0.5.2-165-g76ec67e-dirty` and three after the 0.6.0 bump at `0.6.0-dirty` —
and the same line again against `https://www.roblox.com/`, so it is a property
of the build and not of the sign-in page:

```
[webview] this WebKitGTK build has no WebAuthn
    (PublicKeyCredential/navigator.credentials/isSecureContext
     = undefined undefined true)
```

`isSecureContext` is `true`, so this is not the ordinary "WebAuthn is hidden
outside a secure context" answer — the bindings are absent. The same probe
through `gjs` against the same library agrees, and so does
`org.gnome.Platform//50`'s own `libwebkitgtk-6.0.so.4.16.9`, which is what a
Flatpak build would run against rather than the host's.

The control that makes this a measurement rather than a constant: the identical
expression with `Notification` in place of `PublicKeyCredential` returns
`function undefined true` on the same page, so the probe does distinguish a
name that exists from one that does not. A second control fell out of the same
script — the same expression on a `data:` URL reports `isSecureContext` as
`false`, so the boolean is being read from the page rather than invented.

The probe runs in a **named script world**, not the page's own, and that is not
decoration either. Measured on a page whose only content is
`window.PublicKeyCredential = function () {}`: the main world answers
`function undefined true` and the named world answers `undefined undefined
true` on the same load. A page can therefore talk the main world into
reporting a capability the build does not have, and cannot talk this
diagnostic into it — which matters because the page it runs on is the sign-in
page.

Builds measured: `webkitgtk6.0-2.52.5-1.fc44` (host, and what a
`just build toolbox` binary executes against) and `org.gnome.Platform//50`.

### Why no rebuild fixes it

Read from upstream WebKit's own tree rather than from release notes, and this
is source reading rather than a run — but it is source reading of the thing
that would have to change, which is a different kind of claim from reasoning
about a stripped binary:

- `Source/cmake/WebKitFeatures.cmake` carries
  `WEBKIT_OPTION_DEFINE(ENABLE_WEB_AUTHN "Toggle Web AuthN support" PRIVATE OFF)`.
  `PRIVATE` means it is not a switch a packager is offered.
- `Source/cmake/OptionsGTK.cmake` never mentions `WEB_AUTHN` or `LIBFIDO2` at
  all, so the GTK port does not turn it on.
- `Source/WebKit/UIProcess/WebAuthentication/` holds `Cocoa/`, `Mock/`,
  `Virtual/` and `fido/`. `fido/` is the transport-agnostic CTAP layer.
  **Every actual transport is Objective-C++ under `Cocoa/`** — `HidService.mm`
  (USB), `NfcService.mm`, `CcidService.mm`, `LocalService.mm`/
  `LocalAuthenticator.mm` (the platform authenticator).
  `AuthenticatorTransportService::create` names all four unconditionally.

So there is no Linux `HidService` to link against and no libfido2 backing in
the tree; `-DENABLE_WEB_AUTHN=ON` on the GTK port would not build, let alone
talk to a key. This is upstream work in WebKitGTK, not packaging work, and not
work Cordial can do in this repository.

Consistent with that, and cheap to re-check on any machine: `readelf -d` on
either `libwebkitgtk-6.0.so.4` lists no `libfido2`, and its dynamic symbol
table has no undefined `fido_*`. Enumerating all 782 `WebKitFeature`s through
`webkit_settings_get_all_features` finds no WebAuthn toggle either — only
`LoginStatusAPIRequiresWebAuthn`, which is a sub-flag of a different, also
disabled, feature.

### What Cordial does about it

Reports it, and nothing else. `webview.rs` prints the line above once per
process on the first finished load.

**Deliberately no polyfill.** A shim standing in for
`navigator.credentials.get` — even one that rejects — would be a stub that
lies in the sense AGENTS.md means: the page would proceed believing an
authenticator was asked and declined, when nothing was asked at all, and the
failure would then surface somewhere with no relationship to its cause. The
absence is left where a reader can find it.

**Deliberately no Flatpak device grant.** A USB FIDO2 key is a `/dev/hidraw*`
node, and Flatpak's only lever coarse enough to reach it is `--device=all`,
which is every device node in the sandbox — cameras, input devices, everything.
Granting that for a feature no code in the sandbox can use would be a
permission that lies about a capability, on the one file in this repository
that exists to be audited. `packaging/io.github.luohoa97.Cordial.yml` records
what would be needed, commented out, beside the reason it is not granted.

### What would actually let a passkey user in

Unresolved, and listed rather than chosen, because each of these is a design
decision for the maintainer and none of them has been measured:

1. WebKitGTK grows a Linux WebAuthn backend upstream. **INFERRED** that the
   manifest's device grant would then be the whole of Cordial's side of it —
   nothing has been built that way to check.
2. Sign-in moves out of the embedded view to the user's own browser through
   `org.freedesktop.portal.OpenURI`, and the session comes back by a route the
   user drives. Cordial already seeds a validated `.ROBLOSECURITY` into the
   view (`WindowRequest::roblox_session_cookie`) and already keeps one in the
   secret service (ADR-012), so the storage half exists; the transfer half does
   not, and any design for it must not have Cordial reading another
   application's cookie jar.
3. Roblox's own cross-device sign-in, if the Android client's login page offers
   it under Cordial's user agent. Nobody has looked, because looking means
   reaching a sign-in page with an account.

## 10. Whether the page runs as an app — measured, and it was the sign-in stall

Issue #83 (sign-in with a two-step code ends in "Something went wrong" after
about a minute). Both reporters' logs, one with `CORDIAL_TRACE_BRIDGE=1`, show
the same sequence and nothing else from the page:

    APP_READY ChallengeHybridWebView
    [webview] openWindow message arrived ... host www.roblox.com
    [webview] presented an openWindow request
    [webview] hybrid launch: intercepted Roblox.Hybrid.Game.launchGame
    ... no bridge message of any kind ...
    [webview] closeWindow message arrived from the engine   (60.3 s and 61.0 s after the open)
    APP_READY Login

No `bridge message`, no `rejected`, no `malformed`: the page never called
either registered handler, so the engine had nothing to act on and closed the
window itself after a minute. The question was why a page that was plainly
interactive (a code was typed into it) said nothing.

**The page decides whether it is in an app from the User-Agent.** One page
(`/captcha/app/login`, anonymous, a made-up user name), one injected
`__globalRobloxAndroidBridge__` that logs every read and every call, run
repeatedly (the desktop and mocktail rows four times each, the others once or
twice) with only the User-Agent changed (instrument:
`~/.cache/cordial-scratch/signin83/tools/probe.py`, not kept in the tree):

| User-Agent | `Roblox.Hybrid.Bridge.isNative` | `Bridge.registerCallback` | `Hybrid.Navigation.navigateToFeature` arity | `navigateToFeature({feature:"CaptchaShown"})` |
|---|---|---|---|---|
| Cordial's default (`ROBLOX Windows App ... Desktop Hybrid()`) | `false` | absent | 2 | nothing reaches the bridge |
| bare `RobloxApp/<v>(GlobalDist; Cordial)` | `false` | absent | not asked | not asked |
| mocktail's (`ROBLOX Android App <v> Tablet Hybrid()  GooglePlayStore ...`) | `true` | present | 0 | `executeRoblox` called |
| Cordial's own `android-tablet` string (device block, `GooglePlayStore`) | `true` | present | 0 | `executeRoblox` called |
| `Android App <v> Tablet Hybrid()` with a device block and the `Cordial` channel | `true` | present | 0 | `executeRoblox` called |
| `Android App <v> Tablet Hybrid()  Cordial RobloxApp/<v> (GlobalDist; Cordial)`, no device block (the one now used) | `true` | present | 0 | `executeRoblox` called |
| `...Android App <v> Tablet  Cordial ...` (no `Hybrid()`) | `false` | absent | 2 | nothing reaches the bridge |

The call that does arrive under the Android shape is exactly the envelope
`webview_captcha::parse_captcha_event` was written against from mocktail, so
that part of 2704945 is now observed rather than inferred:

    {"moduleID":"Navigation","functionName":"navigateToFeature",
     "params":{"params":{"feature":"CaptchaShown"}},"callbackID":"<uuid>"}

So under the default `pc-windows-11` identity **nothing a page does inside the
web window reaches the host**, whatever the shim offers. Join works there only
because `Roblox.Hybrid.Game.launchGame` is intercepted in the page (#40), a hook
that exists because the bridge was silent. mocktail's web view helper and Sober
both give their web views the Android shape whatever they tell the engine
(`BuildRobloxAndroidUserAgent` in mocktail's `webview_helper_policy.cc`).

**Not established**, and labelled so in the code:

* That the challenge page's completion message reaches the engine and is acted
  on once the bridge is native. Nobody has seen a real two-step verification
  here; it needs an account. The new `[webview] bridge command Module.function`
  line is what a report will show if it still stalls.
* Which module and function the challenge page calls. The hybrid object lists
  `Game`, `Social`, `Chat`, `Input`, `RealTime`, `Navigation`, `Push`,
  `Overlay`, `Localization` and `AdConsent`; none is named for a challenge.
* That the window needs none of the engine's cookies. Its jar holds the
  session cookie only, and holds nothing before sign-in.
* That the Join hook still works under the Android shape, which is why only
  the windows that need the bridge get it. See
  [ADR-063](../adr/ADR-063-sign-in-challenge-windows-get-the-native-bridge.md).

## The transport, found in mocktail's bridge rather than by tracing

`crates/cordial-runtime/src/webview.rs` said the receiving half could not be
written because `WebViewProtocol`'s non-native methods are obfuscated to single
letters and take `org.json.JSONObject`, so the message transport was unknown.
It is not unknown. mocktail's `roblox_web_view_bridge.cc` names it outright, and
Cordial already speaks it for something else.

**`com/roblox/universalapp/messagebus/MessageBus`.** The engine exports
twenty-one natives for it, and `native/deeplink.cpp` has been using two of them —
`publishRaw` and `getLastRaw` — since deep links were wired up. The web window
is the same bus with a different protocol name.

Signatures, from the dex:

    getMessageId    (Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;
    doSubscribeRaw  (Ljava/lang/String;L...messagebus/RawCallback;Z)
                        L...messagebus/Connection;
    publishRaw      (Ljava/lang/String;Ljava/lang/String;)V
    getLastRaw      (Ljava/lang/String;)Ljava/lang/String;
    doSubscribeProtocolMethodRequestRaw
                    (Ljava/lang/String;Ljava/lang/String;L...RawCallback;Z)
                        L...Connection;

So receiving `openWindow` is three steps, none of them mysterious:

1. `getMessageId("WebView", "openWindow")` — the protocol name and the method id
   both come from `WebViewProtocol`'s own getters, which Cordial already reads
   and prints at startup. Composing them gives the bus id.
2. `doSubscribeRaw(id, callback, false)` — returns a `Connection`.
3. The callback receives the message as raw JSON.

### What Cordial still owes

Two classes the engine constructs or calls into, the `NativeFlagsInitResult`
pattern again:

    com/roblox/universalapp/messagebus/Connection   <init> (J)V
                                                    isConnected (J)Z
                                                    deleteSharedPtr (J)V
                                                    finalize ()V
    com/roblox/universalapp/messagebus/RawCallback  method name unknown

`Connection` is fully specified — a native handle as a `jlong` and three methods.

**`RawCallback` is not, and this is where reading stops being enough.** The name
appears in `doSubscribeRaw`'s descriptor, but no class of that name is declared
in the dex; what is there is
`RawSubscriptionContract.<init>(Ljava/lang/String;Lgn/h;Ljava/lang/String;Z)V`,
and `gn/h` is the obfuscated interface. So the dex will not say what method the
engine calls on the callback.

That is fine, and it is the shape this project handles best: **register the class
and run it.** libjnivm resolves by what the engine asks for at runtime, and
`CORDIAL_JNI_TRACE=1` prints every lookup, including the ones it cannot satisfy.
One startup with a `RawCallback` class present and empty will name the method in
the trace. Guessing it from the C++ would be the mistake AGENTS.md opens with.

### Why this matters more than a feature

`docs/analysis/flag-init.md` §13.1: the `RBXEventTrackerV2` device cookie is set
through `WebViewCookieHandler` during sign-in, not by an API call — the standalone
endpoint answers 500. mocktail has the web view, signs in through it, gets the
cookie, and plays for four minutes on the place Cordial is disconnected from at
sixty seconds. That chain is unproven and it is the most coherent one this
investigation has.

### What is left, precisely

The two natives `openWindow` needs are both exported and both resolve —
`webview.rs` reports so on every startup. `getMessageId` can now be called:
`cordial_deeplink_two_strings_ret_string` and its Rust wrapper
`call_static_two_strings_ret_string` were added for it, because no existing call
shape took two `String`s and returned one. That was the first blocker and it is
gone.

**The second is `doSubscribeRaw`'s callback, and it is not a missing wrapper.**
`native/clipboard.cpp` subscribes to the bus already, and its machinery is
deliberately single-slot: one `g_payload_sink`, one `g_connection`,
one `g_connection_ptr`. Subscribing `openWindow` through it would not add a
second subscription, it would replace the clipboard's — the same silent
overwrite as registering a class twice, which happened once today already.

So the remaining work is to generalise that side: a subscribe that keeps a
callback and a `Connection` per message id rather than one of each. It is
ordinary C++ and it is not large; it is simply not something to bolt onto
clipboard's globals in passing, because the failure mode is the clipboard
quietly ceasing to work with nothing in the log to say why.
