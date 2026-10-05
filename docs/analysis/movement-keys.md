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

## What decides it: input around the join (2026-10-05)

One signed-in `CordialTest` client for the Steal An Egg joins, a second launch
for AI Town, nested headless sway, every join made by clicking Play on the game
page. Movement is scored by a screenshot pair across a 2.5 s hold of W beside an
idle pair, and every pair was read by eye, because UI animation and other
players' speech bubbles fool the pixel score in a busy place. A respawn at the
start of the first session restored movement, which is the positive control for
the instrument. Input is driven only through the development control socket.

Steal An Egg, events hand-driven:

| What reached the engine around the join | Joins | Character walked |
|---|---|---|
| nothing | 9 | 0 |
| pointer motion from the click until the load finished | 4 | 4 |
| one move at the load, to where the pointer already was | 2 | 2 |
| one move at the load, to somewhere else | 2 | 2 |
| pointer motion for five seconds starting at the load | 2 | 2 |
| shift key taps until the load finished | 2 | 2 |
| pointer motion starting ten seconds after the load | 3 | 0 |

AI Town, same client, with the hovers `NewGameSeed` sends switched off (`devctl
gameseed off`) and the events hand-driven, or with them on:

| What reached the engine around the join | Joins | Character walked |
|---|---|---|
| nothing (switch off) | 4 | 0 |
| one move at the load | 2 | 0 |
| first built schedule: once a second from the load for six seconds | 7 | 4 |
| pointer motion from the click until the load | 3 | 3 |
| pointer motion for five seconds from the load | 1 | 1 |
| a hover every 250 ms from the click until six seconds after the load | 5 | 5 |

Things that did **not** bring movement back once it was dead: opening and closing
the Escape menu, moving the mouse, clicking the world, opening and closing the
game's own shop, scrolling the wheel. Respawning did.

Roblox's own Settings, View & Controls, lacked **Movement Mode** in the first
game in the dead state *and* after the respawn that fixed it, so its absence is
not a sign of the state. The Escape menu showed keyboard hints (`L`, `R`, `ESC`)
in the dead state, so the interface did think a keyboard was in use.

What that says, `INFERRED`: the movement controls are chosen once, around
spawn, from the input the new DataModel has seen so far, and a DataModel that has
seen none gets the scheme a phone would. Input has to arrive while it is
deciding. In AI Town the world is on screen about a second after the load
callback and a single move at the load was not enough there, where it was in
Steal An Egg, so the decision can fall before the callback. The mechanism has
not been observed. It would also explain why joining from a link seems safer: a
real pointer usually moves while the place loads.

What was ruled out on the Cordial side: every pointer event goes out as
`SOURCE_MOUSE`/`TOOL_TYPE_MOUSE`, keys go through `nativePassKeyEvent` which
carries no device, and the engine asks for neither an `InputDevice` nor a
keyboard `Configuration`. mocktail sends the same natives with
`isMouseDevice`/`isKeyboardDevice` true and `isTouchDevice` false.

Not measured: a join from a link with no input at all (`devctl joinplace` only
works under `--app-bridge`, so it could not be run inside the same client), a
real pointer, X11, a third place. The built schedule against a join was run
afterwards; see below.

## The fix on the branch

`input::NewGameSeed` hands the engine a zero-delta pointer hover at the last
place a pointer was seen, four times a second from the engine's
experience-start announcement until eight seconds after the last thing it
announced, which for a join is its load. Not sent when no pointer has ever been
seen, so a touchscreen-only machine is not told it has a mouse. The decision is
a pure function with tests; the delivery is the same native a real mouse move
uses. `CORDIAL_NO_GAME_SEED=1` or `gameseed off` on the control socket turns it
off, which is how the table above had a control inside one session.

The first schedule on the branch (once a second, from the load) was run and
fixed four of seven AI Town joins, which is why it was replaced with the
hand-driven one that fixed five of five.

## The built schedule against joins (2026-10-16)

One signed-in `CordialTest` client, nested headless sway, one public place
(`[UP] Just a baseplate.`, place 123974602339071) joined nine times by clicking
Play on the search results, strictly alternating, the seed switched with `devctl
gameseed on|off` (the same gate `CORDIAL_NO_GAME_SEED` closes). No input from
the Play click until twelve seconds after the load; then a W hold of 2.5 s
between screenshot pairs, every pair read by eye as well as scored, because
other players move.

| Seed | Joins | Character walked | Hovers sent from click to load + 12 s |
|---|---|---|---|
| on (default) | 4 | 4 | 44 to 45 each (four a second) |
| off | 5 | 0 | 0 |

Together with the earlier 0 of 13 with nothing reaching the engine, that is 0
of 18 without the seed and 4 of 4 with it. Not covered: a second place or
client, X11, a join from a link, and the `CORDIAL_NO_GAME_SEED=1` spelling of
the control (the socket verb closes the same gate). Why it works stays
`INFERRED`.

Two joins in the same client are left out of the table because the harness was
wrong, not the client: `devctl text` leaves its first key in the held-key list,
and a held key makes the idle keepalive send a zero-delta hover sixty times a
second, so both ran with the seed on *and* a hover stream. Both walked. They
are not evidence for the seed; the hover stream explains them, and the counts
above were taken after releasing the key and checking the stream was gone.

## Still open

- Whether the seed holds in a second place and on a client that has not just
  played several games.
- `devctl text` leaving a key held, and so a 60 Hz hover stream, for any test
  that types before it joins.
- Whether the Movement Mode list in Roblox's settings means anything here.
  `Set by developer` hides it in some experiences, which this one may do.
- A run with `CORDIAL_GAMEPAD=0` is no longer interesting: the reporter has no
  controller and reproduces it with controllers off.
