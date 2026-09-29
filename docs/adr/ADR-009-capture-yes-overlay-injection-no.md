# ADR-009: Cordial is capturable, and ships no overlay injection point

**Status:** accepted
**Related:** [ADR-001](ADR-001-in-process-hooking.md), [ADR-004](ADR-004-plugin-asset-overrides.md) (superseded by [ADR-010](ADR-010-plugin-asset-overlays.md)), [ADR-007](ADR-007-host-resources-are-brokered.md)

## Decision

Recording and streaming Cordial is supported, and requires nothing from Cordial
beyond being a well-behaved window. OBS, GPU Screen Recorder, the freedesktop
ScreenCast portal and anything else that reads composited output all work today.

Cordial ships **no overlay API, no injection point, and no hook for a capture
tool to load code into its process**. If a third-party overlay injects anyway,
that is the user's choice and outside what Cordial supports.

## Why the two halves are different

**Capture reads the output. An overlay writes into the process.** A screen
recorder receives frames the compositor already produced; it needs no privilege
inside Cordial and cannot observe anything a screenshot could not. An overlay
works by loading itself into the game and hooking the presentation path — Steam's
Linux overlay is an `LD_PRELOAD` of `gameoverlayrenderer.so` that wraps
`glXSwapBuffers`/`vkQueuePresentKHR`.

That distinction is the whole decision. Cordial's process contains
`libroblox.so`. An overlay in that process is a third party executing inside the
engine's address space, hooking the exact call Cordial uses to present. Cordial
refuses to do that itself ([ADR-001](ADR-001-in-process-hooking.md)); shipping a
supported way for someone else to do it would be that refusal in name only.

**So the same rule applies a second time, for someone else's code.** ADR-001
refuses in-process execution for Cordial's own code; this refuses it for a
third party's. In each case the protection is that the primitive does not
exist. (ADR-004 once made a third case, refusing plugin asset substitution by
the same reasoning; it has since been superseded by
[ADR-010](ADR-010-plugin-asset-overlays.md), which found that a *non-destructive,
out-of-process* asset overlay is not the same primitive as in-process
injection. That reversal does not touch this decision — an overlay that hooks
`glXSwapBuffers` still executes inside the engine's address space, which is the
one thing this ADR and ADR-001 both refuse.)

## What "capturable" concretely requires

Nothing that is not already done, which is why this is cheap to commit to:

| | |
|---|---|
| `WM_CLASS` / Wayland app_id | `io.github.damnshabu.Stacked`, set at window creation |
| `StartupWMClass` | matches, so the window resolves to the desktop entry |
| Window name | set via `XStoreName` |

Together these are what make Cordial appear as a named entry in OBS's window
picker and in portal capture dialogs rather than as an untitled surface. They
should not regress; a capture tool that cannot identify the window is the only
way this feature breaks.

The id is the reverse-DNS one the desktop entry and icon are named after, not
the bare product name. It was `Stacked` until 2026-09-29, which satisfied
`StartupWMClass` and therefore GNOME, and nothing else: KDE, waybar and the
wlroots bars resolve a window by `<app_id>.desktop` or an icon named after the
app_id, found neither, and showed a generic icon.

## On the Windows-only tools specifically

GeForce Experience and its ShadowPlay capture, the NVIDIA App that replaced it,
Medal.tv's desktop client, and the Xbox Game Bar overlay are all Windows-only.
There is nothing to integrate with on Cordial's target platform, and the Linux
equivalents (GPU Screen Recorder covers the instant-replay use case on NVENC and
VAAPI) need no integration at all.

Steam is the exception worth naming because it is cross-platform: adding Cordial
via *Add a Non-Steam Game* works now and needs nothing. The Steam **overlay** is
the injection case above, and is neither supported nor blocked.

## Consequences

**Accepted:** a plugin cannot draw on top of the game. A plugin that wants to
show something during play has Cordial's own surfaces and brokered effects
(`notify.send`, `presence.set`) and no path into the rendered frame. This is a
real limitation, and a different one from what
[ADR-010](ADR-010-plugin-asset-overlays.md) permits: ADR-010 changes what
loads before the frame exists, not what gets drawn on top of it once it does.

**Accepted:** Cordial does not detect, warn about, or attempt to block an
injected overlay. Detecting other processes' hooks is anti-cheat work, it is
unreliable, and it would put Cordial in the business of policing what runs on the
user's machine. Documenting the exposure is the honest response; enforcing
against it is not Cordial's job.

**Accepted:** "runs the official build unmodified" describes what *Cordial* does.
A user who loads an overlay into the process has changed that, and the claim is
about Cordial's conduct rather than a guarantee about the user's whole system.

## What would change this

A capture protocol that works out-of-process — the game publishing state or
frames to a recorder without the recorder executing inside it — would be
brokerable under ADR-007 like any other effect, and should be reconsidered on
that basis. The objection is to injection, not to cooperation.
