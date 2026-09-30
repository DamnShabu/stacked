# Pointer lock: what other Wayland clients do

Read on 2026-09-30, from the current default branch of each project, to check
the KWin fix in `parent_constraint_changed` against clients that are known to
lock the pointer on Plasma. Source read, nothing run. Ideas only; nothing here
is transcribed.

## The short answer

Every reference puts the constraint on the **surface that already has pointer
focus and a whole input region**, and that surface is always the toplevel.
None of them needs a per-compositor choice of surface, because none of them
lets a subsurface take pointer input in the first place.

Stacked is the odd one out. It punches the canvas out of the toplevel's input
region so the engine's subsurface receives the pointer, and that is what forces
the split: GNOME activates a lock only on the surface with pointer focus (the
subsurface), while KWin consults only the window's main surface and clips the
lock to that surface's input region (see `wayland.rs`, `lock_pointer`).

## Per project

**gamescope, nested Wayland backend** (`src/Backends/WaylandBackend.cpp`).
The window is a toplevel plus subsurface planes, the same shape as Stacked.
Every plane subsurface gets an *empty* input region, with the comment that the
parent should receive input while covered. The lock is taken on plane 0, which
is the toplevel, with a null region and a persistent lifetime. It never commits
for the lock's sake, because the game frame is presented on that same surface
every frame, so a commit follows within one frame anyway.

**SDL3** (`src/video/wayland/SDL_waylandevents.c`,
`Wayland_SeatUpdatePointerGrab`). SDL locks the surface of the window that has
pointer focus, which is its xdg_toplevel with the default infinite input
region. It creates the lock only once pointer focus is on that window, and
drops any confinement first, because a second constraint on one surface is a
protocol error. It has no explicit commit, for the same reason as gamescope.
This is also what Sober's SDL window does.

**Wine, winewayland.drv** (`wayland_surface.c`, `wayland_pointer.c`). This is
the closest analogue, because Wine draws a window's client area on a subsurface
of the toplevel, as Stacked draws the engine. The client subsurface gets an
empty input region ("let parent handle all pointer events"), and constraints go
on the parent surface. After setting a lock's cursor-position hint it commits
the surface explicitly, because the hint is double-buffered.

## What this says about the fix

The fix is consistent with all three on the two points that matter:

- **The constrained surface's input region has to cover the game.** Stacked
  now claims the canvas back into the toplevel's region while a constraint is
  held there. The references get this by default, because they never punch it.
- **Someone has to commit.** Games commit the constrained surface every frame
  anyway. Stacked's toplevel is GTK's surface, which only commits when GTK
  repaints, so Stacked has to commit it explicitly, as Wine does for the hint.

## The alternative the references point at, not taken

Wine and gamescope route all pointer input to the toplevel by giving the
subsurface an empty input region, and **leaving the parent whole**. If Stacked
did the same, it could lock the toplevel on every compositor and drop the
KWin/Hyprland detection entirely. The host cursor could also be hidden through
GTK's own widget cursor, which `pointer_enter` records as impossible today only
because the subsurface holds focus.

07564e2 tried half of this, the empty canvas region, while the parent was still
punched, so clicks fell through to the window behind; it was reverted in
73c74eb. The whole version has not been tried. It would change how every
pointer event reaches the engine: motion would arrive in toplevel coordinates
through the `POINTER_VIA_PARENT` path, which today only runs in fullscreen. It
would also change how the text editor and dialogs share input with the game.
That makes it an ADR-sized change rather than a follow-up, and it should wait
until the targeted fix has been tried on Plasma.
