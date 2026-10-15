# Recording gameplay

Roblox's own Record Video button does nothing on the Android client, and nothing in Stacked can change that. Stacked can record for it, because it owns the window the engine draws into: it reads each finished frame out of its own swapchain and hands it to `ffmpeg`.

It needs `ffmpeg` on your `PATH`. The Flatpak runtime ships one, so the Flatpak has it; on other installs it is the `ffmpeg` package.

## Turn it on

```bash
stacked config set record_session true
```

It applies the next time Stacked starts, not to a game already running. Recording begins with the first frame the game draws and ends when you close the game; the file is finished when the client exits.

Files are named `Stacked-2026-10-15_18-07-11.mp4`.

- **Not in the Flatpak:** `~/Videos/Stacked/` (or your `XDG_VIDEOS_DIR`).
- **In the Flatpak:** `~/.var/app/io.github.damnshabu.Stacked/data/cordial/profiles/<profile>/recordings/`. The Flatpak has no access to `~/Videos`, so it records into its own data instead.

Set `CORDIAL_RECORD_DIR` to choose another folder.

## What you get

MP4 with H.264 where `ffmpeg` has libx264. Where it does not, which is the Flatpak runtime's `ffmpeg`, WebM with VP9. The container follows what the encoder can fill; it never writes a WebM called `.mp4`. The frame rate is 30 unless you set `CORDIAL_RECORD_FPS` (1 to 120).

Playback runs at the speed you played, not faster: the engine draws one frame a second when nothing is moving, and the recorder fills the gaps with the last frame.

If you resize the window while recording, the file ends and a new one starts beside it (`Stacked-…-2.mp4`), because a video cannot change size part way.

An MP4 is written in fragments, so one cut off by a crash or a full disk still plays up to the cut.

## Sound

Off. `stacked config set record_audio true` records everything the default output is playing, every program's, not only the game. A call on another desktop would be in the file, which is why it is separate and off. The same thing from the environment is `CORDIAL_RECORD_AUDIO=desktop`.

The game's own sound alone is not available yet.

## What it costs

Each captured frame holds the engine's present for a few milliseconds while it is copied: 2.5 to 3.4 ms at 1280x754 on one Intel GPU, measured at 30 and 60 captured frames a second. If the encoder cannot keep up, frames are dropped rather than slowing the game; the summary line at the end of the log counts them.

<details>
<summary>Starting and stopping while you play, and the variables</summary>

`record_session` records a whole session. To start and stop by hand, run the client with `CORDIAL_DEV_CONTROL=1` and use the development control surface (`record start`, `record stop`, `record status`, or `cordial_record` in the MCP). There is no hotkey and no button in the game yet.

| Variable | Meaning |
|---|---|
| `CORDIAL_RECORD` | `1` for the default folder, or a folder, or a file ending `.mp4`, `.webm` or `.mkv`. Recording starts at the first frame. |
| `CORDIAL_RECORD_DIR` | The default folder. |
| `CORDIAL_RECORD_FPS` | Frames a second, 1 to 120. |
| `CORDIAL_RECORD_AUDIO` | `desktop` or `off`. |

The client log says what was chosen (`[record] recording to … (libx264 at 30 fps, no audio)`) and, at the end, how many frames were written and dropped.

</details>

Why it is built this way: [ADR-059](adr/ADR-059-recording-is-the-swapchain-read-on-a-timer-handed-to-ffmpeg.md).
