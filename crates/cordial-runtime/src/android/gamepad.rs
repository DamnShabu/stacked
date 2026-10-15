//! Gamepads, from the host's joystick devices into `NativeInputInterface`.
//!
//! **This is off by default, and the reason is a blocker rather than caution.**
//!
//! The six gamepad natives are real: `tools/dex_method.py` reads all six
//! descriptors out of the shipping APK's dex, and `readelf --dyn-syms` finds all
//! six exported by `libroblox.so`. What is missing is one integer's meaning.
//! This build ships no type-less connect entry point -- `nativeGamepadConnectEvent`
//! returns `no match` from the dex and `0` from `readelf | grep -c` -- so a pad
//! cannot be announced without supplying a `gamepadType`, and nothing available
//! here says what the ordinals are:
//!
//! `docs/traces/waydroid-roblox-startup.log.gz` has no gamepad line in it at
//! all, because the capture was taken with no pad plugged in. The dex declares
//! no class with `Gamepad` in its name, and the Java caller is obfuscated.
//! mocktail resolves all six symbols and never calls one of them. The only
//! signal in the binary is three glyph asset folders -- `DefaultController`,
//! `PlayStationController`, `XboxController` -- beside a reflected
//! `RBX::GamepadType`, which suggests how *many* values there are and says
//! nothing about which is which.
//!
//! Reading that as "0 = unknown, 1 = PlayStation, 2 = Xbox" would be exactly the
//! wrong-but-plausible conclusion AGENTS.md's opening rule is about, and the
//! wrong outcome is already a live bug next door: Sober #1018 is
//! *"Sober detects my PS4 controller as a XBOX one"*. So Cordial does not guess
//! it on a user's behalf. Nothing here calls the connect native until somebody
//! sets `CORDIAL_GAMEPAD=1`, and when they do they are told the type is unverified.
//!
//! **The experiment that settles it, and what this module exists to make
//! possible.** Because the glyph folders are per-type, the engine's own UI is a
//! readout of `gamepadType`: announce a pad with type N, open something that
//! draws button glyphs, and photograph the frame with `cordial_screenshot` out
//! of Cordial's swapchain. The N that draws PlayStation glyphs *is* PlayStation,
//! and a different N drawing different glyphs is the control. `CORDIAL_GAMEPAD_TYPE=N`
//! is that sweep, and `CORDIAL_GAMEPAD_PROBE=1` runs it with no hardware at all
//! -- it announces one synthetic pad and sends nothing.
//!
//! **"Which is enough to draw the glyphs" used to follow that sentence, and it
//! is wrong.** Reported by the project owner from ordinary use on 2026-08-30:
//! Roblox switches its displayed glyph set only when an input is actually
//! *used*, not when a device is announced. A pad that connects and then sends
//! nothing changes nothing on screen, so the probe as built cannot produce the
//! reading this module was designed around. Labelled as a user report rather
//! than a measurement here, because it was not taken with an instrument -- but
//! it explains a run that would otherwise look like a mystery, and it should be
//! believed before it is re-tested.
//!
//! It was re-tested first, and agrees. A sweep of N = 0, 1, 2 and 99 on
//! 2026-08-30 found the pre-login Landing and Sign In screens identical for
//! every N, with no button glyph anywhere in the frame -- and 99, far outside
//! any plausible enum, no more distinguishable than 0. Two independent reasons
//! for that null result now stand: no input was sent, and the shell is not
//! where glyphs are drawn in the first place. Sober #584 and #1810 both place
//! the wrong-type symptom inside an experience rather than in the shell.
//!
//! **So the experiment needs two changes before it can work**, and neither is
//! large: the probe must *send* an input after announcing -- a button press and
//! release, or an axis deflection, is enough to make the engine switch -- and
//! the observation must be made inside a joined experience rather than at the
//! shell. Second best is re-capturing the logcat with a pad attached.
//!
//! **Run 2026-08-30, and `Landing`/`Login` are ruled out as the screen.** N
//! swept over 0, 1, 2 and 99 all reach the pre-login `Landing` page identically
//! -- same generic focus outline, no button-shape icon anywhere in the frame,
//! `N=99` no more distinguishable or crash-prone than `N=0`. `Sign In`'s
//! username/password form has no controller affordance either. That is the
//! module's own control working as designed: four different N, zero different
//! glyph sets, is a finding about the *screen* having no readout, not a claim
//! that the ordinals are equal. Sober's issues #584 and #1810 both describe the
//! wrong type surfacing *inside a game*, not in Sober's shell, which points the
//! actual readout in-experience rather than pre-login on that runtime too. A
//! repeat of this sweep needs a signed-in profile that reaches `Home` and joins
//! a place -- see `docs/NEXT.md`'s entry of the same date before spending a
//! session re-confirming Landing draws nothing.
//!
//! **No rumble.** `android/os/Vibrator` is declared in the dex and implemented
//! nowhere in Cordial. Force feedback is out of scope here and is absent rather
//! than stubbed, because a rumble call that silently does nothing is the stub
//! that lies.
//!
//! **The host half is a placeholder and is meant to be replaced.** It reads
//! Linux's legacy joydev nodes (`/dev/input/js*`) with plain `std::fs` and no
//! new dependency, because the alternative -- gilrs, which is the right answer
//! -- carries `libudev-sys`, whose `build.rs` is an unconditional
//! `pkg_config::find_library("libudev").unwrap()` and whose binaries link
//! `libudev.so.1`. Cordial has no udev anywhere in `Cargo.lock` today, and
//! taking that on touches the flatpak, deb, rpm and AppImage manifests and CI.
//! That is a packaging decision, not an implementation detail, and it should not
//! ride in on a feature that cannot be switched on yet.
//!
//! What is lost by waiting is real and worth naming: joydev reports *indices*
//! rather than button codes. [`Layout`] recovers the code behind each index from
//! the capability masks sysfs publishes, which is enough to stop a pad with
//! fewer axes or buttons than the standard one shifting everything after the
//! gap; [`button_keycode`] and [`axis_code`] remain as the standard layout, used
//! when sysfs cannot be read. What it does not do is what gilrs carries in
//! `SDL_GameControllerDB`: a per-model mapping for pads whose drivers report
//! unusual codes, and the vendor and product ids a `gamepadType` classifier would
//! want once the ordinals are known. When the udev dependency is accepted, this
//! module's [`poll`] is the seam to swap underneath.
//!
//! TASKS.md recommends `libmanette` for this and should not: it has no Rust
//! binding at all (`cargo search libmanette` returns nothing), so adopting it
//! means hand-written GObject FFI. TASKS.md also points this work at the
//! GameActivity motion-event path, which is the pipe
//! `native/game_activity.cpp` records as accepted-and-ignored -- every click
//! delivered that way was taken and nothing on screen moved. Gamepad goes
//! through `NativeInputInterface` for the same reason the mouse does.

use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::OnceLock;

use super::input;

/// `CORDIAL_GAMEPAD=1`. The off switch, defaulting to off.
///
/// On by default since 0.13.0. `CORDIAL_GAMEPAD=0` turns it off.
///
/// **This was off, and "off is the honest default while `gamepadType` is
/// unestablished" was the reason. The ordinal is still unestablished; the
/// judgement changed.** Refusing to guess kept Cordial from mislabelling a pad
/// the way Sober #1018 does -- "Sober detects my PS4 controller as a XBOX one"
/// -- and the cost of that refusal was no controller support of any kind. For a
/// handheld, and Steam Deck is the case that forced the question, that is not
/// the better side of the trade.
///
/// What actually goes wrong with a wrong ordinal is worth stating precisely,
/// because it is smaller than "unverified" sounds. Sober #584 is "almost every
/// single game thinks im on xbox" and #1810 is a DualShock 4 drawing the wrong
/// face buttons. **The glyphs are wrong and the buttons work.** That is a
/// cosmetic fault with a documented override, against a feature that otherwise
/// does not exist.
///
/// The parts that are not guesses: the host half reads real `/dev/input/js*`
/// nodes and its decoding is unit-tested, and the six natives are all present
/// in this build. The single unknown is which integer names which brand of
/// glyph, `CORDIAL_GAMEPAD_TYPE` overrides it, and the launch log says so the
/// first time a pad is seen rather than leaving a user to find out.
fn enabled() -> bool {
    match SWITCH.load(Ordering::Relaxed) {
        SWITCH_ON => true,
        SWITCH_OFF => false,
        // First read. Not a `OnceLock` any more: the switch can change while a
        // game is running (the control socket's `gamepad on|off`), and a cache
        // that cannot be written is what made it launch-only.
        _ => {
            let on = enabled_for(std::env::var("CORDIAL_GAMEPAD").ok().as_deref());
            // `compare_exchange` so a live change that landed between the load
            // above and here is not overwritten by the environment.
            let _ = SWITCH.compare_exchange(
                SWITCH_UNSET,
                if on { SWITCH_ON } else { SWITCH_OFF },
                Ordering::Relaxed,
                Ordering::Relaxed,
            );
            SWITCH.load(Ordering::Relaxed) == SWITCH_ON
        }
    }
}

const SWITCH_UNSET: u8 = 0;
const SWITCH_ON: u8 = 1;
const SWITCH_OFF: u8 = 2;
static SWITCH: AtomicU8 = AtomicU8::new(SWITCH_UNSET);

/// Taken by every test that moves [`SWITCH`], here and in `devctl`, so that
/// one never reads it mid-flip by another.
#[cfg(test)]
pub(crate) static TEST_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Set when the switch has gone off and pads may still be held. The pump checks
/// it each tick, so turning the setting off costs a relaxed load and not a lock
/// on a path that runs thousands of times a second.
static WITHDRAW: AtomicBool = AtomicBool::new(false);

/// The live switch, driven by the control socket's `gamepad on|off`; the
/// launch setting (`stacked config set gamepad`) only seeds it through
/// `CORDIAL_GAMEPAD`. Takes effect on the pump's next tick, which is
/// where the engine's natives are called from and therefore where a disconnect
/// has to be sent.
///
/// Turning it on needs nothing further: the rescan that already runs every two
/// seconds finds the pads and announces them as it would at launch. Turning it
/// off is what needs work, because the engine has been told those pads exist and
/// going quiet would leave it with controllers that never move -- see
/// [`withdraw`].
pub fn set_enabled(on: bool) {
    SWITCH.store(if on { SWITCH_ON } else { SWITCH_OFF }, Ordering::Relaxed);
    if !on {
        WITHDRAW.store(true, Ordering::Relaxed);
    }
}

/// The setting in force, for the control socket's reply.
pub fn current_enabled() -> bool {
    enabled()
}

/// Tell the engine every announced pad has gone, and let go of the devices.
///
/// This is the path a pad that is unplugged already takes --
/// `deliver_gamepad_disconnect` for its id -- which is the reason it is
/// reasonable to use here: the engine has to cope with a disconnect whenever a
/// user pulls a cable, so a setting that sends the same thing is asking for
/// nothing it does not already handle. **Only announced pads get one.** A pad
/// that was opened but never announced is one the engine was never told about,
/// and disconnecting an id it has not heard of is the kind of call that has no
/// reason to be safe.
///
/// `disconnect` is a parameter only so the test does not need an engine.
fn withdraw(pads: &mut Vec<Pad>, mut disconnect: impl FnMut(i32)) -> usize {
    let mut sent = 0;
    for pad in pads.drain(..) {
        if pad.announced {
            disconnect(pad.id);
            sent += 1;
        }
    }
    sent
}

/// The gate's decision, separated from where it reads it.
///
/// Split out only so it can be tested: `enabled` caches into a `OnceLock`, so a
/// test that set the variable would decide the answer for every other test in
/// the binary and lose a coin-flip against whichever ran first.
fn enabled_for(v: Option<&str>) -> bool {
    // Present-but-"0" is off; absent is on. `devctl::is_enabled`'s idiom
    // inverted, rather than a second spelling of the same idea.
    !matches!(v, Some("0"))
}

/// `CORDIAL_GAMEPAD_PROBE=1` — announce one pad that does not exist, and send it
/// no events.
///
/// This is the sweep harness, and **it does not currently work.** The paragraph
/// here used to say drawing "needs only the connect call and the capability
/// declaration; it does not need a thumbstick to move". The module comment
/// records why that is wrong: Roblox switches its glyph set when an input is
/// *used*, not when a device is announced, so a pad that connects and stays
/// silent changes nothing on screen. A sweep of N = 0, 1, 2 and 99 confirmed it
/// by finding every value identical.
///
/// Kept rather than deleted because the harness is most of what a working
/// experiment needs -- announcing a pad with no hardware is the hard part, and
/// it does that. What it is missing is sending a press and release afterwards,
/// and being run inside a joined experience rather than at the shell.
fn probe() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("CORDIAL_GAMEPAD_PROBE").is_some())
}

/// `CORDIAL_GAMEPAD_TYPE=N` — the argument nothing here can establish.
///
/// **Defaults to 0 and 0 is a guess.** The engine takes this as an
/// `RBX::GamepadType`, which is a reflected enum whose ordinals are not readable
/// from anything available here; 0 is chosen only because it is the value least
/// likely to be a specific console, on the usual convention that an enum's zero
/// is its unknown. That convention is not evidence, and a client that shows the
/// wrong button glyphs is what being wrong here looks like.
pub fn gamepad_type() -> i32 {
    static TYPE: OnceLock<i32> = OnceLock::new();
    *TYPE.get_or_init(|| {
        let Some(v) = std::env::var_os("CORDIAL_GAMEPAD_TYPE") else {
            return 0;
        };
        match v.to_string_lossy().trim().parse::<i32>() {
            Ok(n) => n,
            _ => {
                eprintln!(
                    "[cordial] CORDIAL_GAMEPAD_TYPE={} is not a number; using 0",
                    v.to_string_lossy()
                );
                0
            }
        }
    })
}

// ------------------------------------------------------------------- joydev
//
// `struct js_event` is 8 bytes: a little-endian u32 of milliseconds, an i16
// value, a u8 type and a u8 index. Decoded by hand rather than through a crate
// because it is eight bytes with a stable layout and the alternative pulls in a
// C library -- see the module comment.

const JS_EVENT_BUTTON: u8 = 0x01;
const JS_EVENT_AXIS: u8 = 0x02;
/// Set on the burst joydev queues at open, one packet per button and per axis,
/// reporting the device's current state. Cordial counts that burst to learn the
/// device's shape: it is the same information `JSIOCGBUTTONS`/`JSIOCGAXES` carry
/// and it arrives without an `ioctl`, which would mean a `libc` dependency this
/// module does not otherwise need.
const JS_EVENT_INIT: u8 = 0x80;

const JS_EVENT_LEN: usize = 8;

/// Full-scale deflection on a joydev axis, which reports `-32767..=32767`.
const JS_AXIS_MAX: f32 = 32767.0;

/// `O_NONBLOCK`. Hardcoded rather than taken from `libc`, whose only use in this
/// crate would be this constant; joydev is a Linux interface and this file does
/// not build for anything else.
const O_NONBLOCK: i32 = 0o4000;

/// How many pads to hold at once.
///
/// Four is Roblox's own limit on connected gamepads. There is no udev here and
/// so no hotplug notification, and [`poll`] re-reads the directory on a timer
/// instead.
///
/// **It bounds the pads held, not the `jsN` numbers looked at**, which it used to
/// do (`js0` to `js3`). A launcher that gives each game its own view of
/// `/dev/input` -- PartyDeck binds in one controller's node and masks the rest
/// -- leaves a pad that is `js5` on the host as the only `js*` the client can
/// see, and a fixed range never found it. ADR-058.
const MAX_PADS: i32 = 4;

/// How often to look for a pad that was plugged in after startup.
///
/// Without udev there is nothing to wake on, so this is a poll, and it is slow
/// on purpose: four `open` attempts a second on a path that usually does not
/// exist is not free, and the pump this runs on is the one that must not
/// acquire work per tick. Two seconds is a delay a human notices once when
/// plugging a pad in and never again.
const RESCAN: std::time::Duration = std::time::Duration::from_secs(2);

struct Pad {
    /// The engine's device id: the lowest free slot, so it stays small and dense
    /// whatever the host numbered the node. That the engine wants a small dense
    /// id is INFERRED -- the dex says only `int`.
    id: i32,
    /// The `N` of `/dev/input/jsN`, which is what sysfs and the open are keyed
    /// by. Equal to `id` when the host's nodes start at `js0`, which is why the
    /// two were once one field.
    node: i32,
    file: std::fs::File,
    /// False until the init burst has been counted and the capability
    /// declaration made. No button or axis event is sent before this is true,
    /// which is the whole reason the flag exists: an event for a device the
    /// engine has not been told the shape of is the half-working gamepad
    /// support this module refuses to ship.
    announced: bool,
    /// What each joydev button and axis index means on *this* pad. See
    /// [`Layout`]: the fixed tables below are only its fallback.
    layout: Layout,
}

static PADS: std::sync::Mutex<Vec<Pad>> = std::sync::Mutex::new(Vec::new());

/// What each joydev button and axis *index* on one pad stands for, read from the
/// pad rather than assumed.
///
/// **joydev numbers the buttons and axes a device has, not the ones a standard
/// pad has, and the fixed tables below assumed the second.** The kernel assigns
/// axis indices to the `ABS_*` codes the device declares in ascending order, and
/// button indices to the declared `BTN_*` codes from `BTN_JOYSTICK` upwards
/// (then the ones below it). A pad that omits one shifts everything after it:
///
/// - A pad with digital triggers declares no `ABS_Z`/`ABS_RZ`, so its right
///   stick arrives as axes 2 and 3, which the table read as the left trigger and
///   the right stick's *horizontal* axis. The right stick then pushes nothing
///   sideways and the camera does not turn -- reported on #113.
/// - `xpad` declares no `BTN_C` and no `BTN_Z`, so X is joydev button 2, not 3,
///   and every button from there on was one or two places out.
///
/// The code each index stands for is exactly what sysfs publishes beside the
/// device (`capabilities/key` and `capabilities/abs`), so no ioctl and no libc
/// are needed; the same files [`is_a_controller`] already reads. If either is
/// unreadable the layout falls back to the standard-pad tables, which is what
/// every pad got before.
///
/// The ordering rule is read from the kernel's `joydev_connect`, and checked
/// here only against the layouts in the tests below, not against hardware: no
/// pad is attached to the machine this was written on. `INFERRED` for the
/// engine's side, as everywhere in this file.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Layout {
    buttons: Vec<Option<i32>>,
    axes: Vec<Option<i32>>,
}

impl Layout {
    /// The standard pad: every button from `BTN_SOUTH` to `BTN_THUMBR` and axes
    /// X, Y, Z, RX, RY, RZ and one hat. Also the answer when sysfs says nothing.
    fn standard() -> Self {
        Layout {
            buttons: (0..15).map(button_keycode).collect(),
            axes: (0..8).map(axis_code).collect(),
        }
    }

    /// From the two sysfs capability masks, in joydev's numbering.
    fn from_capabilities(key_mask: &str, abs_mask: &str) -> Self {
        let keys = set_bits(key_mask);
        let low: Vec<usize> = keys.iter().copied().filter(|&c| c < BTN_JOYSTICK).collect();
        let high: Vec<usize> = keys.iter().copied().filter(|&c| c >= BTN_JOYSTICK).collect();
        // `BTN_JOYSTICK` and above first, then `BTN_MISC` up to it: the order
        // `joydev_connect` fills `keymap` in.
        let buttons = high.into_iter().chain(low.into_iter().filter(|&c| c >= BTN_MISC))
            .map(btn_to_android)
            .collect();
        let axes = set_bits(abs_mask).into_iter().map(abs_to_android).collect();
        Layout { buttons, axes }
    }

    fn button(&self, index: u8) -> Option<i32> {
        self.buttons.get(index as usize).copied().flatten()
    }

    fn axis(&self, index: u8) -> Option<i32> {
        self.axes.get(index as usize).copied().flatten()
    }
}

/// `BTN_MISC`, the bottom of the range joydev considers buttons at all.
const BTN_MISC: usize = 0x100;

/// Every set bit of a sysfs capability mask, ascending.
///
/// The mask prints as space-separated hex words, most significant first, each
/// 64 bits wide -- the layout [`declares_a_gamepad_button`] already relies on.
fn set_bits(mask: &str) -> Vec<usize> {
    let words: Vec<&str> = mask.split_whitespace().collect();
    let mut bits = Vec::new();
    for (i, w) in words.iter().rev().enumerate() {
        let Ok(word) = u64::from_str_radix(w, 16) else { continue };
        for b in 0..64 {
            if word >> b & 1 == 1 {
                bits.push(i * 64 + b);
            }
        }
    }
    bits
}

/// Android `KEYCODE_BUTTON_*` for a Linux `BTN_*` code, or `None` for one with
/// no entry here. The same mapping Android's generic key layout makes, and the
/// one [`button_keycode`] spells positionally for a pad that declares them all.
fn btn_to_android(code: usize) -> Option<i32> {
    Some(match code {
        0x130 => 96,  // BTN_SOUTH  -> BUTTON_A
        0x131 => 97,  // BTN_EAST   -> BUTTON_B
        0x132 => 98,  // BTN_C      -> BUTTON_C
        0x133 => 99,  // BTN_NORTH  -> BUTTON_X
        0x134 => 100, // BTN_WEST   -> BUTTON_Y
        0x135 => 101, // BTN_Z      -> BUTTON_Z
        0x136 => 102, // BTN_TL     -> BUTTON_L1
        0x137 => 103, // BTN_TR     -> BUTTON_R1
        0x138 => 104, // BTN_TL2    -> BUTTON_L2
        0x139 => 105, // BTN_TR2    -> BUTTON_R2
        0x13a => 109, // BTN_SELECT -> BUTTON_SELECT
        0x13b => 108, // BTN_START  -> BUTTON_START
        0x13c => 110, // BTN_MODE   -> BUTTON_MODE
        0x13d => 106, // BTN_THUMBL -> BUTTON_THUMBL
        0x13e => 107, // BTN_THUMBR -> BUTTON_THUMBR
        _ => return None,
    })
}

/// Android `MotionEvent.AXIS_*` for a Linux `ABS_*` code, with the right stick
/// on `AXIS_Z`/`AXIS_RZ` as Android does it. `None` for an axis with no entry.
fn abs_to_android(code: usize) -> Option<i32> {
    Some(match code {
        0x00 => 0,  // ABS_X      -> AXIS_X
        0x01 => 1,  // ABS_Y      -> AXIS_Y
        0x02 => 17, // ABS_Z      -> AXIS_LTRIGGER
        0x03 => 11, // ABS_RX     -> AXIS_Z
        0x04 => 14, // ABS_RY     -> AXIS_RZ
        0x05 => 18, // ABS_RZ     -> AXIS_RTRIGGER
        0x10 => 15, // ABS_HAT0X  -> AXIS_HAT_X
        0x11 => 16, // ABS_HAT0Y  -> AXIS_HAT_Y
        _ => return None,
    })
}

/// The layout of `/dev/input/js{id}`, or the standard one if sysfs cannot say.
fn read_layout(id: i32) -> Layout {
    let dir = format!("/sys/class/input/js{id}/device/capabilities");
    let read = |name: &str| std::fs::read_to_string(format!("{dir}/{name}")).ok();
    let (Some(key), Some(abs)) = (read("key"), read("abs")) else {
        return Layout::standard();
    };
    let layout = Layout::from_capabilities(&key, &abs);
    if input::trace_gamepad() {
        eprintln!(
            "[cordial] gamepad: js{id} buttons {:?} axes {:?} (Android codes by joydev index)",
            layout.buttons, layout.axes
        );
    }
    layout
}

/// Android `KeyEvent.KEYCODE_BUTTON_*` for a joydev button index.
///
/// **INFERRED twice over.** That the engine's `keyCode` argument is an Android
/// keycode at all is read from the platform contract the Java caller would have
/// been working to -- it is handed a `KeyEvent` from an `InputDevice` and has
/// `getKeyCode()` to forward -- and not observed. That joydev index *i* is the
/// button below is the standard layout the mainline HID drivers report
/// (`hid-playstation`, `xpad`, `hid-nintendo` all normalise to `BTN_SOUTH` and
/// friends, which joydev then numbers in ascending code order). A pad whose
/// driver omits `BTN_C`/`BTN_Z` shifts every index after it, and that is the
/// case `SDL_GameControllerDB` exists to handle and this table does not.
///
/// `None` for an index past the end of the table: unknown is reported as unknown
/// rather than folded onto a real button, because a stray keycode is worse than
/// a missing one.
pub fn button_keycode(index: u8) -> Option<i32> {
    // KEYCODE_BUTTON_A, _B, _C, _X, _Y, _Z, _L1, _R1, _L2, _R2,
    // _SELECT, _START, _MODE, _THUMBL, _THUMBR.
    const KEYS: [i32; 15] = [96, 97, 98, 99, 100, 101, 102, 103, 104, 105, 109, 108, 110, 106, 107];
    KEYS.get(index as usize).copied()
}

/// Android `MotionEvent.AXIS_*` for a joydev axis index.
///
/// INFERRED on the same two counts as [`button_keycode`]. The index order is
/// joydev's ascending-`ABS_*` numbering for a standard pad -- left stick, left
/// trigger, right stick, right trigger, hat -- and the Android side follows the
/// convention that a gamepad's right stick is `AXIS_Z`/`AXIS_RZ` rather than
/// `AXIS_RX`/`AXIS_RY`.
pub fn axis_code(index: u8) -> Option<i32> {
    // AXIS_X, AXIS_Y, AXIS_LTRIGGER, AXIS_Z, AXIS_RZ, AXIS_RTRIGGER,
    // AXIS_HAT_X, AXIS_HAT_Y.
    const AXES: [i32; 8] = [0, 1, 17, 11, 14, 18, 15, 16];
    AXES.get(index as usize).copied()
}

/// `InputDevice.SOURCE_GAMEPAD | SOURCE_JOYSTICK`, the `source` half of the
/// `(axis, source)` pair `nativeSetGamepadSupportedMotionWithGamepadType`'s
/// middle two ints are read as. INFERRED, and the least established argument of
/// the six natives -- see the trampoline's own comment.
const SOURCE_GAMEPAD_JOYSTICK: i32 = 0x0000_0401 | 0x0100_0010;

/// `KeyEvent.ACTION_DOWN` / `ACTION_UP`.
const ACTION_DOWN: i32 = 0;
const ACTION_UP: i32 = 1;

/// Decode one 8-byte joydev packet into `(type, index, value)`.
///
/// Split out from the reader so the packet layout is testable without a device,
/// which on the machine this was written on is the only way it can be tested at
/// all -- there is no gamepad attached and no `/dev/input/js*` to open.
fn decode(buf: &[u8; JS_EVENT_LEN]) -> (u8, u8, i16) {
    let value = i16::from_le_bytes([buf[4], buf[5]]);
    (buf[6], buf[7], value)
}

/// Tell the engine what this pad is and what it has, in the order the engine
/// needs it: the device first, then its buttons and axes.
///
/// `n_buttons`/`n_axes` come from the init burst. Connect has to lead, because
/// every declaration names the pad by the id connect established -- and because
/// disconnect carries no type, the engine is evidently keeping the type against
/// that id from this call onwards.
fn announce(id: i32, node: i32, n_buttons: u8, n_axes: u8, layout: &Layout) {
    let name = device_name(node);
    let family = name.as_deref().map(classify);
    let ty = family.map_or_else(gamepad_type, type_for);
    // Printed for every pad, because the sweep that establishes the ordinals
    // needs to know which pad produced which glyphs, and a photograph of a
    // screen does not record what was plugged in.
    eprintln!(
        "[cordial] gamepad {id}: {} -> {:?}, announcing gamepadType={ty} (UNVERIFIED)",
        name.as_deref().unwrap_or("(no name in /sys)"),
        family.unwrap_or(Family::Unrecognised)
    );
    // **Once per process, and it exists because "(UNVERIFIED)" above tells a
    // user nothing they can act on.** Gamepad support ships on from 0.13.0 with
    // the type ordinal still unestablished, so the first person to meet wrong
    // glyphs should meet the override in the same breath rather than finding it
    // in a source comment. Sober #584 and #1810 are the same symptom on the
    // neighbouring runtime; the buttons work there too.
    {
        static SAID: OnceLock<()> = OnceLock::new();
        SAID.get_or_init(|| {
            eprintln!(
                "[cordial] gamepad: the button glyphs Roblox draws may show the wrong \
                 controller brand. Which integer means which brand is not established \
                 -- see docs/analysis or the README. The buttons themselves work. \
                 Override with CORDIAL_GAMEPAD_TYPE=<n>, or set CORDIAL_GAMEPAD=0 to \
                 turn gamepad support off. If you find the value that draws your pad's \
                 own glyphs, please report it: it settles this for everybody."
            );
        });
    }
    input::deliver_gamepad_connect(id, ty);
    for i in 0..n_buttons {
        if let Some(code) = layout.button(i) {
            input::deliver_gamepad_supported_key(id, code, true, ty);
        }
    }
    for i in 0..n_axes {
        if let Some(code) = layout.axis(i) {
            input::deliver_gamepad_supported_motion(id, code, SOURCE_GAMEPAD_JOYSTICK, true, ty);
        }
    }
}

/// Send one axis movement as the `Vector3` the native's three floats are read as.
///
/// The two components this axis is not carries 0.0 rather than a repeat of the
/// value. Both are guesses; a zero is the guess that stays recognisable as one.
fn send_axis(id: i32, index: u8, value: i16, layout: &Layout) {
    let Some(code) = layout.axis(index) else {
        return;
    };
    input::deliver_gamepad_axis(id, code, value as f32 / JS_AXIS_MAX, 0.0, 0.0);
}

/// What kind of pad this is, as far as the host can tell.
///
/// The engine wants a `gamepadType` and nothing here knows its ordinals -- but
/// **the host knows perfectly well what is plugged in**, and that is the half
/// that was missing. A wrong ordinal shows PlayStation owners Xbox glyphs;
/// knowing which pad it actually is turns that from an unanswerable question
/// into one lookup somebody can fill in once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Xbox,
    PlayStation,
    Nintendo,
    /// Something real that matched nothing above. Distinct from "no name to
    /// read": a pad whose name is unrecognised is evidence about the table, and
    /// a pad whose name could not be read is evidence about the machine.
    Unrecognised,
}

/// Classify a joydev device name.
///
/// Matching on substrings of the kernel's name rather than on USB ids, because
/// the name is what `/sys` publishes as text and the ids would need an `ioctl`
/// and therefore a `libc` dependency this module exists without -- the same
/// reasoning as counting the init burst instead of calling `JSIOCGBUTTONS`.
///
/// The strings are the ones Linux's own drivers report: `xpad` names every Xbox
/// pad with "X-Box" or "Xbox", `hid-playstation` and `hid-sony` report "Sony",
/// "DualSense", "DualShock" or "PLAYSTATION", and `hid-nintendo` reports
/// "Nintendo" or "Pro Controller". Third-party pads usually imitate one of
/// those because they imitate its protocol.
pub fn classify(name: &str) -> Family {
    let n = name.to_ascii_lowercase();
    let has = |needle: &str| n.contains(needle);
    if has("dualsense") || has("dualshock") || has("playstation") || has("sony") || has("ps3")
        || has("ps4") || has("ps5")
    {
        return Family::PlayStation;
    }
    if has("nintendo") || has("switch pro") || has("joy-con") || has("joycon") {
        return Family::Nintendo;
    }
    if has("xbox") || has("x-box") || has("xinput") {
        return Family::Xbox;
    }
    Family::Unrecognised
}

/// The pad's name, as the kernel reports it.
///
/// Read out of `/sys` rather than through `JSIOCGNAME`, for the reason
/// [`classify`] gives. `None` means the file was not there or was not readable,
/// which happens on a device node without a matching sysfs entry and must not
/// be confused with a name that matched nothing.
fn device_name(id: i32) -> Option<String> {
    let path = format!("/sys/class/input/js{id}/device/name");
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_owned())
}

/// The `gamepadType` to announce for a pad of this family.
///
/// **Every entry is unknown and the function says so by returning the same
/// fallback for all of them.** This is deliberately a table with nothing in it
/// rather than a plausible guess: the ordinals of `RBX::GamepadType` are not
/// readable from anything available here, and a table of invented numbers would
/// look exactly like a table of established ones six months from now.
///
/// Filling it in is one session's work and needs no code change. With a pad
/// plugged in, sweep `CORDIAL_GAMEPAD_TYPE=N` and photograph the button glyphs
/// the engine draws; the N that draws this family's glyphs is this family's
/// ordinal. The log line in [`announce`] names the family it detected, so the
/// sweep's result can be attributed to a pad rather than to a guess.
fn type_for(family: Family) -> i32 {
    match family {
        // Left as one arm on purpose. When the first ordinal is established,
        // give it its own arm and leave the rest here -- a partially filled
        // table is honest and a fully invented one is not.
        Family::Xbox | Family::PlayStation | Family::Nintendo | Family::Unrecognised => {
            gamepad_type()
        }
    }
}

/// The first `BTN_JOYSTICK`, and the last `BTN_THUMBR`.
///
/// `linux/input-event-codes.h`. A mouse's buttons live below this at
/// `BTN_MOUSE` 0x110, which is the whole point of the range.
const BTN_JOYSTICK: usize = 0x120;
const BTN_THUMBR: usize = 0x13f;

/// Whether a `capabilities/key` bitmask claims any joystick or gamepad button.
///
/// sysfs prints the mask as space-separated hex words, **most significant
/// first**, each holding 64 bits, so the last word is bits 0..63. Split out
/// from the file read so the parse can be tested against real masks without a
/// device to read them from.
fn declares_a_gamepad_button(mask: &str) -> bool {
    let words: Vec<&str> = mask.split_whitespace().collect();
    (BTN_JOYSTICK..=BTN_THUMBR).any(|bit| {
        words
            .len()
            .checked_sub(1 + bit / 64)
            .and_then(|i| u64::from_str_radix(words[i], 16).ok())
            .is_some_and(|word| word >> (bit % 64) & 1 == 1)
    })
}

/// Whether `/dev/input/jsN` is a controller, or something else joydev bound to.
///
/// **`/dev/input/js0` existing does not mean a controller is plugged in, and
/// assuming it did shipped a phantom pad to everybody.** joydev binds to any
/// device advertising `ABS_X`/`ABS_Y`, which a great many things that are not
/// controllers do. Measured on the machine this was written on, with nothing
/// plugged in: `/dev/input/js0` is `keyd virtual pointer` -- a keyboard
/// remapper's virtual mouse -- and udev agrees it is not a joystick, tagging it
/// `ID_INPUT_MOUSE=1` with no `ID_INPUT_JOYSTICK`. Reported as "even when you
/// dont have a controller plugged in, why is cordial actively telling roblox it
/// does have a controller plugged in", and it is exactly that: gamepad support
/// went on by default, the poll opened js0, joydev's init burst announced it,
/// and Roblox was told a pad had arrived.
///
/// The test is the one udev's own `input_id` builtin uses -- a joystick or
/// gamepad button somewhere in `BTN_JOYSTICK..=BTN_THUMBR` -- rather than a
/// list of names to reject. A name list would have to grow for every virtual
/// input device anyone ever installs, and would still let the next one through;
/// this asks the kernel what the device says it can do. keyd's pointer declares
/// `BTN_LEFT..BTN_TASK` and no gamepad button, so it fails, and an unrecognised
/// third-party pad that really is one still passes -- which matters, because
/// `classify` deliberately returns `Unrecognised` for pads like the 8BitDo
/// rather than pretending to know them.
///
/// A device whose capabilities cannot be read at all is **accepted**. The file
/// is missing on nothing this has seen, and refusing on an unreadable file
/// would turn "I could not check" into "you have no controller", which is the
/// stub-that-lies shape pointed the other way.
fn is_a_controller(id: i32) -> bool {
    let path = format!("/sys/class/input/js{id}/device/capabilities/key");
    match std::fs::read_to_string(&path) {
        Ok(mask) => {
            let yes = declares_a_gamepad_button(&mask);
            if !yes {
                // Once per device and only under the trace, but named: the
                // person asking "why does Roblox think I have a controller"
                // needs the answer to be findable, and so does the person
                // asking why their unusual pad is ignored.
                if input::trace_gamepad() {
                    let name = device_name(id);
                    eprintln!(
                        "[cordial] gamepad: /dev/input/js{id} ({}) declares no joystick or \
                         gamepad button, so it is not a controller; ignoring it",
                        name.as_deref().unwrap_or("no name in /sys")
                    );
                }
            }
            yes
        }
        Err(_) => true,
    }
}

/// The `N` in `jsN`, or `None` for any other name in `/dev/input`.
///
/// Digits only: `js` on its own, `jsx` and `js-1` are not joydev nodes, and
/// this is the one place a directory entry becomes a number the rest of the file
/// trusts.
fn js_node(name: &str) -> Option<i32> {
    let digits = name.strip_prefix("js")?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// The joydev nodes this process can see right now, in numeric order.
///
/// A directory read rather than `js0..js3` opened blind, so a launcher that hands
/// a game one controller under whatever number the host gave it still finds it.
fn visible_nodes() -> Vec<i32> {
    let Ok(entries) = std::fs::read_dir("/dev/input") else {
        return Vec::new();
    };
    let mut nodes: Vec<i32> =
        entries.flatten().filter_map(|e| js_node(&e.file_name().to_string_lossy())).collect();
    nodes.sort_unstable();
    nodes
}

/// The lowest engine device id no held pad is using.
fn free_slot(pads: &[Pad]) -> i32 {
    (0..).find(|id| !pads.iter().any(|p| p.id == *id)).unwrap_or(0)
}

fn open_pad(id: i32) -> Option<std::fs::File> {
    // Asked before the open, not after: opening a joydev node makes the driver
    // queue an init burst, and the whole cost of getting this wrong is that
    // burst being read as a pad arriving.
    if !is_a_controller(id) {
        return None;
    }
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        // Without this the first read blocks the pump, and the pump is the loop
        // that drives every other input path and the engine's own idle
        // keepalive. A gamepad that stops the mouse working is a worse bug than
        // no gamepad at all.
        .custom_flags(O_NONBLOCK)
        .open(format!("/dev/input/js{id}"))
        .ok()
}

/// Drain everything this pad has queued, announcing it first if this is the
/// first look at it.
///
/// Returns false when the device has gone -- an unplugged pad fails its read
/// with `ENODEV` rather than reporting EOF -- which is the caller's cue to send
/// the disconnect and drop it.
fn drain(pad: &mut Pad) -> bool {
    let mut buf = [0u8; JS_EVENT_LEN];
    let mut init_buttons = 0u8;
    let mut init_axes = 0u8;
    // Held until the init burst has been counted, because the declaration has to
    // reach the engine before any of them do.
    let mut deferred: Vec<(u8, u8, i16)> = Vec::new();
    loop {
        match pad.file.read(&mut buf) {
            Ok(JS_EVENT_LEN) => {
                let (kind, index, value) = decode(&buf);
                let init = kind & JS_EVENT_INIT != 0;
                match (init, kind & !JS_EVENT_INIT) {
                    // `saturating_add` rather than `+ 1`: an index of 255 would
                    // overflow, and a debug build turns that into a panic in the
                    // input pump. No real pad has 256 buttons, but "no real
                    // device does that" is not a reason to let a byte off a
                    // character device decide whether the client stays up.
                    (true, JS_EVENT_BUTTON) => init_buttons = init_buttons.max(index.saturating_add(1)),
                    (true, JS_EVENT_AXIS) => init_axes = init_axes.max(index.saturating_add(1)),
                    (false, k) if pad.announced => dispatch(pad.id, k, index, value, &pad.layout),
                    (false, k) => deferred.push((k, index, value)),
                    _ => {}
                }
            }
            // A short read on an 8-byte record interface should not happen; if
            // it does, treating it as "nothing more to say this tick" is safer
            // than looping on a partial packet.
            Ok(_) => break,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return false,
        }
    }
    if !pad.announced && (init_buttons > 0 || init_axes > 0) {
        announce(pad.id, pad.node, init_buttons, init_axes, &pad.layout);
        pad.announced = true;
        for (k, index, value) in deferred {
            dispatch(pad.id, k, index, value, &pad.layout);
        }
    } else if !pad.announced && !deferred.is_empty() {
        // Events arriving from a device that never sent an init burst, which
        // joydev is documented to queue at open and which every driver here is
        // expected to produce. Dropping them is right -- the engine has not been
        // told this device exists -- but dropping them *quietly* would be the
        // half-working gamepad this module refuses to ship, arriving by the one
        // route the all-or-nothing symbol gate cannot see. Said once, because it
        // would otherwise repeat every tick the pad is touched.
        static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            eprintln!(
                "[cordial] gamepad: /dev/input/js{} sent events without an init burst; \
                 its shape is unknown so nothing is being forwarded",
                pad.node
            );
        }
    }
    true
}

fn dispatch(id: i32, kind: u8, index: u8, value: i16, layout: &Layout) {
    match kind {
        JS_EVENT_BUTTON => {
            if let Some(code) = layout.button(index) {
                input::deliver_gamepad_button(
                    id,
                    code,
                    if value != 0 { ACTION_DOWN } else { ACTION_UP },
                );
            }
        }
        JS_EVENT_AXIS => send_axis(id, index, value, layout),
        _ => {}
    }
}

/// One tick's worth of gamepad, called from [`super::looper::pump`] beside
/// `input::idle_keepalive`.
///
/// Cheap when there is nothing to do, which is the common case and the one that
/// matters: with the feature off this is two relaxed loads and a return, and
/// with it on and no pad attached it is a `Vec::is_empty` plus four
/// `open` attempts every two seconds.
///
/// Deliberately does *not* drive the engine's idle throttle. `idle_keepalive`
/// exists because the engine watches `nativePassMouseMove` landing rather than
/// input in general, and whether a gamepad counts to it is unmeasured -- a pad
/// held at full deflection may or may not hold presents up. Claiming either way
/// would be a timing result taken with no instrument, so this claims neither and
/// leaves the keepalive to the path it was measured on.
pub fn poll() {
    if !enabled() {
        if WITHDRAW.swap(false, Ordering::Relaxed) {
            let mut pads = PADS.lock().unwrap_or_else(|e| e.into_inner());
            let n = withdraw(&mut pads, input::deliver_gamepad_disconnect);
            if n > 0 {
                eprintln!(
                    "[cordial] gamepad: switched off; {n} controller(s) disconnected from the engine"
                );
            }
        }
        return;
    }
    // The all-or-nothing gate. A build that exported some of the six but not the
    // registration natives must send nothing at all, not events for a device it
    // never described.
    if !input::gamepad_natives_ready() {
        return;
    }
    if probe() {
        poll_probe();
        return;
    }

    let mut pads = PADS.lock().unwrap_or_else(|e| e.into_inner());

    static LAST_SCAN: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
    let due = {
        let mut last = LAST_SCAN.lock().unwrap_or_else(|e| e.into_inner());
        let now = std::time::Instant::now();
        match *last {
            Some(t) if now.duration_since(t) < RESCAN => false,
            _ => {
                *last = Some(now);
                true
            }
        }
    };
    if due {
        for node in visible_nodes() {
            if pads.len() >= MAX_PADS as usize {
                break;
            }
            if pads.iter().any(|p| p.node == node) {
                continue;
            }
            if let Some(file) = open_pad(node) {
                let id = free_slot(&pads);
                if input::trace_gamepad() {
                    eprintln!("[cordial] gamepad: opened /dev/input/js{node} as device {id}");
                }
                pads.push(Pad { id, node, file, announced: false, layout: read_layout(node) });
            }
        }
    }

    let mut gone: Vec<i32> = Vec::new();
    for pad in pads.iter_mut() {
        if !drain(pad) {
            gone.push(pad.id);
        }
    }
    for id in gone {
        let node = pads.iter().find(|p| p.id == id).map(|p| p.node);
        pads.retain(|p| p.id != id);
        input::deliver_gamepad_disconnect(id);
        if input::trace_gamepad() {
            eprintln!("[cordial] gamepad: /dev/input/js{} went away", node.unwrap_or(id));
        }
    }
}

/// The glyph sweep: announce one pad that does not exist, once, and stop.
///
/// A standard sixteen-button, eight-axis layout is declared so the engine has
/// something complete to draw, and nothing is ever sent afterwards -- the
/// question this answers is what `gamepadType` N makes the UI look like, and
/// that is settled by the connect and the declaration alone.
fn poll_probe() {
    static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if DONE.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let ty = gamepad_type();
    eprintln!(
        "[cordial] gamepad: CORDIAL_GAMEPAD_PROBE announcing a synthetic pad, \
         gamepadType={ty}. The ordinals are UNVERIFIED -- compare the button \
         glyphs the engine draws against rbxasset textures/ui/Controls/\
         {{DefaultController,PlayStationController,XboxController}} and sweep \
         CORDIAL_GAMEPAD_TYPE to find which N is which."
    );
    announce(0, 0, 15, 8, &Layout::standard());
}

#[cfg(test)]
mod tests {
    use super::{enabled_for, free_slot, js_node, withdraw, Pad};

    fn pad(id: i32, announced: bool) -> Pad {
        // A real file descriptor that never produces an event; `withdraw` only
        // drops it, which is what letting go of a device is.
        Pad {
            id,
            node: id,
            file: std::fs::File::open("/dev/null").unwrap(),
            announced,
            layout: super::Layout::standard(),
        }
    }

    /// The case that motivated reading the directory: the one node a launcher
    /// left visible is not `js0`..`js3`, and has to be found and given a small
    /// engine id regardless.
    #[test]
    fn a_pad_on_a_high_node_is_a_node_and_gets_a_small_device_id() {
        assert_eq!(js_node("js5"), Some(5));
        assert_eq!(js_node("js12"), Some(12));
        assert_eq!(js_node("js0"), Some(0));
        for not in ["js", "jsx", "js-1", "event5", "mouse0", "js1a", " js1", ""] {
            assert_eq!(js_node(not), None, "{not:?}");
        }

        let mut pads = Vec::new();
        assert_eq!(free_slot(&pads), 0);
        pads.push(Pad { node: 5, ..pad(0, false) });
        assert_eq!(free_slot(&pads), 1, "a second pad takes the next slot, not its node number");
        pads.push(Pad { node: 9, ..pad(1, false) });
        pads.remove(0);
        assert_eq!(free_slot(&pads), 0, "a freed slot is reused, so ids stay small");
    }

    /// **Switching the controllers off has to tell the engine**, or it keeps
    /// pads that never move. Only the pads it was told about get a disconnect.
    #[test]
    fn switching_off_disconnects_each_announced_pad_once_and_lets_go_of_all() {
        let mut pads = vec![pad(0, true), pad(1, false), pad(2, true)];
        let mut told = Vec::new();
        let n = withdraw(&mut pads, |id| told.push(id));
        assert_eq!(told, vec![0, 2], "pad 1 was never announced, so the engine has never heard of it");
        assert_eq!(n, 2);
        assert!(pads.is_empty(), "the device files are released, not kept open while off");

        // Control: nothing held, nothing sent. A second tick while still off is
        // a no-op rather than a second disconnect for the same ids.
        let mut again = Vec::new();
        assert_eq!(withdraw(&mut pads, |id| again.push(id)), 0);
        assert!(again.is_empty());
    }

    /// The setting is a real switch now, in both directions, and the launch
    /// environment only seeds it.
    #[test]
    fn the_switch_can_be_turned_off_and_back_on_after_it_was_seeded() {
        use super::{current_enabled, set_enabled, SWITCH, SWITCH_UNSET, TEST_GUARD};
        use std::sync::atomic::Ordering;
        let _g = TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        SWITCH.store(SWITCH_UNSET, Ordering::Relaxed);
        let seeded = current_enabled();
        set_enabled(!seeded);
        assert_eq!(current_enabled(), !seeded, "the seed must not win over a later change");
        set_enabled(seeded);
        assert_eq!(current_enabled(), seeded);
        // Leave it on, the default, for anything that runs after.
        set_enabled(true);
    }

    /// **A mouse that joydev bound to is not a controller.**
    ///
    /// The masks below are real, read from `/sys/class/input/*/capabilities/key`
    /// on the machine this was written on. `keyd virtual pointer` is the one
    /// that became `/dev/input/js0` with nothing plugged in and had Roblox told
    /// a pad had arrived; it declares `BTN_LEFT..BTN_TASK` (bits 0x110..0x117)
    /// and nothing in the gamepad range.
    #[test]
    fn a_virtual_mouse_that_became_a_js_node_is_not_a_controller() {
        use super::declares_a_gamepad_button;
        // keyd virtual pointer: 0xff0000 in the 0x100..0x13f word.
        assert!(!declares_a_gamepad_button("ff0000 0 0 0 0"));
        // An ordinary keyboard: KEY_A and friends, nothing above 0xff.
        assert!(!declares_a_gamepad_button("10000 0 0 0 0 0 0 ffffffffffffffff fffffffffffffffe"));
        // Nothing at all.
        assert!(!declares_a_gamepad_button("0 0 0 0 0"));
        assert!(!declares_a_gamepad_button(""));
    }

    /// **joydev indexes what a pad declares, not what a standard pad has.**
    ///
    /// #113: the right stick did not turn the camera. The masks are what the
    /// kernel publishes for each shape of pad; the expectations follow
    /// `joydev_connect`, which numbers `ABS_*` ascending and `BTN_*` from
    /// `BTN_JOYSTICK` up. Control: the same pads through the old positional
    /// tables, printed alongside, are wrong where the assertions say so.
    #[test]
    fn indices_follow_what_the_pad_declares_not_the_standard_layout() {
        use super::{axis_code, button_keycode, Layout};
        // An xpad-style Xbox pad: X Y Z RX RY RZ and the hat; no BTN_C, no BTN_Z.
        //   abs 0x3003f = bits 0-5 and 16-17.
        //   key: bits 0x130,0x131,0x133,0x134,0x136,0x137,0x13a-0x13e.
        let xbox_abs = "3003f";
        let xbox_key = {
            // Bits 0x100..0x13f live in the fifth 64-bit word from the bottom.
            let mut word = 0u64;
            for b in [0x130, 0x131, 0x133, 0x134, 0x136, 0x137, 0x13a, 0x13b, 0x13c, 0x13d, 0x13e] {
                word |= 1 << (b - 0x100);
            }
            format!("0 0 {word:x} 0 0 0 0")
        };
        let xbox = Layout::from_capabilities(&xbox_key, xbox_abs);
        assert_eq!(
            xbox.axes,
            [0, 1, 17, 11, 14, 18, 15, 16].map(Some),
            "a full pad is the standard axis order"
        );
        assert_eq!(
            xbox.buttons,
            [96, 97, 99, 100, 102, 103, 109, 108, 110, 106, 107].map(Some),
            "without BTN_C the third button is X"
        );
        // The old table called index 2 BUTTON_C and index 4 BUTTON_Y for this pad.
        assert_eq!(button_keycode(2), Some(98));
        assert_ne!(xbox.button(2), button_keycode(2));

        // A pad with digital triggers: no ABS_Z, no ABS_RZ. Right stick is 2 and 3.
        //   abs: X Y RX RY and the hat = bits 0,1,3,4,16,17 = 0x3001b.
        let digital = Layout::from_capabilities(&xbox_key, "3001b");
        assert_eq!(digital.axis(2), Some(11), "the right stick's horizontal axis is AXIS_Z");
        assert_eq!(digital.axis(3), Some(14), "and its vertical axis is AXIS_RZ");
        assert_eq!(digital.axis(4), Some(15));
        // The old table sent that stick sideways into the left trigger.
        assert_eq!(axis_code(2), Some(17));
    }

    /// Unreadable capabilities must not turn a pad into nothing.
    #[test]
    fn an_unreadable_capability_mask_falls_back_to_the_standard_layout() {
        use super::Layout;
        let standard = Layout::standard();
        assert_eq!(standard.buttons.len(), 15);
        assert_eq!(standard.axes.len(), 8);
        assert_eq!(standard.button(0), Some(96));
        assert_eq!(standard.axis(3), Some(11));
        // A mask that parses to nothing is an empty pad, not a crash.
        let none = Layout::from_capabilities("", "");
        assert!(none.buttons.is_empty() && none.axes.is_empty());
    }

    /// A real pad passes, wherever in the range its buttons sit.
    ///
    /// BTN_SOUTH (0x130) is what a modern gamepad declares and BTN_TRIGGER
    /// (0x120) is what an old joystick does. Both must pass, or the filter
    /// that fixed the phantom pad becomes the reason a real one is ignored --
    /// which is the more expensive failure of the two, because it looks like
    /// gamepad support simply not working.
    #[test]
    fn a_real_pad_passes_from_either_end_of_the_range() {
        use super::declares_a_gamepad_button;
        // bit 0x130 == 304 == word 4 from the end, bit 48.
        assert!(declares_a_gamepad_button("1000000000000 0 0 0 0"));
        // bit 0x120 == 288 == word 4 from the end, bit 32.
        assert!(declares_a_gamepad_button("100000000 0 0 0 0"));
        // bit 0x13f == 319, the top of the range.
        assert!(declares_a_gamepad_button("8000000000000000 0 0 0 0"));
        // A pad that also declares mouse buttons is still a pad: BTN_SOUTH at
        // bit 48 alongside BTN_LEFT..BTN_TASK at 16..23, in the same word.
        assert!(declares_a_gamepad_button("1000000ff0000 0 0 0 0"));
    }

    /// A short or malformed mask must not panic or index out of bounds.
    ///
    /// This parses a kernel file with arithmetic on word offsets, and the
    /// failure mode of getting that wrong is a panic inside the input pump --
    /// which takes the whole client down over a device nobody was using.
    #[test]
    fn a_mask_that_makes_no_sense_is_simply_not_a_gamepad() {
        use super::declares_a_gamepad_button;
        for mask in ["0", "zzz", "0 0", "   ", "ffffffffffffffffff"] {
            assert!(!declares_a_gamepad_button(mask), "{mask:?} must not read as a pad");
        }
    }

    /// **Gamepad support is on unless it is switched off, from 0.13.0.**
    /// Pinned because the flip is a shipping decision rather than a detail: it
    /// trades possibly-wrong glyphs, which Sober #584 and #1810 show are
    /// cosmetic, against no controller support at all. Anyone reverting the
    /// default should have to change a test that says why.
    #[test]
    fn gamepad_is_on_unless_explicitly_switched_off() {
        assert!(enabled_for(None), "absent means on");
        assert!(!enabled_for(Some("0")), "0 means off");
    }

    /// Anything that is not exactly "0" leaves it on, including the values
    /// somebody would reach for expecting them to work.
    #[test]
    fn only_zero_switches_it_off() {
        for on in ["1", "", "true", "yes", "off", "false", "no"] {
            assert!(enabled_for(Some(on)), "CORDIAL_GAMEPAD={on:?} should leave it on");
        }
    }

    /// The names Linux's own drivers report, classified.
    ///
    /// The failure this catches: somebody adds a substring that is too greedy
    /// and swallows a family it should not. "Controller" alone would match every
    /// pad ever made; "pro" would match "Xbox Elite Wireless Controller Pro".
    /// Each string below is one a real driver emits.
    #[test]
    fn real_pad_names_land_in_the_right_family() {
        use super::{classify, Family};
        for (name, want) in [
            ("Microsoft X-Box 360 pad", Family::Xbox),
            ("Xbox Wireless Controller", Family::Xbox),
            ("Microsoft Xbox Series S|X Controller", Family::Xbox),
            ("Sony Interactive Entertainment Wireless Controller", Family::PlayStation),
            ("Sony Computer Entertainment Wireless Controller", Family::PlayStation),
            ("DualSense Wireless Controller", Family::PlayStation),
            ("PS5 Controller", Family::PlayStation),
            ("Nintendo Switch Pro Controller", Family::Nintendo),
            ("Nintendo Switch Left Joy-Con", Family::Nintendo),
            ("8BitDo SN30 Pro", Family::Unrecognised),
            ("Generic USB Joystick", Family::Unrecognised),
        ] {
            assert_eq!(classify(name), want, "{name}");
        }
    }

    /// Case must not decide a family.
    ///
    /// Drivers are inconsistent about it -- "X-Box" and "Xbox" both occur, and a
    /// third-party pad imitating one may capitalise differently. A classifier
    /// that works on the exact strings above and fails on a shouted one would
    /// pass the test above and still be wrong in the field.
    #[test]
    fn case_does_not_decide_a_family() {
        use super::{classify, Family};
        assert_eq!(classify("SONY WIRELESS CONTROLLER"), Family::PlayStation);
        assert_eq!(classify("xbox wireless controller"), Family::Xbox);
        assert_eq!(classify("NINTENDO SWITCH PRO CONTROLLER"), Family::Nintendo);
    }

    /// An unreadable name and an unrecognised one are different facts.
    ///
    /// `Unrecognised` says the host named a pad this table does not cover, which
    /// is evidence the table needs a row. A missing `/sys` entry says nothing
    /// about the table at all. Collapsing them would make the first invisible.
    #[test]
    fn an_empty_name_is_unrecognised_rather_than_a_family() {
        use super::{classify, Family};
        assert_eq!(classify(""), Family::Unrecognised);
        assert_eq!(classify("   "), Family::Unrecognised);
    }

    use super::*;

    /// The packet layout, which is the one thing here that can be checked
    /// without a device. Little-endian u32 time, i16 value, u8 type, u8 index.
    #[test]
    fn decodes_a_js_event() {
        // time = 1, value = -32767, type = JS_EVENT_AXIS | JS_EVENT_INIT, index = 3.
        let buf = [0x01, 0x00, 0x00, 0x00, 0x01, 0x80, 0x82, 0x03];
        let (kind, index, value) = decode(&buf);
        assert_eq!(kind, JS_EVENT_AXIS | JS_EVENT_INIT);
        assert_eq!(index, 3);
        assert_eq!(value, -32767);
        assert!(kind & JS_EVENT_INIT != 0);
        assert_eq!(kind & !JS_EVENT_INIT, JS_EVENT_AXIS);
    }

    #[test]
    fn a_button_press_decodes_as_a_button() {
        let buf = [0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00];
        let (kind, index, value) = decode(&buf);
        assert_eq!(kind, JS_EVENT_BUTTON);
        assert_eq!(index, 0);
        assert_eq!(value, 1);
    }

    /// An index past the table is reported as unknown rather than folded onto a
    /// real button. A pad with more buttons than the standard layout would
    /// otherwise send presses that the engine attributes to the wrong control.
    #[test]
    fn unknown_indices_stay_unknown() {
        assert_eq!(button_keycode(0), Some(96));
        assert_eq!(button_keycode(14), Some(107));
        assert_eq!(button_keycode(15), None);
        assert_eq!(axis_code(0), Some(0));
        assert_eq!(axis_code(7), Some(16));
        assert_eq!(axis_code(8), None);
    }

    /// No two indices may share a keycode or an axis code. A duplicate would
    /// make two physical controls indistinguishable to the engine, which is the
    /// kind of mistake a hand-written table acquires silently.
    #[test]
    fn the_tables_have_no_duplicates() {
        let keys: Vec<i32> = (0..15).filter_map(button_keycode).collect();
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), keys.len(), "duplicate keycode in button_keycode");

        let axes: Vec<i32> = (0..8).filter_map(axis_code).collect();
        let mut sorted = axes.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), axes.len(), "duplicate axis in axis_code");
    }

    /// Full deflection is +/-1.0 and centre is 0.0. joydev's range is
    /// `-32767..=32767`, so the scale is symmetric and needs no offset.
    #[test]
    fn axis_scale_is_symmetric() {
        assert_eq!(32767i16 as f32 / JS_AXIS_MAX, 1.0);
        assert_eq!(-32767i16 as f32 / JS_AXIS_MAX, -1.0);
        assert_eq!(0i16 as f32 / JS_AXIS_MAX, 0.0);
    }
}
