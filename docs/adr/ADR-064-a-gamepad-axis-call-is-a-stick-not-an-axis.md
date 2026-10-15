# ADR-064: A gamepad axis call is a stick, not an axis

**Status:** accepted
**Date:** 2026-10-16
**Related:** [ADR-001](ADR-001-in-process-hooking.md), [ADR-019](ADR-019-development-control-surface.md)
**Issue:** #113 (upstream Cordial)

## Context

#113 reported that on an AYN Odin 3 the left stick would not move up and down and the sticks were "not working properly". The pad's own layout had already been worked out (X, Y, Z, RX, RY, RZ, no hat, a four-button D-pad), so the axes were reaching the engine under the right Android codes. They were reaching it in the wrong shape.

`nativeGamepadAxisEvent` is `(int id, int axis, float x, float y, float z)V` in the dex (`tools/dex_method.py` on `NativeInputInterface`). Cordial sent one call per Linux axis as `(value, 0, 0)`. mocktail resolves the symbol and never calls it (`include/runtime/roblox_capability_resolver.h:81`, `src/runtime/roblox_capability_resolver.cc:101`), and `docs/traces/waydroid-roblox-startup.log.gz` has no gamepad line, so there was no reference for what x, y and z mean.

## What the engine does with the call

Read from the exported function in `libroblox.so` (a 349-byte JNI entry and the 24-entry switch it calls) and then tested by running it, because AGENTS.md's one rule is that conclusions from the binary alone have been wrong nine times running. The reading is only the hypothesis. The table, by Android axis: `AXIS_X` (0) and `AXIS_Y` (1) go to one input, `AXIS_Z` (11) and `AXIS_RZ` (14) to another, `AXIS_LTRIGGER` (17) and `AXIS_RTRIGGER` (18) to two more, `AXIS_RX` (12) and `AXIS_RY` (13) to nothing. The three floats then travel on as a vector. `AXIS_HAT_X` (15) and `AXIS_HAT_Y` (16) are not in the table; the function handles them itself and tests the third float's sign.

So a stick is one input with a position, and either of its axis codes names it. Sending the horizontal and the vertical as separate calls with the value in x means the vertical overwrites the horizontal with its own number.

## Measured

The signed-out landing screen draws a selection ring when a pad is used, which is a readout. Client: 0.32.0 plus the development verbs `padconnect`, `padaxis` and `padbutton` (raw calls with exactly the arguments given), in a nested headless sway, a fresh profile, one synthetic pad announced with the standard layout. Frames are read with the control surface's `screenshot`, out of the engine's own swapchain; the four selectable items are Create Account, Sign In, Terms and Privacy.

- Control: the D-pad button (`nativeGamepadButtonEvent`, key 20) moves the ring, so the pad is seen and the readout works.
- Stick down, `(axis=1, 0, -1, 0)` and `(axis=0, 0, -1, 0)`: the ring moves down. A held call repeats (two steps in 0.6 s); a 0.15 s pulse is one step.
- Stick up, `(0, 1, 0)`: moves up. Stick right, `(1, 0, 0)`: moves right.
- **The old shape, reproduced:** the call Cordial made for "stick down" (`axis=1`, value in x) from Terms moved the ring right to Privacy; the call for "stick up" from Privacy moved it left to Terms. Up and down arrived as left and right.
- Hat, z carries the direction: `(0,0,1)` on `AXIS_HAT_X` is right and `(0,0,-1)` left; `(0,0,-1)` on `AXIS_HAT_Y` is down. Up with x and y both zero did nothing; `(0, 1e-6, 1)`, `(0, ±1, 1)` and `(1, 0, 1)` all moved up. Why is not understood. Carrying the hat's own x and y beside the z satisfies it: `(0, 1, -1)` down, `(0, -1, 1)` up, `(-1, 0, -1)` left, `(1, 0, 1)` right were each seen to move the ring that way.
- Right stick (`AXIS_Z` and `AXIS_RZ` with y = -1), `AXIS_RX`, and both triggers (z = 1 and x = 1): the ring did not move. That screen does not read them, so none of the right stick or trigger shapes below is measured.

Then the same, through the fix and the real fold (`padjs`, joydev-shaped events on the standard layout, one session, one pad, same starting items as the control):

| Start | Event | Old shape (`padaxis 0 1 1 0 0`, as Cordial sent it) | Fixed |
|---|---|---|---|
| Terms | left stick down | Privacy | stays on Terms (nothing below) |
| Privacy | left stick up | Terms | Sign In |
| Privacy | left stick left | | Terms |
| Create Account | stick down | | Sign In |
| Terms | stick up | | Sign In |
| Create Account / Sign In / Terms / Privacy | hat down / up / right / left | | Sign In / Create Account / Privacy / Terms |

The right stick and trigger events were sent through the same fold and appear in the client's trace as `axis=11` pairs and `axis=17`/`axis=18` with z in `0..1`; nothing on screen answered them.

## Decision

Each joydev axis event is folded into the pad's remembered state and goes out as the engine's call:

- Left stick, one call under `AXIS_X`: `(x, -y, 0)`. The engine's y is positive up; Linux and Android are positive down.
- Right stick, one call under `AXIS_Z`: `(x, -y, 0)`. INFERRED to follow the left.
- Triggers: `(0, 0, t)` with `t` in `0.0..=1.0`, rescaled from joydev's `-1.0` rest. INFERRED: the range is Android's, and z is where the engine's own Vector3 has room.
- Hat: `AXIS_HAT_X` as `(hx, hy, hx)`, `AXIS_HAT_Y` as `(hx, hy, -hy)`.
- `AXIS_RX` and `AXIS_RY` make no call, because the engine drops them.

The per-device layout still decides which Linux axis is which.

## Consequences

- A pad's sticks work in the shape the menu measured. Whether a right-stick camera turns and a trigger accelerates is unmeasured and needs a game with a pad attached.
- `padjs` joins `padconnect`, `padaxis` and `padbutton` on the control surface so the next shape question can be asked of a running client without a pad, and without a virtual `uinput` device that every other program on the host would see.
- Android's real Java is unreadable here, so what it sends is not known. This is what the engine accepted, not a transcription of what Android does.
