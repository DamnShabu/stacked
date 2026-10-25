//! Roblox's release notes, so the header-bar button has something to show.
//!
//! The up-to-date state of that button is not a dead end — it shows the current
//! version and its changelog — so this is fetched whether or not there is an
//! update, and it is the half of the check that works for the Android build
//! today. See [`version`](crate::version) for why the other half does not.
//!
//! ## Where these come from, measured 2026-08-02
//!
//! Roblox posts them to the DevForum, which is Discourse, which serves every
//! page as JSON if you ask for `.json`:
//!
//! ```text
//! $ curl -sSL -w '%{http_code} %{url_effective}\n' \
//!     https://devforum.roblox.com/c/updates/release-notes.json
//! 200 https://devforum.roblox.com/c/updates/release-notes/62.json
//! → topic_list.topics[]: {"id":4763851,"slug":"release-notes-for-732",
//!                         "title":"Release Notes for 732",
//!                         "created_at":"2026-07-29T18:44:52.923Z"}
//! ```
//!
//! The slug URL is used rather than the numeric `62` it redirects to: a
//! category id is a database key and a slug is the thing Roblox would keep if
//! they reorganised. ureq follows the redirect.
//!
//! **A topic must be fetched by slug *and* id.** `/t/4763851.json` answers
//! **403**; `/t/release-notes-for-732/4763851.json` answers 200. That is not a
//! permission problem to work around, it is Discourse's routing, and it is
//! written down here because a 403 from a public forum reads like something
//! else entirely.
//!
//! ## The title carries the version
//!
//! Every entry is titled `Release Notes for NNN`, and `NNN` is the engine major
//! — the `732` in `0.732.23.7321040`, and the `Version=732` the client logs
//! about itself. That correspondence is what lets this be useful without the
//! version endpoint: the newest release-notes major is the newest engine Roblox
//! has shipped. Entries whose title carries no number — the category's own
//! "About the Release Notes category" pinned post — are dropped rather than
//! shown, since they are not releases.

use crate::http;
use crate::Unreachable;

/// The release-notes category, by slug.
pub const CATEGORY: &str = "https://devforum.roblox.com/c/updates/release-notes.json";

/// One published set of release notes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// The engine major this announces: `732`.
    pub major: u32,
    pub title: String,
    pub id: u64,
    pub slug: String,
    /// ISO 8601, as Discourse gives it.
    pub created_at: String,
}

impl Release {
    /// The URL its body is fetched from. Slug and id both, because id alone is
    /// a 403.
    pub fn url(&self) -> String {
        format!("https://devforum.roblox.com/t/{}/{}.json", self.slug, self.id)
    }

    /// The URL to open in a browser, for the "read the rest" the popover will
    /// want. Same route without `.json`.
    pub fn web_url(&self) -> String {
        format!("https://devforum.roblox.com/t/{}/{}", self.slug, self.id)
    }
}

/// The body of one set of release notes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notes {
    pub title: String,
    pub created_at: String,
    /// Discourse's rendered HTML, as posted.
    pub html: String,
}

impl Notes {
    /// A plain-text rendering, for a label.
    ///
    /// Crude on purpose and not a sanitiser: it drops tags and unescapes the
    /// five XML entities, which is enough for a summary line and is not enough
    /// for anything that renders markup. Nothing in Cordial should feed this to
    /// a widget that interprets what comes out — the popover shows text, and
    /// [`Release::web_url`] is the way to read the whole thing.
    pub fn text(&self) -> String {
        strip_tags(&self.html)
    }
}

/// Every release in the category listing, newest first.
pub fn releases() -> Result<Vec<Release>, Unreachable> {
    let value = http::get_json(CATEGORY)?;
    parse_releases(&value).map_err(|why| Unreachable::Malformed { url: CATEGORY.into(), why })
}

/// The newest one Roblox has published.
pub fn latest() -> Result<Release, Unreachable> {
    releases()?.into_iter().max_by_key(|r| r.major).ok_or(Unreachable::Malformed {
        url: CATEGORY.into(),
        why: "the release-notes category listed no topic titled \"Release Notes for <number>\""
            .into(),
    })
}

/// Fetch one release's body.
pub fn notes(release: &Release) -> Result<Notes, Unreachable> {
    let url = release.url();
    let value = http::get_json(&url)?;
    parse_notes(&value).map_err(|why| Unreachable::Malformed { url, why })
}

/// Read a Discourse category listing.
///
/// Split from the request so the shape stays pinned to a body that was
/// observed. Sorted newest-major first rather than trusting the listing's own
/// order, which is Discourse's to change and is by activity rather than by
/// version.
pub fn parse_releases(value: &serde_json::Value) -> Result<Vec<Release>, String> {
    let topics = value
        .get("topic_list")
        .and_then(|l| l.get("topics"))
        .and_then(|t| t.as_array())
        .ok_or("no topic_list.topics array")?;

    let mut out: Vec<Release> = topics
        .iter()
        .filter_map(|t| {
            let title = t.get("title")?.as_str()?.to_string();
            let major = major_in_title(&title)?;
            Some(Release {
                major,
                title,
                id: t.get("id")?.as_u64()?,
                slug: t.get("slug")?.as_str()?.to_string(),
                created_at: t
                    .get("created_at")
                    .and_then(|c| c.as_str())
                    .unwrap_or_default()
                    .to_string(),
            })
        })
        .collect();
    if out.is_empty() {
        return Err(format!(
            "{} topics were listed and none was titled \"Release Notes for <number>\"",
            topics.len()
        ));
    }
    out.sort_by_key(|n| std::cmp::Reverse(n.major));
    Ok(out)
}

/// Read a Discourse topic.
pub fn parse_notes(value: &serde_json::Value) -> Result<Notes, String> {
    let html = value
        .get("post_stream")
        .and_then(|s| s.get("posts"))
        .and_then(|p| p.as_array())
        .and_then(|p| p.first())
        .and_then(|p| p.get("cooked"))
        .and_then(|c| c.as_str())
        .ok_or("no post_stream.posts[0].cooked")?;
    Ok(Notes {
        title: value.get("title").and_then(|t| t.as_str()).unwrap_or_default().to_string(),
        created_at: value
            .get("created_at")
            .and_then(|t| t.as_str())
            .unwrap_or_default()
            .to_string(),
        html: html.to_string(),
    })
}

/// `Release Notes for 732` -> `732`.
///
/// The last run of digits in the title, rather than a prefix match on "Release
/// Notes for": the category has been titled several ways over the years and the
/// number is the part that has not moved.
fn major_in_title(title: &str) -> Option<u32> {
    title
        .split(|c: char| !c.is_ascii_digit())
        .rfind(|s| !s.is_empty())
        .and_then(|s| s.parse().ok())
}

// ------------------------------------------------- the Creator Hub docs table
//
// The DevForum post announces a release; it does not list what changed. Its body
// is five lines of "732 has landed, enjoy" and a set of links, one of which goes
// to the Creator Hub — so a window showing the post was showing the covering
// note and calling it a changelog.
//
// **This used to be keyed by the DevForum's engine number, and that was the
// bug.** `release-notes-<major>` was a real route once, but it answers a plain
// 404 now, and the version-keyed fallback this file grew in its place —
// looking three majors back for whichever one existed — was the wrong fix for
// the wrong problem: it kept the version arithmetic that was never the actual
// question. Reported exactly this way: "it shouldnt check the next release, it
// should find the latest release in roblox's release notes. Use their api using
// a browser and checking network requests, id rather a stable API than an
// unstable HTML."
//
// Measured on 2026-08-28 with a browser's own network panel:
//
//   - `create.roblox.com/docs/release-notes/release-notes-736` -> 404.
//     `-735` and `-734` -> 301, to `/docs/updates/2026-08-17` and
//     `/docs/updates/2026-08-10`. The old route is a legacy alias into the
//     replacement, not a route of its own any more.
//   - `create.roblox.com/docs/updates` -> 200, a Next.js page. Its
//     `__NEXT_DATA__` carries the *site's whole navigation tree* and zero
//     `ReleaseNotesText` entries — this is the index, not the content.
//   - That page's navigation lists the release pages Roblox itself keeps, and
//     names one of them for us: a "Release notes" section whose entries are
//     titled `Current release`, `Pending release`, and a `Recent releases`
//     list of `Week of <date>` pages. **"Current release" is the answer to the
//     complaint, by Roblox's own label** — no version comparison required.
//   - `.../_next/data/<buildId>/updates.json` answers with exactly that
//     navigation, as data, once `<buildId>` is known — Next.js's own
//     per-deploy id, stamped into the shell page's `__NEXT_DATA__` as
//     `"buildId":"..."` and changing on every Roblox deploy, which is why it
//     is read fresh each call rather than assumed.
//   - `.../_next/data/<buildId>/updates/2026-08-24.json` (the path the
//     navigation gave for "Current release") answers with clean JSON:
//     `pageProps.data.releaseNoteContents.content`, an array of
//     `{"ReleaseNotesText", "ReleaseNotesType", "Status"}` — the same three
//     fields the old HTML scrape parsed out of a `__NEXT_DATA__` script tag,
//     now the entire body of a JSON response with nothing else to strip away.
//
// **This is not a documented public API.** It is Next.js's own hydration data,
// and Roblox did not design it for outside callers. What makes it a fair trade
// against the HTML scrape it replaces is which parts are load-bearing: the
// `buildId` key and the `pageProps` envelope are the framework's own
// constants, no more Roblox's to rearrange than an HTTP header is, while what
// the old code parsed — the page's rendered markup — was entirely Roblox's
// content to change and did, twice, inside one release cycle.

/// The Creator Hub's release-notes index. Fetched first, and only for its
/// build id — see the module section above for why a build id rather than an
/// engine number is the key everything else here runs on.
pub const DOCS_UPDATES_URL: &str = "https://create.roblox.com/docs/updates";

const DOCS_HOST: &str = "https://create.roblox.com/docs";

/// `https://create.roblox.com/docs/_next/data/<build_id><path>.json`
///
/// Next.js serves a page's own props at this URL once its build id is known,
/// `path` being the ordinary site path — `/updates` for the index,
/// `/updates/2026-08-24` for one week — with `.json` appended. It is the data
/// the shell HTML asks a browser to hydrate with; asking for it directly is
/// what turns "scrape what the page renders" into "read what Roblox already
/// serves as data".
fn docs_data_url(build_id: &str, path: &str) -> String {
    format!("{DOCS_HOST}/_next/data/{build_id}{path}.json")
}

/// One row of a release page's table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub text: String,
    /// `Improvements` or `Fixes`, as Roblox groups them.
    pub kind: String,
    /// `Live` or `Pending`. Every dated page measured 2026-08-28 answered
    /// `Live` throughout — the `Pending` distinction now lives on its own
    /// `/updates/pending` page, in a different shape with no per-entry status
    /// at all — so this is carried rather than assumed, in case that changes
    /// back.
    pub status: String,
}

/// Which rule found the changelog on screen. Kept distinct from a bare
/// `Vec<Entry>` so a caller can say when the ordinary path did not apply,
/// rather than silently passing off a fallback as Roblox's own current
/// release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocsSource {
    /// Roblox's navigation named this one "Current release" — the direct
    /// answer to "find the latest release", with no version comparison
    /// anywhere in reaching it.
    CurrentRelease,
    /// The navigation carried no entry titled "Current release" — renamed,
    /// restructured — so this is the newest `/updates/<date>` entry instead,
    /// found by comparing the dates spelled out in the navigation's own
    /// paths rather than giving up.
    Newest { date: String },
}

/// Fetch and read whatever Roblox currently calls the latest release.
///
/// Three requests, run in order because each needs what the last one found,
/// and none of them an engine number:
///
/// 1. [`DOCS_UPDATES_URL`], for its build id.
/// 2. That build's navigation data, to find the path Roblox filed under
///    "Current release" — or, failing that, the newest dated entry.
/// 3. The page at that path.
pub fn docs_notes() -> Result<(DocsSource, Vec<Entry>), Unreachable> {
    let shell = http::get_text(DOCS_UPDATES_URL)?;
    let build_id = find_build_id(&shell).ok_or_else(|| Unreachable::Malformed {
        url: DOCS_UPDATES_URL.into(),
        why: "the page carried no buildId".into(),
    })?;

    let nav_url = docs_data_url(&build_id, "/updates");
    let nav = http::get_json(&nav_url)?;
    let (source, path) =
        pick_release(&nav).map_err(|why| Unreachable::Malformed { url: nav_url, why })?;

    let page_url = docs_data_url(&build_id, &path);
    let value = http::get_json(&page_url)?;
    let entries =
        parse_docs_notes(&value).map_err(|why| Unreachable::Malformed { url: page_url, why })?;
    Ok((source, entries))
}

/// Pull the build id Next.js stamped this deploy with out of the shell page.
///
/// A plain substring search rather than a JSON parse of the whole blob: the
/// shell's `__NEXT_DATA__` is the entire site's navigation tree, over 100 kB
/// measured 2026-08-28, and every byte of it other than this one field is
/// discarded here.
fn find_build_id(html: &str) -> Option<String> {
    const NEEDLE: &str = "\"buildId\":\"";
    let start = html.find(NEEDLE)? + NEEDLE.len();
    let end = html[start..].find('"')?;
    Some(html[start..start + end].to_string())
}

/// Find "Current release" in the navigation, or the newest dated entry if
/// that label is gone.
fn pick_release(nav: &serde_json::Value) -> Result<(DocsSource, String), String> {
    let mut items = Vec::new();
    collect_nav_items(nav, &mut items);
    if items.is_empty() {
        return Err("no navigation item carried both a title and a path".into());
    }
    if let Some((_, path)) = items.iter().find(|(title, _)| *title == "Current release") {
        return Ok((DocsSource::CurrentRelease, (*path).to_string()));
    }
    // Roblox renaming or restructuring a label is the ordinary hazard for a
    // documentation site, not a reason to show nothing — so fall back to the
    // newest dated entry. `/updates/<date>` sorts correctly as a plain string
    // because the date inside it is fixed-width and big-endian.
    items
        .iter()
        .filter_map(|(_, path)| date_of_updates_path(path).map(|date| (date, *path)))
        .max_by_key(|(date, _)| date.clone())
        .map(|(date, path)| (DocsSource::Newest { date }, path.to_string()))
        .ok_or_else(|| {
            "no entry titled \"Current release\" and no /updates/<date> entry either".into()
        })
}

/// Walk the navigation tree for every `{title, path}` pair, at any depth.
///
/// By shape rather than by the `heading`/`navigation`/`section` route to it —
/// the same reasoning as [`collect_entries`] below: that nesting is Roblox's
/// own content structure to rearrange, and a title next to a path is the part
/// that would have to change for this to break.
fn collect_nav_items<'a>(value: &'a serde_json::Value, out: &mut Vec<(&'a str, &'a str)>) {
    match value {
        serde_json::Value::Object(map) => {
            if let (Some(title), Some(path)) = (
                map.get("title").and_then(|v| v.as_str()),
                map.get("path").and_then(|v| v.as_str()),
            ) {
                out.push((title, path));
            }
            for v in map.values() {
                collect_nav_items(v, out);
            }
        }
        serde_json::Value::Array(items) => {
            for v in items {
                collect_nav_items(v, out);
            }
        }
        _ => {}
    }
}

/// `/updates/2026-08-24` -> `Some("2026-08-24")`. Anything else —
/// `/updates/pending`, a `null` path, a reference page — is `None`, which is
/// what keeps the fallback from ever landing on the pending page: it carries
/// no `ReleaseNotesType`/`Status` at all, a different shape from every dated
/// page, measured 2026-08-28.
fn date_of_updates_path(path: &str) -> Option<String> {
    let date = path.strip_prefix("/updates/")?;
    let bytes = date.as_bytes();
    let is_date = bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes.iter().enumerate().all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit());
    is_date.then(|| date.to_string())
}

/// Pull the entries out of one release page's data.
///
/// Split from the request so the shape stays pinned to a page that was
/// observed, the same arrangement as [`parse_releases`]. Reading `value` by
/// shape in [`collect_entries`] rather than by its
/// `pageProps.data.releaseNoteContents.content` path is what would let this
/// keep working if Roblox nests the same three fields somewhere else again —
/// it is exactly that nesting that changed, from an HTML table's
/// `__NEXT_DATA__` script to a dedicated JSON response, between when this
/// module was first written and 2026-08-28.
pub fn parse_docs_notes(value: &serde_json::Value) -> Result<Vec<Entry>, String> {
    let mut out = Vec::new();
    collect_entries(value, &mut out);
    if out.is_empty() {
        return Err("no ReleaseNotesText entries were found in the page".into());
    }
    Ok(out)
}

/// Walk the blob for anything shaped like an entry.
///
/// By shape rather than by path: the route to it runs through Next's own
/// `props`/`pageProps` nesting, which is a framework's private arrangement and
/// not something Roblox has promised to keep. The field names are the part that
/// would have to change for this to break, and if they do it fails loudly in
/// [`parse_docs_notes`] rather than showing an empty changelog.
fn collect_entries(value: &serde_json::Value, out: &mut Vec<Entry>) {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(text) = map.get("ReleaseNotesText").and_then(|v| v.as_str()) {
                let text = markdown_to_pango(text);
                if !text.is_empty() {
                    out.push(Entry {
                        text,
                        kind: map
                            .get("ReleaseNotesType")
                            .and_then(|v| v.as_str())
                            .unwrap_or("Notes")
                            .to_string(),
                        status: map
                            .get("Status")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                    });
                }
            }
            for v in map.values() {
                collect_entries(v, out);
            }
        }
        serde_json::Value::Array(items) => {
            for v in items {
                collect_entries(v, out);
            }
        }
        _ => {}
    }
}

/// One note's text, as Pango markup for the label that shows it.
///
/// These notes are markdown — Roblox writes `**bold**` in them, three of the
/// thirty on 732 — so they go through a real parser rather than the hand-rolled
/// marker-stripping this used to do. That version paired `**` by searching for
/// the next one, which happily matched the stars in `2 ** 8` against an
/// unrelated marker later in the line and deleted everything between them.
///
/// The output is Pango markup because a `GtkLabel` renders it natively, so bold
/// arrives as bold rather than as words with the stars filed off. Everything
/// interpolated is escaped first: these strings come off the network, and a note
/// containing a stray `<` would otherwise turn the label into a parse error and
/// blank the changelog.
///
/// Code spans carry a second syntax that is Roblox's rather than markdown's:
/// `Class.Decal.Rotation` is a link to that property and
/// `Class.Decal.Rotation|Rotation` is the same link with its label after the
/// pipe. [`display_of`] reduces those, and it runs only inside a code span --
/// which is also what keeps the literal in "replaced with `**error-type**`"
/// intact, since the parser hands that back as code rather than as emphasis.
pub fn markdown_to_pango(text: &str) -> String {
    use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

    let mut out = String::with_capacity(text.len());
    for event in Parser::new_ext(text, Options::empty()) {
        match event {
            Event::Text(t) => out.push_str(&escape(&t)),
            Event::Code(c) => {
                out.push_str("<tt>");
                out.push_str(&escape(&display_of(&c)));
                out.push_str("</tt>");
            }
            Event::Start(Tag::Strong) => out.push_str("<b>"),
            Event::End(TagEnd::Strong) => out.push_str("</b>"),
            Event::Start(Tag::Emphasis) => out.push_str("<i>"),
            Event::End(TagEnd::Emphasis) => out.push_str("</i>"),
            Event::Start(Tag::Strikethrough) => out.push_str("<s>"),
            Event::End(TagEnd::Strikethrough) => out.push_str("</s>"),
            // A note is one paragraph. Anything block-level collapses to a
            // space rather than a newline, because these are bullet items in a
            // list this crate is building itself.
            Event::SoftBreak | Event::HardBreak => out.push(' '),
            Event::End(TagEnd::Paragraph) => out.push(' '),
            // Raw HTML in a note is shown as the characters it is made of, not
            // dropped and not interpreted. Dropping it silently loses whatever
            // the note said — a bullet mentioning a `<script>` tag came out
            // with a hole in the middle of the sentence — and interpreting it
            // would let a fetched string choose the markup in a widget.
            Event::Html(h) | Event::InlineHtml(h) => out.push_str(&escape(&h)),
            // Links keep their words and lose their destination: the window has
            // no browser in it and a bare URL in a bullet is noise.
            _ => {}
        }
    }
    out.trim().to_string()
}

/// The three characters Pango's parser treats as markup.
///
/// `&` first, or the escapes introduced by the other two get escaped again.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// `Class.Decal.Rotation|Rotation` -> `Rotation`; `Class.Decal` -> `Decal`.
fn display_of(reference: &str) -> String {
    if let Some((_, label)) = reference.split_once('|') {
        return label.to_string();
    }
    // Only the documented prefixes are unwrapped. A backticked `true` or a bare
    // code sample has no dots to strip and must survive untouched.
    const PREFIXES: [&str; 6] =
        ["Class.", "Enum.", "Datatype.", "Global.", "Library.", "Property."];
    if PREFIXES.iter().any(|p| reference.starts_with(p)) {
        if let Some(last) = reference.rsplit('.').next() {
            return last.to_string();
        }
    }
    reference.to_string()
}

/// The table as Pango markup, grouped the way Roblox groups it.
///
/// Improvements first and Fixes after, each in the order published, with
/// `Pending` marked because the distinction is the useful part: a Live note
/// describes the build you can be running now and a Pending one does not.
///
/// **Markup, not text.** `Entry::text` is already Pango markup from
/// [`markdown_to_pango`], so everything this adds around it — the headings, the
/// status suffix — is escaped on the way in and the whole string has to go to a
/// label with `use_markup` set. Handing this to one without it shows the tags.
pub fn render_entries(entries: &[Entry]) -> String {
    let mut out = String::new();
    let mut kinds: Vec<&str> = Vec::new();
    for e in entries {
        if !kinds.contains(&e.kind.as_str()) {
            kinds.push(&e.kind);
        }
    }
    // Improvements before Fixes when both are present, and anything Roblox adds
    // later after them in the order it arrived, rather than dropped.
    kinds.sort_by_key(|k| match *k {
        "Improvements" => 0,
        "Fixes" => 1,
        _ => 2,
    });

    for kind in kinds {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!("<b>{}</b>\n", escape(kind)));
        for e in entries.iter().filter(|e| e.kind == kind) {
            if e.status.eq_ignore_ascii_case("Live") || e.status.is_empty() {
                out.push_str(&format!("  • {}\n", e.text));
            } else {
                out.push_str(&format!(
                    "  • {} <i>({})</i>\n",
                    e.text,
                    escape(&e.status)
                ));
            }
        }
    }
    out.trim_end().to_string()
}

/// Tags out, the five XML entities back in.
fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut depth = 0usize;
    for c in html.chars() {
        match c {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    let out = out
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    // Discourse emits a `<br>` per line, so what is left is a run of blank
    // lines where the markup was.
    out.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed from the real listing fetched on 2026-08-02, keeping the pinned
    /// category post because dropping it is one of the things being tested.
    const LISTING: &str = r#"{"topic_list":{"topics":[
        {"id":60921,"slug":"about-the-release-notes-category",
         "title":"About the Release Notes category","created_at":"2019-01-01T00:00:00.000Z"},
        {"id":4754888,"slug":"release-notes-for-731",
         "title":"Release Notes for 731","created_at":"2026-07-22T18:00:00.000Z"},
        {"id":4763851,"slug":"release-notes-for-732",
         "title":"Release Notes for 732","created_at":"2026-07-29T18:44:52.923Z"}
    ]}}"#;

    /// Trimmed from the real response to
    /// `.../_next/data/<buildId>/updates/2026-07-27.json`, fetched 2026-08-28.
    /// Four of the real fifteen entries, kept whole rather than edited: entry
    /// zero is the same `Class.Decal.Rotation|Rotation` note this file has
    /// quoted since 2026-08-03, when it was read out of the old HTML scrape --
    /// Roblox's own text has outlived the two page shapes this module has
    /// parsed it from. The wrapper levels are kept deliberately: walking by
    /// shape rather than by path in [`collect_entries`] is the thing being
    /// tested, because that nesting is the framework's to rearrange.
    const DOCS_PAGE: &str = r#"{"pageProps":{"data":{"releaseNoteContents":{"title":"Week of July 27, 2026","lastUpdated":"2026-08-25T21:10:17.410Z","content":[
        {"ReleaseNotesText":"Adds the `Class.Decal.Rotation|Rotation` property to `Class.Texture` and `Class.Decal` instances to support UV map rotations.","ReleaseNotesType":"Improvements","Status":"Live"},
        {"ReleaseNotesText":"Adds `Class.InputBinding.DisplayName` and `Class.InputBinding.DisplayImage` in service of `Class.InputActionLabel`. Also adds the read-only `Class.InputAction.PreferredBinding` property for creators who want full control over how bindings are displayed in custom UI.","ReleaseNotesType":"Improvements","Status":"Live"},
        {"ReleaseNotesText":"Fixes `Class.ImageHandleAdornment` edge sampling.","ReleaseNotesType":"Fixes","Status":"Live"},
        {"ReleaseNotesText":"Fixes night sky star twinkle rate at high FPS.","ReleaseNotesType":"Fixes","Status":"Live"}
    ],"prev":"/updates/2026-07-20"}}}}"#;

    fn docs_page() -> serde_json::Value {
        serde_json::from_str(DOCS_PAGE).unwrap()
    }

    #[test]
    fn the_entries_come_out_of_the_per_page_json_directly() {
        // No `__NEXT_DATA__` script and no rendered table to fall back to any
        // more -- the endpoint this module asks for now answers with exactly
        // this, and nothing else, which is the whole improvement over the HTML
        // scrape it replaced.
        let entries = parse_docs_notes(&docs_page()).unwrap();
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].kind, "Improvements");
        assert_eq!(entries[0].status, "Live");
        assert_eq!(entries[3].kind, "Fixes");
    }

    #[test]
    fn the_rendering_groups_improvements_first() {
        let out = render_entries(&parse_docs_notes(&docs_page()).unwrap());
        let improvements = out.find("Improvements").expect("no Improvements heading");
        let fixes = out.find("Fixes").expect("no Fixes heading");
        assert!(improvements < fixes, "{out}");
        assert!(out.starts_with("<b>Improvements</b>"), "{out}");
    }

    #[test]
    fn the_rendering_marks_whatever_is_not_live_yet() {
        // Every dated page measured 2026-08-28 answered `Live` throughout -- the
        // `Pending` distinction now lives on `/updates/pending`, in a shape with
        // no per-entry status at all -- so `DOCS_PAGE` cannot exercise this any
        // more and these are constructed directly instead, to keep the marking
        // logic itself covered against the day a dated page carries one again.
        let entries = vec![
            Entry { text: "Shipped already.".into(), kind: "Improvements".into(), status: "Live".into() },
            Entry { text: "Not out yet.".into(), kind: "Improvements".into(), status: "Pending".into() },
        ];
        let out = render_entries(&entries);
        // Pending is the useful distinction: a Live note describes the build
        // you can be running now and a Pending one does not. Live is unmarked
        // because marking the ordinary case is noise on every line.
        assert!(out.contains("Not out yet. <i>(Pending)</i>"), "{out}");
        assert!(!out.contains("(Live)"), "{out}");
    }

    #[test]
    fn markdown_becomes_pango_markup_and_a_code_span_keeps_its_stars() {
        // Both of these are real 732 notes, shortened. The second is why a real
        // parser replaced the hand-rolled marker stripping: `**error-type**` is
        // the literal string the bug produced, and the parser hands it back as
        // code rather than as emphasis, so it survives whole.
        assert_eq!(
            markdown_to_pango("Adds a **Use Bounding Boxes** setting for the draggers."),
            "Adds a <b>Use Bounding Boxes</b> setting for the draggers."
        );
        assert_eq!(
            markdown_to_pango("Fixes a bug where types were replaced with `**error-type**`."),
            "Fixes a bug where types were replaced with <tt>**error-type**</tt>."
        );
        // The case the old pairing rule got wrong: it searched for the next
        // `**`, matched the stars in an exponent against an unrelated marker
        // later in the line, and deleted everything between them.
        assert_eq!(markdown_to_pango("A 2 ** 8 shift"), "A 2 ** 8 shift");
    }

    #[test]
    fn a_note_off_the_network_cannot_break_the_label_it_is_drawn_into() {
        // These strings come off the network and land in a widget that parses
        // markup. One stray `<` in a note would otherwise make Pango reject the
        // whole block, which blanks the changelog rather than mangling one line.
        assert_eq!(
            markdown_to_pango("Fixes `a < b` when a & b are <script> tags"),
            "Fixes <tt>a &lt; b</tt> when a &amp; b are &lt;script&gt; tags"
        );
        // An entity in the source round-trips: markdown decodes `&lt;` to a
        // literal `<`, and the escape puts it back, so the label renders the
        // `<` the note meant rather than the six characters that spelled it.
        assert_eq!(markdown_to_pango("a &lt; b"), "a &lt; b");
        // `&` is escaped before the angle brackets, or the escapes those two
        // introduce get escaped a second time and show as `&amp;lt;`.
        assert_eq!(escape("<&>"), "&lt;&amp;&gt;");
    }

    #[test]
    fn the_docs_cross_reference_syntax_survives_the_markdown_parser() {
        let entries = parse_docs_notes(&docs_page()).unwrap();
        // The reference reduction runs inside code spans, so it still applies
        // once markdown owns the parse -- and what comes out is a `<tt>` run
        // rather than bare words.
        assert_eq!(
            entries[0].text,
            "Adds the <tt>Rotation</tt> property to <tt>Texture</tt> and <tt>Decal</tt> \
             instances to support UV map rotations."
        );
        assert_eq!(entries[2].text, "Fixes <tt>ImageHandleAdornment</tt> edge sampling.");
    }

    #[test]
    fn a_page_that_changed_shape_fails_loudly_rather_than_showing_an_empty_changelog() {
        // The failure that matters. If Roblox renames the fields or moves them
        // somewhere `collect_entries` does not reach, an empty list reads
        // exactly like a release with nothing in it, and the window would
        // quietly claim Roblox published no changes.
        assert!(parse_docs_notes(&serde_json::json!({"pageProps": {"data": {}}})).is_err());
        assert!(parse_docs_notes(&serde_json::json!(null)).is_err());
    }

    /// The live service, on request only.
    ///
    /// `cargo test -p cordial-update -- --ignored --nocapture` prints what the
    /// window would show. Ignored by default because it is three network
    /// requests and the rest of this file is pinned to captured shapes -- but
    /// the whole point of this module is a service somebody else controls, and
    /// a fixture cannot tell you the day they rename a field.
    #[test]
    #[ignore = "fetches create.roblox.com"]
    fn the_real_service_still_answers() {
        let (source, entries) = docs_notes().expect("the current release");
        println!("{source:?} -- {} entries", entries.len());
        println!("{}", render_entries(&entries));
        assert!(!entries.is_empty());
        // Every entry has to survive the parse with something in it: an entry
        // that renders empty is what a changed field name looks like from here.
        assert!(entries.iter().all(|e| !e.text.trim().is_empty()));
    }

    #[test]
    fn a_data_url_is_the_build_id_and_the_site_path_joined() {
        assert_eq!(
            docs_data_url("LkiE_NqiUYTmYBZV17_Xx", "/updates"),
            "https://create.roblox.com/docs/_next/data/LkiE_NqiUYTmYBZV17_Xx/updates.json"
        );
        assert_eq!(
            docs_data_url("LkiE_NqiUYTmYBZV17_Xx", "/updates/2026-08-24"),
            "https://create.roblox.com/docs/_next/data/LkiE_NqiUYTmYBZV17_Xx/updates/2026-08-24.json"
        );
    }

    /// The tail of the real shell page's `__NEXT_DATA__`, fetched 2026-08-28.
    /// The rest of that blob is the whole site's navigation tree -- over
    /// 100 kB -- and none of it is the field this is pinning.
    const DOCS_SHELL_TAIL: &str = r#"{"props":{"pageProps":{}},"page":"/updates","query":{},"buildId":"LkiE_NqiUYTmYBZV17_Xx","assetPrefix":"https://assets.create.roblox.com/docs/93e5457f3047d1f35c22c74cf428dec3fa9f2006","isFallback":false,"isExperimentalCompile":false,"gsp":true,"scriptLoader":[]}"#;

    #[test]
    fn the_build_id_is_read_out_of_the_shells_next_data() {
        let html = format!(
            r#"<!doctype html><html><body><script id="__NEXT_DATA__" type="application/json">{DOCS_SHELL_TAIL}</script></body></html>"#
        );
        assert_eq!(find_build_id(&html).as_deref(), Some("LkiE_NqiUYTmYBZV17_Xx"));
    }

    #[test]
    fn a_shell_with_no_build_id_is_a_named_failure_not_a_panic() {
        assert_eq!(find_build_id("<html><body>no script here</body></html>"), None);
    }

    /// Trimmed from the real response to
    /// `.../_next/data/<buildId>/updates.json`, fetched 2026-08-28: the
    /// "Release notes" section of the navigation, with `Recent releases` cut
    /// to two of its real nine entries. `Current release` and the `null` path
    /// on `Recent releases` itself are both real and both worth keeping --
    /// the second is what [`collect_nav_items`] has to skip rather than
    /// mistake for a release.
    const DOCS_NAV: &str = r#"{"pageProps":{"navigation":{"navigationContent":[
        {"heading":"Release notes","navigation":[
            {"title":"Current release","path":"/updates/2026-08-24"},
            {"title":"Pending release","path":"/updates/pending"},
            {"title":"Recent releases","path":null,"section":[
                {"title":"Week of August 17, 2026","path":"/updates/2026-08-17"},
                {"title":"Week of August 10, 2026","path":"/updates/2026-08-10"}
            ]}
        ]}
    ]}}}"#;

    #[test]
    fn pick_release_finds_current_release_by_name() {
        let nav = serde_json::from_str(DOCS_NAV).unwrap();
        let (source, path) = pick_release(&nav).unwrap();
        assert_eq!(source, DocsSource::CurrentRelease);
        assert_eq!(path, "/updates/2026-08-24");
    }

    #[test]
    fn pick_release_falls_back_to_the_newest_dated_entry_if_the_label_is_gone() {
        // Roblox renaming the label is the hazard this exists for -- not a
        // hypothetical, since the DevForum-keyed route this replaced was itself
        // a rename. `Current release` is edited out here to force the path
        // this function must not treat as a failure.
        let without_label = DOCS_NAV.replace("Current release", "This week");
        let nav = serde_json::from_str(&without_label).unwrap();
        let (source, path) = pick_release(&nav).unwrap();
        assert_eq!(source, DocsSource::Newest { date: "2026-08-24".into() });
        assert_eq!(path, "/updates/2026-08-24");
    }

    #[test]
    fn pick_release_never_falls_back_to_the_pending_page() {
        // `/updates/pending` is not a date and must never win the "newest"
        // comparison just because it sorts after every real date as a string.
        let without_label = DOCS_NAV.replace("Current release", "This week");
        let nav = serde_json::from_str(&without_label).unwrap();
        let (_, path) = pick_release(&nav).unwrap();
        assert_ne!(path, "/updates/pending");
    }

    #[test]
    fn a_navigation_with_no_dates_at_all_is_a_named_failure() {
        let nav = serde_json::json!({"pageProps": {"navigation": {"navigationContent": []}}});
        assert!(pick_release(&nav).is_err());
    }

    #[test]
    fn the_listing_shape_is_the_one_that_was_observed() {
        let releases = parse_releases(&serde_json::from_str(LISTING).unwrap()).unwrap();
        assert_eq!(releases.len(), 2, "the category's own About post is not a release");
        assert_eq!(releases[0].major, 732);
        assert_eq!(releases[0].id, 4763851);
        assert_eq!(releases[0].slug, "release-notes-for-732");
        assert_eq!(releases[1].major, 731);
    }

    #[test]
    fn a_topic_url_carries_both_slug_and_id() {
        // `/t/<id>.json` answers 403. Measured, and not obvious enough to be
        // left to whoever next edits this line.
        let releases = parse_releases(&serde_json::from_str(LISTING).unwrap()).unwrap();
        assert_eq!(
            releases[0].url(),
            "https://devforum.roblox.com/t/release-notes-for-732/4763851.json"
        );
        assert!(!releases[0].web_url().ends_with(".json"));
    }

    #[test]
    fn a_listing_with_nothing_recognisable_is_a_named_failure() {
        // Not an empty list. "Roblox retitled their release notes" and "there
        // are no release notes" have to look different, or the button says
        // "up to date" forever.
        let e = parse_releases(&serde_json::json!({"topic_list":{"topics":[
            {"id":1,"slug":"x","title":"Something else entirely"}
        ]}}))
        .unwrap_err();
        assert!(e.contains("Release Notes"), "{e}");
    }

    #[test]
    fn a_reshaped_listing_is_refused_rather_than_read_as_empty() {
        assert!(parse_releases(&serde_json::json!({"topics":[]})).is_err());
    }

    #[test]
    fn the_topic_shape_is_the_one_that_was_observed() {
        let topic = serde_json::json!({
            "title": "Release Notes for 732",
            "created_at": "2026-07-29T18:44:52.923Z",
            "post_stream": {"posts": [{"cooked": "<p>Hi all,<br>\nPleased to announce that 732 has landed. Enjoy!</p>"}]}
        });
        let notes = parse_notes(&topic).unwrap();
        assert_eq!(notes.title, "Release Notes for 732");
        assert!(notes.html.starts_with("<p>"));
        assert_eq!(notes.text(), "Hi all,\nPleased to announce that 732 has landed. Enjoy!");
    }

    #[test]
    fn the_plain_rendering_drops_markup_and_restores_entities() {
        let notes = Notes {
            title: String::new(),
            created_at: String::new(),
            html: "<p>Fixed <code>a &lt; b &amp;&amp; c</code></p>".into(),
        };
        assert_eq!(notes.text(), "Fixed a < b && c");
    }

    #[test]
    fn the_major_is_the_number_in_the_title() {
        assert_eq!(major_in_title("Release Notes for 732"), Some(732));
        assert_eq!(major_in_title("Release Notes for Release 645"), Some(645));
        assert_eq!(major_in_title("About the Release Notes category"), None);
    }
}
