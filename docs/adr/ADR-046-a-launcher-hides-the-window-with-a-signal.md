# ADR-046: A launcher hides the window with a signal

**Status:** Accepted
**Date:** 2026-10-03
**Related:** [ADR-043](ADR-043-the-launcher-is-a-command-line.md), [ADR-019](ADR-019-development-control-surface.md), [ADR-024](ADR-024-x11-is-supported-again.md)

## Context

A launcher running several clients (the Roblox manager runs one per account)
wants the windows nobody is watching out of the way, with their sessions still
running, and wants to bring each one back later. That launcher already talks to
a client in exactly one way: it starts it, and it ends it with `SIGTERM`.

The compositor's minimise is not usable for this. A client may only ask to be
minimised (`xdg_toplevel.set_minimized`), niri ignores the request, and only
the compositor can restore. Another process cannot hide our window on Wayland
at all. ADR-019's control socket is opt-in and a development aid, not something
a launcher should depend on.

## Decision

**`SIGUSR1` unmaps the window and `SIGUSR2` maps it again.** Both are
idempotent, so a launcher says what it wants and never toggles. The handler
only records the request; the pump acts on it on the window's thread
(`android::hidden`).

On Wayland that is `gtk_widget_set_visible(false)` on GTK's toplevel. GDK keeps
the `wl_surface` across a hide and drops only its xdg role (GTK 4.22,
`gdk_wayland_surface_hide_surface`), so the engine's subsurface is unmapped with
its parent and returns with it, and `window_closed` does not read the hide as a
close. On X11 it is `XUnmapWindow`/`XMapWindow`.

While hidden, `vkQueuePresentKHR` is paced to 10 per second and
`backend_visible` answers `Some(false)`, so the keepalive stops and the engine's
own idle throttle can go lower. The GLES path is not paced.

The state is the file `<profile>/window-state`, `shown` or `hidden`, written
once the handlers are in and removed on the way out. A launcher that restarted
reads which windows it has to show again, and it signals only a client with the
file: `SIGUSR1`'s default action terminates, so a client from an earlier build
would be killed rather than hidden.

## Consequences

Measured on niri 2026-10-03 against a signed-out profile: the toplevel left
`niri msg windows` on `SIGUSR1` and came back on `SIGUSR2` with the canvas
drawing; the process stayed up throughout; hidden, the health line read 2.6
presents/s against 69.7/s shown. A client killed with `SIGKILL` leaves its
state file behind until that profile next starts; a launcher reads it only for
a profile it sees running.

`INFERRED`: the engine does not rely on `SIGUSR1`/`SIGUSR2` itself. Nothing in
the logs of the runs above suggests it, but it was not looked for in the engine.
