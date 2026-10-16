# Known issues

These are the open bugs reported against [Cordial](https://github.com/luohoa97/cordial/issues),
the project Stacked is forked from, and where each one stands in Stacked. The
engine layer is the same code, so anything reported there applies here unless
this page says otherwise.

"Changed, unverified" means the code now handles the reported cause, but
nobody has confirmed it on the hardware or compositor where the bug happens.
If you have that setup, a report either way helps.

| Issue | Symptom | In Stacked |
|---|---|---|
| [#31](https://github.com/luohoa97/cordial/issues/31) | Keyboard dies after another app takes focus (Hyprland) | **Fixed in the code the fork started from.** Hyprland names the canvas surface on refocus, and that surface is now accepted. |
| [#52](https://github.com/luohoa97/cordial/issues/52) | Client sometimes hangs on exit and ignores SIGTERM | **Changed, unverified.** If the engine's shutdown callbacks haven't returned after 10 seconds, the client says so and exits with status 124, so a cut-off no longer looks like a clean exit and is not saved as a crash. Cookies are saved before those callbacks start. `CORDIAL_TEARDOWN_TIMEOUT_S` changes the limit. |
| [#39](https://github.com/luohoa97/cordial/issues/39) | Fullscreen freezes, and leaving it crashes | **Changed, unverified here.** From upstream: on a Wayland fullscreen resize the driver reports a swapchain image as `SUBOPTIMAL`, and passing that on to the engine was followed by a crash; it now counts as success, and a resize reaches the engine at most every 100 ms during a drag instead of only once it stops. |
| [#41](https://github.com/luohoa97/cordial/issues/41) | X11: camera spins 180°, and movement continues for a few seconds | **Changed, unverified on a real X session.** From upstream: queued mouse motion was counted several times over, and motion from before the pointer locked turned into a constant spin; both measured fixed on Xvfb. Keys held at focus loss are released. Auto-repeat now reaches the engine as one press, so a hold-to-interact bar no longer restarts. |
| [#56](https://github.com/luohoa97/cordial/issues/56) | Hyprland never confirms the pointer lock, and the cursor drifts | **Partly addressed, unverified.** Fullscreen *confinement* is now placed on the surface Hyprland gives pointer focus to, and pointer input on that surface reaches the game in fullscreen. The camera *lock* is unchanged. |
| — | Pointer lock never engages on KDE Plasma (Wayland); the cursor leaves the window during a camera drag or in first person | **Changed, unverified.** KWin only locks the pointer inside the window's input region, which had the game area cut out of it, and only applies a lock or its release when the window next commits. Both are now handled. Not yet run on Plasma; `CORDIAL_TRACE_MOUSE=1` shows whether the compositor sends `locked`. |
| [#36](https://github.com/luohoa97/cordial/issues/36) | First touch on a touchscreen crashes | **Changed, unverified on real hardware.** From upstream: the engine's surface now has a GTK surface behind it, so a touch no longer faults GTK. Tested upstream with a simulated touchscreen only. |
| [#44](https://github.com/luohoa97/cordial/issues/44) | Crash on second launch ("TaskScheduler before flags") | **Fixed in the code the fork started from**, and confirmed by its reporter upstream. The client waits for the settings to be delivered before starting the engine's task scheduler. |
| [#35](https://github.com/luohoa97/cordial/issues/35) | SIGSEGV at launch on the Steam Deck | **Open.** Closed upstream with no code fix. The DeviceUtils error in the report also appears in logs of runs that didn't crash, and no dex in the APK declares that class. A `coredumpctl info` from a crashing launch is what would move it. |
| [#38](https://github.com/luohoa97/cordial/issues/38) | No window on COSMIC, KWin and wlroots compositors | **Open.** It doesn't reproduce on a headless sway 1.9 with GTK 4.14, and the report's log isn't reachable from here. |
| [#53](https://github.com/luohoa97/cordial/issues/53), [#54](https://github.com/luohoa97/cordial/issues/54) | Typing a camera sensitivity greys the screen | **Open.** The code rules out the canvas being left hidden. What's needed is a run with `CORDIAL_TRACE_TEXT=1`, then a second with `CORDIAL_PASS_TEXT_ON_KEY=1`, to see whether the value applies. |
| [#64](https://github.com/luohoa97/cordial/issues/64), [#29](https://github.com/luohoa97/cordial/issues/29) | Keyboard sometimes dead after joining an experience | **Changed, unverified in Stacked.** From upstream: while a game starts, the client hands the engine pointer hovers, which upstream measured as walking in 4 of 4 joins against 0 of 18 without. `CORDIAL_NO_GAME_SEED=1` turns it off; `CORDIAL_TRACE_KEYS=1` prints every key handed to the engine. |
