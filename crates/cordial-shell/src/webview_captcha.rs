//! The two hybrid-bridge commands a Roblox security-check page sends, and the
//! reply it waits for.
//!
//! The sign-in security check (`/captcha/app/login`) tells the host about
//! itself with `Navigation.navigateToFeature` commands: `CaptchaShown` when the
//! challenge is on screen and `CaptchaSuccess` when the user passed it. Each
//! carries a `callbackID`, and the page does not carry on until the host
//! answers it by calling `Roblox.Hybrid.Bridge.nativeCallback(id, true, {})`.
//! Cordial forwarded these commands to the engine and answered nobody, so the
//! page sat at "Verification complete" with its window open (#83).
//!
//! The command shape, the reply and the rule that a success closes the window
//! are taken from mocktail's `webview_helper_policy.cc` (Copyright 2026
//! komaruworld, Apache-2.0; see `NOTICE` and `third_party/mocktail-webview/`),
//! rewritten here rather than translated. Like [`crate::webview_policy`] this is
//! kept free of WebKit so the parts that decide what is acted on are testable
//! without a browser engine or a display.

/// Longest `moduleID`/`functionName`/`feature` accepted. mocktail's bound.
const MAX_IDENTIFIER_BYTES: usize = 128;

/// Longest `callbackID` accepted. mocktail's bound.
const MAX_CALLBACK_ID_BYTES: usize = 1024;

/// What the page reported about the challenge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptchaEventKind {
    /// The challenge is on screen. Answered, window left open.
    Shown,
    /// The user passed. Answered, then the window is closed.
    Success,
}

/// One recognised command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptchaEvent {
    pub kind: CaptchaEventKind,
    /// Empty when the page sent none, in which case there is nothing to answer.
    pub callback_id: String,
}

impl CaptchaEventKind {
    /// The feature name as the page spells it, for the log.
    pub fn name(self) -> &'static str {
        match self {
            CaptchaEventKind::Shown => "CaptchaShown",
            CaptchaEventKind::Success => "CaptchaSuccess",
        }
    }
}

/// A string field that is present, a string, short, and free of control bytes.
fn bounded<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
    max: usize,
) -> Option<&'a str> {
    let text = object.get(key)?.as_str()?;
    (text.len() <= max && !text.bytes().any(|b| b < 0x20 || b == 0x7f)).then_some(text)
}

/// Recognise a captcha event in a bridge command, or `None` for anything else.
///
/// Every other command (a game launch, a purchase) returns `None` and is left
/// for the engine alone; only these two get a reply from the host.
pub fn parse_captcha_event(command: &str) -> Option<CaptchaEvent> {
    let value: serde_json::Value = serde_json::from_str(command).ok()?;
    let message = value.as_object()?;
    if bounded(message, "moduleID", MAX_IDENTIFIER_BYTES)? != "Navigation"
        || bounded(message, "functionName", MAX_IDENTIFIER_BYTES)? != "navigateToFeature"
    {
        return None;
    }
    let navigation = message.get("params")?.as_object()?.get("params")?.as_object()?;
    let kind = match bounded(navigation, "feature", MAX_IDENTIFIER_BYTES)? {
        "CaptchaShown" => CaptchaEventKind::Shown,
        "CaptchaSuccess" => CaptchaEventKind::Success,
        _ => return None,
    };
    // Optional, as in mocktail: an event with no id is still recognised, so the
    // log can say it could not be answered rather than saying nothing arrived.
    let callback_id = match message.get("callbackID") {
        None => String::new(),
        Some(_) => bounded(message, "callbackID", MAX_CALLBACK_ID_BYTES)?.to_string(),
    };
    Some(CaptchaEvent { kind, callback_id })
}

/// The script that answers a command, or `None` when it carried no id.
///
/// The id is embedded as a JSON string literal, so a quote or backslash in a
/// page-chosen value cannot end the literal and run as script. The script
/// returns `true` only when `nativeCallback` was really there to call, which is
/// what lets the log tell "answered" from "the page had nothing to answer".
pub fn callback_script(callback_id: &str) -> Option<String> {
    if callback_id.is_empty() {
        return None;
    }
    let quoted = serde_json::Value::String(callback_id.to_string()).to_string();
    Some(format!(
        "(() => {{ try {{ if (window.Roblox && window.Roblox.Hybrid && \
         window.Roblox.Hybrid.Bridge && \
         typeof window.Roblox.Hybrid.Bridge.nativeCallback === \"function\") {{ \
         window.Roblox.Hybrid.Bridge.nativeCallback({quoted}, true, {{}}); return true; }} }} \
         catch (_) {{}} return false; }})();"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(feature: &str, callback: Option<&str>) -> String {
        let mut v = serde_json::json!({
            "moduleID": "Navigation",
            "functionName": "navigateToFeature",
            "params": { "params": { "feature": feature } },
        });
        if let Some(id) = callback {
            v["callbackID"] = serde_json::Value::String(id.to_string());
        }
        v.to_string()
    }

    #[test]
    fn shown_and_success_are_recognised_with_their_callback_ids() {
        assert_eq!(
            parse_captcha_event(&command("CaptchaShown", Some("cb-1"))),
            Some(CaptchaEvent { kind: CaptchaEventKind::Shown, callback_id: "cb-1".into() })
        );
        assert_eq!(
            parse_captcha_event(&command("CaptchaSuccess", Some("cb-2"))),
            Some(CaptchaEvent { kind: CaptchaEventKind::Success, callback_id: "cb-2".into() })
        );
    }

    #[test]
    fn an_event_without_a_callback_id_is_recognised_but_cannot_be_answered() {
        let event = parse_captcha_event(&command("CaptchaSuccess", None)).expect("recognised");
        assert_eq!(event.kind, CaptchaEventKind::Success);
        assert_eq!(callback_script(&event.callback_id), None);
    }

    #[test]
    fn other_features_and_other_commands_are_left_alone() {
        assert_eq!(parse_captcha_event(&command("Home", Some("x"))), None);
        // A game launch must keep going to the engine only.
        assert_eq!(
            parse_captcha_event(r#"{"requestType":"RequestGameJob","placeId":"1"}"#),
            None
        );
        let wrong_module = command("CaptchaSuccess", Some("x")).replace("Navigation", "Other");
        assert_eq!(parse_captcha_event(&wrong_module), None);
    }

    #[test]
    fn malformed_or_hostile_input_is_refused_not_panicked_on() {
        assert_eq!(parse_captcha_event("not json"), None);
        assert_eq!(parse_captcha_event("[]"), None);
        assert_eq!(parse_captcha_event(r#"{"moduleID":"Navigation"}"#), None);
        // A callbackID that is present but not a usable string is a refusal,
        // not "no id".
        let numeric = serde_json::json!({
            "moduleID": "Navigation",
            "functionName": "navigateToFeature",
            "params": { "params": { "feature": "CaptchaShown" } },
            "callbackID": 7,
        });
        assert_eq!(parse_captcha_event(&numeric.to_string()), None);
        assert_eq!(parse_captcha_event(&command("CaptchaShown", Some("a\nb"))), None);
        let long = "x".repeat(MAX_CALLBACK_ID_BYTES + 1);
        assert_eq!(parse_captcha_event(&command("CaptchaShown", Some(&long))), None);
    }

    #[test]
    fn the_reply_calls_native_callback_with_true_and_an_empty_object() {
        let script = callback_script("abc").expect("an id");
        assert!(script.contains(r#"nativeCallback("abc", true, {})"#), "{script}");
    }

    #[test]
    fn a_quote_in_the_id_cannot_leave_the_string_literal() {
        let script = callback_script(r#"a"); alert(1); ("#).expect("an id");
        assert!(script.contains(r#"nativeCallback("a\"); alert(1); (", true, {})"#), "{script}");
    }
}
