//! A `roblox-player://` link the desktop handed over, checked before it is
//! believed.
//!
//! Cordial registers as the handler for `roblox-player:` and `roblox:` in
//! `packaging/io.github.damnshabu.Stacked.desktop`, which is the thing Sober did and the
//! reason a Play button on the website has anywhere to go on this machine. What
//! arrives is a string produced by a browser acting on a click, so it is treated
//! as hostile input rather than as an instruction: nothing here builds a path
//! from it and nothing interpolates it into a shell string. Joins reach the
//! child through [`std::process::Command::arg`]. The optional browser-account
//! resolver extracts and consumes the authentication ticket before that handoff.
//!
//! Acceptance checks the scheme, length and printable bytes. Account routing
//! recognises a narrower desktop grammar separately. Ticket stripping preserves
//! recognised desktop fields; unknown field-like text inside `gameinfo` is
//! removed with the ticket rather than forwarded as a possible credential suffix.
//!
//! **`cordial_runtime::deeplink` checks the same three things again**, and the
//! duplication is deliberate rather than an oversight: `cordial-shell` does not
//! link the runtime at all — see `main.rs` — and the client has to refuse a bad
//! `--join-url` whatever started it, including a hand-typed one. What the two
//! must not do is disagree, because a link this accepts and the client refuses
//! is a launch that dies after the launcher has already said it was joining. So
//! the limit is 2048 bytes in both, the schemes are the same pair in both, and
//! both require printable ASCII.

/// The schemes Cordial answers for.
///
/// **These have to stay in step with `MimeType` in
/// `packaging/io.github.damnshabu.Stacked.desktop`.** A scheme registered there and
/// missing here is a link the desktop hands over and the shell throws away with
/// a line on a stdout nobody is reading; the reverse is a scheme nothing will
/// ever deliver.
pub const SCHEMES: [&str; 2] = ["roblox-player", "roblox"];

/// The longest link that will be carried, in bytes.
///
/// Not a protocol limit — Roblox's own links are a couple of hundred bytes — but
/// the cap that stops an unbounded string from a browser becoming an argument
/// vector or a `GtkLabel` that has to be laid out. 2048 is the conventional URL
/// ceiling and is an order of magnitude past anything real that has been seen
/// here. **Bytes rather than characters, because `cordial_runtime::deeplink`
/// counts bytes**, and a link one side takes and the other refuses is the worst
/// of the three possible arrangements.
pub const MAX_LENGTH: usize = 2048;

/// Why a link was not taken. Each one is said out loud rather than swallowed:
/// a launcher that silently drops what the desktop handed it is indistinguish-
/// able from one that was never registered as the handler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejected {
    Empty,
    TooLong(usize),
    Scheme(String),
    /// Anything outside printable ASCII: control characters, spaces, newlines,
    /// or bytes past 0x7e. A URL carries none of them unescaped, and a newline
    /// in something that ends up in a log line or an argument list is the shape
    /// of an injection attempt rather than of a mistyped link.
    NotPrintableAscii,
}

impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Rejected::Empty => write!(f, "an empty link"),
            Rejected::TooLong(len) => {
                write!(f, "a link of {len} bytes, over the {MAX_LENGTH} this will carry")
            }
            Rejected::Scheme(scheme) => write!(
                f,
                "the scheme {scheme:?}, and Stacked only handles {}",
                SCHEMES.map(|s| format!("{s}:")).join(" and ")
            ),
            Rejected::NotPrintableAscii => {
                write!(f, "a link with something other than printable ASCII in it")
            }
        }
    }
}

/// Take the link, or say why not.
///
/// Returns the string exactly as it arrived when it passes. Nothing is
/// normalised, lowercased or re-encoded: the payload after the scheme belongs to
/// the client, and a launcher that rewrote it would be changing the join it was
/// asked to perform.
pub fn accept(raw: &str) -> Result<String, Rejected> {
    if raw.is_empty() {
        return Err(Rejected::Empty);
    }
    if raw.len() > MAX_LENGTH {
        return Err(Rejected::TooLong(raw.len()));
    }
    // 0x21 to 0x7e: everything printable and no space. A space is not a
    // character a URL carries unescaped, so there is no reason to allow one and
    // one good reason not to — it is the character that would split this into
    // two of something further down.
    if raw.bytes().any(|b| !(0x21..=0x7e).contains(&b)) {
        return Err(Rejected::NotPrintableAscii);
    }

    let Some((scheme, _)) = raw.split_once(':') else {
        return Err(Rejected::Scheme(raw.to_string()));
    };
    // Case-insensitively, because a scheme is case-insensitive by RFC 3986 and
    // the desktop is not the only thing that will ever hand one of these over.
    let lowered = scheme.to_ascii_lowercase();
    if !SCHEMES.contains(&lowered.as_str()) {
        return Err(Rejected::Scheme(scheme.to_string()));
    }
    Ok(raw.to_string())
}

/// A short form for the banner: enough to recognise the link by, and no more.
///
/// The launcher shows a queued join so that a click on a website is visibly
/// waiting rather than apparently ignored. What it must not do is stretch the
/// window to whatever length Roblox's payload happens to be this year.
pub fn summarise(url: &str) -> String {
    // The scheme, and deliberately nothing after it.
    //
    // **This used to truncate to 64 characters and it leaked a credential.**
    // `roblox-player:1+launchmode:play+gameinfo:` is forty of those, so a
    // sixty-four character prefix put twenty-four characters of the one-time
    // auth ticket into the banner on screen — and into anything that copied the
    // banner text. The bound was chosen for how much would fit in a row, which
    // is a layout question being asked to answer a disclosure one.
    //
    // Showing parameter *names* was the obvious repair and is still wrong here.
    // It requires parsing a format Roblox owns and changes, in the crate whose
    // stated rule is that the payload after the scheme belongs to the client;
    // `cordial_runtime::deeplink` had exactly that fix and it took two attempts,
    // because a `+` inside a ticket can turn a slice of the secret into
    // something that looks like a name.
    //
    // So the banner says which kind of link is waiting and stops. Nobody
    // deciding whether to press Roblox needs the payload, and a string that
    // cannot contain a secret cannot leak one however the format moves.
    match url.split_once(':') {
        Some((scheme, _)) => format!("{scheme}:"),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_banner_cannot_show_any_part_of_the_auth_ticket() {
        // This is a regression test for a live credential leak that shipped.
        // summarise() truncated to 64 characters, and
        // `roblox-player:1+launchmode:play+gameinfo:` is 40 of them, so the
        // banner displayed 24 characters of a one-time auth ticket -- under a
        // doc comment claiming the payload was the client's business.
        //
        // The assertion is deliberately about the ticket rather than about the
        // output format: a later change may well want a richer banner, and this
        // must fail if that change reintroduces any of the secret.
        let ticket = "SUPERSECRETTICKETVALUE0123456789ABCDEFGHIJKLMNOP";
        let url = format!(
            "roblox-player:1+launchmode:play+gameinfo:{ticket}+placelauncherurl:https%3A%2F%2Fx"
        );
        let shown = summarise(&url);
        for n in [4, 8, 16] {
            assert!(
                !shown.contains(&ticket[..n]),
                "summarise leaked {n} characters of the ticket: {shown}"
            );
        }
        assert!(!shown.contains("placelauncherurl"), "{shown}");
    }

    #[test]
    fn both_registered_schemes_are_taken_and_the_payload_is_left_alone() {
        // The payload is the client's to parse. Roblox's own links carry
        // placeId, launchmode and a ticket, and a launcher that normalised any
        // of that would be changing which game it was asked to join.
        let url = "roblox-player://placeId=1818/launchmode:play+gameinfo:AAAA";
        assert_eq!(accept(url).unwrap(), url);
        assert_eq!(accept("roblox://navigation/home").unwrap(), "roblox://navigation/home");
        // A scheme is case-insensitive, and a browser is not the only thing
        // that will ever hand one of these over.
        assert!(accept("Roblox-Player://placeId=1").is_ok());
    }

    #[test]
    fn anything_that_is_not_one_of_the_two_schemes_is_refused() {
        // This arrives from a browser click, so the refusals are the security
        // half: a `file:` or a `javascript:` reaching the client's argv because
        // the shell only checked that something was there is the failure this
        // function exists for.
        for hostile in [
            "file:///etc/passwd",
            "http://example.invalid/base.apk",
            "javascript:alert(1)",
            "roblox-studio://placeId=1",
            "/etc/passwd",
            "--lib-dir",
        ] {
            assert!(matches!(accept(hostile), Err(Rejected::Scheme(_))), "{hostile} was accepted");
        }
        assert_eq!(accept(""), Err(Rejected::Empty));
    }

    #[test]
    fn a_link_with_a_newline_or_a_space_in_it_is_refused_rather_than_carried() {
        // A URL carries none of these unescaped. Something that does is either
        // mangled or is trying to become two of something further down —
        // a second argument, a second log line — and neither is worth carrying.
        assert_eq!(accept("roblox-player://a\nb"), Err(Rejected::NotPrintableAscii));
        assert_eq!(accept("roblox-player://a b"), Err(Rejected::NotPrintableAscii));
        assert_eq!(accept("roblox-player://a\0b"), Err(Rejected::NotPrintableAscii));
        // Non-ASCII too, and this one is not about hostility: it is what keeps
        // this agreeing with `cordial_runtime::deeplink`, which counts bytes and
        // demands printable ASCII. A link the launcher took and the client
        // refused would fail after the user had been told it was joining.
        assert_eq!(accept("roblox-player://caf\u{e9}"), Err(Rejected::NotPrintableAscii));
    }

    #[test]
    fn an_unbounded_link_is_capped_and_the_refusal_says_by_how_much() {
        let long = format!("roblox-player://{}", "a".repeat(MAX_LENGTH));
        match accept(&long) {
            Err(Rejected::TooLong(len)) => assert!(len > MAX_LENGTH, "{len}"),
            other => panic!("a link past the cap must be refused: {other:?}"),
        }
        // And the boundary is not off by one: exactly the cap is carried.
        let exact = format!("roblox-player://{}", "a".repeat(MAX_LENGTH - "roblox-player://".len()));
        assert_eq!(exact.len(), MAX_LENGTH);
        assert!(accept(&exact).is_ok());
    }

    #[test]
    fn the_refusal_names_the_schemes_cordial_does_handle() {
        // Somebody meeting this in a terminal has to be able to tell a link
        // Cordial declined from a Cordial that was never registered as the
        // handler at all.
        let message = accept("http://example.invalid").unwrap_err().to_string();
        assert!(message.contains("roblox-player:"), "{message}");
        assert!(message.contains("roblox:"), "{message}");
    }

    #[test]
    fn gio_reshapes_a_roblox_link_and_is_therefore_not_where_the_string_comes_from() {
        // **Measured, and it is why `main.rs` reads `argv` rather than the
        // `GFile`s the `open` signal hands over.** The first version of this
        // took `GFile::uri()`, which looked obviously right and quietly rewrote
        // the payload, on this machine, with these links:
        //
        //   roblox-player://placeId=1818&launchData=hello%20there
        //     -> roblox-player://placeId=1818&launchData=hello%20there/
        //   roblox-player:1+launchmode:play+gameinfo:AAA
        //     -> roblox-player:///1+launchmode:play+gameinfo:AAA
        //
        // The second is the shape Roblox's own links take — an opaque payload
        // after the colon, no authority — and GIO parses it as a URL, decides
        // it has an empty authority and an absolute path, and hands back
        // something the client would have to guess its way back from.
        // `parse_name()` gives the identical string, so there is no second
        // accessor that escapes it.
        //
        // This test is a tripwire rather than a specification of GLib: if it
        // ever fails because GIO stopped reshaping these, that is a finding and
        // not a breakage.
        use libadwaita::gtk::gio;
        use libadwaita::gtk::prelude::*;

        // **The tripwire fired, on 2026-08-27, and the finding is better than the
        // guess it replaced.** This asserted the reshaping still happened, and
        // it does on this host and does not in CI's container -- same code,
        // different glib. So GIO's handling of an opaque payload is version
        // dependent, which is a stronger reason to read `argv` than "GIO
        // rewrites it": there is no glib behaviour to depend on either way.
        //
        // The observation is kept and reported rather than asserted, because a
        // test that fails when a dependency legitimately changes is a test that
        // gets deleted the third time somebody hits it. What is asserted below
        // is the thing that must be true regardless, and it is what the route
        // this comment describes actually guarantees.
        let opaque = "roblox-player:1+launchmode:play+gameinfo:AAA";
        let through_gio = gio::File::for_commandline_arg(opaque);
        if through_gio.uri() == opaque {
            eprintln!(
                "[deep_link] note: this glib leaves `{opaque}` untouched through \
                 GFile::uri. It reshaped it when the argv route was chosen; the \
                 route is correct either way and this is only worth knowing."
            );
        } else {
            assert_eq!(through_gio.uri(), "roblox-player:///1+launchmode:play+gameinfo:AAA");
            assert_eq!(through_gio.parse_name(), through_gio.uri());
        }

        // And the raw string is carried untouched, which is the whole point of
        // the route that replaced it.
        assert_eq!(accept(opaque).unwrap(), opaque);
    }

    #[test]
    fn the_banner_text_cannot_stretch_the_window() {
        let long = format!("roblox-player://{}", "a".repeat(500));
        let short = summarise(&long);
        assert_eq!(short, "roblox-player:", "{short}");
        assert_eq!(summarise("roblox://home"), "roblox:");
    }
}
