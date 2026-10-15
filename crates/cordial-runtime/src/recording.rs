//! Recording the game's own frames to a video file (ADR-059, issue #33).
//!
//! Roblox's Record Video button does nothing on the Android client, and no
//! compositor route reaches a native Wayland window without a person clicking a
//! portal dialog. What Cordial does have is the engine's swapchain, read at the
//! moment a frame is complete (`android::capture`, ADR-019). Recording is that
//! same read on a timer, handed to an encoder.
//!
//! **The encoder is a child process, `ffmpeg`, fed raw frames on its stdin.**
//! Nothing is linked: an encoder library would put a codec licence and a build
//! dependency into every Cordial binary for a feature most sessions never use,
//! and `ffmpeg` is already in the Flatpak runtime (with libvpx, so WebM works
//! there) and on nearly every desktop (with libx264, so MP4 does). Which codecs
//! exist is asked of the binary itself rather than assumed, because a build
//! without libx264 and a request for `.mp4` is the ordinary case on the
//! Flatpak and must produce a file, not an error.
//!
//! **The engine never waits for the encoder.** A frame is read inside
//! `vkQueuePresentKHR`, copied into a buffer, and offered to a bounded channel
//! a writer thread drains into the pipe. If the encoder is slower than the
//! frame rate the offer fails and the frame is dropped and counted, because the
//! alternative is a game that stutters whenever the disk does. The drop count is
//! in the summary so a choppy recording says why.
//!
//! **Timestamps are the wall clock at the encoder's end of the pipe**, with
//! `-vf fps` filling the gaps. The engine presents at the display rate while
//! input flows and at exactly one frame a second when it does not (the idle
//! throttle, AGENTS.md), so a fixed frame period assumed here would make every
//! quiet stretch play back at 60 times speed.

use crate::android::capture::{self, Readback, Target};
use std::ffi::c_void;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Frames a second asked of the engine when nobody said.
pub const DEFAULT_FPS: u32 = 30;
/// Frames waiting for the writer. Four of a 1080p frame is 32 MB; more would
/// only delay the moment a slow encoder shows up as a drop.
const QUEUE_DEPTH: usize = 4;

// ---------------------------------------------------------------- encoder choice

/// A container and the codecs that go in it, as `ffmpeg` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Codec {
    pub ext: &'static str,
    pub name: &'static str,
    video: Vec<String>,
    audio: Vec<String>,
    movflags: bool,
}

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// The names `ffmpeg -encoders` lists, ignoring its banner and legend.
///
/// A line is `" V....D libx264   H.264 ..."`: six flag characters, then the
/// name. The legend above it has lines like `" V..... = Video"`, whose second
/// token is `=`, so requiring the first token to be six flag characters and the
/// second not to be `=` is what tells an encoder from its own key.
pub fn encoder_names(listing: &str) -> Vec<&str> {
    listing
        .lines()
        .filter_map(|line| {
            let mut it = line.split_whitespace();
            let flags = it.next()?;
            let name = it.next()?;
            let kind = flags.chars().next()?;
            (flags.len() == 6 && matches!(kind, 'V' | 'A' | 'S') && name != "=").then_some(name)
        })
        .collect()
}

/// Pick a container and codecs from what the local `ffmpeg` can encode.
///
/// `want` is the extension the caller asked for, if any. With none, the best
/// container this `ffmpeg` can fill is chosen: MP4 where there is H.264, WebM
/// where there is libvpx, Matroska for anything else. An extension that
/// `ffmpeg` cannot fill is an error that names it, not a silent switch to a
/// different container, because a file called `.mp4` that is actually WebM is
/// the kind of surprise that costs somebody an upload.
pub fn choose_codec(want: Option<&str>, encoders: &[&str]) -> Result<Codec, String> {
    let has = |n: &str| encoders.contains(&n);
    let x264 = || {
        has("libx264").then(|| ("libx264", args(&["-c:v", "libx264", "-preset", "veryfast", "-crf", "23"])))
    };
    let openh264 = || has("libopenh264").then(|| ("libopenh264", args(&["-c:v", "libopenh264", "-b:v", "8M"])));
    let vp9 = || {
        has("libvpx-vp9").then(|| {
            (
                "libvpx-vp9",
                args(&["-c:v", "libvpx-vp9", "-deadline", "realtime", "-cpu-used", "8", "-row-mt", "1", "-b:v", "0", "-crf", "34"]),
            )
        })
    };
    let vp8 = || {
        has("libvpx").then(|| {
            ("libvpx", args(&["-c:v", "libvpx", "-deadline", "realtime", "-cpu-used", "8", "-b:v", "6M"]))
        })
    };
    let svtav1 = || has("libsvtav1").then(|| ("libsvtav1", args(&["-c:v", "libsvtav1", "-preset", "10", "-crf", "36"])));
    let mpeg4 = || has("mpeg4").then(|| ("mpeg4", args(&["-c:v", "mpeg4", "-q:v", "4"])));

    let aac = || args(&["-c:a", "aac", "-b:a", "160k"]);
    let opus = || {
        if has("libopus") {
            args(&["-c:a", "libopus", "-b:a", "128k"])
        } else {
            aac()
        }
    };

    let ext = want.map(|e| e.to_ascii_lowercase());
    let (ext, video): (&'static str, Option<(&'static str, Vec<String>)>) = match ext.as_deref() {
        None => {
            if let Some(v) = x264().or_else(openh264) {
                ("mp4", Some(v))
            } else if let Some(v) = vp9().or_else(vp8) {
                ("webm", Some(v))
            } else {
                ("mkv", svtav1().or_else(mpeg4))
            }
        }
        Some("mp4") | Some("m4v") => ("mp4", x264().or_else(openh264).or_else(svtav1).or_else(mpeg4)),
        Some("webm") => ("webm", vp9().or_else(vp8).or_else(svtav1)),
        Some("mkv") => ("mkv", x264().or_else(openh264).or_else(vp9).or_else(vp8).or_else(svtav1).or_else(mpeg4)),
        Some(other) => {
            return Err(format!("cannot record to .{other}; use .mp4, .webm or .mkv"));
        }
    };
    let Some((name, video)) = video else {
        return Err(format!(
            "this ffmpeg has no encoder for .{ext} (it lists none of libx264, libopenh264, libvpx-vp9, libvpx, libsvtav1 or mpeg4)"
        ));
    };
    Ok(Codec {
        ext,
        name,
        video,
        audio: if ext == "mp4" { aac() } else { opus() },
        // A fragmented MP4 is playable up to wherever it was cut off. A plain
        // one has its index written last, so a client that is killed, or a
        // disk that fills, leaves a file no player will open.
        movflags: ext == "mp4",
    })
}

/// Everything one encoder process needs to be started.
#[derive(Debug, Clone)]
pub struct EncodeSpec {
    pub width: u32,
    pub height: u32,
    pub bgra: bool,
    pub fps: u32,
    pub codec: Codec,
    pub out: PathBuf,
    /// The PulseAudio source to record alongside, when the recording has sound.
    /// PipeWire answers to the same name through its Pulse compatibility layer.
    pub audio_source: Option<String>,
}

/// The `ffmpeg` command line for one segment.
pub fn ffmpeg_args(s: &EncodeSpec) -> Vec<String> {
    let mut a = args(&["-hide_banner", "-loglevel", "warning", "-nostdin", "-y"]);
    a.extend(args(&[
        "-f",
        "rawvideo",
        "-pixel_format",
        if s.bgra { "bgra" } else { "rgba" },
        "-video_size",
        &format!("{}x{}", s.width, s.height),
        "-use_wallclock_as_timestamps",
        "1",
        "-i",
        "pipe:0",
    ]));
    if let Some(src) = &s.audio_source {
        a.extend(args(&["-thread_queue_size", "512", "-f", "pulse", "-i", src]));
    }
    // `crop` to an even size because yuv420p, which every player accepts, has
    // half-resolution chroma and the encoders refuse an odd edge. `fps` last of
    // the timing filters so the constant rate is what the encoder sees.
    a.extend(args(&[
        "-vf",
        &format!("fps={},crop=trunc(iw/2)*2:trunc(ih/2)*2,format=yuv420p", s.fps),
    ]));
    a.extend(s.codec.video.iter().cloned());
    if s.audio_source.is_some() {
        a.extend(s.codec.audio.iter().cloned());
        // A live audio source never ends, so without this ffmpeg would keep
        // recording silence after the video pipe closed and the file would
        // never be finished.
        a.push("-shortest".into());
    }
    if s.codec.movflags {
        a.extend(args(&["-movflags", "+frag_keyframe+empty_moov+default_base_moof"]));
    }
    a.push(s.out.to_string_lossy().into_owned());
    a
}

// ------------------------------------------------------------------- where it goes

/// The directory a recording lands in when nobody named one.
///
/// `CORDIAL_RECORD_DIR`, else the user's Videos folder under `Stacked/`, else
/// the profile. **Inside a Flatpak it is the profile, always:** the sandbox has
/// no grant on `~/Videos`, and writing there succeeds into the sandbox's own
/// temporary home, so the file would be reported as saved and gone at exit.
pub fn default_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("CORDIAL_RECORD_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    let profile = crate::profile::active().join("recordings");
    if Path::new("/.flatpak-info").exists() {
        return profile;
    }
    match videos_dir() {
        Some(v) => v.join("Stacked"),
        None => profile,
    }
}

fn videos_dir() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let conf = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    let text = std::fs::read_to_string(conf.join("user-dirs.dirs")).unwrap_or_default();
    Some(parse_videos_dir(&text, &home).unwrap_or_else(|| home.join("Videos")))
}

/// `XDG_VIDEOS_DIR="$HOME/Vidéos"` out of `user-dirs.dirs`.
pub fn parse_videos_dir(text: &str, home: &Path) -> Option<PathBuf> {
    let line = text.lines().find(|l| l.trim_start().starts_with("XDG_VIDEOS_DIR="))?;
    let value = line.split_once('=')?.1.trim().trim_matches('"');
    if let Some(rest) = value.strip_prefix("$HOME") {
        Some(home.join(rest.trim_start_matches('/')))
    } else if value.starts_with('/') {
        Some(PathBuf::from(value))
    } else {
        None
    }
}

/// A file name that sorts by time and has no character a file manager objects to.
pub fn stamp_name(t: &Stamp) -> String {
    format!(
        "Stacked-{:04}-{:02}-{:02}_{:02}-{:02}-{:02}",
        t.year, t.month, t.day, t.hour, t.minute, t.second
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

fn now_local() -> Stamp {
    // SAFETY: `localtime_r` fills the caller's `tm` completely and is the
    // reentrant form, so nothing static is shared with other threads.
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now, &mut tm);
        Stamp {
            year: tm.tm_year + 1900,
            month: (tm.tm_mon + 1) as u32,
            day: tm.tm_mday as u32,
            hour: tm.tm_hour as u32,
            minute: tm.tm_min as u32,
            second: tm.tm_sec as u32,
        }
    }
}

/// Where a recording goes, and which extension was asked for.
///
/// `arg` is empty (the default directory), a directory (an existing one, or
/// anything ending in `/`), or a file. The file name is the stem for every
/// segment: a window resized mid-recording needs a new encoder, because a raw
/// frame cannot change size under it, and the second file is `stem-2.ext`.
pub fn resolve_target(arg: &str, default_dir: &Path, stamp: &Stamp) -> (PathBuf, Option<String>) {
    let arg = arg.trim();
    let dir_like = arg.is_empty() || arg.ends_with('/') || Path::new(arg).is_dir();
    if dir_like {
        let dir = if arg.is_empty() { default_dir.to_path_buf() } else { PathBuf::from(arg) };
        return (dir.join(stamp_name(stamp)), None);
    }
    let p = PathBuf::from(arg);
    match p.extension().and_then(|e| e.to_str()) {
        Some(e) => (p.with_extension(""), Some(e.to_string())),
        None => (p, None),
    }
}

fn segment_path(stem: &Path, segment: u32, ext: &str) -> PathBuf {
    let mut name = stem.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    if segment > 1 {
        name.push(format!("-{segment}"));
    }
    name.push(".");
    name.push(ext);
    stem.with_file_name(name)
}

// -------------------------------------------------------------------- the session

/// Checked once per present, so recording off costs one atomic load.
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// When the next frame is wanted, in nanoseconds since [`base`].
static NEXT_DUE_NS: AtomicU64 = AtomicU64::new(0);
static SESSION: Mutex<Option<Session>> = Mutex::new(None);
/// A request from the environment, waiting for the first frame to exist.
static ARMED: Mutex<Option<String>> = Mutex::new(None);
static ARMED_FAST: AtomicBool = AtomicBool::new(false);

fn base() -> Instant {
    static BASE: OnceLock<Instant> = OnceLock::new();
    *BASE.get_or_init(Instant::now)
}

fn now_ns() -> u64 {
    base().elapsed().as_nanos() as u64
}

/// Whether the present path should offer a frame to [`on_frame`].
pub fn frame_due() -> bool {
    if ARMED_FAST.load(Ordering::Acquire) {
        return true;
    }
    ACTIVE.load(Ordering::Acquire) && now_ns() >= NEXT_DUE_NS.load(Ordering::Relaxed)
}

pub fn active() -> bool {
    ACTIVE.load(Ordering::Acquire)
}

struct Encoding {
    child: Child,
    tx: Option<SyncSender<Vec<u8>>>,
    writer: Option<std::thread::JoinHandle<Result<(), String>>>,
    stderr: Option<std::thread::JoinHandle<()>>,
    pool: Arc<Mutex<Vec<Vec<u8>>>>,
    tail: Arc<Mutex<Vec<String>>>,
}

struct Session {
    stem: PathBuf,
    fps: u32,
    ffmpeg: PathBuf,
    codec: Codec,
    segment: u32,
    dims: (u32, u32),
    enc: Option<Encoding>,
    readback: Option<Readback>,
    files: Vec<PathBuf>,
    started: Instant,
    taken: u64,
    dropped: u64,
    read_ns: u64,
    /// The source being recorded with the video, if any.
    audio: Option<String>,
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

fn probe_encoders(ffmpeg: &Path) -> Result<String, String> {
    let out = Command::new(ffmpeg)
        .args(["-hide_banner", "-encoders"])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("could not run {}: {e}", ffmpeg.display()))?;
    let listing = String::from_utf8_lossy(&out.stdout).into_owned();
    if encoder_names(&listing).is_empty() {
        // An `ffmpeg` that runs and lists nothing is a wrapper or a sandbox
        // refusing it, and its own stderr is the only place that says so.
        let why = String::from_utf8_lossy(&out.stderr);
        return Err(format!("{} listed no encoders: {}", ffmpeg.display(), why.trim()));
    }
    Ok(listing)
}

/// The default output device's monitor: what the user is hearing, every
/// application's sound mixed.
pub const DESKTOP_SOURCE: &str = "@DEFAULT_MONITOR@";

/// Whether this `ffmpeg` can read a PulseAudio source at all (`pulse` is an
/// input device, listed by `-devices` and not by `-demuxers`). The Flatpak
/// runtime's can; a distribution's build usually can; a Homebrew one does not.
fn has_pulse_input(ffmpeg: &Path) -> bool {
    Command::new(ffmpeg)
        .args(["-hide_banner", "-devices"])
        .stdin(Stdio::null())
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .any(|l| l.split_whitespace().nth(1) == Some("pulse"))
        })
        .unwrap_or(false)
}

/// Which audio to record, from `CORDIAL_RECORD_AUDIO`, and a phrase for the
/// log saying what was decided.
///
/// **Off unless asked for, and the only thing to ask for is the desktop mix.**
/// A recording that quietly included whatever else was playing -- a call on
/// another desktop, a notification -- would be a privacy surprise sitting in a
/// file the user is about to share. The sound of the game alone is not offered
/// because it is not done (ADR-059): the engine's streams are not mixed in any
/// one place Cordial owns.
pub fn audio_choice(setting: Option<&str>, can_pulse: impl FnOnce() -> bool) -> (Option<String>, String) {
    match setting.map(str::trim) {
        None | Some("") | Some("0") | Some("off") | Some("none") => (None, "no audio".into()),
        Some("desktop") | Some("1") => {
            if can_pulse() {
                (Some(DESKTOP_SOURCE.into()), "audio: everything the default output is playing".into())
            } else {
                (None, "no audio: this ffmpeg cannot read PulseAudio".into())
            }
        }
        Some(other) => (None, format!("no audio: CORDIAL_RECORD_AUDIO={other:?} is not off or desktop")),
    }
}

/// Start recording. `arg` is as [`resolve_target`] reads it.
///
/// Returns once the session exists; the encoder is started by the first frame,
/// because its input size is that frame's size and nobody knows it before then.
pub fn start(arg: &str) -> Result<String, String> {
    let mut slot = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    if slot.is_some() {
        return Err("already recording; stop first".into());
    }
    let ffmpeg = find_in_path("ffmpeg").ok_or(
        "ffmpeg was not found on PATH; install it to record (the Flatpak runtime ships one)",
    )?;
    let listing = probe_encoders(&ffmpeg)?;
    let (stem, want_ext) = resolve_target(arg, &default_dir(), &now_local());
    let codec = choose_codec(want_ext.as_deref(), &encoder_names(&listing))?;
    if let Some(dir) = stem.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    }
    let (audio, audio_note) = audio_choice(
        std::env::var("CORDIAL_RECORD_AUDIO").ok().as_deref(),
        || has_pulse_input(&ffmpeg),
    );
    let fps = std::env::var("CORDIAL_RECORD_FPS")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .filter(|v| (1..=120).contains(v))
        .unwrap_or(DEFAULT_FPS);
    let first = segment_path(&stem, 1, codec.ext);
    let desc = format!("{} ({} at {} fps, {})", first.display(), codec.name, fps, audio_note);
    *slot = Some(Session {
        stem,
        fps,
        ffmpeg,
        codec,
        segment: 0,
        dims: (0, 0),
        enc: None,
        readback: None,
        files: Vec::new(),
        started: Instant::now(),
        taken: 0,
        dropped: 0,
        read_ns: 0,
        audio,
    });
    NEXT_DUE_NS.store(0, Ordering::Relaxed);
    ACTIVE.store(true, Ordering::Release);
    println!("[record] recording to {desc}");
    Ok(desc)
}

/// Ask for a recording to begin on the first presented frame. For
/// `CORDIAL_RECORD`, which is read before any swapchain exists.
pub fn arm(arg: &str) {
    *ARMED.lock().unwrap_or_else(|e| e.into_inner()) = Some(arg.to_string());
    ARMED_FAST.store(true, Ordering::Release);
}

/// `CORDIAL_RECORD`: `1` records to the default directory, anything else is
/// the directory or file to record to. `0` and empty are off.
pub fn arm_from_env() {
    match std::env::var("CORDIAL_RECORD") {
        Ok(v) if v != "0" && !v.is_empty() => arm(if v == "1" { "" } else { &v }),
        _ => {}
    }
}

pub fn status() -> String {
    let slot = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    match slot.as_ref() {
        None => "not recording".into(),
        Some(s) => format!(
            "recording {} frames={} dropped={} elapsed={:.1}s",
            s.files.last().map(|p| p.display().to_string()).unwrap_or_else(|| "(waiting for a frame)".into()),
            s.taken,
            s.dropped,
            s.started.elapsed().as_secs_f32()
        ),
    }
}

/// Stop, flush the encoder and report what was written.
pub fn stop() -> Result<String, String> {
    ACTIVE.store(false, Ordering::Release);
    ARMED_FAST.store(false, Ordering::Release);
    *ARMED.lock().unwrap_or_else(|e| e.into_inner()) = None;
    let session = SESSION.lock().unwrap_or_else(|e| e.into_inner()).take();
    let Some(session) = session else { return Err("not recording".into()) };
    // Outside the lock: flushing waits on the encoder, and the present thread
    // takes the lock every frame it is asked for.
    finish(session, false)
}

/// For the end of the process, which leaves through `_exit` and would
/// otherwise abandon the encoder mid-file.
pub fn finish_at_exit() {
    ACTIVE.store(false, Ordering::Release);
    ARMED_FAST.store(false, Ordering::Release);
    let session = SESSION.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(session) = session {
        match finish(session, true) {
            Ok(s) => println!("[record] {s}"),
            Err(e) => println!("[record] {e}"),
        }
    }
}

/// `at_exit` is for a process that is leaving: the engine has already torn its
/// Vulkan device down by then, and destroying the readback's command pool on a
/// device that no longer exists crashed inside the driver (measured: a
/// segfault in `vkDestroyCommandPool` under `anv_state_pool_free`, every
/// time). The readback is leaked instead, which the kernel reclaims a moment
/// later.
fn finish(mut s: Session, at_exit: bool) -> Result<String, String> {
    if at_exit {
        if let Some(rb) = s.readback.take() {
            std::mem::forget(rb);
        }
    }
    let mut problems = Vec::new();
    if let Some(enc) = s.enc.take() {
        if let Err(e) = close_encoding(enc) {
            problems.push(e);
        }
    }
    let secs = s.started.elapsed().as_secs_f32();
    let per = if s.taken > 0 { s.read_ns as f64 / s.taken as f64 / 1e6 } else { 0.0 };
    let mut out = format!(
        "{} frames in {:.1}s, {} dropped, {:.2} ms per frame read on the present thread; ",
        s.taken, secs, s.dropped, per
    );
    if s.files.is_empty() {
        out.push_str("no frame was presented, so no file was written");
    } else {
        let list: Vec<String> = s.files.iter().map(|p| p.display().to_string()).collect();
        out.push_str(&format!("wrote {}", list.join(", ")));
    }
    if !problems.is_empty() {
        return Err(format!("{out}; {}", problems.join("; ")));
    }
    Ok(out)
}

fn close_encoding(mut enc: Encoding) -> Result<(), String> {
    drop(enc.tx.take());
    let mut problems = Vec::new();
    if let Some(w) = enc.writer.take() {
        match w.join() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => problems.push(e),
            Err(_) => problems.push("the writer thread panicked".into()),
        }
    }
    // The encoder finishes its output once stdin closes. Bounded, because a
    // client that exits must not hang on an encoder that has wedged.
    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        match enc.child.try_wait() {
            Ok(Some(st)) => break Some(st),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = enc.child.kill();
                let _ = enc.child.wait();
                problems.push("the encoder did not finish within 20 s and was killed".into());
                break None;
            }
        }
    };
    // Joined only when the encoder exited by itself. After a kill, a process the
    // wrapper started may still hold the pipe open, and waiting for its end of
    // file would hang the client's exit on the very thing the kill was for.
    if let Some(h) = enc.stderr.take() {
        if status.is_some() {
            let _ = h.join();
        }
    }
    if let Some(st) = status {
        if !st.success() {
            let tail = enc.tail.lock().map(|t| t.join(" | ")).unwrap_or_default();
            problems.push(format!("ffmpeg exited with {st}: {tail}"));
        }
    }
    if problems.is_empty() { Ok(()) } else { Err(problems.join("; ")) }
}

fn open_encoding(ffmpeg: &Path, spec: &EncodeSpec) -> Result<Encoding, String> {
    let mut cmd = Command::new("nice");
    cmd.args(["-n", "10"]).arg(ffmpeg).args(ffmpeg_args(spec));
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .or_else(|_| {
            // No `nice` on PATH; run it at the engine's own priority rather
            // than not at all.
            Command::new(ffmpeg)
                .args(ffmpeg_args(spec))
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
        })
        .map_err(|e| format!("could not start ffmpeg: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("ffmpeg has no stdin")?;
    let stderr = child.stderr.take().ok_or("ffmpeg has no stderr")?;

    let tail: Arc<Mutex<Vec<String>>> = Arc::default();
    let tail2 = tail.clone();
    let err_thread = std::thread::Builder::new()
        .name("cordial-rec-log".into())
        .spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                println!("[record] ffmpeg: {line}");
                if let Ok(mut t) = tail2.lock() {
                    t.push(line);
                    if t.len() > 6 {
                        t.remove(0);
                    }
                }
            }
        })
        .ok();

    let (tx, rx) = sync_channel::<Vec<u8>>(QUEUE_DEPTH);
    let pool: Arc<Mutex<Vec<Vec<u8>>>> = Arc::default();
    let pool2 = pool.clone();
    let writer = std::thread::Builder::new()
        .name("cordial-rec-write".into())
        .spawn(move || {
            for frame in rx {
                stdin.write_all(&frame).map_err(|e| format!("writing to the encoder: {e}"))?;
                if let Ok(mut p) = pool2.lock() {
                    if p.len() < QUEUE_DEPTH + 2 {
                        p.push(frame);
                    }
                }
            }
            Ok(())
        })
        .map_err(|e| format!("could not start the writer thread: {e}"))?;
    Ok(Encoding {
        child,
        tx: Some(tx),
        writer: Some(writer),
        stderr: err_thread,
        pool,
        tail,
    })
}

/// Called from `vkQueuePresentKHR` when [`frame_due`] said so, with the image
/// being presented. Never panics and never blocks on the encoder.
pub fn on_frame(
    queue: u64,
    target: Target,
    gdpa: extern "C" fn(u64, *const std::ffi::c_char) -> *mut c_void,
    gpdmp: extern "C" fn(*mut c_void, *mut capture::PhysicalDeviceMemoryProperties),
    physical_device: *mut c_void,
) {
    if ARMED_FAST.swap(false, Ordering::AcqRel) {
        let arg = ARMED.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(arg) = arg {
            if let Err(e) = start(&arg) {
                println!("[record] CORDIAL_RECORD was set but recording could not start: {e}");
            }
        }
    }
    if !ACTIVE.load(Ordering::Acquire) {
        return;
    }
    let started = Instant::now();
    let mut slot = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    let Some(s) = slot.as_mut() else { return };

    let result = take_frame(s, queue, &target, gdpa, gpdmp, physical_device);
    s.read_ns += started.elapsed().as_nanos() as u64;
    // The next frame is one period after this one was *wanted*, not after it
    // was taken, so the average rate holds at the asked-for one however the
    // presents happen to fall. A stall is not followed by a burst: the next
    // frame is never due sooner than half a period from now.
    let period = 1_000_000_000 / s.fps as u64;
    let now = now_ns();
    let due = NEXT_DUE_NS.load(Ordering::Relaxed);
    let anchor = if due == 0 { now } else { due };
    NEXT_DUE_NS.store((anchor + period).max(now + period / 2), Ordering::Relaxed);

    if let Err(why) = result {
        ACTIVE.store(false, Ordering::Release);
        let session = slot.take();
        drop(slot);
        println!("[record] stopping: {why}");
        if let Some(session) = session {
            // Finished on a thread of its own: this is the engine's present.
            let _ = std::thread::Builder::new().name("cordial-rec-stop".into()).spawn(move || {
                match finish(session, false) {
                    Ok(summary) => println!("[record] {summary}"),
                    Err(e) => println!("[record] {e}"),
                }
            });
        }
    }
}

fn take_frame(
    s: &mut Session,
    queue: u64,
    target: &Target,
    gdpa: extern "C" fn(u64, *const std::ffi::c_char) -> *mut c_void,
    gpdmp: extern "C" fn(*mut c_void, *mut capture::PhysicalDeviceMemoryProperties),
    physical_device: *mut c_void,
) -> Result<(), String> {
    let dims = (target.rect.2, target.rect.3);
    let bgra = capture::is_bgra(target.format)?;

    if s.dims != dims || s.enc.is_none() {
        // A new size needs a new encoder and a new buffer. The first frame is
        // this path too, with nothing to close.
        if let Some(old) = s.enc.take() {
            if let Err(e) = close_encoding(old) {
                println!("[record] {e}");
            }
        }
        s.readback = None;
        s.segment += 1;
        s.dims = dims;
        let path = segment_path(&s.stem, s.segment, s.codec.ext);
        let spec = EncodeSpec {
            width: dims.0,
            height: dims.1,
            bgra,
            fps: s.fps,
            codec: s.codec.clone(),
            out: path.clone(),
            audio_source: s.audio.clone(),
        };
        s.enc = Some(open_encoding(&s.ffmpeg, &spec)?);
        s.files.push(path.clone());
        println!("[record] {}x{} -> {}", dims.0, dims.1, path.display());
    }
    if s.readback.is_none() {
        s.readback = Some(Readback::new(target, gdpa, gpdmp, physical_device)?);
    }
    let enc = s.enc.as_mut().ok_or("no encoder")?;
    let rb = s.readback.as_mut().ok_or("no readback")?;
    let pixels = rb.read(queue, target)?;
    let mut buf = enc.pool.lock().ok().and_then(|mut p| p.pop()).unwrap_or_default();
    buf.clear();
    buf.extend_from_slice(pixels);
    let tx = enc.tx.as_ref().ok_or("encoder already closed")?;
    match tx.try_send(buf) {
        Ok(()) => s.taken += 1,
        Err(TrySendError::Full(_)) => s.dropped += 1,
        Err(TrySendError::Disconnected(_)) => {
            let tail = enc.tail.lock().map(|t| t.join(" | ")).unwrap_or_default();
            return Err(format!("the encoder stopped reading ({tail})"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LISTING: &str = " Encoders:\n V..... = Video\n A..... = Audio\n ------\n V....D libx264              libx264 H.264 (codec h264)\n V....D mpeg4                MPEG-4 part 2\n A....D aac                  AAC (Advanced Audio Coding)\n A....D libopus              libopus Opus\n";

    #[test]
    fn the_legend_is_not_an_encoder() {
        let names = encoder_names(LISTING);
        assert_eq!(names, vec!["libx264", "mpeg4", "aac", "libopus"]);
    }

    #[test]
    fn mp4_with_h264_is_the_default() {
        let c = choose_codec(None, &encoder_names(LISTING)).unwrap();
        assert_eq!((c.ext, c.name), ("mp4", "libx264"));
    }

    #[test]
    fn a_build_without_h264_falls_to_webm_rather_than_failing() {
        // The Flatpak runtime's ffmpeg: libvpx and AV1, no libx264.
        let c = choose_codec(None, &["libvpx-vp9", "libvpx", "libsvtav1", "mpeg4", "aac", "libopus"]).unwrap();
        assert_eq!((c.ext, c.name), ("webm", "libvpx-vp9"));
    }

    #[test]
    fn an_extension_that_cannot_be_filled_is_an_error_not_a_different_container() {
        let e = choose_codec(Some("webm"), &["libx264", "aac"]).unwrap_err();
        assert!(e.contains(".webm"), "{e}");
        assert!(choose_codec(Some("avi"), &["libx264"]).unwrap_err().contains(".avi"));
    }

    #[test]
    fn mp4_is_fragmented_and_webm_is_not() {
        let spec = |ext| EncodeSpec {
            width: 1280,
            height: 720,
            bgra: true,
            fps: 30,
            codec: choose_codec(Some(ext), &["libx264", "libvpx-vp9", "aac", "libopus"]).unwrap(),
            out: PathBuf::from("/o/x"),
            audio_source: None,
        };
        assert!(ffmpeg_args(&spec("mp4")).join(" ").contains("empty_moov"));
        assert!(!ffmpeg_args(&spec("webm")).join(" ").contains("movflags"));
    }

    #[test]
    fn the_command_line_names_the_input_geometry_and_pixel_order() {
        let spec = EncodeSpec {
            width: 1920,
            height: 1080,
            bgra: false,
            fps: 60,
            codec: choose_codec(None, &["libx264", "aac"]).unwrap(),
            out: PathBuf::from("/o/x.mp4"),
            audio_source: Some("@DEFAULT_MONITOR@".into()),
        };
        let a = ffmpeg_args(&spec).join(" ");
        assert!(a.contains("-pixel_format rgba -video_size 1920x1080"), "{a}");
        assert!(a.contains("fps=60,crop=trunc(iw/2)*2:trunc(ih/2)*2,format=yuv420p"), "{a}");
        assert!(a.contains("-f pulse -i @DEFAULT_MONITOR@"), "{a}");
        assert!(a.contains("-c:a aac"), "{a}");
        assert!(a.contains("-shortest"), "{a}");
        assert!(a.ends_with("/o/x.mp4"), "{a}");
    }

    #[test]
    fn no_audio_input_means_no_audio_codec() {
        let spec = EncodeSpec {
            width: 640,
            height: 480,
            bgra: true,
            fps: 30,
            codec: choose_codec(None, &["libx264", "aac"]).unwrap(),
            out: PathBuf::from("/o/x.mp4"),
            audio_source: None,
        };
        assert!(!ffmpeg_args(&spec).join(" ").contains("-c:a"));
    }

    #[test]
    fn audio_is_off_unless_asked_and_only_the_desktop_mix_is_on_offer() {
        assert_eq!(audio_choice(None, || panic!("not asked")).0, None);
        assert_eq!(audio_choice(Some("off"), || panic!("not asked")).0, None);
        assert_eq!(audio_choice(Some("desktop"), || true).0.as_deref(), Some(DESKTOP_SOURCE));
        let (src, why) = audio_choice(Some("desktop"), || false);
        assert!(src.is_none() && why.contains("cannot read PulseAudio"), "{why}");
        let (src, why) = audio_choice(Some("game"), || true);
        assert!(src.is_none() && why.contains("game"), "{why}");
    }

    #[test]
    fn videos_dir_reads_user_dirs() {
        let home = Path::new("/home/u");
        assert_eq!(
            parse_videos_dir("XDG_VIDEOS_DIR=\"$HOME/Clips\"\n", home),
            Some(PathBuf::from("/home/u/Clips"))
        );
        assert_eq!(
            parse_videos_dir("# c\nXDG_VIDEOS_DIR=\"/data/v\"\n", home),
            Some(PathBuf::from("/data/v"))
        );
        assert_eq!(parse_videos_dir("XDG_MUSIC_DIR=\"$HOME/M\"\n", home), None);
    }

    #[test]
    fn targets_resolve_to_a_stem_and_an_optional_extension() {
        let t = Stamp { year: 2026, month: 10, day: 5, hour: 9, minute: 3, second: 7 };
        let d = Path::new("/vid/Stacked");
        assert_eq!(
            resolve_target("", d, &t),
            (PathBuf::from("/vid/Stacked/Stacked-2026-10-05_09-03-07"), None)
        );
        assert_eq!(
            resolve_target("/x/y/", d, &t),
            (PathBuf::from("/x/y/Stacked-2026-10-05_09-03-07"), None)
        );
        assert_eq!(resolve_target("/x/clip.webm", d, &t), (PathBuf::from("/x/clip"), Some("webm".into())));
        assert_eq!(resolve_target("/x/clip", d, &t), (PathBuf::from("/x/clip"), None));
    }

    #[test]
    fn the_second_segment_is_numbered_and_the_first_is_not() {
        let stem = Path::new("/x/clip");
        assert_eq!(segment_path(stem, 1, "mp4"), PathBuf::from("/x/clip.mp4"));
        assert_eq!(segment_path(stem, 2, "mp4"), PathBuf::from("/x/clip-2.mp4"));
    }

    /// A stand-in encoder: copies its stdin to the file named by its last
    /// argument, which is where `ffmpeg_args` puts the output.
    fn fake_encoder(dir: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join("fake-ffmpeg");
        std::fs::write(&p, format!("#!/bin/sh\nfor last; do :; done\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cordial-rec-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn spec(out: PathBuf) -> EncodeSpec {
        EncodeSpec {
            width: 4,
            height: 2,
            bgra: true,
            fps: 30,
            codec: choose_codec(None, &["libx264", "aac"]).unwrap(),
            out,
            audio_source: None,
        }
    }

    #[test]
    fn frames_sent_to_the_encoder_arrive_whole_and_in_order() {
        let dir = scratch("pipe");
        let enc_bin = fake_encoder(&dir, "cat > \"$last\"");
        let out = dir.join("out.raw");
        let enc = open_encoding(&enc_bin, &spec(out.clone())).unwrap();
        let tx = enc.tx.clone().unwrap();
        for n in 0u8..5 {
            tx.send(vec![n; 32]).unwrap();
        }
        drop(tx);
        close_encoding(enc).unwrap();
        let got = std::fs::read(&out).unwrap();
        assert_eq!(got.len(), 5 * 32);
        for (n, chunk) in got.chunks(32).enumerate() {
            assert!(chunk.iter().all(|b| *b == n as u8), "frame {n} was corrupted");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_encoder_that_dies_is_reported_with_what_it_said() {
        let dir = scratch("die");
        let enc_bin = fake_encoder(&dir, "echo 'Unknown encoder libx264' >&2; exit 1");
        let enc = open_encoding(&enc_bin, &spec(dir.join("o.mp4"))).unwrap();
        let err = close_encoding(enc).unwrap_err();
        assert!(err.contains("Unknown encoder libx264"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_full_queue_drops_a_frame_instead_of_blocking() {
        // The engine's present thread must never wait on the encoder. An
        // encoder that reads nothing fills the channel and the pipe, and the
        // sixth offer is refused at once.
        let dir = scratch("full");
        let enc_bin = fake_encoder(&dir, "sleep 30");
        let mut enc = open_encoding(&enc_bin, &spec(dir.join("o.mp4"))).unwrap();
        let tx = enc.tx.take().unwrap();
        let big = vec![0u8; 1 << 20];
        let started = Instant::now();
        let mut refused = 0;
        for _ in 0..64 {
            if let Err(TrySendError::Full(_)) = tx.try_send(big.clone()) {
                refused += 1;
            }
        }
        assert!(refused > 0, "nothing was ever refused");
        assert!(started.elapsed() < Duration::from_secs(5), "an offer blocked");
        let _ = enc.child.kill();
        let _ = enc.child.wait();
        drop(tx);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
