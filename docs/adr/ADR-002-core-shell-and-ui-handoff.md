# ADR-002: Core shell, UI handoff, and the cold-start ordering

**Status:** Superseded by [ADR-043](ADR-043-the-launcher-is-a-command-line.md). The core shell is now a command line with no windows of its own; this record stays for why it was shaped as it was.
**Date:** 2026-07-31
**Amends:** architecture spec §5 (bootstrap shell exception), §9b (feature parity), §15
**Related:** ADR-001, spec §13 principle 7

---

## Context

The spec's §5 grants core a narrow exception: a startup progress indicator, because a
plugin-hosted UI cannot render during the window it exists to cover. That exception is
scoped too small, and the ordering it implies is wrong.

Two separate problems.

**Ordering.** If core boots the plugin host and *then* paints, the plugin host's cold start
(Deno process spawn plus IPC handshake) is time the user spends looking at nothing.

**Recoverability.** If the UI is a plugin and that plugin fails — corrupt install, bad
update, protocol mismatch after a Cordial upgrade — the user has no interface. They cannot
launch, cannot reach settings, and cannot disable the plugin that is broken. The only
recovery is a terminal, which Cordial's target user does not have open.

## Decision

**Core owns a shell. The UI plugin takes over from it.**

Cold-start ordering, with the plugin host warming in parallel rather than in sequence:

```
T0  user launches Cordial
T1  core shell paints the chooser        <100 ms, native GTK, no IPC, no plugin dependency
    └─ plugin host boots in parallel, invisibly
T2  user picks an entry                  they spend 1-3 s deciding; the host is up by now
T3  UI plugin takes over
T4  runtime loads; progress bar
```

The chooser is a decision window, and the plugin host warms inside it for free.

*Corrected:* this originally claimed human decision time is the exact budget a Deno cold
start needs. It is not — measured, Deno starts in 20-30 ms warm and 140 ms cold, so
blocking on the host would cost a flicker, not a blank window. The ordering above is still
right, but for the reason in §Reasoning below rather than for latency. See
[ADR-008](ADR-008-plugins-are-typescript-on-deno.md).

### The split

**Core shell** — window, branding, the chooser, and a minimal settings fallback sufficient
to disable a plugin. Native GTK/libadwaita, no IPC, no plugin dependency. Boots and paints
without the plugin host existing.

**UI plugin** — takes over after handoff and owns everything persistent: rich settings,
themes, plugin-contributed chooser entries, instance management.

Core's shell is a bridge measured in milliseconds, not a product. "The UI is a plugin"
remains true in every way that matters.

## Reasoning

**Recoverability is the real argument, not startup latency.** The parallel warmup is worth
having and costs nothing, but it only buys a second. The failure mode is what matters: with
a core shell, a broken UI plugin degrades the experience; without one, it ends it. This is
the same principle as §7.3 — the mechanism that recovers from a failure cannot live inside
the thing that failed.

**It is the smallest exception that achieves that.** Core does not gain a UI framework, a
theming system, or plugin-contributed views. It gains a window, three buttons and an escape
hatch. Everything that grows lives in the plugin.

## Corrections to the proposal as originally stated

### 1. `cap:core.process.spawn` must not exist

The proposal suggests a first-party Studio plugin exercising `cap:core.process.spawn`.

That capability is `--allow-run` re-admitted through the broker. §6.1 says of `--allow-run`
that it is not discouraged but *absent*, because it "spawns arbitrary subprocesses, escaping
the sandbox entirely". A core capability that spawns a caller-specified process has exactly
that effect; routing it through the broker changes who types the exec, not what the plugin
can do. §13 principle 7 lists it among the one-way doors, and the meta-lesson — "too much
power granted too early, then unremovable" — describes precisely this shape of mistake.

**Instead: the plugin declares *what*, core decides *how*.** A launcher-contributing plugin
registers a chooser entry naming a target that core validates and launches itself:

```
cap:core.launcher.register    contribute an entry to the chooser
```

The plugin supplies a label, an icon, and an identifier for something core knows how to
launch — a Flatpak application ID, say. Core resolves and spawns it. The plugin never holds
a spawn primitive, cannot pass arguments core did not sanction, and cannot reach anything
not already installed. The capability stays enforceable, and the honest consent string is
"this plugin can add launcher entries", not "this plugin can run programs".

This is still a genuine test of the plugin layer owning a real feature. It just tests it
without opening the door.

### 2. Cross-Flatpak launching is a sandbox hole, not a detail

Launching another Flatpak from inside Cordial's sandbox requires `--talk-name=org.freedesktop.Flatpak`,
which permits running arbitrary commands on the host outside the sandbox. Flathub reviewers
treat it as such and so should Cordial: it would hand every plugin the escape the previous
section just closed, this time at the manifest level where no broker sees it.

Resolve before building the Studio plugin. Portal-based activation of an installed
application is the direction to look; if no acceptable mechanism exists, the honest answer
is that Cordial does not launch Studio, and the entry links to Vinegar's install page.

## The Studio note — accepted, with the reason recorded

**Roblox Studio is not this runtime and cannot be.** Studio ships no Android build. It runs
under Wine, which works because Studio does not carry Hyperion — that is what VinegarHQ's
Vinegar does, and it is a separate Flatpak from Sober for exactly this reason. A Studio
entry in Cordial means a Wine-based path beside the Android runtime: different loader,
different graphics stack, different everything. Two products in one binary.

**Decision: Studio is out of scope for the runtime.** It may exist later as a plugin that
activates an existing Vinegar install, subject to the two corrections above. §9b should say
so explicitly, because "run Roblox on Linux" reads as including Studio and a reader will
otherwise assume it is planned.

## Consequences

- §5's bootstrap-shell exception widens from "progress indicator" to "shell": window,
  chooser, minimal settings fallback. The boundary is *what fails without it*, and the
  test for admitting anything else to core is whether its absence leaves the user unable
  to recover.
- Core acquires a GTK dependency it would otherwise have deferred to Phase 4. Accepted.
- *Extended 2026-08-02:* **the runtime acquires it too, and not by choice.**
  [ADR-011](ADR-011-wayland-and-libadwaita.md) makes the engine's `wl_surface` a
  subsurface of this window, and a Wayland subsurface must share a connection —
  therefore a process — with its parent. So `cordial-shell` is now a library as well
  as a binary: `cordial_shell::host_window` builds the window, the shell binary puts
  the chooser in its content slot, and `cordial-runtime` puts the engine's canvas
  there instead. "That shell and this window are the same window" is now one
  definition rather than two that resemble each other.
- What that does **not** yet do is join the two processes. `cordial-shell` and
  `cordial-load` still each build their own instance of that window, and the T1→T3
  handoff below remains unspecified and unimplemented. Sharing the definition is
  what makes the handoff expressible later — the header bar, theming and app_id no
  longer differ between the two — not a claim that it exists.
- The plugin host must tolerate being started before it is needed and never being used —
  if the user launches and quits, a warm host is discarded. Cheap.
- There must be a handoff protocol: core paints, plugin takes over, and the seam must not
  flicker. Unspecified for now; it belongs with Phase 3.
- A "safe mode" that skips the UI plugin entirely follows for free once the shell exists,
  and should be reachable without a terminal.

## Open

- What does core do if the UI plugin takes over and *then* crashes? Repaint the shell, or
  stay dark until restart? Repainting is friendlier and is probably right, but it means the
  shell cannot be torn down at handoff — it has to be hidden and retained.
