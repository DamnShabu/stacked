# ADR-060: Hyprland takes the camera button when the lock activates, and a lone frame says it came up

**Status:** accepted
**Date:** 2026-10-15
**Related:** [ADR-011](ADR-011-wayland-and-libadwaita.md), issues #56, #105, #97

## Context

A right-button camera drag asks the compositor for a pointer lock, so the
cursor stays where the drag began. On Hyprland that was broken twice over.

- **0.26.0 and 0.27.0** locked the toplevel (the only surface Hyprland
  activates a lock on) at the press. The reporter of #56 traced the result: the
  lock was destroyed 130 microseconds after it was requested, before `locked`
  arrived, and the camera never moved.
- **0.28.0** stopped asking for a lock during a drag on Hyprland. The camera
  turned, but the cursor drifted and jumped on release (#56 again).

0.28.0's commit called the mechanism `INFERRED` and described Hyprland's
release as carrying "the press's own timestamp". Both are now settled, and the
second was not quite right.

## What Hyprland does

Read from Hyprland 0.56.2's source and then seen on the wire in a real,
nested Hyprland (below).

1. `onNewConstraint` activates a lock only when the *keyboard-focused* surface
   is the constrained one. That is the window's main surface, never the canvas
   subsurface, which is why the lock has to be on the toplevel.
2. `CPointerConstraint::activate` then moves pointer focus to that surface.
   `CSeatManager::setPointerFocus` calls `sendLeave` on every `wl_pointer`, and
   `CWLPointerResource::sendLeave` releases each button that pointer had pressed,
   **stamped with the current time**. The reporter's release matched the press
   to the millisecond only because it followed it by 133 microseconds. A
   virtual pointer with its own clock shows the difference: press at 32,
   release at 1033012382.
3. That pointer now holds no pressed buttons. When the physical release comes,
   `sendButton` refuses it ("release on a non-pressed button") and **no
   `wl_pointer.button` is ever sent.**
4. `CInputManager::onMouseButton` still ends in `sendPointerFrame()`. So the
   physical release leaves exactly one trace: a `wl_pointer.frame` with no
   event in front of it.

## What was measured

A real Hyprland 0.56.2 cannot start under sway or weston (it wants
`wl_compositor` and `xdg_wm_base` at version 6; they offer 5), and with
`WAYLAND_DISPLAY` unset it opens on whatever session it finds. It starts under
`kwin_wayland --virtual`, which offers 6. `tools/hyprland-drag-e2e.py` builds
that stack in the `cordial` distrobox under a private `XDG_RUNTIME_DIR`
(KWin first, with its own session bus; Hyprland inside it; Cordial inside that)
and drives a `zwlr_virtual_pointer_v1` into the nested Hyprland only.

Same stack, same virtual pointer, signed out, at the title screen:

| Binary | Right drag |
|---|---|
| 0.29.0 with `CORDIAL_DRAG_LOCK=1` (the 0.27 behaviour) | `lock_pointer`, then button release, `leave`, `destroy`, then a discarded `locked`: the reporter's trace, line for line. The engine is told of the press and, under a millisecond later, of a release |
| 0.29.0 default (the 0.28 behaviour) | no lock requested; `confirmed=false` throughout |
| This change | lock requested, confirmed, held; relative motion reaches the engine (`nativePassMouseMove dx=30`); one release to the engine when the player lets go; lock given back |

The tool fails on the first two rows and passes on the third. It also drives
five quick right clicks, a three-second held drag, a left click in the middle
of a right drag, a middle drag and twenty drags in a row.

## Decision

Keep the lock, and find the release.

1. **A second `wl_pointer` at version 5**, from a second `wl_seat` bound at
   that version, used only to receive `frame`. Cordial's own pointer is version
   1 because its seat is, and a child cannot be newer than its parent. Binding
   the seat again leaves the keyboard and touch objects as they were.
2. **A camera-button release that arrives while the lock is still pending is
   set aside.** If the canvas's `leave` follows it in the same batch it was
   Hyprland's: the button stays down as far as the engine knows. If no `leave`
   follows, it was the player's and goes to the engine when the batch ends.
3. **Once the lock is confirmed, a frame with nothing in front of it ends the
   drag.** "Nothing" counts the watching pointer's own events and the
   relative-pointer events, which is what camera motion under a lock is.
4. Everything else that can end a hold does: a new press of the same button, the
   compositor's `unlocked`, `locked` never arriving within a second, and twenty
   seconds without any pointer event. The last is a failsafe for a compositor
   that stops sending the frame, which costs a stuck camera until the next click
   and not longer.

Only on Hyprland, and only when the second pointer exists. Elsewhere the drag
lock behaves as before. `CORDIAL_DRAG_LOCK=0` restores 0.28.0 on Hyprland.
`CORDIAL_DRAG_LOCK=1` used to mean "lock at the press" and now means nothing,
because that without the hold is the bug.

## A second bug, found while measuring

A lock on the toplevel is in toplevel coordinates and the engine's pointer is in
canvas coordinates. The hint sent when a drag's lock was given back was the
canvas point, so the cursor reappeared one title-bar height (46 px here) above
where it had been. In a nested Hyprland the press point climbed 185, 139, 93,
47, 1 over five drags without the pointer moving. Only the shift-lock path,
which recentres and added the offset, was right. In a nested KWin 6.7.5 the
hint is not acted on for a drag and nothing moved, so this was not the KDE
report, but it is the "jumps on release" half of #56.

## A second button held under the drag (#114)

The same focus change releases **every** button the pointer held, and the hold
above is only for the camera buttons, so a left button held when the right one
went down was passed on as a real release: the engine was told it had come up
about 0.7 s before it did. A camera button pressed while another button is held
therefore takes no lock on Hyprland, as before 0.26.0; the camera turns on a free
cursor and the held button stays held. Measured in the nested Hyprland: with the
change the engine sees left pressed once and released once, after the real
release, where before it was released as the right button went down. A lock
already held when the second button goes down is kept (right first, then left
passes both ways). Not fixed: right, then left, then right released while left
is still held — giving the lock back moves focus again and the left button is
released early. Elsewhere a lock leaves held buttons alone and nothing changed.

## Alternatives rejected

- **A grace window around the release.** Tried by the reporter on a local build
  of 0.13.2: the camera pans, and every drag then lasts until Escape, because
  the physical release never arrives. The frame is what was missing.
- **Reading the release from `/dev/input`.** Works, as the reporter showed, and
  needs read access to input devices that the Flatpak deliberately does not have.
- **Making the toplevel hold pointer focus from the start** (an empty input
  region on the canvas) so that activation moves nothing and nothing is released.
  Correct in principle and a rewrite of the pointer path (coordinates, the
  header bar, the cursor, the text editor's region) for a camera drag.
- **Leaving it unlocked.** 0.28.0. The camera turns and the cursor wanders.

## Consequences

- The release is inferred from an absence, in a compositor Cordial does not
  control. If Hyprland stops sending the frame, a drag ends by the silence
  failsafe or the next click rather than at once. `CORDIAL_DRAG_LOCK=0` is the
  way out.
- The tool exercises one virtual pointer in one nested Hyprland. A real mouse
  and a touchpad produce the same `onMouseButton` path; that is `INFERRED`.
- Hyprland versions other than 0.56.2 are untested.
