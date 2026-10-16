//! Re-delivering the surface and platform parameters to a running engine.
//!
//! The two `nativeAppBridgeV2UpdateSurface{App,Game}WithPlatformParams` natives
//! are what the Java side calls whenever a `SurfaceView` it owns changes, and
//! Sober's log shows them again well after startup, not once. Cordial calls them
//! at start (`bin/load.rs`) and never again.
//!
//! This exists to run one experiment on a client already in the state under
//! investigation: after a join through the app shell's own Play button, a new
//! game DataModel is created inside the running engine, and nothing here hands
//! it the parameters a Java `SurfaceView` callback would have. Whether that is
//! why WASD and Space do nothing there is `INFERRED` until a client in that
//! state is seen to recover from this call, so it is reachable only through the
//! development control socket's `updatesurface` verb.
use std::ffi::c_void;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::Mutex;

static UPDATE_APP: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static UPDATE_GAME: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static GEOMETRY: Mutex<Option<(String, i32, i32)>> = Mutex::new(None);

/// Remember the natives and the arguments `load.rs` delivered them with, so a
/// later call builds the same objects.
pub fn register(app: *mut c_void, game: *mut c_void, assets: &str, width: i32, height: i32) {
    UPDATE_APP.store(app, Ordering::Relaxed);
    UPDATE_GAME.store(game, Ordering::Relaxed);
    if let Ok(mut g) = GEOMETRY.lock() {
        *g = Some((assets.to_string(), width, height));
    }
}

/// Deliver again, on the calling thread, which must be the pump's: the Java side
/// makes these calls from the main thread. Returns what was called, for the log.
pub fn redeliver(app: bool, game: bool) -> Vec<String> {
    let Some((assets, w, h)) = GEOMETRY.lock().ok().and_then(|g| g.clone()) else {
        return vec!["no natives registered (the engine never reached the start-up delivery)".into()];
    };
    let (cw, ch) = crate::android::vulkan::last_extent();
    let (w, h) = if cw > 0 && ch > 0 { (cw as i32, ch as i32) } else { (w, h) };
    let mut said = Vec::new();
    for (which, wanted, ptr, is_game) in [
        ("app", app, UPDATE_APP.load(Ordering::Relaxed), false),
        ("game", game, UPDATE_GAME.load(Ordering::Relaxed), true),
    ] {
        if !wanted {
            continue;
        }
        if ptr.is_null() {
            said.push(format!("{which}: not exported by this build"));
            continue;
        }
        // SAFETY: `ptr` was resolved by symbol lookup against the loaded
        // libroblox.so in `load.rs` and the library is never unloaded.
        let r = unsafe { cordial_linker_sys::game_activity::appbridge_update_surface(ptr, &assets, w, h, is_game) };
        said.push(match r {
            Ok(()) => format!("{which}: delivered at {w}x{h}"),
            Err(e) => format!("{which}: failed: {e}"),
        });
    }
    said
}
