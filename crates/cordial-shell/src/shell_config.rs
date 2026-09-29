//! The launcher's preferences: which profile and Roblox build to run, and
//! the settings it turns into the client's environment at launch.
//!
//! `$XDG_CONFIG_HOME/cordial/shell.json`, falling back to `$HOME/.config` —
//! the same layout `cordial_plugins::grants::path` and
//! `cordial_plugins::manifest::plugin_root` use — and the same
//! default-on-anything-wrong behaviour as `grants::load`: a missing or
//! malformed file means "use the defaults", not "refuse to start". Nobody
//! but the launcher ever writes this file, so a malformed one is far likelier
//! to be an interrupted write than anything adversarial, and refusing to
//! start over that would be a worse failure than quietly falling back.
//!
//! `stacked config` reads and writes it. Fields that only the old GTK
//! launcher's own windows used -- its colour scheme, the update timer, the
//! marketplace directory, the dialog it showed once -- were removed with that
//! launcher; an older file that still carries them loads, and the next save
//! drops them.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub use cordial_shell::theme::Theme;
pub use cordial_shell::title_bar::TitleBar;

/// When Cordial stops holding the engine at full rate.
///
/// **The engine has an idle throttle of its own and this decides when to stop
/// defeating it.** `input::idle_keepalive` sends `nativePassMouseMove` on every
/// pump tick while a key is held, because without it the engine collapses from
/// about sixty presents a second to exactly one about thirteen seconds after
/// the last mouse movement — a player walking in a straight line gets throttled
/// mid-play. That workaround used to run unconditionally, so Cordial held the
/// engine at full rate in the background on purpose.
///
/// **This governs Cordial's own keepalive and nothing else.**
/// `onWindowFocusChangedNative` is reported to the engine truthfully on every
/// real transition whatever this is set to; answering a platform question
/// honestly is not a setting. On [`ThrottleWhen::Off`] the engine still knows
/// it is unfocused and Cordial simply carries on driving it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThrottleWhen {
    /// Throttle only when the window is not visible — minimised, on another
    /// workspace, or fully covered.
    ///
    /// The default, and deliberately not `Unfocused`. A second monitor with
    /// Roblox running on it while the user types in Discord on the first is an
    /// unfocused window somebody is watching, and throttling it would be worse
    /// than the problem this setting exists to fix; so is alt-tabbing away from
    /// a load screen, which is when a stream most wants the frames. Visibility
    /// is the question actually being asked and Wayland answers it directly —
    /// `xdg_toplevel`'s `suspended` state, which reaches this process as
    /// `GdkToplevelState::SUSPENDED`.
    Visible,
    /// Throttle whenever the window loses focus. The most saving, and wrong for
    /// anyone who watches Cordial while working in another window.
    Unfocused,
    /// Never throttle. Full rate in the background, which is what a recording
    /// or a long download inside the client wants.
    Off,
}

impl Default for ThrottleWhen {
    fn default() -> Self {
        ThrottleWhen::Visible
    }
}

impl ThrottleWhen {
    /// The word the client parses out of `CORDIAL_THROTTLE`. Passed through the
    /// environment rather than read from `shell.json` for the same reason
    /// `graphics` is: the client is a separate process with its own idea of
    /// where configuration lives, and the launch is the one place that already
    /// knows both.
    pub fn as_str(self) -> &'static str {
        match self {
            ThrottleWhen::Visible => "visible",
            ThrottleWhen::Unfocused => "unfocused",
            ThrottleWhen::Off => "off",
        }
    }
}

/// Whether the desktop's own pointer acceleration reaches the camera.
///
/// `zwp_relative_pointer_v1` delivers two deltas per event: one the
/// compositor has run through the desktop's pointer profile, and one it has
/// not. While Roblox holds the cursor -- first person, shift lock, right-drag
/// -- Cordial chooses between them, and unaccelerated is right for a camera:
/// acceleration is superlinear in speed, so a fast sweep turns further than a
/// slow one covering the same distance, and in-game sensitivity would
/// otherwise follow whatever pointer speed the desktop happens to be set to.
///
/// **There is still no `Never`, but not for the reason this comment used to
/// give.** It used to say that, with the cursor unlocked, Cordial was handed
/// an absolute position the compositor had already accelerated and so had no
/// unaccelerated absolute to fall back to -- making the desktop's setting
/// apply outside the lock whether Cordial liked it or not, and a "never" a
/// switch that would silently do nothing. That stopped being true on
/// 2026-08-28: reported as "it's set on only the cursor, it should work and
/// accelerate in roblox ui. It doesn't", `relative_pointer_motion` now feeds
/// the unlocked cursor from `zwp_relative_pointer_v1`'s own accelerated pair
/// rather than from the arithmetic difference of two absolute positions --
/// see that function and `input.rs`'s `resolve_mouse_delta`. An unaccelerated
/// *cursor* is therefore possible now, the same way the camera's is: the
/// unaccelerated pair is sitting right there in the same event. It is
/// deliberately not offered here anyway: nobody asked for a cursor that
/// ignores the desktop's pointer profile, and the report this enum exists to
/// answer was the opposite complaint. Adding it would be a third menu entry
/// with no user behind it -- if that changes, `PointerAcceleration` is a
/// two-variant enum and `NeverCursor` is a small addition, not a redesign.
///
/// Keyed on the pointer lock rather than on "first person" because first
/// person is engine state and Cordial cannot see it -- Roblox exposes no
/// accessibility tree, and reading it any other way is out of scope under
/// ADR-001. The lock is Cordial's own, and Roblox takes it for exactly the
/// three camera cases that want raw movement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PointerAcceleration {
    /// The desktop's setting moves the cursor, and camera movement is raw.
    /// The default, and the only honest description of the status quo.
    UnlockedCursor,
    /// The desktop's setting moves the camera too, for anyone who has tuned
    /// their pointer profile and wants the client to obey it.
    Always,
}

impl Default for PointerAcceleration {
    fn default() -> Self {
        PointerAcceleration::UnlockedCursor
    }
}

impl PointerAcceleration {
    /// The word the client parses out of `CORDIAL_POINTER_ACCEL`.
    pub fn as_str(self) -> &'static str {
        match self {
            PointerAcceleration::UnlockedCursor => "unlocked",
            PointerAcceleration::Always => "always",
        }
    }
}

/// Which Vulkan present mode the client asks the driver for.
///
/// **It is a latency setting and a power setting at the same time, and those
/// pull opposite ways.** FIFO queues one image per display refresh, so the GPU
/// renders exactly the frames that get shown and wastes nothing -- and the
/// cursor and camera lag the hand by however deep that queue is. MAILBOX has no
/// queue to wait behind, it replaces the pending image, so it is the
/// responsive one and it burns power drawing frames the display never scans
/// out. IMMEDIATE does not synchronise at all: the lowest latency there is, and
/// the one that tears.
///
/// **MAILBOX is the default because the latency was measured and the power was
/// not.** This shipped as FIFO for about an hour on the power argument, and the
/// report came straight back -- "the mouse feels floaty and weird in roblox",
/// then the control run, "switching back to Mailbox fixes the floaty fealing".
/// The power cost of MAILBOX is real and nobody here has a watt meter; the
/// latency cost of FIFO is something a person felt within minutes. FIFO is one
/// row away for anyone who would rather pay it.
///
/// FIFO is also the only mode `VkSurfaceKHR` guarantees -- the other two may
/// simply not be advertised, in which case
/// `cordial_runtime::android::vulkan` leaves the engine's own choice alone
/// rather than substituting something nobody asked for.
///
/// **[`PresentMode::Automatic`] is not a fourth mode, it is the absence of an
/// opinion**, and it is here for the same reason `graphics`'s "automatic" is:
/// an absent `CORDIAL_PRESENT_MODE` is the one state in which a plugin's
/// `CordialPresentMode` entry counts (ADR-007, ADR-020). Without it, shipping
/// this row would have quietly made a documented plugin capability
/// unreachable for everybody, which is the kind of silent contradiction
/// AGENTS.md asks to be argued in an ADR rather than introduced in a widget.
/// Choosing Automatic lands on the runtime's own `auto`, which is MAILBOX
/// (`vulkan.rs`), when no plugin says otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PresentMode {
    /// One image per refresh: no tearing, no wasted frames, and the latency of
    /// a queue. The power-saving choice, and not the default -- see above.
    Fifo,
    /// No queue, no tearing, where the driver advertises it. The default:
    /// responsive, and it costs power.
    Mailbox,
    /// Uncapped, unsynchronised, tears. The lowest latency there is.
    Immediate,
    /// No opinion, so a plugin may have one. Falls back to the runtime's
    /// `auto`, which is MAILBOX.
    Automatic,
}

impl Default for PresentMode {
    fn default() -> Self {
        PresentMode::Mailbox
    }
}

impl PresentMode {
    /// The word `cordial_runtime::android::vulkan::parse_present_mode` takes
    /// out of `CORDIAL_PRESENT_MODE`, or `None` for Automatic.
    ///
    /// `None` rather than the string "auto" because the two are not the same
    /// thing to the runtime's precedence rules -- an absent variable and an
    /// explicit `auto` both let a plugin through, but only an absent one keeps
    /// the launcher out of a decision it was not asked to make. Sending
    /// nothing is the smaller claim.
    pub fn as_env(self) -> Option<&'static str> {
        match self {
            PresentMode::Fifo => Some("fifo"),
            PresentMode::Mailbox => Some("mailbox"),
            PresentMode::Immediate => Some("immediate"),
            PresentMode::Automatic => None,
        }
    }
}

/// Which PipeWire sink Roblox's audio goes to, by stable `node.name`.
///
/// Empty — the default — means *follow the system default sink*, and it has to
/// keep meaning that rather than being resolved to a name once. A stream with
/// no `PW_KEY_TARGET_OBJECT` is moved by PipeWire when the default changes, so
/// storing today's default here would quietly pin somebody to the speakers
/// they happened to be using the day they opened settings, and they would find
/// out by plugging in a headset that no longer worked.
///
/// **`node.name`, not an index and not a description.** A PipeWire global id
/// renumbers across an unplug/replug, so a stored index eventually names a
/// different device; `node.description` is localised and is what a user's
/// volume control renames when they rename a device. `node.name` is the
/// routing target PipeWire itself takes and is the only one of the three meant
/// to be persisted.
///
/// **Global rather than per profile, deliberately, and it is worth saying
/// which side of ADR-013's line this falls.** That ADR splits configuration
/// from code by asking whether a thing belongs to an account or to the
/// machine: grants and flags moved into the profile because an approval given
/// on a throwaway account was silently in force on the one somebody plays. An
/// audio device is neither an approval nor an identity — it is the hardware in
/// front of the person sitting there, on the same footing as `graphics` and
/// `roblox` above. Switching profiles must not move the sound to a different
/// speaker.
///
/// **This is Cordial's choice of sink, not Roblox's device picker.** Roblox
/// does have one — `FmodAudioDevice::setOutputDevice`, `GetOutputDevices` — but
/// it is populated by FMOD's own output backend, which sees a single device on
/// every path Cordial provides, and the AAudio path has no
/// `AAudioStreamBuilder_setDeviceId` among the 25 symbols the engine looks up.
/// So the in-game list cannot be filled from here; see
/// `docs/analysis/aaudio-contract.md`. What this does instead is decide where
/// the one stream Roblox opens actually lands, which is what somebody asking
/// to "choose my audio device" wants either way.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct AudioOutput(pub String);

impl AudioOutput {
    /// Nothing chosen: follow whatever the session calls the default sink.
    pub fn is_system_default(&self) -> bool {
        self.0.trim().is_empty()
    }

    /// The value for `CORDIAL_AUDIO_SINK`, or `None` when the variable should
    /// not be set at all.
    ///
    /// Absent and empty mean the same thing to the client — see
    /// `configured_output_device()` in `native/pipewire_backend.cpp`, which
    /// treats `CORDIAL_AUDIO_SINK=` as unset for exactly this reason — but
    /// omitting it is the honest encoding of "the user expressed no opinion",
    /// on the same argument `launch.rs` makes for leaving `CORDIAL_GRAPHICS`
    /// out when the Renderer row says Automatic.
    pub fn env_value(&self) -> Option<&str> {
        if self.is_system_default() { None } else { Some(self.0.trim()) }
    }

}

/// One choice that settles both of the things Cordial can ask the engine to do
/// differently about graphics: which device it says it is, and how many of the
/// machine's cores the engine's own worker pools may use.
///
/// **Why one setting and not two.** They are genuinely two parameters, and a
/// cross product of them would be six rows describing combinations nobody has
/// measured. But they are the same question to the person asking it — "make
/// this run better" — and a Settings page with two graphics dropdowns whose
/// interaction is undocumented is worse than one list of named intents. So the
/// list below is intents, each naming exactly what it sets, and the
/// combinations that make no sense are simply not offered.
///
/// **What the device identity is and is not.** It decides the `User-Agent` the
/// engine sends and `InitParams.isTablet` — see `native/init_params.cpp`'s
/// `device_identity`, which carries the measurement of what roblox.com serves
/// for each. It is **not** established that Roblox tiers graphics defaults off
/// any of it; `isTablet` is the only field here the engine has been seen to
/// read, and only [`GraphicsOptimization::MobileTier`] sets it. Anyone
/// choosing a mode expecting a frame rate to move should measure it, and
/// `stacked config` says so rather than implying an effect nothing has shown.
///
/// **The CPU modes are unmeasured too**, and that is `cordial_runtime::flags`'s
/// own admission about `Performance`: its tables are adapted from mocktail's
/// policy and nothing on this project's hardware has compared them. They are
/// offered because an inference belongs behind a switch somebody chooses, which
/// is exactly the argument `flags::BUILTIN`'s comment makes at length — and for
/// the same reason the default sets neither of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum GraphicsOptimization {
    /// The client's own defaults: the `pc-windows-11` identity, and the
    /// engine's thread sizing left alone. What Cordial has shipped since
    /// 2026-08-20.
    ///
    /// **Still the default, deliberately.** A tablet identity was tried by a
    /// user and reported breaking PC features, and nothing has measured what
    /// the bare app token does to a frame rate either. The position when
    /// nothing has been measured is that the working default keeps working
    /// and the alternatives become choices.
    #[default]
    Balanced,
    /// `roblox-app`: the bare `RobloxApp/<version>(GlobalDist; Cordial)` token
    /// and nothing else — no platform words, no device block.
    ///
    /// The only identity that claims no form factor at all, which is the one
    /// Cordial can make honestly. Choosing it is safe for the in-experience
    /// web view: roblox.com serves the embedded app layout for this token as
    /// readily as for the other two, measured in
    /// `native/init_params.cpp`'s `device_identity`.
    RobloxApp,
    /// Claim an Android tablet, which is the only thing Cordial can say that
    /// asserts a mobile form factor to the engine (`InitParams.isTablet`).
    ///
    /// **Reported to break PC features** by the user who tried it. Offered
    /// because it is the only route to mobile-tier defaults if they exist,
    /// not because it is recommended.
    MobileTier,
    /// The default `pc-windows-11` identity, plus
    /// [`cordial_runtime::flags::Performance::Throughput`]:
    /// more engine worker threads and parallel prerender.
    MoreCores,
    /// The default `pc-windows-11` identity, plus
    /// [`cordial_runtime::flags::Performance::Latency`]:
    /// fewer threads and a smaller physics batch, for a machine short of cores.
    FewerCores,
}

impl GraphicsOptimization {
    /// The `CORDIAL_DEVICE_PROFILE` value this mode wants, or `None` when it
    /// wants the client's own default.
    ///
    /// `None` rather than `Some("roblox-app")` for the default, on exactly the
    /// argument `launch.rs` makes about `CORDIAL_GRAPHICS`: an absent variable
    /// is what tells the runtime the user expressed no opinion, which is the
    /// one state in which a plugin's `CordialDeviceProfile` entry is allowed
    /// to count. Sending the default explicitly would be the user silently
    /// outvoting every plugin while the row says the default.
    pub fn device_profile_env(self) -> Option<&'static str> {
        match self {
            GraphicsOptimization::Balanced
            | GraphicsOptimization::MoreCores
            | GraphicsOptimization::FewerCores => None,
            GraphicsOptimization::RobloxApp => Some("roblox-app"),
            GraphicsOptimization::MobileTier => Some("android-tablet"),
        }
    }

    /// The `CORDIAL_PERFORMANCE` value this mode wants, or `None` for the
    /// client's own default. Same reasoning as [`Self::device_profile_env`].
    pub fn performance_env(self) -> Option<&'static str> {
        match self {
            GraphicsOptimization::Balanced
            | GraphicsOptimization::RobloxApp
            | GraphicsOptimization::MobileTier => None,
            GraphicsOptimization::MoreCores => Some("throughput"),
            GraphicsOptimization::FewerCores => Some("latency"),
        }
    }
}

/// The profile a launch runs against when nobody has chosen otherwise.
///
/// ADR-012's migration lands the pre-existing storage at `profiles/default`, so
/// this name is not arbitrary — picking anything else would present as being
/// logged out.
pub const DEFAULT_PROFILE: &str = "default";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ShellConfig {
    /// How tall the game window's header bar is. See [`TitleBar`].
    pub title_bar: TitleBar,
    /// Where the Roblox build is, when the user has pinned it. Empty is the
    /// normal state and means "look every time" — see `install::locate`, which
    /// explains why a remembered answer is the wrong thing to store here.
    pub roblox: crate::install::RobloxInstall,
    /// Which profile an instance started from this shell runs. ADR-012: a
    /// profile is storage, an instance is a window, and one profile is held by
    /// at most one instance.
    pub profile: String,
    /// Ask Feral GameMode to raise the CPU governor, the process priority and
    /// the GPU's performance profile while the client runs, and to hold the
    /// screensaver off.
    ///
    /// Default on, which is what Sober does and what makes it worth having: a
    /// performance setting nobody finds is a performance setting nobody gets.
    /// It costs nothing on a machine without gamemoded — the request is a D-Bus
    /// call that fails, the client says so once and carries on — so there is no
    /// population this default hurts. `false` here becomes `CORDIAL_GAMEMODE=0`
    /// on the client, which is also the control for measuring what it does.
    pub gamemode: bool,
    /// When Cordial stops defeating the engine's idle throttle. See
    /// [`ThrottleWhen`], which carries the whole of the reasoning.
    #[serde(default)]
    pub throttle: ThrottleWhen,
    /// Whether the desktop's pointer acceleration reaches the camera. See
    /// [`PointerAcceleration`], which carries the reasoning and the reason
    /// there is no "never".
    #[serde(default)]
    pub pointer_acceleration: PointerAcceleration,
    /// Show MangoHUD's frame rate and frame time overlay over the client.
    ///
    /// Default off, unlike `gamemode`, and for a reason that is not timidity:
    /// this one is visible. It draws over the game whether or not the user
    /// wanted it there, so it has to be asked for. It is also the setting most
    /// likely to be switched on by somebody who has not got MangoHUD installed
    /// — see `launch::mangohud_layer`, which is what stops that being a silent
    /// no-op.
    /// Which graphics backend the client offers the engine.
    ///
    /// Stored as the same lowercase words `cordial_runtime::graphics::Backend`
    /// parses, and passed to the client as `CORDIAL_GRAPHICS` rather than
    /// written to a file: the backend has to be settled before the engine's
    /// first `dlopen`, which is long before anything opens a profile.
    ///
    /// `"automatic"` is the default and is not merely "Vulkan by another name" —
    /// it is the absence of a user opinion, which is what lets a plugin have
    /// one. See `graphics::resolve`.
    pub graphics: String,
    /// Which device Cordial says it is, and how hard the engine may push the
    /// machine's cores. See [`GraphicsOptimization`], which carries the whole
    /// of the reasoning and the reason the default sets neither.
    ///
    /// Separate from `graphics` above and deliberately so: that row picks
    /// which renderer Cordial offers the engine, which is a question about
    /// this machine's drivers. This one is about what Cordial claims to be and
    /// how many threads it asks for, and the two do not interact.
    #[serde(default)]
    pub graphics_optimization_mode: GraphicsOptimization,
    /// Which present mode the client asks the driver for. See [`PresentMode`],
    /// which carries the reasoning and the reason MAILBOX is the default.
    ///
    /// `#[serde(default)]`, so a `shell.json` written by an older Cordial --
    /// which had no such key at all -- loads rather than failing to parse, and
    /// reads as MAILBOX, which is what those builds were already doing. Nobody
    /// upgrading gets a different feel than they had.
    #[serde(default)]
    pub present_mode: PresentMode,
    /// Whether Cordial reads `/dev/input/js*` and tells Roblox about pads.
    ///
    /// On by default. Off is a real setting rather than a debugging knob:
    /// gamepad support ships with `gamepadType` still unestablished, so the
    /// glyphs Roblox draws may name the wrong brand (Sober #584, #1810), and
    /// somebody who would rather have no controller than the wrong buttons
    /// drawn should not have to find an environment variable to say so.
    ///
    /// It is also the escape hatch for a device that misbehaves. joydev binds
    /// to anything advertising ABS_X/ABS_Y, and while
    /// `gamepad::is_a_controller` now rejects the ones that are plainly not
    /// pads -- a virtual mouse became `/dev/input/js0` on the machine this was
    /// written on -- a filter that reads capabilities cannot anticipate every
    /// device, and the cost of it being wrong is Roblox believing a controller
    /// is plugged in.
    ///
    /// **No `#[serde(default)]` on this field, deliberately.** The container
    /// carries one, which fills a missing key from `ShellConfig::default()` --
    /// `true`, which is what builds before this key existed did. A field-level
    /// attribute would override that with `bool::default()`, silently reading
    /// as controllers-off for every existing install. The first draft had it
    /// and the test below caught it, which is why both the attribute's absence
    /// and the reason are written down.
    pub gamepad: bool,
    /// Quit the client when the user leaves a game and returns to the home
    /// screen.
    ///
    /// Off by default, and it has to be: closing somebody's session is the
    /// least reversible thing Cordial does on its own initiative, and a person
    /// who did not ask for it meets it once and loses whatever they were doing
    /// next. Whoever wants it wants it deliberately -- they launched from a
    /// deep link to play one game and have no use for the home screen.
    ///
    /// Keyed on the engine's own `leaveUGCGameInternal` and not on a
    /// disconnect; see `cordial_runtime::game_log`, which has the capture and
    /// the reason those are different questions.
    pub close_on_leave: bool,
    /// Plugin folders being worked on, loaded from where they live.
    ///
    /// **Each entry is one plugin's own folder** -- the one with `plugin.json`
    /// in it -- and not a folder that plugins are kept in. That is what "load
    /// unpacked" means: you point at the thing you are editing, in the
    /// checkout where you are editing it, and it loads without being packaged
    /// or copied anywhere.
    ///
    /// Empty is the ordinary state, and an empty list is what turns developer
    /// mode off -- there is no separate switch, because a switch that was on
    /// with nothing loaded would be a setting that does nothing. Adding a
    /// folder is turning it on.
    ///
    /// Passed to the client as `CORDIAL_UNPACKED_PLUGINS`, and unpacked
    /// plugins reload as they are edited (`sandbox::command`'s `--watch`).
    #[serde(default)]
    pub unpacked_plugins: Vec<String>,
    /// Pass the browser's sign-in ticket to the engine when launching from a
    /// link on roblox.com.
    ///
    /// **Off by default, because it moves a live credential.** A desktop launch
    /// link carries a one-time authentication ticket, which is what makes
    /// clicking play on the website sign you in on Windows. Whether this engine
    /// takes one is unverified -- see `cordial_runtime::deeplink` -- so the
    /// default is what Cordial has always done: drop it and let you sign in.
    ///
    /// Turning it on is choosing to hand a credential to the engine in exchange
    /// for not typing a password. A reasonable trade to offer, and not one to
    /// make on somebody's behalf.
    ///
    /// This controls engine forwarding only. Browser account routing redeems
    /// and removes the ticket separately; `CORDIAL_BROWSER_ACCOUNT_ROUTING=0`
    /// disables that lookup. See ADR-035 for the credential-use decision.
    #[serde(default)]
    pub carry_launch_ticket: bool,
    pub mangohud: bool,
    /// Sharpen and anti-alias the frame with vkBasalt's Vulkan layer.
    ///
    /// Same shape as `mangohud` above, deliberately: both are implicit Vulkan
    /// layers Cordial does not implement, both draw or alter the frame whether
    /// or not the layer is actually installed, and both need the same guard —
    /// see `launch::vkbasalt_layer`. Default off for the same reason as
    /// `mangohud`: it changes what is on screen, so it is asked for rather than
    /// assumed.
    ///
    /// Turning it on writes a per-profile `vkBasalt.conf` the first time, at
    /// `launch::vkbasalt_config_path`, and never overwrites one that is already
    /// there — see that function's doc for why silently replacing a config
    /// somebody has since edited would be the wrong failure to introduce.
    #[serde(default)]
    pub vkbasalt: bool,
    /// Which audio device Roblox plays through. See [`AudioOutput`], which
    /// carries the whole of the reasoning, including why the stored form is a
    /// `node.name` and why the default must stay "follow the system".
    #[serde(default)]
    pub audio_output: AudioOutput,
    /// The game window's colours. See [`Theme`]; passed as `CORDIAL_THEME`.
    pub theme: Theme,
    /// Keep the cursor inside the window while it is fullscreen and the game
    /// has not locked it. Default on: the case it exists for is a second
    /// monitor taking a click meant for a fullscreen game's menu, and somebody
    /// who wants the cursor free again has this and a compositor that will
    /// release it on alt-tab. `false` becomes `CORDIAL_NO_FULLSCREEN_CONFINE=1`
    /// on the client, which is also the control.
    pub fullscreen_confine: bool,
    /// A frame-rate target for the engine's task scheduler. `None`, the
    /// default, is the refresh rate of the display the game is on, which the
    /// client reads itself; `Some(0)` is the engine's own target (60); any
    /// other number is that target. Passed as `CORDIAL_FPS_CAP`, which the
    /// client turns into a `DFIntTaskSchedulerTargetFps` layer beneath the
    /// user's own `flags.json` -- see `flags.rs`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fps_cap: Option<u32>,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            title_bar: TitleBar::default(),
            roblox: crate::install::RobloxInstall::default(),
            profile: DEFAULT_PROFILE.to_string(),
            gamemode: true,
            throttle: ThrottleWhen::default(),
            pointer_acceleration: PointerAcceleration::default(),
            graphics: "automatic".to_string(),
            graphics_optimization_mode: GraphicsOptimization::default(),
            present_mode: PresentMode::default(),
            gamepad: true,
            close_on_leave: false,
            unpacked_plugins: Vec::new(),
            carry_launch_ticket: false,
            audio_output: AudioOutput::default(),
            mangohud: false,
            vkbasalt: false,
            theme: Theme::default(),
            fullscreen_confine: true,
            fps_cap: None,
        }
    }
}

/// `CORDIAL_SHELL_CONFIG` overrides the path outright, the same override
/// pattern `cordial_plugins::grants::path` and `manifest::plugin_root` use —
/// useful for tests and for running more than one Cordial config side by
/// side without them fighting over the same file.
pub fn path() -> PathBuf {
    std::env::var_os("CORDIAL_SHELL_CONFIG").map(PathBuf::from).unwrap_or_else(|| {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .unwrap_or_else(std::env::temp_dir)
            .join("cordial/shell.json")
    })
}

/// Load the config, or the defaults. A missing file is the ordinary case —
/// most people never open settings — and a malformed one is reported and
/// treated the same as missing, per the module docs above.
pub fn load(path: &Path) -> ShellConfig {
    let Ok(text) = std::fs::read_to_string(path) else {
        return ShellConfig::default();
    };
    match serde_json::from_str(&text) {
        Ok(config) => config,
        Err(e) => {
            println!("  shell: {} is not usable ({e}); using defaults", path.display());
            ShellConfig::default()
        }
    }
}

pub fn save(path: &Path, config: &ShellConfig) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(config).expect("ShellConfig always serialises");
    std::fs::write(path, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("cordial-shell-config-test");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn the_throttle_setting_and_the_stored_word_agree() {
        // The word the file stores and the word the client parses are the
        // same word; nothing else would notice them drifting apart.
        assert_eq!(ThrottleWhen::default(), ThrottleWhen::Visible);
        assert_eq!(ThrottleWhen::Visible.as_str(), "visible");
        assert_eq!(ThrottleWhen::Unfocused.as_str(), "unfocused");
        assert_eq!(ThrottleWhen::Off.as_str(), "off");
    }

    #[test]
    fn an_older_config_without_a_throttle_row_gets_the_visible_default() {
        // The row is new, so every existing shell.json lacks it. Landing on
        // anything but Visible would change behaviour for people who never
        // opened the setting.
        let p = scratch("no-throttle.json");
        std::fs::write(&p, br#"{"appearance":"dark","gamemode":true}"#).unwrap();
        assert_eq!(load(&p).throttle, ThrottleWhen::Visible);
    }

    #[test]
    fn a_missing_file_defaults_to_the_brand_theme() {
        let p = scratch("missing.json");
        let _ = std::fs::remove_file(&p);
        assert_eq!(load(&p).theme, Theme::Stacked);
    }

    #[test]
    fn a_malformed_file_falls_back_to_defaults_rather_than_refusing_to_start() {
        let p = scratch("malformed.json");
        std::fs::write(&p, "{not json").unwrap();
        let config = load(&p);
        assert_eq!(config.theme, Theme::Stacked);
        assert!(config.fullscreen_confine);
        assert_eq!(config.fps_cap, None);
    }

    #[test]
    fn a_saved_choice_round_trips() {
        let p = scratch("roundtrip.json");
        save(
            &p,
            &ShellConfig {
                theme: Theme::System,
                fullscreen_confine: false,
                fps_cap: Some(144),
                ..Default::default()
            },
        )
        .unwrap();
        let back = load(&p);
        assert_eq!(back.theme, Theme::System);
        assert!(!back.fullscreen_confine);
        assert_eq!(back.fps_cap, Some(144));
    }

    #[test]
    fn a_file_from_the_gtk_launcher_still_loads_and_keeps_what_still_means_something() {
        // Every existing shell.json was written by the launcher this CLI
        // replaced, and carries fields that no longer exist. Unknown fields are
        // ignored rather than refused, so the profile and the performance
        // choices in the same file survive the upgrade.
        let p = scratch("gtk-era.json");
        std::fs::write(
            &p,
            r#"{"appearance":"dark","automatic_updates":"manual","fullscreen_accel":"F11",
                "multi_instance_warning_seen":true,"profile":"alt","gamemode":false}"#,
        )
        .unwrap();
        let config = load(&p);
        assert_eq!(config.profile, "alt");
        assert!(!config.gamemode);
    }

    #[test]
    fn a_config_written_before_the_roblox_fields_existed_still_loads() {
        // `#[serde(default)]` is what makes this true, and it is worth a test
        // rather than a note: everyone who has run this shell already has a
        // shell.json holding nothing but `appearance`, and a launcher that
        // refuses to start over a missing field it invented is a worse failure
        // than any of the ones it is meant to report.
        let p = scratch("older-schema.json");
        std::fs::write(&p, r#"{"appearance":"dark"}"#).unwrap();
        let config = load(&p);
        assert_eq!(config.profile, DEFAULT_PROFILE);
        assert_eq!(config.roblox, crate::install::RobloxInstall::default());
        // The performance fields are newer still, and the same argument
        // applies to them: everybody's shell.json predates them.
        assert!(config.gamemode, "an older config must still get GameMode's default");
        assert!(!config.mangohud);
        assert!(!config.vkbasalt, "vkbasalt is newer still and every existing shell.json predates it");
    }

    #[test]
    fn the_performance_switches_round_trip() {
        // Both directions, because both defaults are worth being able to
        // reverse and a setting that only saves the value it already had would
        // pass a one-way test.
        let p = scratch("performance.json");
        save(
            &p,
            &ShellConfig { gamemode: false, mangohud: true, vkbasalt: true, ..Default::default() },
        )
        .unwrap();
        let back = load(&p);
        assert!(!back.gamemode);
        assert!(back.mangohud);
        assert!(back.vkbasalt);
    }

    #[test]
    fn the_roblox_paths_round_trip() {
        let p = scratch("roblox.json");
        let mut config = ShellConfig::default();
        config.roblox.apk = Some(PathBuf::from("/somewhere/base.apk"));
        config.roblox.lib_dir = Some(PathBuf::from("/somewhere/lib/x86_64"));
        config.profile = "alt_account".into();
        save(&p, &config).unwrap();
        let back = load(&p);
        assert_eq!(back.roblox.apk, config.roblox.apk);
        assert_eq!(back.roblox.lib_dir, config.roblox.lib_dir);
        assert_eq!(back.profile, "alt_account");
    }

    fn sinks() -> Vec<String> {
        // The names on the machine this was written on, abbreviated only where
        // the abbreviation cannot change the answer. Two of them share a long
        // prefix on purpose: that is the ordinary case for one sound card with
        // several HDMI outputs, and a prefix match would send audio to the
        // wrong one.
        vec![
            "alsa_output.pci-0000_00_1f.3-platform-skl_hda_dsp_generic.HiFi__HDMI1__sink".into(),
            "alsa_output.pci-0000_00_1f.3-platform-skl_hda_dsp_generic.HiFi__HDMI2__sink".into(),
            "alsa_output.pci-0000_00_1f.3-platform-skl_hda_dsp_generic.HiFi__Speaker__sink".into(),
        ]
    }

    #[test]
    fn nothing_chosen_means_follow_the_system_and_sends_no_variable() {
        // The default, and the one that must not drift: an unset
        // `CORDIAL_AUDIO_SINK` is what leaves PipeWire free to move the stream
        // when the user changes their default sink while playing.
        let out = AudioOutput::default();
        assert!(out.is_system_default());
        assert_eq!(out.env_value(), None);
    }

    #[test]
    fn a_chosen_sink_round_trips_through_the_environment() {
        let list = sinks();
        let out = AudioOutput(list[1].clone());
        assert!(!out.is_system_default());
        assert_eq!(out.env_value(), Some(list[1].as_str()));
    }

    #[test]
    fn a_sink_that_is_no_longer_present_does_not_read_as_the_system_default() {
        // The unplugged-headset case: the value must still say a device was
        // chosen, so the choice is not thrown away by the device being
        // absent for one launch.
        let gone = AudioOutput("bluez_output.AC_12_2F_9E_00_11.1".into());
        assert!(!gone.is_system_default(), "the choice must survive the device going away");
        assert_eq!(gone.env_value(), Some("bluez_output.AC_12_2F_9E_00_11.1"));
    }

    #[test]
    fn whitespace_is_not_a_device() {
        // A hand-edited shell.json is the only way to produce this, and the
        // failure it would otherwise cause is the expensive one: a
        // `CORDIAL_AUDIO_SINK=" "` reaches PipeWire as a target node called
        // " ", which matches nothing, so the client falls back and logs on
        // every stream open for ever.
        let blank = AudioOutput("   ".into());
        assert!(blank.is_system_default());
        assert_eq!(blank.env_value(), None);
    }

    #[test]
    fn the_audio_output_round_trips_through_the_file() {
        let p = scratch("audio-output.json");
        let name = "alsa_output.usb-Generic_USB_Audio-00.analog-stereo";
        save(&p, &ShellConfig { audio_output: AudioOutput(name.into()), ..Default::default() })
            .unwrap();
        assert_eq!(load(&p).audio_output, AudioOutput(name.into()));
    }

    #[test]
    fn a_config_written_before_the_audio_row_existed_follows_the_system_default() {
        // Everybody's shell.json predates this field, and landing on anything
        // but "follow the system" would move somebody's game audio to another
        // speaker because they upgraded Cordial.
        let p = scratch("pre-audio.json");
        std::fs::write(&p, r#"{"appearance":"dark","profile":"default"}"#).unwrap();
        assert!(load(&p).audio_output.is_system_default());
    }

    /// Controllers work out of the box, and the switch that turns them off
    /// survives a round trip through the file.
    ///
    /// The default matters enough to pin: `#[serde(default)]` on the container
    /// means a missing key takes `ShellConfig::default()`'s value, so a plain
    /// `#[derive(Default)]` on a `bool` field would silently make this `false`
    /// for every existing install. That is the failure this asserts against.
    #[test]
    fn controllers_are_on_unless_the_switch_says_otherwise() {
        assert!(ShellConfig::default().gamepad, "a fresh install has controllers on");
        let older = r#"{"gamemode":true,"graphics":"automatic","mangohud":false}"#;
        let parsed: ShellConfig = serde_json::from_str(older).expect("an older shell.json must load");
        assert!(parsed.gamepad, "a config predating the key must not read as controllers off");
        let off: ShellConfig = serde_json::from_str(r#"{"gamepad":false}"#).unwrap();
        assert!(!off.gamepad, "an explicit false must survive");
    }

    /// **A fresh install is responsive, and pays power for it.**
    ///
    /// Pinned with the reason attached because this default has now moved
    /// twice. FIFO is the better argument on paper -- it wastes no frames and
    /// it is the only mode the specification guarantees -- and it shipped for
    /// about an hour before a user reported the mouse felt floaty and then
    /// confirmed, with the control, that Mailbox fixed it. Anybody moving it
    /// back to FIFO should have a power measurement in hand, because the
    /// latency side of this trade now has one and the power side does not.
    #[test]
    fn a_fresh_install_is_responsive_rather_than_frugal() {
        assert_eq!(ShellConfig::default().present_mode, PresentMode::Mailbox);
        assert_eq!(PresentMode::default().as_env(), Some("mailbox"));
    }

    /// Automatic must send nothing, or ADR-020's plugin path is unreachable.
    ///
    /// The runtime's precedence is environment, then flag layers, then its
    /// own `auto` (MAILBOX).
    /// An empty string or the word "auto" would both also fall through today,
    /// but only sending nothing keeps the launcher out of a decision it was
    /// not asked to make -- and only `None` is checked by the `if let` in
    /// `launch.rs`, so this is the assertion that actually holds that branch.
    #[test]
    fn automatic_sends_no_variable_at_all() {
        assert_eq!(PresentMode::Automatic.as_env(), None);
        for mode in [PresentMode::Fifo, PresentMode::Mailbox, PresentMode::Immediate] {
            assert!(mode.as_env().is_some(), "{mode:?} must name itself to the client");
        }
    }

    /// A `shell.json` from a Cordial that predates this key must still load,
    /// and must read as the default, MAILBOX, rather than refusing to parse.
    #[test]
    fn an_older_config_without_the_key_keeps_the_feel_it_had() {
        let older = r#"{"gamemode":true,"graphics":"automatic","mangohud":false}"#;
        let parsed: ShellConfig = serde_json::from_str(older).expect("an older shell.json must load");
        assert_eq!(parsed.present_mode, PresentMode::Mailbox);
    }
}
