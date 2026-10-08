# Changing FastFlags

Roblox is configured by FastFlags, and Stacked lets you override any of them.
The quickest way is the command line, which checks each value against the
flag's type before saving it:

```bash
stacked flags set FIntTaskSchedulerAutoThreadLimit 8
stacked flags import bloxstrap.json    # a Bloxstrap export pastes in unchanged
stacked flags                          # what is set
```

Coming from Sober, `stacked flags import --sober` copies the `fflags` from its
`config.json`.

Roblox said in 2025 that its desktop clients would only honour an allowlist of
FastFlags. Whether that applies to the Android engine Stacked runs, and to the
way Stacked hands flags to it, hasn't been measured here. `FLogGraphics=0` and
`DFIntTaskSchedulerTargetFps` have both been seen to take effect.

[usage.md](usage.md#fastflags) has every `stacked flags` command. Underneath,
they edit `~/.local/share/cordial/profiles/<profile>/flags.json` (or the file
`CORDIAL_FLAGS` names), a flat JSON object you can also write by hand.
`stacked flags path` prints that path for the current profile, and
`--profile NAME` for another. In
a Flatpak you built yourself the sandbox moves `~/.local/share` to
`~/.var/app/io.github.damnshabu.Stacked/data`, so the same file is
`~/.var/app/io.github.damnshabu.Stacked/data/cordial/profiles/<profile>/flags.json`
— `INFERRED` from how Flatpak remaps `XDG_DATA_HOME`, not yet checked against an
installed package.

```json
{
  "DFFlagRbxTransportUseRtcioRna": false,
  "FIntTaskSchedulerAutoThreadLimit": 8,
  "FFlagDebugGraphicsDisableVulkan": false
}
```

**All three of those exist in the Android engine, and this example used to
carry one that does not.** It offered
`"FStringDebugGraphicsPreferredBackend": "Vulkan"`, which reads perfectly and
is not a Roblox flag: `DebugGraphicsPreferredBackend` appears **zero** times in
`libroblox.so`, and nothing resembling it does either — the real names in that
family are `DebugGraphicsDisableVulkan`, `DebugGraphicsDisableOpenGL`,
`DebugGraphicsDisableVulkan11` and so on. Reported by a user, checked against
the binary, and worth stating plainly because a documented example is the first
thing anybody copies.

**A name the engine does not know is accepted and ignored**, silently — it goes
into the settings document like any other key and nothing rejects it, so an
invented flag looks exactly like a working one. If a flag seems to do nothing,
check that it is real before assuming it did not help:

```bash
strings ~/.cache/cordial/lib/x86_64/libroblox.so | grep -x DebugGraphicsDisableVulkan
```

The name in the file carries the `FFlag`/`FInt`/`FString` prefix; the engine's
own table stores it without one, which is why the `grep` above drops it.

To choose a graphics backend, use `stacked config set graphics vulkan` (or
`gles`) rather than a flag — Stacked decides that before the engine starts, and
the setting is what it reads.

**Raising the frame rate takes two separate levers, and neither is in Roblox's
own menu.** The in-game settings have no frame-rate row because the *Android*
client has none — the Windows client does, and so do the desktop menus people
remember, but Stacked runs the Android build and nothing here can add a row the
client does not draw. Reported as a missing feature, which is a fair reading of
an interface that simply has no such control.

| What you want | Where it is |
|---|---|
| Stop drawing being pinned to your display's refresh | `stacked config set present_mode mailbox` (the default) or `immediate`, or the FPS Flex plugin — the same lever, so use one or the other |
| Raise the engine's own target frame rate | `stacked config set fps_cap N`, which sets the `DFIntTaskSchedulerTargetFps` FastFlag for you, or that flag in `flags.json` |

They are not the same setting and neither substitutes for the other: `present_mode`
is `VkSwapchainCreateInfoKHR::presentMode`, which decides whether a finished
frame waits for the next refresh, and the flag is what the engine's scheduler
aims at. Leaving the first on FIFO caps you at your panel's rate whatever the
flag says.

**One report of the flag not holding**, made against Cordial, on a machine
that reached 240 and fell back to 60 after a few minutes. Not reproduced here and not explained; if you
see the same, [say so on the tracker](https://github.com/DamnShabu/stacked/issues)
rather than assuming your value was wrong.

Values may be written as booleans, numbers or strings — Roblox stores them all
as strings and Stacked converts. The overrides are merged into the settings
document the engine is given at startup, and the launch log reports how many
were applied.

**`FFlag`, `FInt` and `FString` are read once at startup**, so changing them
needs a relaunch. Only the `DFFlag`/`DFInt`/`DFString` family is re-read while
the client is running. That distinction matters if you are building anything
that changes flags dynamically — a plugin loaded part-way through a session
cannot change a startup flag, whatever it writes.

## Layers and provenance

Flags come from more than one place, and each source owns its own file:

```text
<profile>/flags.json                             user    (always wins)
shell.json's fps_cap                             launcher setting
CORDIAL_QUALITY                                  launcher setting
~/.local/share/cordial/plugins/<id>/flags.json   plugin
the client-settings document from Roblox         base
```

`CORDIAL_QUALITY` is for a launcher starting several clients at once, such as
the Roblox manager's performance levels: `low`, `medium` or `max` sets a
graphics-quality preset for that one client (`high`, or unset, sets nothing).
The flags are listed in `crates/cordial-runtime/src/graphics_quality.rs`.
Whether they change anything in a 3D game on this engine has not been
measured yet.

Your overrides live in the profile, so a flag you set while testing something on
one account is not silently still set on the account you play. A file left at
the old `~/.config/cordial/flags.json` is moved into the first profile that goes
looking for one — see [ADR-013](adr/ADR-013-per-profile-configuration.md).

A plugin never writes to your file. That keeps three things true: a plugin
cannot silently overwrite a value you chose, removing a plugin removes its
flags, and "why is this flag set to that?" has an answer. Conflicts are reported
rather than resolved quietly:

```text
flags: FIntTaskSchedulerAutoThreadLimit = 8 from user
       (overrides plugin:fps-tweaks=4, plugin:net-tuner=16)
```

Two plugins disagreeing is a real disagreement, so both are named. The later one
wins so the outcome is deterministic, but nothing is hidden.

**If the interface looks coarse**, it is being laid out for a low-density phone.
Raise both — the render resolution is 720p by default and `dpiScale` is 1.0,
which is what Roblox treats as a cheap handset. `stacked` passes its
environment on to the client:

```bash
CORDIAL_RESOLUTION=1920x1200 CORDIAL_DPI_SCALE=1.75 stacked
```

Roblox's graphics-quality FastFlags (`DebugFRMQualityLevelOverride` and the MSAA
overrides) were tested and change nothing here, because they govern 3D scene
rendering and the logged-out landing page is a 2D interface. Resolution and
density are the levers that apply to it.
