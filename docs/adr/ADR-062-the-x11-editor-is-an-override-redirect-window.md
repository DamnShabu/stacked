# ADR-062: The X11 editor is an override-redirect window over the box

**Status:** accepted
**Date:** 2026-10-16
**Supersedes in part:** [ADR-024](ADR-024-x11-is-supported-again.md), the paragraphs under "What this costs" that plan a GTK toplevel holding the engine's X window as a child
**Related:** [ADR-011](ADR-011-wayland-and-libadwaita.md), [ADR-001](ADR-001-in-process-hooking.md)
**Issue:** luohoa97/cordial#86

Ported from upstream Cordial with its number kept, so a reader comparing the two trees finds the same decision under the same name. Upstream ADRs 047 to 061 are not in this fork. The recording hotkey upstream forwards from the editor does not exist here, so only Escape and F11 are forwarded.

## Context

ADR-024 brought X11 back and promised it the focused-TextBox editor. It was never built. On X11 a typed character reached the engine and nothing painted it, because the engine does not draw a focused TextBox's own contents. Upstream's `docs/NEXT.md` and README said so, from the code: an attempt to run Xwayland under the nested sway had failed and no other X11 route to a photograph was set up, so the claim was never observed either way.

ADR-024's plan was to make the engine's X window a child of a GTK toplevel, as the Wayland engine surface is a subsurface of one, and to rebuild keyboard delivery. Its own text priced that as the expensive route, and named two problems: moving the engine under a GTK toplevel moves X keyboard focus to the toplevel, so Cordial's Xlib connection (today the only key source on X11, WASD and Escape included) stops hearing anything; and an X child window always paints above its parent, so the editor cannot be a widget in that toplevel. The second means the editor would be another X window above the engine in any case, and the reparenting buys nothing but the first problem.

## Decision

The engine's window stays what it is, a bare Xlib window. The editor is a **separate GTK toplevel made override-redirect**, sized to the box, moved onto it, raised above it, and given X keyboard focus, for as long as a Roblox TextBox has focus.

- **Override-redirect, set after `realize` and before the first map.** `gtk::Window` is realised, its X window is read with `gdk_x11_surface_get_xid`, and `XChangeWindowAttributes(CWOverrideRedirect)` is applied; GTK then maps it. A window manager never sees it, so nothing places, decorates, animates, restacks or refuses to focus it, and it is positioned with `XMoveResizeWindow` to the pixel.
- **The widget owns the text.** The editor holds a real `gtk::Text`, or a `gtk::TextView` in a scrolled window for a multi-line box, in the engine's colour, size, family, weight and alignment, exactly as on Wayland (`crates/cordial-shell/src/editor.rs`, `x11_editor.rs`). Cordial mirrors it into the engine through `input::adopt_editor_text` and `pass_text`, and Enter on a single-line box is `nativeReturnPressedFromOnScreenKeyboard`. Input methods are GTK's, because the widget is GTK's.
- **Keyboard focus is taken explicitly and handed back.** `XSetInputFocus` once the window is viewable. While the editor holds it the engine's connection hears no keys, which is what makes "the widget owns the text" true here and why the engine's own text path has nothing to guard against. Three things move focus away and each is handled on every pump: a focus-follows-mouse window manager giving it back to the engine when the pointer crosses (taken straight back); another application receiving focus (the editor unmaps, or it would float over that application, and returns when the engine has focus again); the engine window ceasing to be viewable. Escape and F11 are forwarded to the engine from the editor, because the editor would otherwise swallow them.
- **A server with no compositor cannot show a transparent window.** On a composited display the editor's background is transparent over the box, as on Wayland. On a bare X server a transparent window is painted black (measured), so there it is opaque: the colour of one pixel read from the engine window just before the editor covers it, or, if that cannot be read or does not contrast with the text, near-black for light text and near-white for dark. `INFERRED` as a match for the box; it is right where the box is one flat colour.
- **GTK is brought up on the X11 backend**, with `GDK_BACKEND` forced to `x11` and `GSK_RENDERER` defaulting to `cairo`, so the editor does not open a second GL context beside the engine's. GTK's X connection is registered with the looper so a keystroke does not wait for the 50 ms idle tick.
- **`CORDIAL_X11_EDITOR=off`** runs the backend as it was, for the control arm of `tools/text-input-e2e.py --x11`. If GTK or the editor cannot start the log says so and the backend stays on the buffer path rather than refusing to run.

## Alternatives, and what was and was not measured

**Reparent the engine's window under a GTK toplevel (ADR-024's plan).** Rejected for the two reasons in the Context. Not built.

**A managed window: undecorated, transient for the engine.** Measured, in a prototype with no Cordial in it, on openbox, xfwm4 (compositor off) and icewm on a private Xvfb: it takes focus when mapped without being asked, and `XMoveResizeWindow` places it exactly. So the claim that no window manager would cooperate is false, and this ADR does not make it. It was not chosen because it leaves the decision to the window manager, and the ones not measured are where that is most likely to matter. `INFERRED`, not measured: a tiling manager places a new window by its own rule, and a compositing desktop may shadow, animate or clamp one. An override-redirect window removes the window manager from the question. That one was run under the same three, under xfwm4 with its compositor on, and under sway's Xwayland.

**`GtkPopover` / `GdkPopup`.** Needs a GTK toplevel to hang from, and on X11 the engine's window is not one. An invisible parent positioned over the engine would be this decision with a second window in it.

**Draw the typed text into the engine's frame.** An in-process write to the engine's output. ADR-001 and ADR-003.

**XEmbed.** GTK 4 removed `GtkPlug` and `GtkSocket`.

## Measured

`tools/text-input-e2e.py --x11` starts a private Xvfb (1280x800) and a window manager inside the `cordial` container, runs a signed-out client against it with `CORDIAL_X11=1` and `CORDIAL_NO_VULKAN=1`, types with XTest on that server only, and photographs it with ImageMagick. Each run starts with the real sign-in fields (clicked, typed with dummy strings, never submitted) and goes on to a synthetic box (`fakefocus`): where the editor window is, who holds X focus, that the typed characters are in the field (`cordial_textbox`) **and** changed pixels on the display in the box's own colour, editing keys, non-ASCII text, Enter, Escape and F11 forwarded to the engine, multi-line, blur, a moving and resizing window, fullscreen, and another application taking focus.

| Server | Compositing | Result |
|---|---|---|
| Xvfb + openbox | no, editor opaque | 66 of 66 |
| Xvfb + xfwm4, compositor off | no | 66 of 66 |
| Xvfb + xfwm4, compositor on | yes, editor transparent | 66 of 66 |
| Xvfb + icewm | no | 66 of 66 |
| sway's own Xwayland | yes | 66 of 66 |

Control, `CORDIAL_X11_EDITOR=off`, openbox: 7 of 22 assertions fail, and they are the ones about the editor. The readback still says the field holds twelve characters and the photograph changes by 0 pixels where the run above changes by 812. That pair of numbers is what "sent but not drawn" was. `GDK_SCALE=2`, openbox, the sign-in, single-line and multi-line cases: 39 of 39, with the one-pixel offset in the last Consequences bullet allowed for.

## Consequences

- Typed text on X11 is drawn in the box, with the caret, selection and click-to-position the widget brings.
- **Two editors until `HostWindow` adopts the shared one.** ADR-024 committed to the shared text logic moving to a common module. `editor.rs` (the widgets and their Roblox styling) and `android/text_geometry.rs` (where the box is: the volunteered spec, `nativeGetTextBoxInfo`, the carry-over, the stale-rectangle rule, the fallback bar) are written to be that module, and the X11 backend uses them. `HostWindow` and `wayland.rs` still hold their own copies, because both files were being changed by other work when this was written and moving several hundred lines out of them would have been a conflict with no behavioural gain. Until that is done a change to how a box is styled or placed must be made in both places. Doing it is deleting code from `host_window.rs` and `wayland.rs`, and is the next step.
- The geometry rules are Wayland's, so Wayland's limit comes with them: after a resize or a fullscreen change the engine does not re-report a box, and an unchanged rectangle under a different canvas is refused, so the editor sits in the fallback bar until the engine reports again. Measured on X11 with a real sign-in field in fullscreen.
- **Not measured, and the reason each matters.** A compositing desktop's own window manager (KWin, Mutter's X11 mode) and a tiling one: an override-redirect window should not meet either, which is the argument for it, and neither was run. gamescope, which opens X11 clients: a compositor that decides what an unmanaged window is. An input method: the widget is GTK's, so a composing IME is expected to work, and nothing here composed one. Vulkan: the client crashed on Xvfb's software GL in an earlier agent's run, so every figure here is the GLES engine. A box close to a screen edge on a desktop that clamps unmanaged windows. A real in-experience chat box: no signed-in run was made. Real keyboard events under Xwayland: XTest is the only source a headless sway offers.
- Under `GDK_SCALE=2` GDK keeps a window on its own logical grid, so the editor can sit one pixel up and left of the box (measured: asked for 193,273 and placed at 192,272).

## What would change this

A compositing desktop's window manager, a tiling one, or gamescope showing that an override-redirect window cannot take or keep keyboard focus, or cannot be placed on the box, would move the editor to a managed transient window with the focus handling this one has, which is the alternative above and costs a few lines.
