# ADR-060: Vulkan is withheld where the compositor has no dmabuf

**Status:** accepted
**Date:** 2026-10-15
**Related:** [ADR-011](ADR-011-wayland-and-libadwaita.md), [ADR-001](ADR-001-in-process-hooking.md)
**Issue:** #63

## Context

#63 was an intermittent `SIGSEGV` at `libroblox.so+0x29b4349`, fault address `0x5a0`, shortly after `engine initialised`. `docs/NEXT.md` traced it to the engine's recorder for failed Vulkan calls, which reads a graphics-device pointer that is still null during startup, and to the failed call that triggers it: `vkGetPhysicalDeviceSurfaceCapabilitiesKHR` returning `VK_ERROR_SURFACE_LOST_KHR` on compositors that advertise no `zwp_linux_dmabuf_v1` (sway on its pixman renderer, weston headless, cage). It also recorded that asking again forty times over 400 ms changed nothing.

## What makes the query fail

Not Cordial's surface, and not timing. Mesa 26.1's Wayland WSI (`src/vulkan/wsi/wsi_common_wayland.c`) initialises a display for every capability query. In `registry_handle_global` it binds `wl_shm` only when `wsi_device->sw` is set and `zwp_linux_dmabuf_v1` (version 3 or later) only when it is not. If neither was bound after the first round trip, `wsi_wl_display_init` returns `VK_ERROR_SURFACE_LOST_KHR` (`!display->wl_dmabuf && !display->wl_shm`) and `wsi_wl_surface_get_capabilities` passes it on. A hardware driver is not a software device, so with no dmabuf global it has nothing to present through.

Measured with a 60-line C program that makes a Wayland surface and asks, with no Roblox in the process: on sway with the pixman renderer the Intel device returned `-1000000000` for three queries out of three and llvmpipe returned success three out of three; on sway with the GLES renderer both succeeded. The same Intel device succeeded on pixman sway with `MESA_VK_WSI_DEBUG=sw`, which is the switch that sets `wsi->sw`.

So the engine does not lose a surface "first": it fails on both of its two device creations per launch, because the device it chose is the hardware one. It skips llvmpipe on its own (`Vulkan: Device llvmpipe ... is emulated, skipping`), so there is no device it accepts that could present.

## Decision

On the Wayland backend, with the Graphics setting on Automatic, Cordial asks the compositor for its globals before the Vulkan loader is offered to the engine, and if `zwp_linux_dmabuf_v1` (version 3 or later) is absent it withholds Vulkan, which is what `CORDIAL_NO_VULKAN` already does. The engine takes its OpenGL ES path. That drew the landing screen on pixman sway; that Mesa's EGL got there by falling back to software rendering through `wl_shm` is INFERRED from its `failed to get driver name for fd -1` warnings. The client logs one line saying so. `CORDIAL_FORCE_VULKAN_WITHOUT_DMABUF=1` keeps Vulkan, and so does choosing Vulkan by name in Settings.

The question is asked on a connection of its own that is closed straight after, because the registry walk in `wayland.rs` belongs to window construction and the answer is needed earlier. A probe that cannot be asked leaves Vulkan on.

## Tried and not used

`MESA_VK_WSI_DEBUG=sw` set by Cordial when the compositor has no dmabuf. It makes the capability query succeed and the engine builds a swapchain, but `vkCreateSwapchainKHR` then fails with `VK_ERROR_INVALID_EXTERNAL_HANDLE` whenever the window's pixel count is not a multiple of 1024 (a bare probe: 1280x720 and 1280x676 succeed, 1280x674, 1280x675 and 1000x500 fail). The engine retried that once per frame, 1,460 recorded errors in a 25 s run, and drew nothing. The cause is INFERRED to be Mesa importing the shared-memory pool as a host pointer, which the driver wants page-aligned; the page-aligned sizes succeeding and the others not is the observation. Padding the reported extent to hide it would have meant lying about the window's size on a path that is already the slow one.

Retrying the query (forty times over 400 ms) did not help, which the mechanism above explains.

## Consequences

- On a compositor without dmabuf the client now runs on OpenGL ES with a software rasteriser and draws Roblox's landing screen; it drew nothing on Vulkan before. It is slow, and it was never going to be fast there: nothing hardware-accelerated can present to such a compositor.
- On a compositor with dmabuf nothing changes. Measured on pixman sway and on sway with the GLES renderer in `docs/NEXT.md`.
- The engine's mishandling of a failed Vulkan call during startup is untouched, and cannot be touched (ADR-001). Any other early Vulkan failure on any compositor can still reach it. Whether the original two crashes in eleven, taken on X11 on a machine with dmabuf, were this is not established; they were not this failure.
- A user who forces Vulkan on such a compositor keeps the old behaviour, including the crash on a minority of launches.
