# Recording gameplay

Roblox's own Record Video button does nothing on the Android client, and nothing in Stacked can change that. Stacked can record for it, because it owns the window the engine draws into: it reads each finished frame out of its own swapchain and hands it to `ffmpeg`.

It needs `ffmpeg` on your `PATH`. The Flatpak runtime ships one, so the Flatpak has it; on other installs it is the `ffmpeg` package.

**Once recording is switched on, press F9 in the game window to start recording, and F9 again to stop.** The window title carries `REC` while it runs, and the key never reaches the game.

## Turn it on

Recording is off until you switch it on, and until then F9 belongs to the game. Either setting switches it on:

```bash
stacked config set record_session true   # record every session from its first frame
stacked config set record_audio true     # record the game's sound with it (see Sound)
```

Both apply the next time Stacked starts, not to a game already running. With either on, and `ffmpeg` found when the game starts, F9 starts and stops recordings. Without `ffmpeg`, F9 stays the game's and the log says why. To have F9 with neither setting, set `CORDIAL_RECORD_KEY=F9` (below).

Each press toggles: the first starts a file, the second finishes it, and the next press starts another. Control+F9 and Alt+F9 still go to the game. A recording started by `record_session` ends when you close the game or press F9; one started with F9 ends at the next press, or when you close the game. The file is finished when the client exits.

Files are named `Stacked-2026-10-15_18-07-11.mp4`.

- **Not in the Flatpak:** `~/Videos/Stacked/` (or your `XDG_VIDEOS_DIR`).
- **In the Flatpak:** `~/.var/app/io.github.damnshabu.Stacked/data/cordial/profiles/<profile>/recordings/`. The Flatpak has no access to `~/Videos`, so it records into its own data instead.

Set `CORDIAL_RECORD_DIR` to choose another folder.

> [!NOTE]
> F9 is also the key Roblox uses for its developer console on desktop. If a game you play needs it, set `CORDIAL_RECORD_KEY` to another key (below) or to `off`.

## What you get

MP4 with H.264 where `ffmpeg` has libx264. Where it does not, which is the Flatpak runtime's `ffmpeg`, WebM with VP9. The container follows what the encoder can fill; it never writes a WebM called `.mp4`. The frame rate is 30 unless you set `CORDIAL_RECORD_FPS` (1 to 120).

Playback runs at the speed you played, not faster: the engine draws one frame a second when nothing is moving, and the recorder fills the gaps with the last frame.

If you resize the window while recording, the file ends and a new one starts beside it (`Stacked-…-2.mp4`), because a video cannot change size part way. The first few frames after the resize are not recorded while the new file starts.

An MP4 is written in fragments, so one cut off by a crash or a full disk still plays up to the cut.

## Sound

Off until you run `stacked config set record_audio true` (or set `CORDIAL_RECORD_AUDIO=game`). It records what the game plays and nothing else: Stacked copies the audio it hands to your sound server, so music in another window, a call, or a notification is not in the file. That holds on PipeWire, PulseAudio, ALSA and OSS alike.

If you want everything you can hear, other programs included, set `CORDIAL_RECORD_AUDIO=desktop`. That reads the default output's monitor through PulseAudio, and a call on another desktop would be in the file.

Sound is stereo. A game that plays surround is folded down to two channels. In a test with generated frames the sound came about 30 ms ahead of the picture, one frame; a real game has not been measured.

A game that has played nothing gives a silent track, and the log says how many streams were open when the recording ended (`[record] audio: 12.4 s written, 1 stream(s) open at the end`). Zero streams means the engine never opened audio, which is a different fault from a recording that lost it.

The window title marker is on Wayland only. On X11 the key and the log lines work and the title does not change.

## What it costs

Each captured frame holds the engine's present for a few milliseconds while it is copied: 2.5 to 3.4 ms at 1280x754 on one Intel GPU, measured at 30 and 60 captured frames a second. If the encoder cannot keep up, frames are dropped rather than slowing the game; the summary line at the end of the log counts them.

<details>
<summary>Other ways to start and stop, and the variables</summary>

For scripts: run the client with `CORDIAL_DEV_CONTROL=1` and use the development control surface (`record start`, `record stop`, `record status`, or `cordial_record` in the MCP). Pressing F9 through `cordial_key` (evdev code 67) toggles recording the same way the real key does, when the key is in force, and `tone <hz> <seconds>` (`cordial_tone` in the MCP) plays a quiet tone through Stacked's audio path, which is how to check what a recording hears when the game itself is silent.

| Variable | Meaning |
|---|---|
| `CORDIAL_RECORD` | `1` for the default folder, or a folder, or a file ending `.mp4`, `.webm` or `.mkv`. Recording starts at the first frame. |
| `CORDIAL_RECORD_DIR` | The default folder. |
| `CORDIAL_RECORD_FPS` | Frames a second, 1 to 120. |
| `CORDIAL_RECORD_AUDIO` | `game` (also `1`, `on`), `desktop`, or `off`. |
| `CORDIAL_RECORD_KEY` | The toggle key: `F1` to `F10`, `F12`, `Pause`, `ScrollLock`, or `off`. Unset, it is `F9` when `record_session` or `record_audio` is on and `ffmpeg` is found, and off otherwise; set, it applies whether or not either is on. A name that is none of these turns the key off and says so in the log. |

The client log says what was chosen (`[record] recording to … (libx264 at 30 fps, audio: the game's own sound)`, `[record] hotkey: F9 starts and stops recording while the game window has focus`) and, at the end, how many frames were written, dropped, and skipped while an encoder started.

`record_session` and `record_audio` are read at launch, so changing either does not reach a game already running. F9 is the way to start or stop mid-game.

</details>

Why it is built this way: [ADR-059](adr/ADR-059-recording-is-the-swapchain-read-on-a-timer-handed-to-ffmpeg.md).
