# ADR-024: X11 is supported again, and it gets the editor

**Status:** accepted; the plan under "What this costs" (a GTK toplevel holding the engine's window as a child, with X keyboard delivery rebuilt) is superseded by [ADR-062](ADR-062-the-x11-editor-is-an-override-redirect-window.md), which gives X11 its editor as a separate override-redirect window and leaves the engine's window alone. The commitments under "How the rot ADR-011 predicted is prevented" stand, and ADR-062 says how far they are kept.
**Supersedes:** [ADR-011](ADR-011-wayland-and-libadwaita.md)'s "X11 is not developed further" and its deletion trigger
**Related:** [ADR-002](ADR-002-core-shell-and-ui-handoff.md), [ADR-023](ADR-023-host-audio-backends.md)

## Decision

**Wayland remains the primary backend and the window remains GTK4 + libadwaita.
None of that changes.** What changes is the sentence beneath it.

X11 is a supported backend. `crates/cordial-runtime/src/android/window.rs` is
not deleted when the Wayland path reaches sign-in, and the focused-TextBox
editor is brought to it rather than being a thing Wayland has and X11 does not.

## What ADR-011 got right, and why it is still being reversed

ADR-011's reasoning was good and most of it survives. Wayland's
configure/ack/commit makes a resize atomic in a way X11 cannot express;
`zwp_text_input_v3` is where an IME bridge is expressible; one backend is
cheaper than two. All true.

Its rejection of a parallel X11 path was argued on maintenance cost:

> **Rejected: Wayland with an X11 fallback maintained in parallel.** Two
> backends means every input and surface bug is asked "on which one?", and the
> fallback rots because nobody runs it.

And its "what would change this" section contemplated exactly one thing: a
Wayland compositor turning out to be unable to do something Cordial needs.

**It never contemplated a user who cannot run Wayland at all**, and that is who
turned up. From the project's own Discord, a user trying Cordial for the first
time: *"my env is x11 so i can't use the android emulator without workarounds"*,
followed by another regular telling them *"Cordial might give broken input on
x11 not sure though you'd have to ask the dev yourself"*. They were right, and
what they were describing is worse than "might":

**On X11 today, typed characters reach the engine and nothing paints them.**
There is no editor widget, so `sync_text_overlay` has no counterpart, and the
engine does not draw a focused TextBox's own contents — established from the dex
and confirmed by experiment. The field looks empty until it blurs. There is also
no caret, no selection, no click-to-position, and no web views at all, which
means the sign-in flow is gone too. That is not a degraded backend; it is one a
new user cannot sign into.

An architectural preference that is right in every technical particular is still
the wrong call when the people it excludes are the ones arriving. That is the
whole of the reversal.

## What this costs, stated before it is agreed to rather than after

This is not a wiring change and nobody should start it believing otherwise.

`window.rs` opens a raw Xlib toplevel through a `dlopen`'d libX11, before GTK
exists in the process, and `cordial_shell::host_window::HostWindow` — which owns
the `gtk::Text` — is never constructed on that path. `init_wayland` forces
`GDK_BACKEND=wayland`, so it currently cannot be. Bringing the editor over means:

- a GTK toplevel on the X11 path, and a way to build `HostWindow` without the
  three Wayland-only handle accessors;
- the engine's X window created as a child of GDK's, and kept positioned from
  `content_rect()` the way `sync_canvas_geometry` does;
- **X11 keyboard delivery rebuilt.** This is the expensive one and the reason
  for this paragraph. X11 delivers keys to the focus window; reparenting the
  engine under a GTK toplevel moves focus to the toplevel, and Cordial's own
  Xlib connection — today the *only* key source on X11, WASD and Escape included
  — stops receiving anything. It is the same "input has to be rebuilt, not
  ported" that ADR-011 accepted for Wayland, paid a second time;
- a stacking answer. An X11 child window always paints above its parent, so
  `wl_subsurface.place_below` has no analogue and the editor cannot simply be a
  widget in the toplevel.

## How the rot ADR-011 predicted is prevented

Overriding that argument with "users matter" and nothing else would earn exactly
the outcome it warned about. Two commitments, and they are the price of this
decision rather than follow-up work:

**The shared text logic moves to a common module rather than being duplicated.**
Most of it is already backend-agnostic and living in `android/input.rs`. Of what
remains in `wayland.rs`, `splice_preedit`, the `Placed` enum,
`polled_textbox_info`, `resolve_textbox_geometry`, `fallback_textbox_info`,
`editor_owns_text`, `send_current_text` and `LAST_EDITOR_RECT` contain no
Wayland calls at all. They move. Two backends may not mean two editors.

**`tools/text-input-e2e.py` must run on X11 too, and a backend with no passing
run of it is not supported.** ADR-011's "nobody runs it" is a statement about
humans, and the answer is that a machine runs it: thirty-six assertions, both
backends, or the claim in this document is false. A backend that cannot be
tested is the fallback ADR-011 rejected, wearing a different name.

## Consequences

**Every input or surface bug now has to be asked "on which one?".** ADR-011 was
right that this is the cost, and it is accepted rather than argued away. The
harness above is what stops the answer being "nobody knows".

**The X11 path will lag.** Wayland gets features first; X11 gets them when the
shared module and the harness say it does. That is a supported backend, not a
co-equal one, and saying so here is better than implying parity nobody is
maintaining.

**`docs/NEXT.md`'s "still not tested: resize with the editor up" now applies
twice.** X11 has no `xdg_toplevel` configure sequence, so the resize behaviour
ADR-011 catalogued as unfixable is inherited along with everything else.

**This does not reopen anything else in ADR-011.** Wayland-first stands, GTK4 and
libadwaita stand, and the reasoning about resize and `zwp_text_input_v3` stands.
If the Wayland path and the X11 path ever disagree about what Cordial should do,
Wayland is the answer.

## What would change this

If X11 users disappear, or if XWayland becomes good enough that a Wayland-only
Cordial serves them, this reverts to ADR-011 and `window.rs` goes. The trigger
that would justify that is evidence about users, since that is the evidence that
justified reversing it — not an appeal to how much simpler one backend is.
