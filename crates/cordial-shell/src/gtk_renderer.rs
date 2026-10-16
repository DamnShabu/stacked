//! Which renderer GTK draws the host window with, decided before it asks.
//!
//! **Why this exists.** The engine's window is a GTK toplevel with the canvas
//! as a subsurface, and a focused TextBox is drawn by GTK over the game
//! (ADR-047). With no working Vulkan the engine falls back to GLES3 in the same
//! process as GTK, GTK falls back to its GL renderer, and from then on GTK
//! never attaches a buffer to the window again: no frame, no editor, whatever
//! the stacking does. `GSK_RENDERER=cairo` takes GTK off that path and was
//! measured to present normally on the same reproduction (issue #53, nested
//! KWin, `VK_ICD_FILENAMES=/nonexistent`).
//!
//! The window holds a header bar and a transparent hole where the canvas
//! shows, so software drawing costs nothing worth the name, and this chooses
//! it for exactly the machines that would otherwise get nothing.
//!
//! **Why the Vulkan probe and not the Graphics setting.** What breaks GTK is
//! that *GTK* has no Vulkan to render with, not that the engine took GLES:
//! forcing the engine to GLES on a machine whose Vulkan works leaves GTK on
//! Vulkan and presenting (ADR-047), so `CORDIAL_GRAPHICS=gles` alone is not a
//! reason. The question is whether a hardware Vulkan device exists, which is
//! what [`crate::vulkan_probe`] already answers in a child process.
//!
//! **When it runs.** The probe starts in `cordial-run`'s `main`, concurrent
//! with everything the client does before the engine asks for its window, and
//! [`settle`] collects it there. On a healthy machine that is no added
//! startup latency: the child finishes long before the window is wanted. A
//! user's own `GSK_RENDERER` skips the probe entirely.

use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::Duration;

use crate::vulkan_probe::{self, Device, Failure, Kind};

/// The variable GTK reads. Named once so every reader and the tests agree.
pub const ENV: &str = "GSK_RENDERER";

/// How long the probe may take. Upstream's doctor allows eight seconds because a
/// person is waiting on a report; here the answer is wanted before the engine
/// opens its window, which is a few seconds into a launch, and a driver that
/// has not answered by then is not going to be treated as a reason to change
/// anything (see [`Decision::KeepUnknown`]).
pub const PROBE_LIMIT: Duration = Duration::from_secs(8);

/// What to do about GSK_RENDERER, and the words for why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// The user set `GSK_RENDERER`. Never overridden, never probed.
    KeepUsers(String),
    /// A hardware Vulkan device exists, so GTK has a renderer that works.
    KeepHealthy,
    /// The probe did not answer (hung or crashed). Nothing is known about
    /// GTK, and changing a renderer on no evidence is the wrong direction to
    /// be wrong in.
    KeepUnknown(String),
    /// Set `GSK_RENDERER=cairo`; carries what the probe found.
    Cairo(String),
}

impl Decision {
    pub fn is_cairo(&self) -> bool {
        matches!(self, Decision::Cairo(_))
    }
}

/// The decision, with both inputs passed in so it can be tested.
///
/// `user` is the value of `GSK_RENDERER` in the environment, if any. An empty
/// value is not an opinion: GTK reads it as unset.
pub fn decide(user: Option<&str>, probed: &Result<Vec<Device>, Failure>) -> Decision {
    if let Some(v) = user.map(str::trim).filter(|v| !v.is_empty()) {
        return Decision::KeepUsers(v.to_string());
    }
    match probed {
        Ok(devices) if devices.iter().any(|d| d.kind != Kind::Cpu) => Decision::KeepHealthy,
        Ok(devices) => {
            let names = devices.iter().map(|d| d.name.as_str()).collect::<Vec<_>>().join(", ");
            Decision::Cairo(format!("the only Vulkan device is a CPU renderer ({names})"))
        }
        Err(Failure::NoLoader) => Decision::Cairo("there is no Vulkan loader".into()),
        Err(Failure::Instance(code)) => {
            Decision::Cairo(format!("Vulkan could not start (vkCreateInstance returned {code})"))
        }
        Err(Failure::NoDevices) => Decision::Cairo("Vulkan lists no GPU".into()),
        Err(Failure::TimedOut) => Decision::KeepUnknown("the Vulkan probe did not finish".into()),
        Err(Failure::Crashed(why)) => {
            Decision::KeepUnknown(format!("the Vulkan probe failed ({why})"))
        }
    }
}

/// The probe, started and not yet collected. `None` when [`begin`] was not
/// called, which is what a test binary or a client that never opens a GTK
/// window looks like, and [`settle`] then does nothing.
static PENDING: Mutex<Option<JoinHandle<Result<Vec<Device>, Failure>>>> = Mutex::new(None);

/// Start the probe in the background, unless the user already chose.
///
/// Called early in `cordial-run`, before the engine is loaded. A thread rather
/// than a wait: the child process is the isolation, and this only keeps the
/// launch from standing still for it.
pub fn begin() {
    if user_choice().is_some() {
        return;
    }
    let mut slot = PENDING.lock().unwrap_or_else(|p| p.into_inner());
    if slot.is_some() {
        return;
    }
    *slot = Some(std::thread::spawn(|| vulkan_probe::probe(PROBE_LIMIT)));
}

fn user_choice() -> Option<String> {
    std::env::var(ENV).ok().filter(|v| !v.trim().is_empty())
}

/// Collect the probe and set `GSK_RENDERER` if it says to. Must run before
/// GTK creates the window's renderer, which happens when the toplevel is
/// realised, so `host_window::init_wayland` is the latest it can be called.
///
/// Prints one line saying what it decided and why, whichever way. `None` when
/// no probe was started, which includes the user having chosen already.
pub fn settle() -> Option<Decision> {
    let handle = PENDING.lock().unwrap_or_else(|p| p.into_inner()).take()?;
    let asked = std::time::Instant::now();
    let probed = handle
        .join()
        .unwrap_or_else(|_| Err(Failure::Crashed("the probe thread panicked".into())));
    // How long the launch stood still for the answer, which is nothing when
    // the probe finished while the engine was loading. In the line below so a
    // slow driver is a number and not an unexplained pause.
    let waited = asked.elapsed().as_millis();
    let user = user_choice();
    let decision = decide(user.as_deref(), &probed);
    match &decision {
        Decision::Cairo(why) => {
            // SAFETY: `g_setenv` is not thread-safe against a concurrent
            // `getenv`, and the engine's threads are already running here --
            // the same compromise `init_wayland` makes for `GDK_BACKEND`, and
            // for the same reason: nothing in `libroblox.so` reads this
            // variable, GTK reads it once at realisation, and this is the
            // last moment before that.
            match unsafe { gtk4::glib::setenv(ENV, "cairo", true) } {
                Ok(()) => println!(
                    "[gtk] renderer: cairo, because {why} (probe wait {waited} ms); GTK's GL \
                     renderer stops presenting beside the engine's GLES, and a focused text box \
                     would have no editor. Set {ENV} yourself to choose another."
                ),
                Err(e) => eprintln!("[gtk] could not set {ENV}=cairo: {e}"),
            }
        }
        Decision::KeepUnknown(why) => println!(
            "[gtk] renderer: left to GTK, because {why} (probe wait {waited} ms); if a focused \
             text box shows no editor, try {ENV}=cairo"
        ),
        Decision::KeepHealthy => println!(
            "[gtk] renderer: left to GTK, a hardware Vulkan device exists (probe wait {waited} ms)"
        ),
        // The user's own choice is never probed, so `settle` is not reached
        // with this today; kept total so a change to `begin` cannot make it a
        // silent override.
        Decision::KeepUsers(v) => println!("[gtk] renderer: {ENV}={v}, set by the user"),
    }
    Some(decision)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(kind: Kind, name: &str) -> Device {
        Device {
            vendor_id: 0x1002,
            device_id: 1,
            kind,
            name: name.into(),
            driver_name: None,
            driver_info: None,
            driver_version: 0,
            api_version: 0,
        }
    }

    #[test]
    fn a_hardware_device_leaves_gtk_alone() {
        for kind in [Kind::Integrated, Kind::Discrete, Kind::Virtual, Kind::Other] {
            let probed = Ok(vec![device(kind, "gpu")]);
            assert_eq!(decide(None, &probed), Decision::KeepHealthy, "{kind:?}");
        }
    }

    #[test]
    fn a_cpu_renderer_beside_a_real_gpu_is_still_healthy() {
        let probed = Ok(vec![device(Kind::Cpu, "llvmpipe"), device(Kind::Discrete, "RTX")]);
        assert_eq!(decide(None, &probed), Decision::KeepHealthy);
    }

    #[test]
    fn only_a_cpu_renderer_is_cairo() {
        let probed = Ok(vec![device(Kind::Cpu, "llvmpipe (LLVM 19.1.7, 256 bits)")]);
        match decide(None, &probed) {
            Decision::Cairo(why) => assert!(why.contains("llvmpipe"), "{why}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn no_loader_no_driver_and_no_devices_are_each_cairo() {
        for failure in [Failure::NoLoader, Failure::Instance(-9), Failure::NoDevices] {
            let d = decide(None, &Err(failure.clone()));
            assert!(d.is_cairo(), "{failure:?} -> {d:?}");
        }
    }

    #[test]
    fn a_probe_that_did_not_answer_changes_nothing() {
        for failure in [Failure::TimedOut, Failure::Crashed("signal 11".into())] {
            let d = decide(None, &Err(failure.clone()));
            assert!(matches!(d, Decision::KeepUnknown(_)), "{failure:?} -> {d:?}");
        }
    }

    #[test]
    fn the_users_own_renderer_beats_every_probe_outcome() {
        let outcomes: Vec<Result<Vec<Device>, Failure>> = vec![
            Ok(vec![device(Kind::Discrete, "gpu")]),
            Ok(vec![device(Kind::Cpu, "llvmpipe")]),
            Err(Failure::NoLoader),
            Err(Failure::Instance(-9)),
            Err(Failure::TimedOut),
        ];
        for probed in &outcomes {
            assert_eq!(decide(Some("ngl"), probed), Decision::KeepUsers("ngl".into()), "{probed:?}");
        }
    }

    #[test]
    fn an_empty_variable_is_not_an_opinion() {
        let probed = Err(Failure::NoLoader);
        assert!(decide(Some(""), &probed).is_cairo());
        assert!(decide(Some("  "), &probed).is_cairo());
    }
}
