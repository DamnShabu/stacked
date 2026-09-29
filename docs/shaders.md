# Shaders (vkBasalt): sharpening and anti-aliasing over the game

Stacked can hand the client's frame to
[vkBasalt](https://github.com/DadSchoorse/vkBasalt), an open-source Vulkan
implicit layer, for a sharpen pass and an anti-alias pass before it reaches the
screen. Off by default, because it changes what is drawn:

```bash
stacked config set vkbasalt true
```

Every launch checks that vkBasalt's layer is actually installed before using
it. If it is not, the launch output says so, with how to install it, and the
game runs without shaders rather than with a setting that does nothing.

## Install

- **Fedora / rpm-ostree layering:** `dnf install vkBasalt` (`sudo dnf install
  -y vkBasalt` in a `distrobox` if the host is immutable).
- **Arch:** `pacman -S vkbasalt` (multilib for a 32-bit game).
- **Flatpak:** `flatpak install org.freedesktop.Platform.VulkanLayer.vkBasalt`.
  This is for a Flatpak you built yourself. It is a runtime extension, not a
  change to Stacked's package — it mounts under `org.freedesktop.Platform`'s
  own `VulkanLayer` extension point, the same one MangoHUD uses, so the
  manifest needs nothing added for it to be seen.

The next launch after installing picks it up.

## Config

The first launch with the setting on writes
`<profile>/vkBasalt.conf` inside that profile's own data directory (next to its
`appData` and cookie store) with sharpening (CAS) and anti-aliasing (SMAA) at
vkBasalt's own documented defaults. **Stacked never rewrites this file again**
— edit the effects list, the sharpening strength, or anything else vkBasalt
supports, and your changes stay. Every launch with vkBasalt on prints the
exact path, in a line starting `shell: vkBasalt on`.

The full key reference is vkBasalt's own:
<https://github.com/DadSchoorse/vkBasalt/blob/master/config/vkBasalt.json.in>.

## Toggle key

Stacked's generated config sets `toggleKey = Scroll_Lock`, not vkBasalt's own
`Home` default. Home is a real Roblox chat key — it jumps the cursor to the
start of a line — and vkBasalt does not consume the key or care which window
has focus, so the upstream default would toggle the effect on and off every
time somebody typed a message starting with that jump.

**On the default Wayland backend, the toggle key does nothing at all.**
Confirmed by reading vkBasalt's own source
(`src/keyboard_input_x11.cpp`): it polls a real X11 keyboard with
`XQueryKeymap`, and does so only when `$DISPLAY` is set. Wayland sets
`WAYLAND_DISPLAY`, not `DISPLAY`, so with no XWayland running the check
degrades to "no X11 support" and the key can never register as pressed. The
generated config sets `enableOnLaunch = True` for exactly this reason: it is
the only lever there is on Wayland. The toggle key works on the X11
backend, or if XWayland happens to be running alongside a Wayland session.

## What was verified and how

- **The layer loads.** Running the client with `ENABLE_VKBASALT=1` and
  `VKBASALT_LOG_LEVEL=info` shows the Vulkan loader inserting
  `VK_LAYER_VKBASALT_post_processing` as both an instance and a device layer,
  and vkBasalt logging the exact config file and values Cordial generated.
- **The effect is visible.** Compared with `grim`, taken in a nested Wayland
  compositor rather than through `cordial_screenshot` — Cordial's own
  screenshot verb reads the frame out of its Vulkan swapchain, which is filled
  before vkBasalt's layer runs, so it cannot show the layer's own work. The
  landing screen's edges are visibly sharper with the layer on; the
  pixel-level difference is real but modest on that mostly-flat screen, and was
  not checked against in-game 3D content.
- **Frame cost**, CPU on the whole `cordial-run` process with synthetic pointer
  input flowing continuously for 60 s, two runs each, on the landing screen
  only (a throwaway signed-out profile, not a loaded game): roughly 6.7% CPU
  with shaders off and 7.0–7.1% with them on. The frame rate itself did not
  move in this measurement, because it was paced by the synthetic input rate
  in a headless nested compositor rather than by a real display's vsync — not
  a general "vkBasalt costs nothing" claim, just what this one screen and this
  one input pattern showed.
- **Layers are not disabled.** Cordial's own Vulkan interposition
  (`crates/cordial-runtime/src/android/vulkan.rs`) forwards
  `enabled_layer_count` and `pp_enabled_layer_names` unchanged when it patches
  `vkCreateInstance`, and nothing in Cordial sets `VK_LOADER_LAYERS_DISABLE` or
  any other loader variable that would suppress an implicit layer.

**Untested:** AMD and NVIDIA GPUs — the above was measured on the Mesa driver
present in the build container. vkBasalt's own anti-cheat interaction risk
(Sober disabled and later restored MangoHUD and vkBasalt over exactly this
concern — [sober#868](https://github.com/vinegarhq/sober/issues/868)) is the
same one Cordial already accepts for MangoHUD; see
[ADR-041](adr/ADR-041-vkbasalt-post-processing.md).
