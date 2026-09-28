# ADR-041: vkBasalt post-processing is a driver-stack layer, not in-process hooking

**Status:** accepted
**Date:** 2026-09-29
**Supersedes:** nothing
**Related:** [ADR-001](ADR-001-in-process-hooking.md), [ADR-003](ADR-003-plugin-isolation.md), [ADR-011](ADR-011-wayland-and-libadwaita.md)

## Decision

Cordial offers an opt-in, off-by-default Settings switch that sets
`ENABLE_VKBASALT=1` and `VKBASALT_CONFIG_FILE` on the client's environment when
[vkBasalt](https://github.com/DadSchoorse/vkBasalt) (zlib licence) is detected
on the machine. Cordial writes a starting config — CAS sharpening plus SMAA
anti-aliasing, at vkBasalt's own documented defaults — the first time a profile
turns the switch on, and never touches that file again. See
[`docs/shaders.md`](../shaders.md) for the user-facing detail; this records why
the feature is in scope at all.

## Why this does not need ADR-001 revisited

ADR-001 rejects one specific thing: **Cordial-authored code with access to the
Roblox process's own memory or code**, because such a facility cannot be
governed once it holds that authority — nothing outside the process can revoke
it mid-run, and no client-side integrity signal can be trusted to say whether
it is present. That argument is about where authority lives relative to the
process boundary. vkBasalt does not cross that boundary in the direction ADR-001
is about.

**What vkBasalt actually is, read from its own source rather than assumed:**

- It is a Vulkan **implicit layer** — a shared object the platform's own
  Vulkan loader inserts between the application and the driver, the same
  mechanism `VK_LAYER_MESA_device_select` and validation layers use, and the
  same mechanism Cordial already ships a settings switch for: MangoHUD
  (`crates/cordial-shell/src/launch.rs`, `mangohud_layer`/`mangohud`). The
  loader decides whether to insert it, based on a manifest under
  `/usr/share/vulkan/implicit_layer.d` and the `ENABLE_VKBASALT` environment
  variable it declares — Cordial's part is exactly the same as `MANGOHUD=1`:
  setting a variable the loader itself acts on. Confirmed by installing the
  Fedora package and reading its manifest, `/usr/share/vulkan/implicit_layer.d/vkBasalt.json`,
  which declares `"enable_environment": {"ENABLE_VKBASALT": "1"}`.
- It intercepts `vkQueuePresentKHR` and the swapchain images the *driver*
  sees, not `libroblox.so`'s address space. Cordial's own Vulkan interposition
  in `crates/cordial-runtime/src/android/vulkan.rs` forwards
  `enabled_layer_count` and `pp_enabled_layer_names` unchanged when it patches
  `vkCreateInstance` (see `vk_create_instance`'s `patched` struct, built with
  `..*info`), so a layer the loader would otherwise insert is not being
  specially permitted by Cordial's shim — it was never in Cordial's shim's
  power to exclude one silently in the first place, short of setting a loader
  variable Cordial does not set (`VK_LOADER_LAYERS_DISABLE` and similar are
  absent from this codebase; checked by grep).
- Cordial holds no vkBasalt code, patches nothing into it, and cannot revoke it
  mid-run any differently than it can revoke the Mesa driver itself — which is
  to say, by not launching the next process with the variable set. That is
  the ordinary shape of every other environment-variable capability Cordial
  already offers (`CORDIAL_GRAPHICS`, `CORDIAL_PRESENT_MODE`, GameMode), not
  the shape ADR-001 rejects.

So the question ADR-001 answers — "should Cordial's own code get to run inside
Roblox's process and rewrite it" — is not the question a Vulkan layer raises.
The engine's pixels pass through vkBasalt on their way to the compositor in
exactly the sense they already pass through Mesa; neither is Cordial code, and
neither is governed by an in-process capability Cordial would have to police.

**Argue with this if you think it is wrong.** The place this reasoning could
fail is if vkBasalt's own implementation reached into the application process
rather than operating purely on the driver side of the swapchain — it does
not, per its source (`src/vkdispatch.cpp`'s device/instance dispatch tables
operate only on the Vulkan handles the loader hands it), but that is the
specific claim to check if this ADR is revisited.

## Why it is off by default and gated on detection

Same reasoning as MangoHUD, which is the direct precedent this feature copies
end to end (settings row shape, detection function, Flatpak install hint,
per-profile storage):

- It changes what is drawn whether or not the user meant it to, so it is asked
  for rather than assumed — unlike GameMode, which is invisible when it works
  and invisible when it silently does not.
- `ENABLE_VKBASALT=1` with no vkBasalt installed is not an error the loader
  reports; the client starts exactly as it would without it, with no shaders
  and nothing said. A switch that does not check first is a switch that
  appears to work and does nothing — the exact failure AGENTS.md's stub
  guidance is about, one layer out in a settings row instead of a stub
  function. `launch::vkbasalt_layer` is the check; the settings row disables
  itself and names the missing package when it fails.

## The toggle key is a deliberate departure from upstream

vkBasalt's own example config ships `toggleKey = Home`. Read from
`src/keyboard_input_x11.cpp`, vkBasalt polls a real X11 keyboard with
`XQueryKeymap` regardless of which window has focus — it does not consume the
key or go through Cordial's input path at all. Home is also a real Roblox chat
key (cursor to line start), so the upstream default would toggle the effect on
and off as a side effect of typing. Cordial's generated config sets
`toggleKey = Scroll_Lock` instead, and `enableOnLaunch = True` so the effect is
reachable at all on Wayland, where `$DISPLAY` is normally unset and the poll
above never fires. Both are recorded in `docs/shaders.md` and pinned by a unit
test (`vkbasalt_toggle_key_is_not_a_key_roblox_chat_uses`) so neither
regresses quietly.

## What this does not settle

**Anti-cheat interaction is a real, disclosed risk Cordial already accepts for
MangoHUD, and vkBasalt is the same shape of layer.** Sober — running the same
engine — disabled both MangoHUD and vkBasalt over exactly this concern and
later restored them
([sober#868](https://github.com/vinegarhq/sober/issues/868), comment: *"Mangohud,
by design, uses very similar hooking mechanisms seen inside of cheating
software... A similar incident happened with Apex Legends... falsely banned
players"*). Cordial already ships the MangoHUD switch with this risk
unaddressed beyond the user's own choice to enable it; this ADR does not
introduce a new risk category, and does not attempt to resolve the one that
already exists.

**Not verified on AMD or NVIDIA.** Measured only against the Mesa driver
available in the build container (`docs/shaders.md` has the readings). The
loader-level mechanism (implicit layer manifest, `ENABLE_VKBASALT`) is
vendor-independent by design, so there is no specific reason to expect a
different result, but that is an inference from the mechanism, not a
measurement on that hardware.

**vkBasalt's config is written once, generated from vkBasalt's own documented
defaults (`config/vkBasalt.json.in`, zlib licence), not copied from VineShade**
— the Sober-community project that packages vkBasalt with a tuned config for
Roblox. VineShade's repository carries no licence file, so nothing in it may be
copied; only the idea ("vkBasalt plus a launcher, tuned for Roblox") was
taken, which is the same line AGENTS.md draws for Sober's binary and mocktail's
source.
