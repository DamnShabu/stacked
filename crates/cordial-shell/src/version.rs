//! What version this is, and which build of it.
//!
//! **Two facts, not one, and the mistake was treating them as alternatives.**
//! The version is what the manifest says and is the thing anything ordering two
//! releases compares. The commit is provenance: it answers "which exact build
//! is this" in a bug report and nothing else.
//!
//! Before this split, `build.rs` stamped `git describe --tags --always --dirty`
//! as the version, which meant a tree whose `Cargo.toml` said 0.11.0 displayed
//! `0.10.0-26-g571e69b-dirty` -- the *previous* release. Meanwhile
//! `cordial-update`'s User-Agent has always been built from
//! `CARGO_PKG_VERSION`, so the same binary told a mirror it was 0.11.0 while
//! its title bar said 0.10.0. Two numbers that can disagree eventually do.
//!
//! [`GIT_SHA`] is `option_env!` rather than `env!` deliberately: an AUR source
//! package, a `cargo publish`, a vendored build and the Flatpak's `type: dir`
//! source all arrive without a usable `.git`, and none of them should fail to
//! compile over it. They get a clean [`VERSION`] and no commit, which is the
//! correct answer rather than a degraded one.

/// The version. `Cargo.toml`'s, always, and the only thing that may be compared.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The commit this was built from, when there was a git to ask.
///
/// Carries `-dirty` when the tree had uncommitted changes. AGENTS.md asks for
/// the full string in any report for exactly that reason: a build made from a
/// working tree several agents were editing is otherwise indistinguishable from
/// a committed one.
pub const GIT_SHA: Option<&str> = option_env!("CORDIAL_GIT_SHA");

/// What this is, under what licence, and where it came from.
///
/// Compiled into every binary that links this crate -- `cordial-run` included,
/// since `cordial-runtime` depends on this one -- so that `strings` answers the
/// question for a build that arrived without its repository. An AUR source
/// package, a vendored tarball and a fork's own release all reach somebody that
/// way, and the licence Cordial is under is not a thing they should have to go
/// looking for.
///
/// GPL-3.0-or-later asks a modified version to carry its notices forward. A
/// notice that is already in the binary is more likely to survive that than one
/// somebody has to remember to copy across.
///
/// **This is a notice and deliberately not a watermark.** Nothing reads it
/// back, nothing checks whether it is still there, and stripping it breaks
/// nothing -- AGENTS.md rules out client-side integrity marks, and anything
/// that behaved differently when this string was missing would be one. It is
/// here to be read, including by somebody who has forked this and is deciding
/// what they owe.
pub const NOTICE: &str = concat!(
    "Stacked ",
    env!("CARGO_PKG_VERSION"),
    " — GPL-3.0-or-later — https://github.com/DamnShabu/stacked\n",
    "A modified version of Cordial — https://github.com/luohoa97/cordial"
);

/// Version and provenance together, for a title bar or a bug report.
///
/// `0.11.0 (0fdbb44a1)` with a git, `0.11.0` without. Never a third shape: the
/// parenthesis is what stops the commit reading as part of the number.
pub fn full() -> String {
    match GIT_SHA {
        Some(sha) => format!("{VERSION} ({sha})"),
        None => VERSION.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **`VERSION` is a bare semver and stays one.**
    ///
    /// The whole point of the split is that this string can be compared against
    /// another release's. The moment it grows a `-dev` or a commit count, every
    /// comparison needs a parser for Cordial's own build metadata -- which is
    /// what the scheme this replaced was heading for.
    #[test]
    fn the_version_is_a_plain_semver_with_nothing_appended() {
        let parts: Vec<&str> = VERSION.split('.').collect();
        assert_eq!(parts.len(), 3, "not major.minor.patch: {VERSION}");
        for p in parts {
            assert!(
                p.chars().all(|c| c.is_ascii_digit()),
                "{VERSION} has a non-numeric component, so it is not orderable"
            );
        }
    }

    /// **The notice names this build, its licence and where it came from.**
    ///
    /// Asserted by content rather than against a copy of the string, because a
    /// test that repeats the literal only proves the literal was not edited by
    /// accident. What matters is that somebody holding the binary can answer
    /// all three questions from it, and that the version in it is this build's
    /// rather than one frozen when the constant was written.
    #[test]
    fn the_notice_carries_the_version_the_licence_and_the_project() {
        assert!(NOTICE.contains(VERSION), "{NOTICE} does not name this build");
        assert!(NOTICE.contains("GPL-3.0-or-later"), "{NOTICE} does not state the licence");
        assert!(
            NOTICE.contains("https://github.com/DamnShabu/stacked"),
            "{NOTICE} does not say where this came from"
        );
        // A fork carries the notice of what it was forked from forward, which
        // is the whole reason this constant exists.
        assert!(
            NOTICE.contains("https://github.com/luohoa97/cordial"),
            "{NOTICE} does not credit the upstream project"
        );
    }

    /// The commit never leaks into the version half of the display string.
    #[test]
    fn full_keeps_the_commit_in_brackets() {
        let text = full();
        assert!(text.starts_with(VERSION), "{text} does not start with {VERSION}");
        match GIT_SHA {
            Some(sha) => {
                assert_eq!(text, format!("{VERSION} ({sha})"));
                assert!(text.contains('('), "the commit must be bracketed: {text}");
            }
            None => assert_eq!(text, VERSION),
        }
    }
}
