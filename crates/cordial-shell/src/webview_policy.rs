//! What the in-experience web window is allowed to load, and what it is allowed
//! to talk to the engine about.
//!
//! Derived from mocktail's `webview_helper_policy.cc`, Copyright 2026
//! komaruworld, Apache-2.0 — see `NOTICE` and `third_party/mocktail-webview/`.
//! Rewritten in Rust against `glib::Uri` rather than translated line by line,
//! and the two rules below are theirs. Apache-2.0 section 4(b): this file is a
//! modified derivation and says so here.
//!
//! ## Why this is a separate module with no WebKit in it
//!
//! Because it is the part that has to be right. Everything else in a web window
//! is plumbing — if the header bar is wrong somebody notices and fixes it. If
//! this is wrong, a page the user did not expect gets to send commands to the
//! Roblox process, and nobody notices at all.
//!
//! `glib` comes through `gtk4`'s re-export rather than as a dependency of its
//! own: two glib versions in one process would be two sets of GObject type
//! registrations for the same C types, which is the same reasoning that pins
//! gtk4 across the workspace.
//!
//! Keeping it free of WebKit means it is testable without a browser engine,
//! without a display, and without the `webkitgtk6.0-devel` headers that the view
//! itself needs. The tests below run everywhere `cargo test` runs.
//!
//! ## The two rules
//!
//! They are separate on purpose, and the separation is the design:
//!
//! 1. **May this be loaded at all?** HTTPS, a real host, the default port, and
//!    *no userinfo component*.
//! 2. **May this page use the privileged bridge?** Only if rule 1 passed *and*
//!    the host is Roblox's.
//!
//! A page can pass the first and fail the second, and that is the common case —
//! a payment provider or an identity provider inside the checkout flow should
//! render, and should not be able to ask the engine for anything.
//!
//! The userinfo clause in rule 1 is the one that looks like a detail and is not.
//! `https://www.roblox.com@evil.example/` has host `evil.example`, and a reader
//! skimming the address sees Roblox. Rejecting any URI carrying userinfo removes
//! the ambiguity rather than trying to be clever about it.

/// The domain whose pages may use the privileged bridge.
const ROBLOX_ROOT: &str = "roblox.com";

/// What the policy decided about one URI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UriPolicy {
    /// May be navigated to.
    pub allowed: bool,
    /// May additionally use the privileged engine bridge.
    pub privileged_bridge_allowed: bool,
    /// For logging. `"invalid"` when the URI did not parse.
    pub scheme: String,
    /// For logging. `"none"` when there was no host.
    pub host: String,
}

impl UriPolicy {
    /// The refusal every failure path returns.
    ///
    /// A single constructor so that adding a field cannot leave one early return
    /// quietly defaulting it to permissive.
    fn refused() -> Self {
        UriPolicy {
            allowed: false,
            privileged_bridge_allowed: false,
            scheme: "invalid".into(),
            host: "none".into(),
        }
    }
}

/// Whether `host` is `roblox.com` or a subdomain of it.
///
/// The boundary check is the whole function. A plain "ends with roblox.com"
/// accepts `notroblox.com` and `myroblox.com`, which anyone can register, and
/// which would then be handed the bridge. Either the host *is* the root, or the
/// character before the suffix is a dot.
fn is_roblox_host(host: &str) -> bool {
    let Some(rest) = host.len().checked_sub(ROBLOX_ROOT.len()).map(|i| (i, &host[i..])) else {
        return false;
    };
    let (prefix_len, suffix) = rest;
    if !suffix.eq_ignore_ascii_case(ROBLOX_ROOT) {
        return false;
    }
    prefix_len == 0 || host.as_bytes()[prefix_len - 1] == b'.'
}

/// Decide what `uri` may do.
///
/// `about:blank` is allowed because a window has to start somewhere before its
/// real address is set, and it carries no content and no origin. It is not
/// privileged.
pub fn evaluate(uri: &str) -> UriPolicy {
    if uri == "about:blank" {
        return UriPolicy {
            allowed: true,
            privileged_bridge_allowed: false,
            scheme: "about".into(),
            host: "none".into(),
        };
    }

    let Ok(parsed) = gtk4::glib::Uri::parse(uri, gtk4::glib::UriFlags::NONE) else {
        return UriPolicy::refused();
    };

    let scheme = parsed.scheme().to_string();
    let host = parsed.host().map(|h| h.to_string()).unwrap_or_default();
    let port = parsed.port();

    // Every clause is a separate way to be dangerous, so they are listed rather
    // than folded together: not HTTPS at all; no host to judge; a userinfo
    // component making the address misread; a non-default port, which on this
    // path means somebody is aiming the window somewhere unusual.
    let allowed = scheme.eq_ignore_ascii_case("https")
        && !host.is_empty()
        && parsed.userinfo().is_none()
        && (port == -1 || port == 443);

    UriPolicy {
        privileged_bridge_allowed: allowed && is_roblox_host(&host),
        allowed,
        scheme: if scheme.is_empty() { "invalid".into() } else { scheme },
        host: if host.is_empty() { "none".into() } else { host },
    }
}

/// The largest bridge message that will be accepted, in bytes.
///
/// mocktail's figure, kept. A cap is needed because the page chooses the length
/// and the engine does not; without one, a hostile or merely broken page can
/// make the client allocate until it dies. 64 KiB is far above any real command
/// and far below anything that matters.
pub const MAX_BRIDGE_COMMAND_BYTES: usize = 64 * 1024;

/// Whether a bridge message from a page is acceptable in principle.
///
/// Separate from [`evaluate`] because origin and size are independent failures
/// and the log should be able to say which happened.
pub fn bridge_message_acceptable(policy: &UriPolicy, len: usize) -> Result<(), &'static str> {
    if !policy.privileged_bridge_allowed {
        return Err("the page's origin may not use the bridge");
    }
    if len > MAX_BRIDGE_COMMAND_BYTES {
        return Err("the command is larger than the bridge accepts");
    }
    Ok(())
}

/// The path of `uri` for a log line: no query, no fragment, and any segment
/// that is not a short plain word replaced by `<id>`.
///
/// A web-view address can carry a one-time ticket in its query
/// (`docs/analysis/webview-surface.md` section 4), and a path segment can be an
/// identifier nobody wants in a log somebody pastes into an issue. The
/// segments that are left (`captcha`, `app`, `login`) are what says which page
/// the engine asked for, which is the part a bug report needs (#83 had the host
/// and nothing else).
pub fn loggable_path(uri: &str) -> String {
    let Ok(parsed) = gtk4::glib::Uri::parse(uri, gtk4::glib::UriFlags::NONE) else {
        return "(unparseable)".into();
    };
    let path = parsed.path().to_string();
    if path.is_empty() {
        return "/".into();
    }
    let word = |segment: &str| {
        segment.len() <= 32 && segment.bytes().all(|b| b.is_ascii_alphabetic() || b == b'-' || b == b'_')
    };
    let mut shown: Vec<&str> = Vec::new();
    for (index, segment) in path.split('/').enumerate() {
        if index >= 9 {
            shown.push("...");
            break;
        }
        shown.push(if segment.is_empty() || word(segment) { segment } else { "<id>" });
    }
    shown.join("/")
}

/// Name a hybrid-bridge command for a log line: `Module.function`, with the
/// feature in brackets for `Navigation.navigateToFeature`, which is the
/// command the sign-in checks speak and whose feature says which one.
///
/// Names only, never `params`: the parameters are whatever the page and the
/// engine are mid-conversation about. `None` when the command is not a hybrid
/// command or a name is not a plain identifier.
pub fn describe_command(command: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(command).ok()?;
    let object = value.as_object()?;
    let name = |v: Option<&serde_json::Value>| {
        v?.as_str()
            .filter(|s| !s.is_empty() && s.len() <= 64)
            .filter(|s| s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.')))
            .map(str::to_string)
    };
    let module = name(object.get("moduleID"))?;
    let function = name(object.get("functionName"))?;
    let feature = object
        .get("params")
        .and_then(|p| p.get("params"))
        .and_then(|p| name(p.get("feature")));
    Some(match feature {
        Some(feature) => format!("{module}.{function}({feature})"),
        None => format!("{module}.{function}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_logged_path_keeps_words_and_drops_query_and_identifiers() {
        assert_eq!(
            loggable_path("https://www.roblox.com/captcha/app/login?credentialsValue=somebody&x=1#frag"),
            "/captcha/app/login"
        );
        assert_eq!(
            loggable_path("https://www.roblox.com/users/123456/profile"),
            "/users/<id>/profile"
        );
        assert_eq!(
            loggable_path("https://www.roblox.com/challenge/0f8fad5b-d9cb-469f-a165-70867728950e"),
            "/challenge/<id>"
        );
        assert_eq!(loggable_path("https://www.roblox.com"), "/");
        assert_eq!(loggable_path("not a uri"), "(unparseable)");
    }

    #[test]
    fn a_command_is_named_without_its_parameters() {
        let navigate = r#"{"moduleID":"Navigation","functionName":"navigateToFeature","params":{"params":{"feature":"CaptchaSuccess","secret":"x"}},"callbackID":"c1"}"#;
        assert_eq!(describe_command(navigate).as_deref(), Some("Navigation.navigateToFeature(CaptchaSuccess)"));
        let plain = r#"{"moduleID":"Game","functionName":"launchGame","params":{"placeId":1}}"#;
        assert_eq!(describe_command(plain).as_deref(), Some("Game.launchGame"));
        for not_a_command in [
            "not json",
            r#"{"placeId":1}"#,
            r#"{"moduleID":"a b","functionName":"f"}"#,
            r#"{"moduleID":"M","functionName":"f/g"}"#,
        ] {
            assert_eq!(describe_command(not_a_command), None, "{not_a_command}");
        }
        // A feature that is not a plain identifier is left out rather than printed.
        let odd = r#"{"moduleID":"Navigation","functionName":"navigateToFeature","params":{"params":{"feature":"a b"}}}"#;
        assert_eq!(describe_command(odd).as_deref(), Some("Navigation.navigateToFeature"));
    }

    #[test]
    fn a_roblox_page_may_load_and_may_use_the_bridge() {
        let p = evaluate("https://www.roblox.com/my/account");
        assert!(p.allowed);
        assert!(p.privileged_bridge_allowed);
        assert_eq!(p.host, "www.roblox.com");
    }

    #[test]
    fn the_bare_root_counts_as_roblox() {
        assert!(evaluate("https://roblox.com/").privileged_bridge_allowed);
    }

    /// The reason `is_roblox_host` checks a boundary rather than a suffix.
    /// Both of these are registrable by anybody.
    #[test]
    fn a_lookalike_domain_may_load_but_never_gets_the_bridge() {
        for host in ["https://notroblox.com/", "https://myroblox.com/"] {
            let p = evaluate(host);
            assert!(p.allowed, "{host} should still render");
            assert!(!p.privileged_bridge_allowed, "{host} must not get the bridge");
        }
    }

    /// The phishing shape. Host is `evil.example`; a user skimming the address
    /// bar sees Roblox. Refused outright rather than merely unprivileged,
    /// because there is no legitimate reason for userinfo here at all.
    #[test]
    fn a_userinfo_component_is_refused_outright() {
        let p = evaluate("https://www.roblox.com@evil.example/pay");
        assert!(!p.allowed);
        assert!(!p.privileged_bridge_allowed);
    }

    #[test]
    fn plaintext_http_is_refused_even_for_roblox() {
        let p = evaluate("http://www.roblox.com/");
        assert!(!p.allowed);
        assert!(!p.privileged_bridge_allowed);
    }

    #[test]
    fn a_non_default_port_is_refused() {
        assert!(!evaluate("https://www.roblox.com:8443/").allowed);
        // ...and the default, written out, is still the default.
        assert!(evaluate("https://www.roblox.com:443/").allowed);
    }

    #[test]
    fn the_scheme_and_host_comparisons_ignore_case() {
        let p = evaluate("HTTPS://WWW.ROBLOX.COM/");
        assert!(p.allowed);
        assert!(p.privileged_bridge_allowed);
    }

    #[test]
    fn about_blank_loads_but_is_not_privileged() {
        let p = evaluate("about:blank");
        assert!(p.allowed);
        assert!(!p.privileged_bridge_allowed);
    }

    #[test]
    fn nonsense_is_refused_and_says_so_in_the_log_fields() {
        let p = evaluate("not a uri at all");
        assert!(!p.allowed);
        assert_eq!(p.scheme, "invalid");
        assert_eq!(p.host, "none");
    }

    #[test]
    fn other_schemes_do_not_sneak_through() {
        for uri in ["file:///etc/passwd", "javascript:alert(1)", "data:text/html,<b>x"] {
            assert!(!evaluate(uri).allowed, "{uri}");
        }
    }

    #[test]
    fn an_oversized_command_is_refused_separately_from_a_bad_origin() {
        let good = evaluate("https://www.roblox.com/");
        assert!(bridge_message_acceptable(&good, 32).is_ok());
        assert_eq!(
            bridge_message_acceptable(&good, MAX_BRIDGE_COMMAND_BYTES + 1),
            Err("the command is larger than the bridge accepts")
        );
        let bad = evaluate("https://notroblox.com/");
        assert_eq!(
            bridge_message_acceptable(&bad, 32),
            Err("the page's origin may not use the bridge")
        );
    }
}
