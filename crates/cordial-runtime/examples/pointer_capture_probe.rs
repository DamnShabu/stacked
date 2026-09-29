//! Opens the Wayland window and drives the pointer-capture path with no
//! Roblox APK, no engine and no `libjnivm` — the same shape as
//! `accessibility_probe.rs`, and for the same reason.
//!
//! Pointer capture is four new hand-written protocol messages
//! (`zwp_pointer_constraints_v1.lock_pointer`,
//! `zwp_locked_pointer_v1.set_cursor_position_hint`, its `destroy`, and
//! `zwp_relative_pointer_manager_v1.get_relative_pointer`), and a
//! hand-written signature that is wrong by one argument corrupts the wire
//! rather than failing cleanly — `wayland.rs`'s module doc records that
//! family of crash twice. The riskiest part of the change is therefore the
//! part a full client run is *worst* at exercising, because reaching it
//! needs a person holding a mouse button.
//!
//! It also exists because the honest place to run a lock test is a nested
//! headless compositor on its own `WAYLAND_DISPLAY`, and the Roblox engine
//! does not survive in one — measured, on an unmodified tree as a control:
//! `mutter --headless --virtual-monitor 1280x800` dies within a second of
//! the engine starting to present, and takes the client with it
//! (`Gdk-Message: Error 32 (Broken pipe)`). An ordinary GTK client in the
//! same nested compositor runs indefinitely. So the choice was between
//! testing the lock against the developer's own session — where a bug in it
//! captures their real cursor, which is precisely the accident this is
//! supposed to prevent — and testing it without the engine. This is the
//! second option.
//!
//! ```text
//! mutter --headless --wayland --wayland-display=probe --virtual-monitor 1280x800 &
//! WAYLAND_DISPLAY=probe CORDIAL_FORCE_POINTER_LOCK=1 CORDIAL_TRACE_MOUSE=1 \
//!     cargo run --release --example pointer_capture_probe -- 8
//! ```
//!
//! `CORDIAL_PROBE_TIMELINE` drives the fullscreen confinement and its swap
//! with the lock, which is the other way two constraints can end up on one
//! surface. It is a comma-separated list of `<seconds>:<action>`, where the
//! action is `fs=1`/`fs=0` (`instr_set_fullscreen`) or `lock=1`/`lock=0`/
//! `lock=clear` (the `fakeenginelock` seam, so the engine "asks" for the lock
//! and stops asking with no engine loaded):
//!
//! ```text
//! WAYLAND_DEBUG=1 CORDIAL_TRACE_MOUSE=1 \
//! CORDIAL_PROBE_TIMELINE=2:fs=1,4:lock=1,6:lock=0,8:fs=0,10:fs=1,12:fs=0 \
//!     cargo run --release --example pointer_capture_probe -- 14
//! ```
//!
//! Two things decide whether anything is ever *answered*. A headless seat has
//! no pointer until something holds one open (`tools/wl-pointer-holder.c`
//! under sway), and with none GDK exposes no `wl_pointer` and nothing is sent.
//! And with no engine the canvas subsurface never gets a buffer, so pointer
//! focus only ever lands on the toplevel: a constraint on the canvas is
//! accepted and never activates. `CORDIAL_POINTER_LOCK_SURFACE=toplevel` puts
//! it where the pointer is; with that and a held pointer, sway 1.9 sent both
//! `locked` and `confined` on 2026-09-29.
//!
//! What it cannot show: that a lock, once granted, actually moves the
//! camera, or that a confinement keeps a real cursor off a second monitor.
//! Those need a person with a mouse in an experience and are not something
//! this binary should pretend to have done.

fn main() {
    let seconds: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(8);

    println!(
        "pointer capture probe: no engine is involved here. Nothing this prints is a \n\
         claim about what Roblox does with a locked pointer — only about what Cordial \n\
         and the compositor say to each other."
    );

    let window = match cordial_runtime::android::wayland::open(960, 540, "Stacked pointer probe") {
        Ok(w) => w,
        Err(e) => {
            eprintln!("no Wayland window: {e}");
            std::process::exit(1);
        }
    };
    let (w, h, _) = window.geometry();
    println!("window up at {w}x{h}; pumping for {seconds}s");

    // `CORDIAL_PROBE_FULLSCREEN=1` — the only way to get the pointer over the
    // canvas without moving somebody's mouse for them.
    //
    // A compositor grants a lock when the surface has pointer focus, and a
    // client cannot place its own window under the cursor on Wayland. It can
    // ask to cover the whole output, which puts the cursor over the canvas
    // wherever the cursor already is, and `gtk_window_fullscreen` is a request
    // this client makes about its own window — the same call
    // `looper.rs`'s `CORDIAL_SCRIPT` timeline uses, and nothing that touches
    // anyone else's session.
    if std::env::var_os("CORDIAL_PROBE_FULLSCREEN").is_some() {
        println!("going fullscreen so that the pointer is over the canvas");
        cordial_runtime::android::wayland::instr_set_fullscreen(true);
    }

    // Parsed up front so that a typo fails before the window has done
    // anything, rather than being silently skipped at its second.
    let mut timeline: Vec<(f64, &'static str, u8)> = Vec::new();
    if let Ok(spec) = std::env::var("CORDIAL_PROBE_TIMELINE") {
        for step in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let parsed = step.split_once(':').and_then(|(at, action)| {
                let at: f64 = at.parse().ok()?;
                let (what, val) = match action {
                    "fs=1" => ("fs", 1),
                    "fs=0" => ("fs", 0),
                    "lock=1" => ("lock", 2),
                    "lock=0" => ("lock", 1),
                    "lock=clear" => ("lock", 0),
                    _ => return None,
                };
                Some((at, what, val))
            });
            match parsed {
                Some(p) => timeline.push(p),
                None => {
                    eprintln!("CORDIAL_PROBE_TIMELINE: cannot read step {step:?}");
                    std::process::exit(2);
                }
            }
        }
        timeline.sort_by(|a, b| a.0.total_cmp(&b.0));
    }
    let mut next_step = 0;

    let start = std::time::Instant::now();
    let deadline = start + std::time::Duration::from_secs(seconds);
    while std::time::Instant::now() < deadline {
        while let Some(&(at, what, val)) = timeline.get(next_step) {
            if start.elapsed().as_secs_f64() < at {
                break;
            }
            next_step += 1;
            // Focus is printed because a compositor may activate a constraint
            // only on the focused surface (sway does), and an unanswered
            // request on an unfocused window says nothing about the request.
            println!(
                "[probe] t={at}s focused={:?}",
                cordial_runtime::android::wayland::focused()
            );
            match what {
                "fs" => {
                    println!("[probe] t={at}s fullscreen {}", val == 1);
                    cordial_runtime::android::wayland::instr_set_fullscreen(val == 1);
                }
                _ => {
                    println!(
                        "[probe] t={at}s engine lock request {}",
                        ["cleared", "false", "true"][val as usize]
                    );
                    cordial_runtime::android::input::FAKE_ENGINE_LOCK
                        .store(val, std::sync::atomic::Ordering::Relaxed);
                }
            }
        }
        // `0` is "no GameActivity handle", which is the case this whole binary
        // is: the AGDK touch path is skipped and only the Wayland side runs.
        cordial_runtime::android::wayland::pump_input_events(0);
        std::thread::sleep(std::time::Duration::from_millis(16));
    }
    println!("done");
}
