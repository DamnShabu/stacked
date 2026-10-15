# ADR-059: Recording is the swapchain read on a timer, handed to ffmpeg

**Status:** accepted
**Date:** 2026-10-15
**Extends:** [ADR-019](ADR-019-development-control-surface.md)
**Related:** [ADR-001](ADR-001-in-process-hooking.md), [ADR-003](ADR-003-plugin-isolation.md), [ADR-007](ADR-007-host-resources-are-brokered.md), [ADR-044](ADR-044-settings-reach-a-running-game.md)
**User page:** [`docs/recording.md`](../recording.md)
**Issue:** #33

## Context

Roblox's Record Video button does nothing on the Android client (Sober #1092). Cordial already reads the frame the engine just presented out of its own Vulkan swapchain for `cordial_screenshot` (ADR-019), at the moment inside `vkQueuePresentKHR` when the image is complete and in `PRESENT_SRC_KHR`. The issue left four things open: what the read costs at a stream's rate, where the encoding happens, whether audio is possible, and whether it belongs in a plugin capability. The maintainer's answer to the last was already shell-native (ADR-007: a continuous stream plus an encoder plus a user's file is not a payload).

Reading our own swapchain is not hooking the engine. It is the same read ADR-019 records, and nothing here executes in, or patches, the Roblox process (ADR-001).

## Decision

**The recorder is the screenshot read on a timer, and its encoder is a child process.** `recording.rs` asks, once per present, whether a frame is due (one atomic load when recording is off). If so, the present thread copies the image into a persistent host buffer (`capture::Readback`, the screenshot code split into allocate-once and read-per-frame), copies the bytes into a pooled buffer and offers it to a bounded channel (four frames). A writer thread drains the channel into the stdin of `ffmpeg -f rawvideo`. A full channel drops the frame and counts it: the engine never waits for an encoder.

**`ffmpeg` is spawned, never linked.** No codec library enters a Cordial binary, so no codec licence does either, and no build gains a dependency for a feature most sessions do not use. The Flatpak runtime ships `ffmpeg` 7.1.3 (with libvpx, libaom, SVT-AV1, libopus, PulseAudio input, and no libx264), and most distributions' builds have libx264. The recorder asks the binary what it can encode (`ffmpeg -encoders`) and picks a container to match: MP4/H.264 where libx264 or libopenh264 exist, WebM/VP9 or VP8 where they do not, Matroska otherwise. A container the binary cannot fill is an error naming it, not a different container under the same extension. Encoders run under `nice -n 10`.

**Timestamps are the wall clock at the encoder's end of the pipe, with `-vf fps=N` filling gaps.** The engine presents at the display rate with input flowing and at one frame a second when idle (AGENTS.md), so assuming a fixed period would play quiet stretches back at speed.

**A size change starts a new file** (`stem-2.ext`), because a raw frame cannot change size under a running encoder. **MP4 is fragmented**, so a file cut short still plays.

**Where it is driven from.** `CORDIAL_RECORD` (a folder, a file, or `1` for the default) arms it at the first presented frame, and the launcher's **Record gameplay** switch sets it at launch. devctl gets `record start|stop|status` and the MCP gets `cordial_record`. There is no live toggle on the `cordial.runtime/1` control socket and no hotkey: a hotkey lives in the input and window code, which was held by other work when this was written, and a new `settings.set` key is a protocol change (ADR-052, ADR-055) that deserves its own decision.

**Where it lands.** `CORDIAL_RECORD_DIR`, else `Videos/Cordial` (from `user-dirs.dirs`), else `<profile>/recordings`. **Inside a Flatpak it is always the profile**, because the sandbox has no grant on `~/Videos` and a write there succeeds into the sandbox's own temporary home and is gone at exit.

**Audio is off, and the only option is the desktop mix.** `CORDIAL_RECORD_AUDIO=desktop` reads the default sink's monitor through `ffmpeg -f pulse -i @DEFAULT_MONITOR@`. It records every program's sound, which is a privacy surprise if it is on by default, so it is a separate switch and off.

## Measured

On 2026-10-15, one Intel GPU, a signed-out client on the landing page in a nested headless sway at 1280x754, the engine presenting at about 60 Hz because pointer motion was being driven. Single runs, not repeated.

- Per captured frame, time on the present thread (copy and queue wait): **3.14 ms** (x264, 30 fps asked), **2.50 ms** (x264, 30 fps, a second run with a resize), **3.36 ms** (VP9 and Opus, 60 fps asked). The screenshot's per-call cost, which allocates and destroys four objects, was not measured here and is not the number to extrapolate from.
- Frames dropped by a full encoder queue: 0 of 185 (x264, 30 fps) and 4 of 745 (VP9 with audio, 60 fps).
- A 60 fps VP9 recording of 724 captured frames covered 12.4 s, 58 frames a second, with none dropped over that stretch.
- Engine frame times with and without: control run median 16.7 ms, p95 17.3, p99 24.5, max 98.6; the 60 fps VP9 and audio run median 16.7, p95 17.3, p99 32.5, max 329.7. One run each; the tail is not shown to be the recorder's.
- `ffprobe` of the 18.1 s x264 recording reported h264 1280x754, 30 fps, duration 18.03 s, and the process had counted 185 captured frames over 18.1 s of wall clock: the file is as long as the session was, not as long as 185 frames at 30 fps would be.

## Not done, and why

- **The game's own audio.** The engine's playback streams (OpenSL ES and AAudio, over PipeWire, Pulse, ALSA or OSS) are not mixed in any one place Cordial owns, so a tap would mean one per backend and a mixer. `pw-record --target <stream node>` was seen to capture a single named playback stream (a test tone, not the engine's), so linking this process's streams to one capture stream with `pw-link` is the route most likely to work. It is untested against the engine and is `INFERRED`.
- **Start and stop while playing from the launcher or the keyboard.** See above.
- **A hardware encoder.** `h264_vaapi` and the NVENC encoders are listed by the Flatpak's `ffmpeg` and not used.
- **Recording under VR.** The OpenXR path has a different capture entry point (`capture_image`) and was not touched.

## Consequences

`capture.rs` changes shape: `copy_to_png` now builds a `Readback`, reads once and drops it, which is what it did before; `frame_target` is shared. **Destroying a `Readback` after the engine has torn its Vulkan device down crashed the process** (a segfault in `vkDestroyCommandPool`, every time, found with gdb), so the exit path leaks it on purpose.

Someone about to change this should know that the first frame starts the encoder because only then is its input size known, and that `finish_at_exit` in `bin/load.rs` is what finishes a recording before `_exit`, which runs nothing.
