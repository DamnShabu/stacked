# Movement keys dead after joining (#29, #64)

The symptom: after joining a game, W, A, S, D, Space and the arrows do nothing,
while Escape, other keys, the mouse and the camera still work. Respawning or
re-joining brings movement back. Sober has the same report several times
(#2088, #2142, #1315, #196), with respawn or a Movement Mode toggle as the fix.

## What the code says (2026-10-01, read, not run)

- **Cordial does not label the mouse as a touch.** Every pointer event goes to
  the engine as `SOURCE_MOUSE` / `TOOL_TYPE_MOUSE` unless it arrived on a
  `wl_touch` (`native/game_activity.cpp`, `Create`, `CreateScroll`,
  `CreateTouch`). The default before `80f9ccb` was the same, so the dead state
  seen on 2026-08-22 was already under mouse labels.
- **`PlatformParams.isTouchDevice` follows the seat.** It is true only when the
  Wayland seat advertises a touchscreen, and always false on X11. The log line
  `PlatformParams.isTouchDevice follows` says which.
- **The engine can still end up on a non-keyboard control scheme.** That is a
  statement about engine state, not about what Cordial labels, and it fits the
  symptom best: the whole keyboard movement binding set is dead together, and a
  respawn or a Movement Mode toggle restores it. `INFERRED`.

## Experiment: re-delivering the surface parameters

The two `nativeAppBridgeV2UpdateSurface{App,Game}WithPlatformParams` natives
are called by Java whenever a `SurfaceView` changes, and Sober's log shows them
again after startup. Cordial calls them once, at start. A join made through the
Home page's own Play button creates a new game DataModel inside the running
engine, and nothing hands it these parameters again. The question was whether
that is why movement is dead.

The development control socket gained `updatesurface [app|game|both]` to
deliver them again into a running client (`android::surface_params`).

Method: a nested headless sway, the `CordialTest` profile, Mini War, a join
through Play. Movement scored as the screenshot difference across a 2 s hold of
W, beside the same difference with no key held (the idle control). A ratio near
1 means the hold changed nothing beyond idle motion.

| Run (2026-10-01) | Join | Movement |
|---|---|---|
| L1, 12:42 | Play, clicked with a virtual pointer | ratio 0.5 to 1.6 at 20 to 100 s after `onGameLoaded` |
| L2, 12:48, same client | Left the game, Play again by devctl click (no AGDK mouse copy) | ratio 0.5 to 2.0 at 20 to 100 s |
| L3, 12:55 | Play, then `updatesurface game` | before: 16.5 at 10 s, 0.5 at 30 s, 6.5 at 50 s, 1.0 at 70 s; after: 1.7 then 1.1 |
| L3, same client | `updatesurface both` | 1.0 then 1.6 |

Both clients were signed in (`cachedUserId` set, Home reached) with no
`DID_LOG_OUT`.

**Result: re-delivering the surface parameters did not bring movement back.**
That is a weak negative. The instrument has no clean positive control in this
data: L3's 16.5 at 10 s may be movement that worked and then stopped, or camera
motion during the hold, and the idle readings themselves vary from 0.001 to
0.098. A better score needs the character's position, not the whole frame,
and a run where movement is known to work, scored the same way.

## Still open

- Whether any Cordial lever moves the control scheme at all.
- Whether the Movement Mode list in Roblox's settings shows a touch or
  click-to-move mode in the dead state, and whether changing it brings WASD
  back. The reporter on #29 has been asked; nobody has looked here yet.
- A run with `CORDIAL_GAMEPAD=0`, since a gamepad being reported may change the
  scheme the engine picks. `INFERRED`, not run.
