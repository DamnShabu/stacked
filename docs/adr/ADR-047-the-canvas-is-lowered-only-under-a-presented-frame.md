# ADR-047: The canvas is lowered only once GTK has presented a frame

**Status:** accepted
**Date:** 2026-09-30
**Related:** [ADR-011](ADR-011-wayland-and-libadwaita.md), upstream issue luohoa97/cordial#53

## Context

A focused Roblox TextBox, or a web-view dialog, is drawn by GTK over the game by
lowering the engine's canvas subsurface beneath the toplevel and making the
toplevel transparent over the canvas rectangle. That is only right if the
compositor already holds a GTK buffer that is transparent there. Until it does,
lowering shows GTK's previous buffer, which was painted with the canvas above
and is opaque across the window: a grey screen for as long as GTK does not
present.

`repaint_now` waited 40 ms for the frame clock's `after-paint`, logged that none
had come, and lowered anyway.

## What was measured (nested KWin 6.7, own machine, signed out, `fakefocus`)

`VK_ICD_FILENAMES=/nonexistent` gives the engine no Vulkan, so it falls back to
GLES3 in the same process as GTK, and GTK falls back to its GL renderer. That
reproduces the reporter's log line for line: `gdk_gl_context_make_current()
failed` every 15 s, and on the second and third focus `repaint_now: no GTK frame
landed within 40ms`. With the old ordering, three focuses gave three
`place_below` and no `attach` on the window surface at any point after startup:
GDK sent the frame's regions, `frame` and presentation feedback, and Mesa never
attached a buffer. Even the focus where `repaint_now` reported "painted after
6 ms" had no attach behind it, so `after-paint` is not evidence of a commit.

The same binary with `GSK_RENDERER=cairo`, or with Vulkan available (GTK then
renders through Vulkan; forcing the engine to GLES with Vulkan present does not
break it), presents normally: the window surface attaches, `repaint_now` reports
2-5 ms, and every restack is safe.

## Decision

1. `cordial_shell::stacking_gate::Gate` decides. The canvas is lowered only
   after GTK has been asked for a frame with the transparent background
   (`HostWindow::arm_present_probe`) and GDK's frame timings show a frame laid
   out after that request with a presentation time
   (`present_probe_committed`). Raising is never gated.
2. No frame within 750 ms: the transparency is reverted, the canvas stays above
   the window (the game stays visible, the editor does not), a line names the
   renderer and the frame-clock state, and the attempt is repeated a second
   later for as long as the box has focus.
3. Waiting is spread over pump ticks instead of blocking one, and Cordial sends
   no parent commit of its own while waiting, because a commit of ours would
   resolve GDK's pending presentation feedback and pass the check without GTK
   having attached anything.
4. Without `wp_presentation` GDK's timings cannot report a presentation, so the
   check falls back to `after-paint`, the weaker old answer.
5. `CORDIAL_STACKING_GATE=off` restores the old ordering. It is the control for
   a before-and-after on one binary.

## Alternatives

- **Keep the window permanently transparent over the canvas.** Removes the
  dependency on a new frame at lowering time, but the last buffer GTK committed
  may itself never have been the transparent one (GTK in the failing
  configuration commits nothing at all), and 5a295e3 records the invisible
  window an always-transparent toplevel gives when the engine stops painting.
- **Draw the editor in a popover, above the canvas without lowering it.**
  Would end the dependency for the editor but not for dialogs, and changes
  focus and input-method behaviour. Not attempted.

## What this does not fix

In the failing configuration GTK presents nothing, so the editor is still not
visible; the game is. The cause is GTK's GL renderer sharing a process with the
engine's GLES. `GSK_RENDERER=cairo` cures it (measured above). Choosing cairo
automatically when the engine will run without Vulkan is not done, because
Cordial has no probe for that before GTK initialises.

INFERRED: that this is the reporter's cause. It reproduces their log signature
on a second KWin with the same trigger; their GPU stack was not seen. The claim
that a presentation time implies an attached buffer follows from the protocol
and matches every trace here, and was not tested against a compositor that
withholds feedback.
