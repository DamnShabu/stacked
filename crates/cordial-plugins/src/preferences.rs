//! What a plugin declares it wants asked, and the answers Cordial keeps.
//!
//! A plugin cannot draw. It runs in its own process with no display, no
//! toolkit and no handle on anything Cordial has on screen (ADR-003), so the
//! GNOME Shell arrangement — where an extension opens a window and builds it
//! itself — is not available even in principle. What is available is the shape
//! ADR-007 already uses everywhere else: the plugin describes what it wants and
//! Cordial performs it. Here the effect is a preferences page.
//!
//! So a plugin declares fields in its `plugin.json` and Cordial renders an
//! `AdwPreferencesPage` from the declaration. The page looks native because it
//! *is* native, built by the launcher out of the same rows every other page in
//! Settings uses. See
//! [ADR-020](../../../docs/adr/ADR-020-declarative-plugin-preferences.md).
//!
//! ```json
//! {
//!   "id": "example",
//!   "entry": "main.ts",
//!   "preferences": [
//!     { "key": "loud", "type": "bool", "title": "Be loud", "default": false },
//!     { "key": "level", "type": "int", "title": "Level", "default": 3,
//!       "minimum": 1, "maximum": 10, "group": "Tuning" }
//!   ]
//! }
//! ```
//!
//! **The declaration is the signal.** There is no `has-preferences` manifest key
//! and no capability meaning "I have a settings page", because two facts that
//! can disagree eventually do: a plugin would ship the key and no fields, or
//! fields and no key, and one of those shows the user a gear that opens nothing.
//! A plugin has a page exactly when it declares at least one field, which cannot
//! be wrong about itself.
//!
//! **Cordial owns the answers, and this is a different document from
//! `settings.json`.** [`crate::settings`] holds what a plugin chooses to
//! remember, and the plugin is its only writer — `settings.set` replaces it
//! whole, which is right for scratch state and fatal for anything a user typed.
//! Two writers of one document, one of whom replaces it wholesale, loses the
//! user's answers the first time the plugin saves anything. So preferences live
//! beside it at `<profile>/plugins/<id>/preferences.json`, Cordial is the only
//! writer, and there is deliberately no `preferences.set` for a plugin to call.
//! A plugin that could rewrite its own preference values could set them to
//! whatever it liked and the page would show the plugin's choice back to the
//! user as though they had made it.
//!
//! Per profile for the reason everything else in ADR-013 is: the same installed
//! plugin, tuned one way on a test account and another way on the account
//! somebody plays, must not carry one profile's answers into the other.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// How many fields one plugin may declare.
///
/// A preferences page is a page a person reads. Past a certain length that
/// stops being true, and the failure is not a crash — it is a wall of switches
/// nobody can find anything in. The cap is also what stops a malformed or
/// generated manifest from asking Cordial to build ten thousand widgets on the
/// GTK thread, which would present as the launcher hanging on a button press.
pub const MAX_FIELDS: usize = 64;

/// How long a `text` answer may be.
///
/// Not a security boundary — [`Store::write`] caps the whole document as well.
/// It exists so a single-line `AdwEntryRow` cannot be pasted into until the
/// document is megabytes of one string, which is the ordinary accident rather
/// than the hostile one.
pub const MAX_TEXT_BYTES: usize = 4 * 1024;

/// One declared field, as it appears in `plugin.json`.
///
/// `title` is what the row says and `description` is its subtitle. Both are the
/// plugin author's words shown in Cordial's own window, which is the reason
/// [`Declaration::check`] refuses control characters in them: a title carrying
/// a newline draws over the row beneath it, and a row that can draw outside
/// itself is the beginning of the thing ADR-020 exists to prevent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Declaration {
    pub key: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// Which `AdwPreferencesGroup` this row joins. Fields with no group go in
    /// an untitled group at the top, which is what libadwaita does with a
    /// group that has no title anyway.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(flatten)]
    pub field: Field,
}

/// The four shapes a field can have, and the row each becomes.
///
/// Four rather than "whatever the plugin sends" because every variant here is a
/// widget Cordial has to build, and a type nobody can render is a type that
/// cannot be declared. Adding one is a change to this enum *and* to the
/// renderer, together, which is the property that keeps the two from drifting
/// apart — a schema that can express something the page cannot draw would
/// present to the user as a field that silently does not appear.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Field {
    /// `AdwSwitchRow`.
    Bool {
        #[serde(default)]
        default: bool,
    },
    /// `AdwSpinRow`. `minimum` and `maximum` bound it; without them the row
    /// still needs an adjustment, so [`Field::bounds`] supplies a range rather
    /// than leaving the renderer to invent one per call site.
    Int {
        #[serde(default)]
        default: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        minimum: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        maximum: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        step: Option<i64>,
    },
    /// `AdwComboRow`, one entry per option.
    Choice {
        default: String,
        options: Vec<Choice>,
    },
    /// `AdwEntryRow`.
    Text {
        #[serde(default)]
        default: String,
    },
}

/// One entry of a [`Field::Choice`]: the value stored, and the words shown.
///
/// Split because they are not the same thing and conflating them is a
/// translation and a rename waiting to break saved answers. `value` is what
/// lands in the document and what the plugin's code compares against; `label`
/// is prose, and changing it must not silently reset everybody's choice.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Choice {
    pub value: String,
    pub label: String,
}

impl Field {
    /// The value this field takes when nothing has been saved, or when what was
    /// saved no longer fits the declaration.
    pub fn default_value(&self) -> serde_json::Value {
        match self {
            Field::Bool { default } => serde_json::Value::Bool(*default),
            Field::Int { default, .. } => serde_json::Value::from(*default),
            Field::Choice { default, .. } => serde_json::Value::String(default.clone()),
            Field::Text { default } => serde_json::Value::String(default.clone()),
        }
    }

    /// The spin range for an `Int`, with the declaration's own default inside
    /// it. `None` for every other shape.
    ///
    /// An unbounded integer still has to be given an adjustment, and picking
    /// the bound here rather than in the renderer means the value a plugin can
    /// be handed and the value the row can produce are decided in one place. A
    /// declaration with no `minimum` gets a range wide enough to be no
    /// practical constraint and narrow enough that the row's own arithmetic
    /// stays in `i32`, which is what GTK's adjustment reduces to on the way to
    /// the screen.
    pub fn bounds(&self) -> Option<(i64, i64)> {
        let Field::Int { default, minimum, maximum, .. } = self else {
            return None;
        };
        let low = minimum.unwrap_or(i64::from(i32::MIN)).min(*default);
        let high = maximum.unwrap_or(i64::from(i32::MAX)).max(*default);
        Some((low, high))
    }

    /// Whether `value` is something this field could have produced.
    ///
    /// Deliberately strict about JSON types. A `true` where an integer belongs
    /// is not coerced to `1`, because the plugin reading the document is
    /// entitled to assume the type it declared, and a coercion here is a
    /// surprise there.
    pub fn accepts(&self, value: &serde_json::Value) -> bool {
        match self {
            Field::Bool { .. } => value.is_boolean(),
            Field::Int { minimum, maximum, .. } => {
                let Some(n) = value.as_i64() else { return false };
                minimum.is_none_or(|lo| n >= lo) && maximum.is_none_or(|hi| n <= hi)
            }
            Field::Choice { options, .. } => value
                .as_str()
                .is_some_and(|s| options.iter().any(|o| o.value == s)),
            Field::Text { .. } => value.as_str().is_some_and(|s| s.len() <= MAX_TEXT_BYTES),
        }
    }

    /// The name used in errors and in the manifest, so a message about a bad
    /// declaration quotes the word the author actually typed.
    pub fn type_name(&self) -> &'static str {
        match self {
            Field::Bool { .. } => "bool",
            Field::Int { .. } => "int",
            Field::Choice { .. } => "choice",
            Field::Text { .. } => "text",
        }
    }
}

/// Whether a string may be a preference key.
///
/// The same alphabet [`crate::manifest::is_valid_id`] allows, and for a related
/// reason: a key is a JSON object key that Cordial writes, a plugin reads, and
/// a person may well have to type into a bug report. It never becomes a path
/// component — the document is one file per plugin — so this is about being
/// legible rather than about traversal.
pub fn is_valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 64
        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Text that can be drawn in a row without escaping it.
///
/// A newline in a title makes one row overlap the next, and a control character
/// can do stranger things depending on the font. Refused at parse rather than
/// stripped at render, so the plugin author is told rather than quietly
/// corrected into something they did not write.
fn is_drawable(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 200
        && !text.chars().any(|c| c.is_control())
}

impl Declaration {
    /// Whether this one declaration is coherent on its own.
    ///
    /// Uniqueness across a plugin's whole list is [`check_all`]'s job, because
    /// it is the only thing here that one field cannot know about itself.
    pub fn check(&self) -> Result<(), String> {
        if !is_valid_key(&self.key) {
            return Err(format!(
                "preference key {:?} may only contain letters, digits, dashes and underscores",
                self.key
            ));
        }
        if !is_drawable(&self.title) {
            return Err(format!(
                "preference {:?} needs a title of at most 200 characters and no line breaks",
                self.key
            ));
        }
        if !self.description.is_empty() && !is_drawable(&self.description) {
            return Err(format!(
                "preference {:?} has a description with a line break or control character in it",
                self.key
            ));
        }
        if let Some(group) = &self.group {
            if !is_drawable(group) {
                return Err(format!(
                    "preference {:?} names a group with a line break or control character in it",
                    self.key
                ));
            }
        }
        match &self.field {
            Field::Int { default, minimum, maximum, step } => {
                if let (Some(lo), Some(hi)) = (minimum, maximum) {
                    if lo > hi {
                        return Err(format!(
                            "preference {:?} has minimum {lo} above maximum {hi}",
                            self.key
                        ));
                    }
                }
                // A default outside its own range is the mistake that would
                // otherwise show a row already holding an impossible value, or
                // silently clamp on first open and save something the author
                // never chose.
                if !self.field.accepts(&serde_json::Value::from(*default)) {
                    return Err(format!(
                        "preference {:?} defaults to {default}, which is outside its own range",
                        self.key
                    ));
                }
                if let Some(step) = step {
                    if *step <= 0 {
                        return Err(format!(
                            "preference {:?} has a step of {step}; it must be positive",
                            self.key
                        ));
                    }
                }
            }
            Field::Choice { default, options } => {
                if options.is_empty() {
                    return Err(format!("preference {:?} is a choice with no options", self.key));
                }
                let mut seen = BTreeSet::new();
                for option in options {
                    if !seen.insert(&option.value) {
                        return Err(format!(
                            "preference {:?} offers {:?} twice",
                            self.key, option.value
                        ));
                    }
                    if !is_drawable(&option.label) {
                        return Err(format!(
                            "preference {:?} has an option label with a line break or control \
                             character in it",
                            self.key
                        ));
                    }
                }
                if !options.iter().any(|o| &o.value == default) {
                    return Err(format!(
                        "preference {:?} defaults to {default:?}, which is not one of its options",
                        self.key
                    ));
                }
            }
            Field::Text { default } => {
                if default.len() > MAX_TEXT_BYTES {
                    return Err(format!(
                        "preference {:?} has a default longer than {MAX_TEXT_BYTES} bytes",
                        self.key
                    ));
                }
                if default.chars().any(|c| c.is_control()) {
                    return Err(format!(
                        "preference {:?} has a control character in its default",
                        self.key
                    ));
                }
            }
            Field::Bool { .. } => {}
        }
        Ok(())
    }
}

/// Whether a whole declaration list is usable, refusing rather than pruning.
///
/// A bad field is refused with the plugin, the same way
/// [`crate::manifest::parse`] refuses an unknown capability rather than
/// skipping it. Pruning would install a plugin whose page is quietly missing
/// the row it needs most, and the author would be debugging their own code.
pub fn check_all(fields: &[Declaration]) -> Result<(), String> {
    if fields.len() > MAX_FIELDS {
        return Err(format!(
            "{} preferences declared; the limit is {MAX_FIELDS}",
            fields.len()
        ));
    }
    let mut seen = BTreeSet::new();
    for field in fields {
        field.check()?;
        if !seen.insert(&field.key) {
            return Err(format!("preference key {:?} is declared twice", field.key));
        }
    }
    Ok(())
}

/// The complete, valid set of answers for `fields`, given whatever is saved.
///
/// Every declared key is present and every value fits its declaration, so a
/// plugin never has to write `?? default` and never has to guard against a type
/// it did not declare. That is most of what the declaration buys the author:
/// the parsing, the range check and the fallback are done once here rather than
/// once in every plugin, in whatever way each author thought of.
///
/// A saved value that no longer fits — because the plugin was updated and
/// narrowed a range, or renamed an option — falls back to the current default
/// and is **reported**, not silently corrected. A preference that quietly
/// reverts is indistinguishable from one that never saved.
///
/// Keys in `saved` that nothing declares are dropped. They belong to a field
/// the plugin has removed, and carrying them forever would mean the document
/// only ever grows.
pub fn effective(
    fields: &[Declaration],
    saved: &serde_json::Value,
    report: &mut dyn FnMut(&str),
) -> BTreeMap<String, serde_json::Value> {
    let mut out = BTreeMap::new();
    for field in fields {
        let value = match saved.get(&field.key) {
            Some(v) if field.field.accepts(v) => v.clone(),
            Some(v) => {
                report(&format!(
                    "{:?} was saved as {v}, which no longer fits its {} declaration; using the \
                     default instead",
                    field.key,
                    field.field.type_name()
                ));
                field.field.default_value()
            }
            None => field.field.default_value(),
        };
        out.insert(field.key.clone(), value);
    }
    out
}

/// Cordial's hold on every plugin's answers within one profile.
///
/// Deliberately shaped like [`crate::settings::Store`] rather than merged into
/// it: same directory, same atomic write, different file and — the part that
/// matters — a different writer. That one is the plugin's, this one is
/// Cordial's.
#[derive(Debug, Clone)]
pub struct Store {
    profile_dir: PathBuf,
}

/// How large a preferences document may be.
///
/// Smaller than `settings.json`'s megabyte because this document is bounded by
/// construction: at most [`MAX_FIELDS`] keys, each at most [`MAX_TEXT_BYTES`].
/// The cap is the backstop for a file edited by hand or left behind by an
/// older, wider declaration, so that a page opening never has to parse
/// something enormous on the GTK thread.
const MAX_BYTES: usize = 512 * 1024;

impl Store {
    pub fn new(profile_dir: impl Into<PathBuf>) -> Self {
        Store { profile_dir: profile_dir.into() }
    }

    pub fn profile_dir(&self) -> &Path {
        &self.profile_dir
    }

    /// Where `plugin_id`'s answers live, refusing an id that could name
    /// anything else.
    ///
    /// Checked here as well as upstream for the reason
    /// [`crate::settings::Store::path_for`] gives: this is the function that
    /// turns a string into a path, and a check kept somewhere else is a check
    /// a later caller can skip without noticing.
    pub fn path_for(&self, plugin_id: &str) -> Result<PathBuf, String> {
        if !crate::manifest::is_valid_id(plugin_id) {
            return Err(format!(
                "{plugin_id:?} is not a usable plugin id, so it has no preferences"
            ));
        }
        Ok(self
            .profile_dir
            .join("plugins")
            .join(plugin_id)
            .join("preferences.json"))
    }

    /// What is saved for this plugin, or an empty document.
    ///
    /// A missing file is a first launch, not a failure. A present but
    /// unreadable one is reported, for [`crate::settings::Store::read`]'s
    /// reason: answering "you have nothing saved" invites the caller to write a
    /// fresh document straight over whatever the user actually had.
    pub fn read(&self, plugin_id: &str) -> Result<serde_json::Value, String> {
        let path = self.path_for(plugin_id)?;
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(serde_json::json!({}))
            }
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(v) if v.is_object() => Ok(v),
            Ok(_) => Err(format!("{} is not a JSON object", path.display())),
            Err(e) => Err(format!("{} is not usable ({e})", path.display())),
        }
    }

    /// The answers a plugin should be handed: saved where valid, declared
    /// defaults everywhere else.
    ///
    /// The one call both the renderer and the plugin host should use, so the
    /// page a user is looking at and the document the plugin was given cannot
    /// disagree about what a preference currently is.
    pub fn effective_for(
        &self,
        plugin_id: &str,
        fields: &[Declaration],
    ) -> Result<BTreeMap<String, serde_json::Value>, String> {
        let saved = self.read(plugin_id)?;
        let mut out = Vec::new();
        let values = effective(fields, &saved, &mut |line| out.push(line.to_string()));
        for line in out {
            println!("  plugin {plugin_id}: preference {line}");
        }
        Ok(values)
    }

    /// Save one answer, leaving every other key alone.
    ///
    /// Read-modify-write rather than a whole-document replace, which is the
    /// opposite of what [`crate::settings::Store::write`] does and is right for
    /// the opposite reason: there, the plugin knows the complete state it means
    /// to leave behind. Here the writer is one row in a page, it knows about
    /// exactly one key, and replacing the document would discard every other
    /// answer the moment a switch was flipped.
    ///
    /// Refuses a value the declaration would not accept, so the file can never
    /// hold something [`effective`] will only report and discard on the next
    /// read.
    pub fn set(
        &self,
        plugin_id: &str,
        fields: &[Declaration],
        key: &str,
        value: serde_json::Value,
    ) -> Result<(), String> {
        let Some(field) = fields.iter().find(|f| f.key == key) else {
            return Err(format!("{plugin_id:?} declares no preference called {key:?}"));
        };
        if !field.field.accepts(&value) {
            return Err(format!(
                "{value} is not a usable value for the {} preference {key:?}",
                field.field.type_name()
            ));
        }

        // Read the file rather than a cached copy: the page may have been open
        // while something else wrote, and losing an answer because a stale map
        // was written back is precisely the failure this method's shape exists
        // to avoid.
        let mut document = match self.read(plugin_id) {
            Ok(d) => d,
            // A document too broken to parse is replaced rather than refused.
            // The alternative is a preferences page that cannot be used at all
            // until somebody finds the file and deletes it by hand, and the
            // answers in an unparseable file are not recoverable anyway. Said
            // out loud so it is not discovered as data loss.
            Err(why) => {
                println!("  plugin {plugin_id}: starting a fresh preferences document ({why})");
                serde_json::json!({})
            }
        };
        let Some(object) = document.as_object_mut() else {
            return Err("preferences must be a JSON object".into());
        };
        object.insert(key.to_string(), value);

        // Keys nothing declares are dropped on the way out, so the document
        // does not accumulate the leavings of every field the plugin has ever
        // had. `effective` already ignores them; this is what stops the file
        // itself from growing forever.
        let declared: BTreeSet<&str> = fields.iter().map(|f| f.key.as_str()).collect();
        object.retain(|k, _| declared.contains(k.as_str()));

        self.write_document(plugin_id, &document)
    }

    /// Forget every answer for this plugin, so the page returns to its
    /// declared defaults.
    ///
    /// Deleting the file rather than writing the defaults into it: a plugin
    /// that later changes a default should govern a preference the user never
    /// set, and a file full of yesterday's defaults would pin it to the old
    /// one with nothing to show why.
    pub fn reset(&self, plugin_id: &str) -> Result<(), String> {
        let path = self.path_for(plugin_id)?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    fn write_document(&self, plugin_id: &str, document: &serde_json::Value) -> Result<(), String> {
        let text = serde_json::to_string_pretty(document).map_err(|e| e.to_string())?;
        if text.len() > MAX_BYTES {
            return Err(format!(
                "preferences are {} bytes; the limit is {MAX_BYTES}",
                text.len()
            ));
        }
        let path = self.path_for(plugin_id)?;
        let dir = path.parent().expect("path_for always joins at least one component");
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        // Written alongside and renamed, the same as every other per-plugin
        // document here: a launcher killed mid-write must leave the previous
        // valid answers rather than a half-file that reads back as malformed.
        let tmp = path.with_extension("json.new");
        std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// Answer one already-authorised `preferences.get`.
///
/// There is no `preferences.set` here and there is not going to be one. The
/// answers on this page are the user's, and a plugin able to rewrite them could
/// set them to whatever it liked and have Cordial show the result back as
/// though the user had chosen it. Writing is the launcher's, through
/// [`Store::set`], which is reached only from a row the user touched.
///
/// `plugin_id` is Cordial's own record of which process is on the pipe, never
/// a field of the request -- `settings.rs` makes the argument at length and it
/// applies here unchanged.
pub fn serve(
    store: Option<&Store>,
    fields: &[Declaration],
    plugin_id: &str,
    req: &crate::protocol::Request,
) -> crate::protocol::Response {
    use crate::protocol::Response;
    match req.method.as_str() {
        "preferences.get" => {
            let Some(store) = store else {
                // Answering `{}` with no profile behind it would tell the
                // plugin the user had chosen every default, which is a claim
                // and not a reading. `settings.rs` refuses the same way.
                return Response::Error {
                    id: req.id,
                    message: format!(
                        "{} needs an open profile; this Cordial has nowhere to keep answers",
                        req.method
                    ),
                };
            };
            match store.effective_for(plugin_id, fields) {
                Ok(values) => Response::Ok {
                    id: req.id,
                    result: serde_json::to_value(values).unwrap_or_default(),
                },
                Err(message) => Response::Error { id: req.id, message },
            }
        }
        other => Response::Error {
            id: req.id,
            message: format!("{other:?} is not a preferences method"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A schema invented for these tests and belonging to no real plugin.
    ///
    /// Deliberately so. A renderer or a store tested against the one plugin
    /// somebody happened to be writing at the time grows to fit that plugin,
    /// and the next author finds out which parts were general and which were
    /// coincidence. Everything below exercises the contract, not a customer.
    fn fabricated() -> Vec<Declaration> {
        serde_json::from_value(serde_json::json!([
            {"key": "loud", "type": "bool", "title": "Be loud", "default": true},
            {"key": "level", "type": "int", "title": "Level", "default": 3,
             "minimum": 1, "maximum": 10, "group": "Tuning"},
            {"key": "mode", "type": "choice", "title": "Mode", "default": "slow",
             "options": [{"value": "slow", "label": "Slow"}, {"value": "fast", "label": "Fast"}]},
            {"key": "note", "type": "text", "title": "Note", "default": ""}
        ]))
        .expect("the fabricated schema should parse")
    }

    fn scratch(tag: &str) -> Store {
        let dir = std::env::temp_dir().join(format!("cordial-preferences-test-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Store::new(dir)
    }

    fn quietly(
        fields: &[Declaration],
        saved: &serde_json::Value,
    ) -> BTreeMap<String, serde_json::Value> {
        effective(fields, saved, &mut |_| {})
    }

    #[test]
    fn the_four_field_shapes_parse_from_a_manifests_json() {
        let fields = fabricated();
        assert_eq!(fields.len(), 4);
        assert!(check_all(&fields).is_ok());
        assert_eq!(fields[0].field.type_name(), "bool");
        assert_eq!(fields[1].field.type_name(), "int");
        assert_eq!(fields[2].field.type_name(), "choice");
        assert_eq!(fields[3].field.type_name(), "text");
        assert_eq!(fields[1].group.as_deref(), Some("Tuning"));
    }

    #[test]
    fn a_plugin_that_declares_nothing_has_no_page() {
        // The whole signal the gear is drawn from. An empty list must be a
        // valid manifest and must mean "no page", not "an empty page".
        assert!(check_all(&[]).is_ok());
        assert!(quietly(&[], &serde_json::json!({"stale": 1})).is_empty());
    }

    #[test]
    fn every_declared_key_comes_back_even_when_nothing_is_saved() {
        // What the declaration buys a plugin author: no `?? default` anywhere
        // in their code, because the document is always complete.
        let fields = fabricated();
        let values = quietly(&fields, &serde_json::json!({}));
        assert_eq!(values["loud"], serde_json::json!(true));
        assert_eq!(values["level"], serde_json::json!(3));
        assert_eq!(values["mode"], serde_json::json!("slow"));
        assert_eq!(values["note"], serde_json::json!(""));
    }

    #[test]
    fn a_saved_answer_wins_over_the_declared_default() {
        let fields = fabricated();
        let values = quietly(&fields, &serde_json::json!({"level": 9, "mode": "fast"}));
        assert_eq!(values["level"], serde_json::json!(9));
        assert_eq!(values["mode"], serde_json::json!("fast"));
        assert_eq!(values["loud"], serde_json::json!(true), "untouched keys keep their default");
    }

    #[test]
    fn a_saved_answer_that_no_longer_fits_falls_back_and_says_so() {
        // The plugin narrowed a range or renamed an option in an update. The
        // value must not be handed over — the plugin declared it impossible —
        // and the fallback must not be silent, because a preference that
        // quietly reverts looks exactly like one that never saved.
        let fields = fabricated();
        let mut said = Vec::new();
        let values = effective(
            &fields,
            &serde_json::json!({"level": 99, "mode": "instant"}),
            &mut |line| said.push(line.to_string()),
        );
        assert_eq!(values["level"], serde_json::json!(3));
        assert_eq!(values["mode"], serde_json::json!("slow"));
        assert_eq!(said.len(), 2, "both fallbacks should be reported: {said:?}");
        assert!(said.iter().any(|l| l.contains("level")), "{said:?}");
    }

    #[test]
    fn a_value_of_the_wrong_type_is_not_coerced() {
        // `true` is not 1 and "3" is not 3. The plugin is entitled to the type
        // it declared, and a coercion here is a surprise on the other side of
        // the pipe.
        let fields = fabricated();
        let values = quietly(&fields, &serde_json::json!({"level": "9", "loud": 1}));
        assert_eq!(values["level"], serde_json::json!(3));
        assert_eq!(values["loud"], serde_json::json!(true));
    }

    #[test]
    fn keys_nothing_declares_are_dropped() {
        let fields = fabricated();
        let values = quietly(&fields, &serde_json::json!({"removed": "yesterday"}));
        assert!(!values.contains_key("removed"), "{values:?}");
    }

    #[test]
    fn a_default_outside_its_own_range_is_refused() {
        let bad: Vec<Declaration> = serde_json::from_value(serde_json::json!([
            {"key": "level", "type": "int", "title": "Level", "default": 50,
             "minimum": 1, "maximum": 10}
        ]))
        .unwrap();
        let why = check_all(&bad).unwrap_err();
        assert!(why.contains("outside its own range"), "{why}");
    }

    #[test]
    fn a_choice_defaulting_to_something_it_does_not_offer_is_refused() {
        let bad: Vec<Declaration> = serde_json::from_value(serde_json::json!([
            {"key": "mode", "type": "choice", "title": "Mode", "default": "warp",
             "options": [{"value": "slow", "label": "Slow"}]}
        ]))
        .unwrap();
        assert!(check_all(&bad).is_err());
    }

    #[test]
    fn a_choice_with_no_options_is_refused() {
        let bad: Vec<Declaration> = serde_json::from_value(serde_json::json!([
            {"key": "mode", "type": "choice", "title": "Mode", "default": "", "options": []}
        ]))
        .unwrap();
        assert!(check_all(&bad).is_err());
    }

    #[test]
    fn a_duplicate_key_is_refused_rather_than_letting_one_row_shadow_another() {
        let bad: Vec<Declaration> = serde_json::from_value(serde_json::json!([
            {"key": "loud", "type": "bool", "title": "One"},
            {"key": "loud", "type": "bool", "title": "Two"}
        ]))
        .unwrap();
        let why = check_all(&bad).unwrap_err();
        assert!(why.contains("declared twice"), "{why}");
    }

    #[test]
    fn a_title_that_could_draw_outside_its_own_row_is_refused() {
        // The plugin author's words are shown in Cordial's window. A newline
        // in a title overlaps the row beneath it, and a row that can draw
        // outside itself is the start of the thing ADR-020 is about.
        for title in ["", "two\nlines", "bell\u{7}"] {
            let bad: Vec<Declaration> = serde_json::from_value(serde_json::json!([
                {"key": "k", "type": "bool", "title": title}
            ]))
            .unwrap();
            assert!(check_all(&bad).is_err(), "{title:?} should not be a usable title");
        }
    }

    #[test]
    fn markup_in_a_title_is_accepted_and_must_therefore_be_drawn_literally() {
        // The other half of the test above, and the one that says what the
        // renderer owes. `<b>` is ordinary text a plugin author might
        // reasonably write, so refusing it here would be wrong -- which means
        // `plugin_preferences.rs` is what has to stop libadwaita parsing it as
        // Pango markup in Cordial's own chrome. If this test is ever "fixed"
        // by refusing markup here, that renderer guard is what to check first.
        let fields: Vec<Declaration> = serde_json::from_value(serde_json::json!([
            {"key": "k", "type": "bool",
             "title": "<b>Bold</b> & <span size='xx-large'>huge</span>",
             "description": "<i>also not italic</i>"}
        ]))
        .unwrap();
        assert!(check_all(&fields).is_ok());
    }

    #[test]
    fn a_key_that_is_not_boring_is_refused() {
        for key in ["", "has space", "a/b", "..", "e\u{301}"] {
            assert!(!is_valid_key(key), "{key:?} should not be a usable preference key");
        }
        assert!(is_valid_key("frame_cap-2"));
    }

    #[test]
    fn more_fields_than_a_page_can_be_read_as_is_refused() {
        let many: Vec<Declaration> = (0..=MAX_FIELDS)
            .map(|i| Declaration {
                key: format!("k{i}"),
                title: format!("Field {i}"),
                description: String::new(),
                group: None,
                field: Field::Bool { default: false },
            })
            .collect();
        assert!(check_all(&many).is_err());
        assert!(check_all(&many[..MAX_FIELDS]).is_ok());
    }

    #[test]
    fn an_unbounded_integer_still_has_a_range_the_renderer_can_use() {
        // The renderer must never have to invent a bound of its own, or two
        // call sites will invent different ones.
        let f = Field::Int { default: 0, minimum: None, maximum: None, step: None };
        let (lo, hi) = f.bounds().unwrap();
        assert!(lo < 0 && hi > 0);
        let narrow = Field::Int { default: 5, minimum: Some(1), maximum: Some(10), step: None };
        assert_eq!(narrow.bounds(), Some((1, 10)));
        assert!(Field::Bool { default: false }.bounds().is_none());
    }

    #[test]
    fn one_answer_saves_without_disturbing_the_others() {
        // The read-modify-write property. A whole-document replace here would
        // discard every other answer the moment a switch was flipped.
        let store = scratch("set-one");
        let fields = fabricated();
        store.set("example", &fields, "level", serde_json::json!(7)).unwrap();
        store.set("example", &fields, "mode", serde_json::json!("fast")).unwrap();
        let values = store.effective_for("example", &fields).unwrap();
        assert_eq!(values["level"], serde_json::json!(7));
        assert_eq!(values["mode"], serde_json::json!("fast"));
    }

    #[test]
    fn a_value_the_declaration_refuses_never_reaches_the_file() {
        let store = scratch("refuse");
        let fields = fabricated();
        store.set("example", &fields, "level", serde_json::json!(4)).unwrap();
        assert!(store.set("example", &fields, "level", serde_json::json!(99)).is_err());
        assert!(store.set("example", &fields, "level", serde_json::json!("nine")).is_err());
        assert!(store.set("example", &fields, "nonesuch", serde_json::json!(1)).is_err());
        assert_eq!(store.effective_for("example", &fields).unwrap()["level"], serde_json::json!(4));
    }

    #[test]
    fn preferences_land_beside_settings_and_not_in_it() {
        // The two-writers problem this file exists to avoid. `settings.json`
        // is replaced wholesale by the plugin; if the user's answers lived
        // there they would be gone the first time the plugin saved anything.
        let store = scratch("layout");
        let path = store.path_for("example").unwrap();
        assert_eq!(path, store.profile_dir().join("plugins/example/preferences.json"));

        let settings = crate::settings::Store::new(store.profile_dir());
        store.set("example", &fabricated(), "level", serde_json::json!(7)).unwrap();
        settings.write("example", &serde_json::json!({"scratch": "state"})).unwrap();
        assert_ne!(path, settings.path_for("example").unwrap());
        assert_eq!(
            store.effective_for("example", &fabricated()).unwrap()["level"],
            serde_json::json!(7),
            "a plugin replacing its own settings must not touch the user's answers"
        );
    }

    #[test]
    fn an_id_that_is_a_path_is_refused_rather_than_sanitised() {
        let store = scratch("escape");
        for bad in ["..", "../../etc", "a/b", "/etc/passwd", ".", ""] {
            assert!(store.path_for(bad).is_err(), "{bad:?} should not resolve to a path");
        }
    }

    #[test]
    fn resetting_returns_the_page_to_its_declared_defaults() {
        let store = scratch("reset");
        let fields = fabricated();
        store.set("example", &fields, "level", serde_json::json!(9)).unwrap();
        store.reset("example").unwrap();
        assert_eq!(store.effective_for("example", &fields).unwrap()["level"], serde_json::json!(3));
        // Twice, because a reset with nothing saved is the ordinary state of a
        // page a user opens and closes.
        assert!(store.reset("example").is_ok());
    }

    #[test]
    fn a_plugin_reading_its_preferences_gets_a_complete_document() {
        let store = scratch("serve");
        let fields = fabricated();
        store.set("example", &fields, "mode", serde_json::json!("fast")).unwrap();
        let req = crate::protocol::Request {
            id: 1,
            method: "preferences.get".into(),
            params: serde_json::json!({}),
        };
        match serve(Some(&store), &fields, "example", &req) {
            crate::protocol::Response::Ok { result, .. } => {
                assert_eq!(result["mode"], "fast");
                assert_eq!(result["level"], 3, "an unset field still arrives: {result}");
            }
            other => panic!("expected the answers, got {other:?}"),
        }
    }

    #[test]
    fn there_is_no_way_for_a_plugin_to_write_a_preference() {
        // The claim ADR-020 rests on, expressed as a test rather than as a
        // sentence in a doc comment: no method here writes, so a plugin
        // cannot set an answer and have the page show it back as the user's.
        let store = scratch("read-only");
        let fields = fabricated();
        for method in ["preferences.set", "preferences.write", "preferences.put"] {
            let req = crate::protocol::Request {
                id: 1,
                method: method.into(),
                params: serde_json::json!({"key": "level", "value": 9}),
            };
            let res = serve(Some(&store), &fields, "example", &req);
            assert!(
                matches!(res, crate::protocol::Response::Error { .. }),
                "{method} should not exist, got {res:?}"
            );
        }
        assert_eq!(store.effective_for("example", &fields).unwrap()["level"], serde_json::json!(3));
        assert_eq!(crate::protocol::required_capability("preferences.set"), None);
    }

    #[test]
    fn reading_preferences_without_a_profile_fails_loudly() {
        let req = crate::protocol::Request {
            id: 1,
            method: "preferences.get".into(),
            params: serde_json::json!({}),
        };
        match serve(None, &fabricated(), "example", &req) {
            crate::protocol::Response::Error { message, .. } => {
                assert!(message.contains("profile"), "{message}")
            }
            other => panic!("expected an explicit failure, got {other:?}"),
        }
    }

    #[test]
    fn a_key_the_plugin_has_stopped_declaring_is_dropped_on_the_next_write() {
        let store = scratch("prune");
        let fields = fabricated();
        store.set("example", &fields, "level", serde_json::json!(7)).unwrap();

        let narrowed = &fields[..2];
        store.set("example", narrowed, "level", serde_json::json!(8)).unwrap();
        let saved = store.read("example").unwrap();
        assert!(saved.get("mode").is_none(), "{saved}");
        assert_eq!(saved["level"], serde_json::json!(8));
    }
}
