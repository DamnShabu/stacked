//! `stacked config`: read and change `shell.json` from the command line.
//!
//! Generic over the file rather than one subcommand per setting, with the
//! config type itself as the validator: a change is applied to the JSON,
//! deserialised back into [`ShellConfig`], and refused if that fails or if the
//! value that comes back out is not the value that went in. The second check is
//! the one that matters. `#[serde(default)]` makes an unknown key vanish
//! without an error, so without it `stacked config set fps_Cap 144` would
//! report success and do nothing -- the control that reports success and does
//! not act, which this codebase keeps finding in its own settings pages.

use crate::shell_config::{self, ShellConfig};
use serde_json::{Map, Value};

/// Every key, with the values it takes and what it does. The listing
/// `stacked config` prints, and the set of keys `set` accepts.
const KEYS: &[(&str, &str, &str)] = &[
    ("profile", "NAME", "Which profile `stacked` plays on. Also `stacked profiles use`."),
    ("theme", "stacked | system", "The game window's colours. `system` follows the desktop."),
    ("title_bar", "default | compact | hidden", "The game window's title bar. Hidden in fullscreen either way."),
    ("fullscreen_confine", "true | false", "Keep the cursor on the window in fullscreen."),
    ("fps_cap", "1-1000", "A frame-rate target for the engine. Unset leaves the engine's own."),
    ("present_mode", "mailbox | fifo | immediate | automatic", "How frames reach the screen. mailbox: low latency, no tearing. fifo: vsync, least power."),
    ("graphics", "automatic | vulkan | gles", "Which renderer the engine is offered."),
    ("graphics_optimization_mode", "balanced | roblox-app | mobile-tier | more-cores | fewer-cores", "The device the engine is told it is on, and its worker threads. Unmeasured beyond balanced."),
    ("gamemode", "true | false", "Ask Feral GameMode for the performance governor while playing."),
    ("throttle", "visible | unfocused | off", "When to let the engine slow down: once hidden, once unfocused, or never held awake."),
    ("pointer_acceleration", "unlockedcursor | always", "Whether the desktop's pointer acceleration also moves the camera."),
    ("gamepad", "true | false", "Controllers."),
    ("audio_output", "SINK", "An output from `stacked audio-outputs`. Empty follows the system."),
    ("close_on_leave", "true | false", "Close Roblox when you leave an experience."),
    ("mangohud", "true | false", "MangoHUD's frame-rate overlay, if MangoHUD is installed."),
    ("vkbasalt", "true | false", "vkBasalt sharpening, if vkBasalt is installed. See docs/shaders.md."),
    ("carry_launch_ticket", "true | false", "Hand a website Play button's login ticket to the engine."),
    ("unpacked_plugins", "[\"/path\", ...]", "Plugin folders loaded in place, for plugin development."),
    ("roblox.apk", "PATH", "Run this Roblox APK instead of finding one."),
    ("roblox.lib_dir", "PATH", "Use this extracted engine directory with it."),
];

pub fn run(args: &[String]) -> u8 {
    let path = shell_config::path();
    match args.first().map(String::as_str) {
        None | Some("list" | "show") => {
            let current = current_json();
            for (key, values, what) in KEYS {
                let value = lookup(&current, key).map(render).unwrap_or_else(|| "(unset)".into());
                println!("{key} = {value}\n    {values}\n    {what}");
            }
            println!("\n{}", path.display());
            0
        }
        Some("path") => {
            println!("{}", path.display());
            0
        }
        Some("get") => {
            let Some(key) = args.get(1) else {
                eprintln!("stacked: config get needs a KEY");
                return 2;
            };
            if !known(key) {
                return unknown(key);
            }
            println!("{}", lookup(&current_json(), key).map(render).unwrap_or_else(|| "(unset)".into()));
            0
        }
        Some("set") => {
            let (Some(key), true) = (args.get(1), args.len() > 2) else {
                eprintln!("stacked: config set needs a KEY and a VALUE");
                return 2;
            };
            // Anything after the key is joined back on, so an audio sink with
            // spaces in its name does not need quoting.
            apply(key, Some(parse_value(&args[2..].join(" "))))
        }
        Some("unset" | "reset") => {
            let Some(key) = args.get(1) else {
                eprintln!("stacked: config unset needs a KEY");
                return 2;
            };
            apply(key, None)
        }
        Some(other) => {
            eprintln!("stacked: unknown config command {other:?}. Try `stacked help`.");
            2
        }
    }
}

fn apply(key: &str, value: Option<Value>) -> u8 {
    if !known(key) {
        return unknown(key);
    }
    let path = shell_config::path();
    match change(&current_json(), key, value.clone()) {
        Ok(config) => match shell_config::save(&path, &config) {
            Ok(()) => {
                let shown = lookup(&serde_json::to_value(&config).unwrap_or_default(), key)
                    .map(render)
                    .unwrap_or_else(|| "(unset)".into());
                println!("{key} = {shown}");
                0
            }
            Err(e) => {
                eprintln!("stacked: could not save {}: {e}", path.display());
                1
            }
        },
        Err(why) => {
            eprintln!("stacked: {why}");
            2
        }
    }
}

/// The config as JSON, with defaults filled in, so a key the file does not
/// mention still shows the value it has.
fn current_json() -> Value {
    let config = shell_config::load(&shell_config::path());
    serde_json::to_value(config).unwrap_or(Value::Object(Map::new()))
}

fn known(key: &str) -> bool {
    KEYS.iter().any(|(k, _, _)| *k == key)
}

fn unknown(key: &str) -> u8 {
    eprintln!("stacked: there is no setting {key:?}. `stacked config` lists them.");
    2
}

/// A value as typed. JSON where it parses -- numbers, `true`, a quoted string,
/// an array -- and a plain string otherwise, so `theme system` needs no quotes.
fn parse_value(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_string()))
}

fn render(v: &Value) -> String {
    match v {
        Value::String(s) if s.is_empty() => "\"\"".into(),
        Value::String(s) => s.clone(),
        Value::Null => "(unset)".into(),
        other => other.to_string(),
    }
}

fn lookup<'a>(root: &'a Value, key: &str) -> Option<&'a Value> {
    key.split('.').try_fold(root, |v, part| v.get(part)).filter(|v| !v.is_null())
}

/// Apply one change and prove it took.
///
/// Pure over its inputs so the refusal cases are testable without a file.
fn change(current: &Value, key: &str, value: Option<Value>) -> Result<ShellConfig, String> {
    let mut edited = current.clone();
    let mut parts: Vec<&str> = key.split('.').collect();
    let last = parts.pop().ok_or("an empty key")?;
    let mut node = &mut edited;
    for part in parts {
        node = node
            .as_object_mut()
            .ok_or_else(|| format!("{key} is not a setting"))?
            .entry(part)
            .or_insert_with(|| Value::Object(Map::new()));
    }
    let object = node.as_object_mut().ok_or_else(|| format!("{key} is not a setting"))?;
    match &value {
        Some(v) => {
            object.insert(last.to_string(), v.clone());
        }
        None => {
            object.remove(last);
        }
    }

    let config: ShellConfig = serde_json::from_value(edited)
        .map_err(|e| format!("{key} cannot be {}: {e}", value.as_ref().map(render).unwrap_or_default()))?;

    // `unset` means "the default", which by definition round-trips; only a
    // value the user supplied needs proving.
    if let Some(v) = value {
        let back = serde_json::to_value(&config).map_err(|e| e.to_string())?;
        if lookup(&back, key) != Some(&v) {
            return Err(format!("{key} did not take the value {}", render(&v)));
        }
    }
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> Value {
        serde_json::to_value(ShellConfig::default()).unwrap()
    }

    #[test]
    fn every_listed_key_is_a_real_setting() {
        // A key in the listing that `set` then refuses, or that `set` accepts
        // and serde throws away, is the lying control this module exists to
        // prevent. Each is set to a value of its own type and read back.
        for (key, _, _) in KEYS {
            let value = match *key {
                "fps_cap" => Value::from(144),
                "theme" => Value::from("system"),
                "title_bar" => Value::from("compact"),
                "present_mode" => Value::from("fifo"),
                "graphics" => Value::from("vulkan"),
                "graphics_optimization_mode" => Value::from("more-cores"),
                "throttle" => Value::from("off"),
                "pointer_acceleration" => Value::from("always"),
                "unpacked_plugins" => serde_json::json!(["/tmp/p"]),
                "profile" | "audio_output" | "roblox.apk" | "roblox.lib_dir" => Value::from("x"),
                _ => Value::Bool(true),
            };
            change(&defaults(), key, Some(value)).unwrap_or_else(|e| panic!("{key}: {e}"));
        }
    }

    #[test]
    fn a_value_of_the_wrong_kind_is_refused_rather_than_ignored() {
        assert!(change(&defaults(), "present_mode", Some(Value::from("vsync"))).is_err());
        assert!(change(&defaults(), "fps_cap", Some(Value::from("fast"))).is_err());
        assert!(change(&defaults(), "gamemode", Some(Value::from("maybe"))).is_err());
    }

    #[test]
    fn a_theme_nobody_recognises_is_refused_by_the_round_trip_not_saved_as_something_else() {
        // `Theme` deserialises strictly from the file even though the runtime
        // parses the environment leniently, so a typo here is an error the
        // user sees rather than a silently different theme.
        assert!(change(&defaults(), "theme", Some(Value::from("neon"))).is_err());
    }

    #[test]
    fn unset_puts_a_setting_back_to_its_default() {
        let set = serde_json::to_value(change(&defaults(), "fps_cap", Some(Value::from(90))).unwrap()).unwrap();
        assert_eq!(lookup(&set, "fps_cap"), Some(&Value::from(90)));
        let back = change(&set, "fps_cap", None).unwrap();
        assert_eq!(back.fps_cap, None);
    }

    #[test]
    fn plain_words_need_no_quotes_and_numbers_stay_numbers() {
        assert_eq!(parse_value("system"), Value::from("system"));
        assert_eq!(parse_value("144"), Value::from(144));
        assert_eq!(parse_value("true"), Value::Bool(true));
        assert_eq!(parse_value("Built-in Audio Analog Stereo"), Value::from("Built-in Audio Analog Stereo"));
    }

    #[test]
    fn nested_keys_reach_the_roblox_paths() {
        let config = change(&defaults(), "roblox.apk", Some(Value::from("/x/base.apk"))).unwrap();
        assert_eq!(config.roblox.apk.as_deref(), Some(std::path::Path::new("/x/base.apk")));
    }
}
