# ADR-043: The launcher is a command line, and the game's window is the only window

**Status:** Accepted; the "Background updates" item is superseded by [ADR-044](ADR-044-updates-happen-at-launch.md)
**Date:** 2026-09-29
**Supersedes:** [ADR-002](ADR-002-core-shell-and-ui-handoff.md) (the core shell's chooser and settings fallback), [ADR-031](ADR-031-the-launcher-outlives-its-window.md) in part (the launcher is no longer a window, so it no longer has one to outlive)
**Related:** [ADR-011](ADR-011-wayland-and-libadwaita.md), [ADR-012](ADR-012-profiles-and-instances.md), [ADR-035](ADR-035-browser-account-routing.md)

## Context

This fork is **Stacked**. It was asked to be a command-line launcher with no
graphical interface other than the game's own window.

Before this, `cordial-shell` was a single-instance `AdwApplication`: a chooser,
a settings dialog of a dozen pages, a profile switcher, an update dialog, a
first-run download screen, a crash page and plugin management, around 14,000
lines between them. It stayed resident for the whole of a session (ADR-031),
with its own GTK main loop, a child watch, a background update timer and a
`GtkApplication` registered on the session bus.

## Decision

`crates/cordial-shell` builds a binary called `stacked`. It has subcommands
instead of windows, and it opens no window of its own. The game window, which
`cordial-run` opens through `cordial_shell::host_window`, is unchanged, apart
from its colours (below).

- `stacked` or `stacked play` starts the client and waits for it. A bare
  `roblox-player:` or `roblox:` argument joins that link, which is what the
  desktop entry's `Exec=stacked %u` does.
- `install`, `update`, `versions`, `pin` and `unpin` call `cordial_update` the
  same way the first-run screen, the update dialog and the Version page did.
- `profiles` lists, creates and selects profiles (ADR-012).
- `config` reads and writes `shell.json`. It validates each change by
  deserialising it back into `ShellConfig` and checking that the value comes
  out as it went in. Without that check, `#[serde(default)]` would accept an
  unknown key without complaint and the command would report a success that
  did nothing.
- `plugins` installs, removes, enables, disables, grants and revokes. The
  consent rules are `cordial_plugins`'s, not the old page's, so they still
  hold. A new plugin that contains code starts switched off. Installing it
  grants nothing: each capability is its own command, and that command prints
  what the capability allows.

The pure logic under each window was kept and is the same code:
`install.rs`, `launch.rs`, `shell_config.rs`, `browser_account`, `crash::is_crash`
and `diagnostics.rs`. The deleted modules were the widgets built on top of it.

The game window gains a theme. `Stacked` is the default: a dark palette taken
from the icon, with libadwaita held in dark mode. `System` gives the desktop's
own colours, which was the behaviour before this change. The launcher passes
the choice as `CORDIAL_THEME`, the same way it passes `CORDIAL_TITLE_BAR`.

## What is lost, and what replaces it

- **Single-instance link forwarding.** A second invocation used to hand its
  link to the running launcher over D-Bus. Now a second `stacked` against a
  profile that is already open is refused by ADR-012's lock, which names the
  process holding it. That is also what happened before once the old launcher
  tried to start a second client, so a user reaches the same outcome by a
  shorter route.
- **Browser account routing when nothing matches.** ADR-035 still picks the
  saved profile that is signed in as the browser's account. When no profile
  matches, the old launcher held the link and waited for a profile to be
  chosen. The CLI launches the current profile instead. `--profile` still
  overrides the routing.
- **The crash page.** The client's output was already streaming to the
  terminal. On a crash, the CLI prints `crash::describe` and the command line
  that was run.
- **Background updates.** There is no resident process left to run a timer.
  The `automatic_updates` and `download_on` keys were removed from
  `shell.json`. An older file that still contains them loads normally, and the
  next save drops them. *Superseded by [ADR-044](ADR-044-updates-happen-at-launch.md):
  `stacked play` now checks for a newer build and installs it before the game
  starts, instead of leaving `stacked update` as the only way.*
- **Plugin preferences.** The Settings page drew a form from each plugin's
  declared preferences. There is no command for them, so they are edited by
  hand in the plugin's `preferences.json`, and three of the four shipped
  plugins declare some. A `stacked plugins prefs` is the obvious follow-up.
- **The Deno download** is `stacked plugins deno` now, which was a button
  before.
- **Consent.** The page only offered to grant capabilities a plugin
  requested. `stacked plugins grant` refuses anything else for the same
  reason: a grant the manifest never asked for is consent to something nobody
  was shown.
- **The marketplace browser.** Plugins install from a local `.tar.zst`. The
  registry and marketplace code in `cordial_plugins` still exists, but no
  command drives it.

## Consequences

- Nothing is resident while a game runs apart from the client and a `stacked`
  process blocked in `wait()`. **INFERRED** that this saves anything
  measurable. The old launcher's own CPU cost was never measured here, and this
  environment has no Roblox build to measure it with.
- ADR-002's T1/T3 ordering and its UI-plugin handoff no longer describe
  anything that exists. ADR-002 stays as the record of why the shell was
  shaped as it was.
- ADR-031 is superseded only in part. Its launcher-outlives-its-window reading
  no longer applies. Its other decision, that the client is a separate child
  process and not linked in, is unchanged, and so is the profile lock passing
  to the client at spawn.
- Ctrl-C in the terminal reaches the whole foreground process group. `stacked`
  ignores SIGINT once the client is running, so the client does its own
  orderly shutdown and `stacked` can still report how it exited. Before this,
  `stacked` would die first and take the client's output pipe down with it.
  Not ignored before the spawn, because an ignored signal is inherited across
  `exec` and the client would then never hear Ctrl-C at all.
