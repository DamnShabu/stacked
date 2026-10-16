# ADR-063: Sign-in challenge windows get the native bridge

**Status:** accepted
**Date:** 2026-10-16
**Related:** `docs/analysis/webview-surface.md` section 10, `docs/analysis/platform-identity.md`
**Issue:** #83

## Context

Sign-in with a two-step code ended in "Something went wrong" about a minute after the verification window opened. Two reports with the full log, one with `CORDIAL_TRACE_BRIDGE=1`, show `APP_READY ChallengeHybridWebView`, an `openWindow`, and then nothing from the page at all, until the engine closed the window itself 60 to 61 seconds after opening it. `OPEN_CAPTCHA_VIEW` never appeared in either, so the handling added for it in 2704945 was not on this path.

The page was interactive (a code was typed into it), so it was not that the page failed to load. It sent nothing because it did not believe it was in an app.

## What was measured

`Roblox.Hybrid.Bridge.isNative` is chosen from the User-Agent. Under Cordial's default identity, `ROBLOX Windows App ... Desktop Hybrid()`, it is `false` and a call such as `Roblox.Hybrid.Navigation.navigateToFeature({feature: "CaptchaShown"})` reaches nothing. Under `ROBLOX Android App ... Tablet Hybrid()` it is `true` and the same call posts the JSON-RPC envelope to `window.__globalRobloxAndroidBridge__.executeRoblox`, which Cordial's shim already forwards to the engine. Both tokens are needed. Same page, same injected bridge, only the User-Agent changed; the table is in section 10 of `webview-surface.md`.

mocktail's web view helper and Sober both send the Android shape to the web view whatever they tell the engine. Cordial gave the web view the engine's own string, so under the default identity no page in a web window could reach the host, and Join works only because `Roblox.Hybrid.Game.launchGame` is intercepted inside the page (#40).

## Decision

A window gets an Android hybrid User-Agent (`Mozilla/5.0 AppleWebKit/605.1.15 (KHTML, like Gecko)  ROBLOX Android App <v> Tablet Hybrid()  Cordial RobloxApp/<v> (GlobalDist; Cordial)`, with `<v>` read from the engine's own string) in two cases: the engine opened it while the last `APP_READY` it reported was `ChallengeHybridWebView`, or it is the security check opened for `OPEN_CAPTCHA_VIEW`. Every other window keeps the engine's string. The engine's own HTTP identity is not touched. The channel stays `Cordial` rather than borrowing `GooglePlayStore`.

Each window now also logs the page's own answer to `Bridge.isNative`, the engine screen it was opened under, the path (no query, identifiers replaced) and the name of every bridge command the page sends (`Module.function`, with the feature for `navigateToFeature`; never the parameters). A report of a stall can then say which of "the page said nothing", "the page spoke and the engine did not answer" and "the page was never native" it is.

## Why not every window

Join was measured working under the desktop identity, five of five, through the in-page hook. Under the Android shape the page would also send its own `Game.launchGame` through the bridge, and nothing here has seen what the engine does with that beside the hook's copy, or whether a second launch could follow. That needs a signed-in click. The challenge window is the one that is known to be silent, so it is the one that changes. If the Android shape is later shown to be safe for Join, making it the web view's default is a one-line change and removes the screen test.

## Not established

That the challenge page's completion message reaches the engine and is acted on. It needs a real two-step verification. The cookie jar of the window is empty before sign-in, as it was before this change; whether the challenge needs the engine's cookies is untested. Nothing answers a callback the engine does not answer: the sign-in check's `CaptchaShown` and `CaptchaSuccess` are acknowledged by Cordial because mocktail does, and an unknown command is forwarded and logged, not acknowledged, since an acknowledgement for something nobody handled would claim it had been.
