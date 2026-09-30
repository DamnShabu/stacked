# Using Stacked

`stacked` is a command-line launcher. It never opens a window of its own: it
finds Roblox, picks a profile, turns your settings into the client's
environment, starts `cordial-run`, and waits. The only window is the game's.
The reasons are in [ADR-043](adr/ADR-043-the-launcher-is-a-command-line.md).

`stacked help` prints the same command list as below.

## Commands

| Command | What it does |
|---|---|
| `stacked`, `stacked play` | Start Roblox on the current profile and wait for it to exit. The first time, this downloads Roblox. After that it installs a newer build first, if there is one (see [Updates](#updates)). |
| `stacked play --profile NAME` | Start Roblox on a different profile. |
| `stacked play --no-update` | Start the build you have without checking for a newer one. |
| `stacked play --run SECS` | Stop the client after SECS seconds. Useful for testing. |
| `stacked LINK` | Join a `roblox-player:` or `roblox:` link. This is what the desktop entry runs when you press Play on the website. |
| `stacked status` | Show which profile and Roblox build a launch would use. |
| `stacked install` | Find a Roblox build, or download one if there isn't one. A copy Sober has already downloaded counts. |
| `stacked update` | Download the newest build. |
| `stacked versions` | List the builds kept on disk. `versions available` lists the ones you can download. `versions get V` downloads one, and `versions remove V` deletes one. |
| `stacked pin V`, `stacked unpin` | Make a profile always run build V, or go back to the current build. Both take `--profile`. |
| `stacked profiles` | List profiles. `profiles new NAME` creates one, and `profiles use NAME` makes it the current one. |
| `stacked config` | List every setting with its current value (see below). |
| `stacked flags` | List, set and remove the current profile's FastFlags (see [FastFlags](#fastflags)). |
| `stacked audio-outputs` | List the names `audio_output` accepts. |
| `stacked plugins …` | Manage plugins (see below). |
| `stacked diagnostics` | Print the version, distribution and install method for a bug report. |

Exit status is 0 on success, 1 on failure, 2 for a usage mistake, and 3 when
the profile is already open in another client.

**Ctrl-C** stops the game the same way closing its window does. `stacked`
stays running until the client has exited, then reports whether it crashed.

**F11** toggles fullscreen in the game window.

## Settings

`stacked config set KEY VALUE` changes a setting. `stacked config unset KEY`
puts it back to the default. The settings live in
`~/.config/cordial/shell.json`, the same file Cordial used.

`set` refuses an unknown key or a value of the wrong kind rather than saving
something that silently does nothing. `stacked config` lists every key with the
values it accepts.

| Key | Default | Values |
|---|---|---|
| `profile` | `default` | Which profile `stacked` plays on. |
| `auto_update` | `true` | Install a newer Roblox build, if there is one, when you press Play. `false` leaves it to `stacked update`. |
| `theme` | `stacked` | `stacked` or `system`. The title bar colours. `system` follows your desktop. |
| `title_bar` | `default` | `default`, `compact` or `hidden`. It is always hidden in fullscreen. |
| `fullscreen_confine` | `true` | Keep the cursor on the window while it is fullscreen. |
| `fps_cap` | unset | Frame-rate target. Unset follows the refresh rate of the screen the game is on. `0` uses the engine's own target, 60. `1` to `1000` sets a fixed target. |
| `present_mode` | `mailbox` | `mailbox` (low latency), `fifo` (vsync, least power), `immediate` (tears), or `automatic`. |
| `graphics` | `automatic` | `automatic`, `vulkan` or `gles`. |
| `graphics_optimization_mode` | `balanced` | `balanced`, `roblox-app`, `mobile-tier`, `more-cores` or `fewer-cores`. None of these is measured to help. |
| `gamemode` | `true` | Ask Feral GameMode for the performance governor while you play. |
| `throttle` | `visible` | `visible`, `unfocused` or `off`. When the engine is allowed to slow down in the background. |
| `pointer_acceleration` | `unlockedcursor` | `unlockedcursor` or `always`. Whether desktop pointer acceleration also affects the camera. |
| `gamepad` | `true` | Controllers. |
| `audio_output` | empty | An output name from `stacked audio-outputs`. Empty follows the system. |
| `close_on_leave` | `false` | Close Roblox when you leave an experience. |
| `mangohud` / `vkbasalt` | `false` | Overlays, if they're installed. See [shaders.md](shaders.md) for vkBasalt. |
| `carry_launch_ticket` | `false` | Hand a website Play button's one-use login ticket to the engine as well. |
| `unpacked_plugins` | `[]` | Plugin folders loaded in place, for developing a plugin. |
| `roblox.apk`, `roblox.lib_dir` | unset | Run a specific APK or extracted engine instead of the one Stacked finds. |

### About `fps_cap`

The engine thinks it is running on a phone and targets 60 fps whatever screen
it is on. So by default, Stacked reads your screen's refresh rate before the
game starts and sets that as the target instead. A 144 Hz monitor gets a
target of 144, and a 59.94 Hz one gets 60.

With several screens, the game's window doesn't exist yet when the target is
chosen, so the fastest screen's rate is used. It's also chosen once, at
launch: moving the window to a slower screen doesn't lower it. X11 isn't
covered either; there the engine keeps its own 60.

Setting `fps_cap` to a number replaces the automatic target with that number,
and `0` gives the engine's own target back. The target becomes
`DFIntTaskSchedulerTargetFps`, layered above plugins and below your own
`flags.json`, so a value in that file still wins.

That flag has been measured to **lower** the frame rate. Whether it can
**raise** the rate above 60 has not been measured here. It can't take the rate
past your screen's refresh rate under `fifo`.

### About `fullscreen_confine`

While the window is fullscreen and the game isn't locking the pointer, for
example in menus, Stacked asks the compositor to keep the cursor inside the
window. Alt-tab and your desktop's usual shortcuts still release it, because
the compositor controls that and not the game.

It is dropped when one of Stacked's own dialogs or the text editor is in
front, so the cursor can still reach them.

On Hyprland it goes on the window's main surface, because Hyprland keeps
pointer focus there. That path hasn't been run on Hyprland itself.

It has been checked against sway, which accepted the request and confined the
pointer. It has not been tried on a real multi-monitor desktop, or on X11. If
it misbehaves, run `stacked config set fullscreen_confine false`.

## Updates

Roblox stops letting old clients join, usually within a week or two of a new
build. So `stacked play` asks whether a newer build is available and, if there
is one, installs it before the game starts. The first launch on a new machine
downloads Roblox the same way.

The check takes one small request. It gives up after five seconds, and after a
check that found nothing new it doesn't ask again for ten minutes. If it can't
get an answer, or the download fails, the build you already have starts.

It doesn't run when:

- you chose the APK yourself (`roblox.apk` or `CORDIAL_APK`), since Stacked
  won't replace that
- the profile is pinned to a version (`stacked pin`)
- NetworkManager says the connection is metered. Stacked tells you a newer
  build exists, and `stacked update` gets it when you choose
- `auto_update` is `false`, or you pass `--no-update`

`stacked status` says which of these applies.

## FastFlags

Each profile has its own FastFlags file. The commands below edit it, and each
takes `--profile NAME`. Changes apply the next time Roblox starts.

```bash
stacked flags                                   # list them
stacked flags set DFIntTaskSchedulerTargetFps 144
stacked flags set FFlagDebugDisplayFPS true     # written as True
stacked flags unset FFlagDebugDisplayFPS
stacked flags import bloxstrap.json             # merge a Bloxstrap export; --replace to start over
stacked flags edit                              # open it in $EDITOR
stacked flags path                              # where the file is
```

`set` checks the value against the type in the flag's name. `FFlag…` takes
`true` or `false`, `FInt…` and `FLog…` take a whole number, and `FString…`
takes any text. A `D` or `S` in front doesn't change the type. A name without
one of those prefixes is saved with a warning.

`import` and `edit` save nothing if any value is invalid, and say which one.
`edit` works on a copy, so a half-finished edit never reaches the game. More on
what flags do, and which ones Stacked sets itself, is in
[fastflags.md](fastflags.md).

## Themes

The game window's title bar follows `theme`.

- **`stacked`** (the default) uses the icon's palette: a dark brown bar with
  warm text and an amber accent, always in dark mode.
- **`system`** uses your desktop's colours and light or dark setting,
  unchanged.

The title bar is hidden in fullscreen either way. `title_bar compact` makes
it shorter, and `title_bar hidden` removes it.

## Plugins

Plugins run as separate processes with named capabilities. They are
default-deny, and grants are per profile. See [plugins.md](plugins.md).

```bash
stacked plugins                       # what is installed, on or off, and what is granted
stacked plugins deno                  # plugins with code run on Deno; this installs it if it is missing
stacked plugins install thing.tar.zst # a plugin with code starts switched off
stacked plugins grant thing presence.set
stacked plugins enable thing
stacked plugins disable thing
stacked plugins revoke thing presence.set
stacked plugins remove thing
```

`grant` accepts only the capabilities the plugin's manifest asks for;
`stacked plugins` lists them. Plugin preferences have no command and are edited
in the plugin's `preferences.json` ([plugins.md](plugins.md)).

`enable`, `disable`, `grant` and `revoke` take `--profile NAME`. A client that
is already running picks up the change within a second or two.

## Environment variables

These are read by the client. The settings above set most of them for you;
the rest are for testing and troubleshooting.

| Variable | Effect |
|---|---|
| `CORDIAL_NO_FULLSCREEN_CONFINE=1` | Don't confine the cursor in fullscreen. |
| `CORDIAL_NO_POINTER_LOCK=1` | Never capture the cursor, even for first-person or a camera drag. |
| `CORDIAL_FPS_CAP=N` | The same as `fps_cap`. `0` is the engine's own target; unset follows the screen. |
| `CORDIAL_THEME=stacked\|system` | The same as `theme`. |
| `CORDIAL_PRESENT_MODE=…` | The same as `present_mode`, plus `uncapped` and `fifo-relaxed`. |
| `CORDIAL_GAMEMODE=0` | Don't ask for GameMode. |
| `CORDIAL_POLL_COALESCE_US=N` | How long the engine's main loop sleeps when it has nothing to do, in microseconds. The default is 250. `0` turns it off, which costs a whole CPU core. |
| `CORDIAL_TEARDOWN_TIMEOUT_S=N` | How long the engine gets to shut down before the client exits without it. The default is 10. |
| `CORDIAL_APK=PATH` | Run this APK for this launch only. |
| `XDG_DATA_HOME=DIR` | Move every profile and all client data somewhere else. |

Profiles and data are still stored under `~/.local/share/cordial`, and
downloads under `~/.cache/cordial`. Those paths are Cordial's, and they were
kept so that an existing sign-in carries over.
