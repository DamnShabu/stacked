# The startup freeze, photographed

First capture of a frozen client, 2026-08-26. Everything here is one run,
`/tmp/cordial-freeze/capture-cap2`, taken with
`CAPTURE=1 tools/startup-freeze-survey.sh 2 CordialTest`.

## How to reproduce it, which is the first thing that changed

**The freeze is a signed-in phenomenon.** Twenty runs on a signed-in profile
froze sixteen times; forty runs on a signed-out one froze not once, same
machine, same binary, same evening.

```text
signed in  (ready=RootSwitchNavigator)   16 / 20 frozen   80%
signed out (ready=Landing)                0 / 40 frozen    0%
```

It also did not reproduce in seven direct launches onto the host's real GNOME
session, only inside the survey's nested headless sway. That is a second
variable and it is **not** established which of the two matters, or whether
both do. Do not read this document as saying the freeze needs a headless
compositor; it says nobody has separated them yet.

**Corrected 2026-09-17: the freeze does not need a nested compositor.** The
survey was rerun as a two-by-two, ten launches an arm, one client on the
machine at a time, on a `just build toolbox` binary of `4c9d1b5`:

```text
signed in,  nested sway   7 / 10 frozen
signed in,  host GNOME    4 / 10 frozen
signed out, nested sway   0 / 10 frozen
signed out, host GNOME    0 / 10 frozen
```

Sign-in is the variable. The host session reproduces it, so the 0/7 above was
too few launches rather than a property of the host. Whether nested sway
raises the rate is not separable at n=10; the intervals overlap.

**The same descriptor has now been caught in two states.** One frozen client
captured that day spun: the engine thread was in `looper_poll_once` with
`timeout_millis=0`, at 9.8 M polls a second and 103% CPU, which matches the
captures below. The three `0-gdb` captures in `docs/NEXT.md` (2026-09-01) found
the same pipe parked instead: `timeout_millis=-1`, clamped by
`BLOCK_CEILING_MS` to 20 polls a second, at 1.6% CPU. In both, Cordial's own
pump was healthy and nothing wrote the pipe. Two engine call sites waiting on
one pipe that is never written would explain both readings. That is
**INFERRED**: no session has caught both states. So a CPU reading alone does
not tell you whether a client is frozen.

## What a frozen client is doing

Two readings, and the second one is only meaningful because of the first.

```text
CPU  103%          one full core, so it is spinning and not blocked
loopers=2
  tid=498547  fds=2  polls=516          events=17  since_event=24991ms
  tid=498578  fds=1  polls=239,323,738  events=9   since_event=25067ms
```

**239 million polls in twenty-five seconds** -- about 9.6 million a second --
on a looper with one registered descriptor that has delivered nine events in
the life of the process, the last of them twenty-five seconds ago.

The backtraces name both halves:

```text
Thread 1  (LWP 498547) "Main"        -- Cordial's own pump
  epoll_wait
  looper::looper_poll_once (timeout_millis=50, ...)   looper.rs:1620
  looper::pump                                        looper.rs:1240
  cordial_run::main                                   load.rs:4063

Thread 53 (LWP 498578) "Main"        -- an engine thread
  epoll_wait
  looper::looper_poll_once (timeout_millis=0, ...)    looper.rs:1620
  0x00007effbf398bef in ??                            (inside libroblox.so)
```

So: **a Roblox thread is calling `ALooper_pollOnce` with a zero timeout in a
tight loop, waiting for something on that descriptor that never arrives**, and
burning a core doing it. Cordial's own pump is polling normally on the other
looper and seeing nothing either -- seventeen events, none for twenty-five
seconds.

`timeout_millis=0` is the engine's choice, not Cordial's. A zero timeout means
"tell me if anything is ready and return immediately", which is a poll designed
to be called from a loop that has other work. Here there is no other work: the
loop is all there is.

## Why the CPU reading is the load-bearing one

A spinning pump and a blocked one produce **identical backtraces** -- both sit
in `epoll_wait` -- and this project has got that backwards before. Without
`103%` beside the stack, thread 53 looks like a thread waiting patiently. The
looper census is the other half: `polls=239323738` against `events=9` says the
same thing in a form that survives being pasted into an issue.

## Three captures agree, and they name the descriptor

`d1` and `d2`, taken the same way, report the same shape as `cap2` -- and the
census now names what is registered rather than counting it:

```text
spinning  fds=1  [21:1:-]                    polls 244-254 million, events=9
main      fds=2  [19:-2:cb, 27:1131377252:-] polls ~500,       events=6-18
CPU       103% in all three; the spinning thread is the only one in state R
```

`21:1:-` is fd 21, ident 1, no callback. `/proc` says what it is:

```text
19 -> pipe:[12929303]      20 -> pipe:[12929303]   (write end, same process)
21 -> pipe:[12929304]      22 -> pipe:[12929304]   (write end, same process)
27 -> socket:[12922088]                            (the Wayland display)
```

So the spinning engine thread is polling **the read end of a pipe whose write
end is open in this same process**. Nothing is closed and nothing has gone
away; something simply stops writing. Ident 1 is the app-glue's main command
channel, and `APP_CMD` appears nowhere in Cordial -- these pipes belong to the
engine's own glue inside `libroblox.so`, not to anything this project wrote.

And Cordial's main thread is starved too: its Wayland display socket has
produced no event for twenty-five seconds.

**Correction, 2026-09-29: this census does not separate a frozen client from a
healthy one, and the reading above ("waiting for something that never arrives")
should not be used as evidence of the freeze.** Eight healthy signed-in clients
sampled at 60-400 s, `devctl loopers`: the engine's looper is `fds=1 [31:1:-]`
with `events=9` in seven of them and `events=6` in the eighth, `since_event`
equal to the age of the process, and 315-484 million polls. A healthy client
delivers those nine events during start-up and then never another on that
descriptor; it is simply not a channel that carries traffic afterwards. What
probably differs is that the poll count stops growing once presents pass 120
and the idle back-off engages, which a frozen client never reaches. `INFERRED`:
the counts at 66-107 s (315-484 million) and at 406 s (445 million, one run) are
in the same range, but no healthy client was sampled twice. The spin is a
consequence of the stall, as the 2026-09-28 entry in `docs/NEXT.md` already said
of the back-off gate, and the thread is not evidence of what the stall is
waiting for.

## Input does not recover a frozen client. Measured, and it refutes the obvious theory

The shape above suggests a starvation cycle -- the engine waits for a command,
the command comes from work the main thread drives, the main thread waits for
events, the events come from presenting, and presenting needs the engine. If
that were the whole story, one input event should break it.

It does not. Driven through Cordial's own entry points on a live frozen client:

```text
before                  presents=2  accepted=0
after 20 pointer moves  presents=2  accepted=20
4s later                presents=2  accepted=20
```

**The moves were accepted -- the count proves they reached the client -- and
nothing moved.** So whatever the engine thread is waiting for, an input event
is not it, and a frozen client does not come back.

This does **not** contradict the original report, and the distinction matters:
that report says input *prevents* the freeze, not that it recovers one. Those
are different claims and only the second is refuted here.

## Input does not prevent it either, with the caveat that matters

The remaining half of the original report -- that input *prevents* the freeze --
measured on a signed-in profile, ten runs per arm, strictly interleaved:

```text
arm A, no input     8 / 10 frozen
arm B, nudged       9 / 10 frozen
```

No effect, and the point estimate runs the wrong way for the theory.

**Read the instrument note before believing this arm.** It has been a silent
no-op three times in this project's history. The first two started a virtual
pointer after the client had latched seat capabilities. The third wrote into a
holder's fifo and was measured on 2026-08-27 delivering exactly nothing:
`accepted=0` on the client's own counter after a full run of it. Arm B in the
earlier ten-per-arm run was that version, and measured nothing at all.

This arm drives devctl `move`, which is the one path proven to arrive -- twenty
moves into a live frozen client took `accepted` from 0 to 20 -- and one
verification run showed `accepted=13` during startup. That run froze.

**The caveat, and it is not small.** devctl's socket only exists once the client
has bound it, which is well into startup. So this arm cannot deliver anything
during the earliest phase, and if the freeze is decided before the socket
appears, it has not been tested. What can be said is narrower than "input does
not prevent it": input delivered from the moment Cordial can accept it does not
prevent it.

## What this does not establish

- **Which side stops writing.** The descriptor is now named -- the read end of
  the engine's own app-glue command pipe -- and the write end is open in the
  same process. What is not known is what would have written to it on a healthy
  run and why it does not here. That is inside `libroblox.so`, so the way at it
  is the difference between a healthy and a frozen run's command sequence, not
  a debugger on engine code.
- **Whether the main thread is a cause or a casualty.** Seventeen events and
  nothing for twenty-five seconds is consistent with both.
- **Whether the nested compositor is required**, as above.
- **Whether this is one bug.** Three captures now agree on every reading, which
  is much better than one. They were taken minutes apart on one machine, so
  they establish a consistent shape rather than a general one.

## Repeating it

```bash
CAPTURE=1 TAG=cap tools/startup-freeze-survey.sh 1 CordialTest
```

Off by default: it costs a gdb attach on every frozen run, and the survey's job
is to count freezes rather than explain one.

gdb rather than lldb, deliberately. On a genuinely deadlocked client here lldb
produced one frame per thread, twice; gdb walked the same process and named
both halves. A one-frame backtrace is not an answer.

## Recovery and prevention, both attempted and neither proven, 2026-09-18

See `docs/NEXT.md`'s "Recovery and a settings-race prevention attempt" entry
for the full account. In short: `CORDIAL_STARTUP_RETRY` is re-confirmed to
make a frozen client worse (the retried call never returns, 5/5 this
session); a settings/flags-ordering fix aimed at matching Sober's clean
single-pass finalize (`CORDIAL_SYNC_BOOTSTRAP`) made no measured difference
to whether the finalize-retry line appears at all; and a whole-process
self-relaunch (`CORDIAL_STARTUP_SELFRELAUNCH`) is mechanically sound but was
observed to recover 0 of 3 fired attempts in this session's n=20 batch — not
enough evidence to trust a rate, and possibly evidence that a freeze this
soon after teardown is not an independent event. None of the three is
shipped as default.

## The settings document, 2026-09-24

Cordial fetched the `AndroidApp` client-settings document; the engine's own
reloader fetches `GoogleAndroidApp`. A Roblox rollout that day left
`AndroidApp` unable to start the client (grey window, 10/10), while
`GoogleAndroidApp` reached Landing 10/10. That is fixed in 89d494a (0.18.0).

**It does not fix the freeze, and the mismatch is not what causes it.** For a
few hours that day every run started in one cycle with no
`Forcing finalize`, which looked like the fix. It was the rollout of the
moment. Later the same day, signed in, n=10 each, interleaved:

| | two cycles | FROZEN | GOOD |
|---|---|---|---|
| pre-rollout `AndroidApp` document via `--client-settings` (mismatched) | 10/10 | 0/10 | 10/10 |
| default, `GoogleAndroidApp` (matched) | 10/10 | 3/10 | 7/10 |

The frozen runs have the classic signature: `sync cookies from engine`, then
no `~UgcExperienceController()`, presents stopped. Untested lead: the
control also differs in *how* the document arrives (`--client-settings`
reads a file; the default goes through the fetch and cache path). The arm
that separates the two is the `GoogleAndroidApp` document delivered with
`--client-settings`.

The startup experiments tried before this (`CORDIAL_SYNC_BOOTSTRAP`,
`CORDIAL_STARTUP_RETRY`, `CORDIAL_STARTUP_SELFRELAUNCH`,
`CORDIAL_SKIP_AGDK_SETTINGS` with a V1 `nativeAppBridgeAppStart`,
`CORDIAL_SOBER_ORDER`) were each refuted and are removed; they are recorded
in 179ac13.

## Later on 2026-09-24: delivery path and latency, both refuted

Signed in, interleaved, flag cache reset per run:

| condition | FROZEN |
|---|---|
| default, with `CORDIAL_SETTINGS_HISTORY`'s extra fetch in the callback | 3/10 |
| `--client-settings` file, same extra fetch | 0/10 |
| default, no extra fetch | 0/10 |
| `--client-settings` file of the same document, no extra fetch | 0/10 |
| default, stale cache (synchronous network fetch, 38-261 ms) | 1/8 |
| default, warm cache (0-4 ms) | 0/8 |
| warm cache plus a 300 ms sleep in the callback | 0/3 |

Neither how the document arrives nor how long delivery takes separates
frozen from good: the one stale-cache freeze had a 42 ms fetch, the 261 ms
one did not freeze, and an injected 300 ms did nothing.

**The rate itself has dropped**: 4 frozen in 49 signed-in runs across these
batches, against 40-70% before this day. Two things changed on the default
path that day: the settings document (`GoogleAndroidApp`, 89d494a) and
`DeviceStaticParams.osVersion` "15" to "33" (d803c47). The second can be
compared directly, `"15"` against `"33"` on one build; the first cannot while
the `AndroidApp` document blanks.

## The descriptor and the census, reconfirmed unchanged on `8c85be7`, 2026-09-28

Everything in "What a frozen client is doing" and "Three captures agree, and
they name the descriptor" above still holds, byte for byte in shape, on
current `main`: `polls` past a billion, `events=9`, `presents` fixed. See
`docs/NEXT.md`'s "the missing-AGDK-command theory is refuted live" entry for
what changed this time: a live `devctl redraw` verb was added and used to
feed the spinning thread's command pipe up to 95 more events on an
already-wedged client (one `onSurfaceRedrawNeededNative`, ten more, then a
full `fullscreen`/`windowed` resize cycle) with zero effect on `presents` or
the engine log. **The pipe accepting and counting an event is not the same as
the stall being about that pipe** — this was the mistake the "one missing
command" reading of the 09d315b redraw action invited, and it is now
measured rather than argued. The actual wait is confirmed to be inside
`UgcExperienceController`/`SingleSurfaceApp`'s own finalize retry, which
prints no further `FLog::` line of any kind once it fails to recover, on a
process whose every other thread (including `cordial-secrets`, checked and
found idly parked on its own job queue, not blocked mid-request) stays
observably healthy.

## 2026-10-04: the freeze is decided before the wait, at the first Lua app's teardown

Eight signed-in launches of `CordialTest` (162 s or more apart, all signed in,
no `DID_LOG_OUT`) on 192fdfc gave one freeze, under core pinning plus a busy
loop (1 of 3 amplified, 0 of 5 plain). Harness, logs and captures:
`~/.cache/cordial-handoff/freeze-92/`.

**The HttpClient-completion model does not survive.** With
`DFLogHttpTraceLight=7`, the frozen run had the same set of open requests at
`startLuaApp_` as healthy runs (16 against 17, same endpoints), every socket
established with bytes both ways, and the HttpClient completion thread still
waking (its condvar wait count grew between two captures 82 s apart). The
2026-09-29 watchpoint saw an HttpClient thread wake the finalize thread in
healthy runs; it never identified a request.

**The discriminator is earlier.** Across every signed-in engine log on this
machine with a `Forcing finalize` line (41, recounted independently):
`[Graphics] RenderView destroyed[1]`, logged on the finalize thread inside
`~SurfaceController[_:1]` 10-76 ms before the wait, appears in 22 of 22
healthy runs and 0 of 19 frozen ones. In a frozen run the first app's
RenderView and DataModel are never released.

**A timing correlate, post hoc.** The old DataModel's first render step ("No
data model. Bind workspace now") lands 2-9 ms after `getFlags: success` in 16
of 19 frozen logs and 1 of 19 healthy ones. The eight new runs, taken after the
rule was formed, fit it (the frozen one at +5 ms; all seven healthy outside).

**INFERRED:** a race in the engine between the first Lua app's first render
step and the teardown that `startLuaApp_` begins once settings and flags load;
when it loses, the finalize thread waits for a task nothing posts. Cordial does
not cause it, but it fixes when `initialize` is called relative to the first
render, so it likely sets how often the race is lost.

**Next:** (1) the lever: delay Cordial's `initialize` until the first render
step has run, measured against interleaved controls, at least 20 signed-in
launches per arm (several days at the launch cap); (2) meanwhile, recovery:
detect the frozen shape from the engine log (`Forcing finalize` with no
`RenderView destroyed[1]` before it and no `~UgcExperienceController` within
5 s) and relaunch the client.
