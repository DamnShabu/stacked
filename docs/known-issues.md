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
| [#52](https://github.com/luohoa97/cordial/issues/52) | Client sometimes hangs on exit and ignores SIGTERM | **Changed, unverified.** If the engine's shutdown callbacks haven't returned after 10 seconds, the client says so and exits. Cookies are saved before those callbacks start. `CORDIAL_TEARDOWN_TIMEOUT_S` changes the limit. |
| [#39](https://github.com/luohoa97/cordial/issues/39) | Fullscreen freezes, and leaving it crashes | **Changed, unverified.** While the title bar animated in and out, the engine got a new size, and rebuilt its swapchain, on every frame. It now gets a size only once it has held for 120 ms. |
| [#41](https://github.com/luohoa97/cordial/issues/41) | X11: camera spins 180°, and movement continues for a few seconds | **Half changed, unverified.** When the window loses focus, keys still held are now released, as the Wayland backend already did. The 180° spin is not understood yet. |
| [#56](https://github.com/luohoa97/cordial/issues/56) | Hyprland never confirms the pointer lock, and the cursor drifts | **Partly addressed, unverified.** Fullscreen *confinement* is now placed on the surface Hyprland gives pointer focus to, and pointer input on that surface reaches the game in fullscreen. The camera *lock* is unchanged. |
| [#36](https://github.com/luohoa97/cordial/issues/36) | First touch on a touchscreen crashes | **Open.** No cause can be found from the code. To narrow it down, run three times with `CORDIAL_TRACE_TOUCH=1`: once with `CORDIAL_NO_AGDK_TOUCH=1`, once with `CORDIAL_NO_PASS_INPUT=1`, and once with neither. The last line printed before the crash names the call that failed. |
| [#44](https://github.com/luohoa97/cordial/issues/44) | Crash on second launch ("TaskScheduler before flags") | **Mitigated in the code the fork started from.** The client waits for the settings to be delivered before starting the engine's task scheduler. It is still open upstream. |
| [#35](https://github.com/luohoa97/cordial/issues/35) | SIGSEGV at launch on the Steam Deck | **Open.** The DeviceUtils error in the report also appears in logs of runs that didn't crash. The Vulkan result logged before the crash reaches the engine unmodified. |
| [#38](https://github.com/luohoa97/cordial/issues/38) | No window on COSMIC, KWin and wlroots compositors | **Open.** It doesn't reproduce on a headless sway 1.9 with GTK 4.14, and the report's log isn't reachable from here. |
| [#53](https://github.com/luohoa97/cordial/issues/53), [#54](https://github.com/luohoa97/cordial/issues/54) | Typing a camera sensitivity greys the screen | **Open.** The code rules out the canvas being left hidden. What's needed is a run with `CORDIAL_TRACE_TEXT=1`, then a second with `CORDIAL_PASS_TEXT_ON_KEY=1`, to see whether the value applies. |
| [#64](https://github.com/luohoa97/cordial/issues/64), [#29](https://github.com/luohoa97/cordial/issues/29) | Keyboard sometimes dead after joining an experience | **Open.** The reports have no key trace yet. |
