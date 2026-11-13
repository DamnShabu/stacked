//! Show the startup splash and walk its bar through the launch's steps, without
//! starting the engine. For looking at it, and for photographing it inside a
//! nested compositor.
//!
//! ```
//! cargo run -p cordial-shell --example splash_preview
//! CORDIAL_THEME=system cargo run -p cordial-shell --example splash_preview
//! ```

use cordial_shell::splash;

fn main() {
    splash::show();
    for (fraction, what) in [
        (0.1, "Reading what the engine needs"),
        (0.2, "Resolving the engine's symbols"),
        (0.35, "Loading the Roblox engine"),
        (0.55, "Starting the Java runtime"),
        (0.7, "Fetching client settings"),
        (0.85, "Starting the engine"),
        (0.95, "Opening the game window"),
    ] {
        splash::step(fraction, what);
        let ms = std::env::var("CORDIAL_SPLASH_STEP_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(400);
        let until = std::time::Instant::now() + std::time::Duration::from_millis(ms);
        while std::time::Instant::now() < until {
            splash::step(fraction, what);
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
    }
    splash::finish();
}
