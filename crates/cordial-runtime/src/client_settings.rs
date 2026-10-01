//! Roblox's client settings — the FastFlag set the engine runs on.
//!
//! On Android the *application* fetches this document and hands it to the engine
//! through `NativeGLInterface.nativeInitClientSettings`. Cordial is the host
//! application here, so doing that fetch is the job, not a workaround.
//!
//! **"The engine never fetches it itself" was the rest of this paragraph, and it
//! is wrong.** It rested on breakpoints on `getaddrinfo`, `connect` and
//! `SSL_connect` never being hit during startup, which is a statement about the
//! first second and was read as a statement about the process. The engine runs a
//! `DynamicFastVariableReloader` of its own. Turning the engine's own HTTP
//! tracing on (`DFLogHttpTraceLight`) shows it, on 2.734.0.917, signed in:
//!
//! ```text
//! 2.251  HttpResponse(#22) status:200 bodySize:1305506
//!        url:{ "https://clientsettingscdn.roblox.com/v2/settings-compressed/application/GoogleAndroidApp.zst" }
//! 4.150  [FlagCache] writeFlagCache: Successfully wrote 372673 bytes
//! 120.11 [FLog::DynamicFastVariableReloader] DynamicFastVariableReloader finished flag fetch
//! ```
//!
//! Note the application name: the engine asks for **`GoogleAndroidApp`**, and
//! since 2026-09-24 this file does too by default ([`ENGINE_URL`],
//! `CORDIAL_ENGINE_SETTINGS=0` reverts to the `AndroidApp` document it fetched
//! before that). They are not the same document -- `docs/analysis/flag-init.md`
//! SS23.4 counted 441 differing values -- and a live rollout made the
//! difference between a blank screen and a working Landing page, 20/20 launches
//! each way; see [`ENGINE_URL`]'s doc comment for the measurement.
//!
//! What that reloader costs is written up on [`apply_overrides`], because it is
//! the thing that decides how long an override survives.
//!
//! ## The call contract, established by experiment
//!
//! `nativeInitClientSettings(String, String, String)` returns an `int`. Feeding
//! it known-good and known-bad documents settles what it means:
//!
//! (An earlier version of this note called that return value "the only
//! trustworthy signal, since the engine's own `FLog` output is not routed
//! anywhere visible in this build". The second half was wrong. `FLog` is routed,
//! and always has been — the engine writes it to `appData/logs/*.log`, relative
//! to the working directory. Read that file; it is the best diagnostic Cordial
//! has.)
//!
//! | first argument | result |
//! |---|---|
//! | the real document, `{"applicationSettings": {...}}` | `0` |
//! | `{"applicationSettings":{"FFlagNotARealFlag":"True"}}` | `0` |
//! | `this is not json at all` | `1` |
//! | `{}` — valid JSON, no `applicationSettings` key | `1` |
//!
//! So **`0` is success**, the document goes in the *first* argument, and the
//! `applicationSettings` wrapper must be kept rather than unwrapped. Passing the
//! document in either of the other two positions returns `1` regardless of
//! whether it is valid, which is what "this argument is not the settings" looks
//! like. One of those two is an overrides document — the engine has a
//! `ParseFailure on overrides` log string — but which is still unestablished, so
//! both are left empty rather than guessed at.
//!
//! Those first readings were confounded: `--client-settings` fed *two* call
//! sites, so `nativeInitializeNativeFlags` was receiving the document too and
//! the result could not be attributed to this call alone. On the automatic path
//! the flag-names call gets nothing, and the discriminator survives cleanly —
//! empty string gives `1`, the real document gives `0`. The conclusion held, but
//! it had not actually been shown until the two were separated.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Roblox's settings CDN, application name `AndroidApp` -- not a guess,
/// `AndroidClient`, `AndroidPlayer`, `AndroidClientSettings` and
/// `AndroidAppSettings` all return HTTP 400 "The application name is invalid",
/// and `AndroidApp` returns a real document. **No longer the default fetch**
/// as of 2026-09-24 -- see [`ENGINE_URL`] -- kept as the
/// `CORDIAL_ENGINE_SETTINGS=0` escape hatch, since `AndroidApp` was never
/// shown to be *wrong*, only shown to sometimes disagree with the document
/// the engine asks for itself.
const URL: &str = "https://clientsettingscdn.roblox.com/v2/settings/application/AndroidApp";

/// The exact URL the engine's own `DynamicFastVariableReloader` asks for
/// (captured from `DFLogHttpTraceLight` on a signed-in 2.734.0.917 -- see
/// this module's top doc comment). **The default fetch since 2026-09-24**;
/// `CORDIAL_ENGINE_SETTINGS=0` reverts to [`URL`] for the document handed to
/// `nativeInitClientSettings`.
///
/// **Why this is the default now**: this module's own doc comment had said
/// since it was written that whether the `AndroidApp` and
/// `GoogleAndroidApp` documents differ in anything Cordial cares about was
/// "not established; nobody has diffed them." `docs/analysis/flag-init.md`
/// SS23.4 later diffed them (441 differing values) and called switching
/// "worth correcting on its own terms... but a separate change from this
/// one" -- deferred, not rejected, because the storage bug it was chasing
/// then tested negative either document. It stopped being a deferrable
/// correction on 2026-09-24: a live Roblox rollout made `AndroidApp` (`URL`,
/// what this file fetched by default until now) produce a single-cycle
/// render with no `Forcing finalize` and a flat grey screen, on both a
/// signed-in profile and a brand-new signed-out one, while `GoogleAndroidApp`
/// produced Landing normally -- 20/20 launches each way, delivery mechanism
/// held constant (isolated from document identity by feeding a
/// byte-identical copy of the `AndroidApp` cache file through
/// `--client-settings` and getting the same blank result, ruling out
/// "explicit vs cached" as the variable). This is one incident's evidence,
/// not a general claim that `GoogleAndroidApp` is always the better choice --
/// `CORDIAL_ENGINE_SETTINGS=0` exists because a future rollout could reverse
/// which endpoint works, and this file cannot tell that from either
/// document's shape alone.
const ENGINE_URL: &str =
    "https://clientsettingscdn.roblox.com/v2/settings-compressed/application/GoogleAndroidApp.zst";

/// How long a cached copy is used before refetching.
///
/// Roblox changes flags continuously, so this is not "cache forever". It is long
/// enough that ordinary repeat launches do not each hit the network, and short
/// enough that a machine left running picks up changes within a session or two.
const MAX_AGE: Duration = Duration::from_secs(6 * 60 * 60);

fn cache_path() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
        .join("cordial/clientsettings.json")
}

/// Where a document fetched from [`ENGINE_URL`] is cached, kept separate
/// from [`cache_path`] so the two endpoints never overwrite each other's
/// copy -- important now that they are known to sometimes disagree.
fn engine_cache_path() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
        .join("cordial/enginesettings.json")
}

/// GET `url` and decompress it as a zstd stream, returning the UTF-8 text.
/// Shared between [`history::fetch_engine_copy`] (an observational side
/// fetch, never handed to the engine) and the `CORDIAL_ENGINE_SETTINGS`
/// branch of [`load_base`] (which does hand its result to the engine) --
/// both ask the same compressed endpoint, so one decompressor.
fn fetch_and_decompress(url: &str) -> Result<String, String> {
    let raw = cordial_update::http::get_bytes(url).map_err(|e| e.to_string())?;
    let decompressed =
        zstd::stream::decode_all(std::io::Cursor::new(raw)).map_err(|e| format!("zstd decode: {e}"))?;
    String::from_utf8(decompressed).map_err(|e| format!("not utf-8: {e}"))
}

fn fresh(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    let age = SystemTime::now().duration_since(meta.modified().ok()?).ok()?;
    (age < MAX_AGE).then(|| std::fs::read_to_string(path).ok())?
}

/// Looks like a settings document rather than an error page.
///
/// Worth checking before caching: the CDN answers a bad application name with a
/// perfectly well-formed JSON error body, and caching that would produce six
/// hours of failures that look like a flag problem rather than a fetch problem.
fn plausible(body: &str) -> bool {
    body.contains("\"applicationSettings\"")
}

/// Where the document handed to the engine came from, or why there is none.
///
/// This exists to answer the question GitHub issue #21 could not: reporter A's
/// `nativeInitializeNativeFlags` resolved roughly ten of a hundred and
/// thirty-nine flags against a healthy machine's eighty-one, and the log
/// between `nativeSetCacheDirectory ok (early)` and `bootstrapTheApp
/// installed` said nothing about whether that was an unreadable
/// `--client-settings` path, a fetch that never connected, or a fetch that
/// connected and was refused. It is a name for that gap, not a fix for the
/// bug -- see `docs/analysis/flag-init.md` §50 and its correction for why a
/// wait on any of this would hang instead of help: `bootstrapTheApp` itself
/// runs on the engine's own schedule, not synchronously inside
/// `initializeNativeCode` as an earlier draft of that fix assumed.
pub enum Source {
    /// `--client-settings <path>`, and it read.
    Explicit,
    /// `CORDIAL_ENGINE_SETTINGS=0`'s on-disk cache ([`cache_path`]), still
    /// inside `MAX_AGE`.
    FreshCache,
    /// `CORDIAL_ENGINE_SETTINGS=0`, live fetch from [`URL`].
    Fetched,
    /// The fetch failed and a stale cache answered in its place. Whichever
    /// of the two endpoints was in use, not necessarily [`URL`]'s.
    StaleCache,
    /// Nothing did. The engine gets an empty document and resolves almost
    /// every flag to its own compiled default -- this is why.
    Nothing(String),
    /// The default since 2026-09-24: [`ENGINE_URL`] rather than [`URL`],
    /// fresh from its own cache ([`engine_cache_path`]).
    EngineDocumentCached,
    /// The default since 2026-09-24, live fetch.
    EngineDocumentFetched,
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Source::Explicit => write!(f, "--client-settings"),
            Source::FreshCache => write!(f, "cache"),
            Source::Fetched => write!(f, "fetched"),
            Source::EngineDocumentCached => write!(f, "engine document, cache"),
            Source::EngineDocumentFetched => write!(f, "engine document, fetched"),
            Source::StaleCache => write!(f, "stale cache, fetch failed"),
            Source::Nothing(why) => write!(f, "nothing: {why}"),
        }
    }
}

/// The client settings document, from cache when it is fresh and from Roblox
/// otherwise.
///
/// Returns `None` rather than failing the launch: the engine is given whatever
/// it can be given, and a client that starts without flags is more useful than
/// one that refuses to start because a CDN was unreachable.
pub fn load(explicit: Option<&str>) -> Option<String> {
    load_reporting(explicit).0
}

/// As [`load`], but says where the document came from (or why there is none)
/// rather than discarding that fact. The default (`bootstrapTheApp`) launch
/// path prints it; see `load.rs`'s `BootstrapPlan`.
///
/// Timed, because this runs inside the engine's `bootstrapTheApp` callback on
/// the default path: whatever it takes is time the engine waits on Cordial.
/// A synchronous fetch here was suspected of causing the signed-in startup
/// freeze and measured not to (2026-09-24, docs/analysis/startup-freeze-capture.md).
pub fn load_reporting(explicit: Option<&str>) -> (Option<String>, Source) {
    let start = std::time::Instant::now();
    let (body, source) = load_base(explicit);
    let elapsed = start.elapsed();
    println!("  client settings: load_base took {}ms ({source})", elapsed.as_millis());
    if let Some(dir) = history::dir() {
        history::record(&dir, body.as_deref(), &source);
    }
    (body.map(apply_overrides), source)
}

fn load_base(explicit: Option<&str>) -> (Option<String>, Source) {
    if let Some(path) = explicit {
        return match std::fs::read_to_string(path) {
            Ok(body) => (Some(body), Source::Explicit),
            // Used to be `.ok()`, silently. An unreadable path given with
            // `--client-settings` is a typo or a stale symlink, not "use the
            // network instead" -- and it looked exactly like a healthy launch
            // with nothing on the other end of it. Printed here too, not only
            // returned as a `Source`, because `load()` -- still used at every
            // call site but the default one -- throws the `Source` away.
            Err(e) => {
                let why = format!("--client-settings {path}: {e}");
                println!("  client settings: {why}");
                (None, Source::Nothing(why))
            }
        };
    }
    // Default since 2026-09-24: fetch the document the engine itself asks
    // for (ENGINE_URL, GoogleAndroidApp) rather than the AndroidApp document
    // this file fetched from 2026-07-31 to here. AndroidApp was never chosen
    // for a measured advantage over GoogleAndroidApp -- it was the only
    // application name, of the four tried by probing, that did not answer
    // HTTP 400 "The application name is invalid", at a time before anyone
    // had found the engine fetches its own document under a different name
    // at all (see this module's top doc comment; `docs/analysis/flag-init.md`
    // SS23.4 found the two documents differ by 441 values and called
    // switching "worth correcting on its own terms... but a separate change
    // from this one", deferred rather than rejected). Measured 2026-09-24: a
    // live Roblox rollout made AndroidApp produce a single-cycle render with
    // no `Forcing finalize` and a blank screen -- signed in and signed out
    // alike, 10/10 launches -- while GoogleAndroidApp reached Landing/Home
    // normally in the same 10/10, delivery mechanism held constant. Cordial's
    // own flag overrides (`apply_overrides`, below) still apply on top
    // regardless of which document this fetches -- confirmed live with the
    // existing `DFFlagRbxTransportUseRtcioRna` control this file already
    // uses elsewhere: with it forced to `false`, the engine's own "Initialized
    // RtcIoRna with 1 event loop threads" line is absent from the log; without
    // it, present twice. `CORDIAL_ENGINE_SETTINGS=0` is the escape hatch back
    // to the old endpoint, kept rather than deleted in case some future
    // rollout makes AndroidApp the one that works and GoogleAndroidApp the
    // one that does not.
    if std::env::var("CORDIAL_ENGINE_SETTINGS").as_deref() != Ok("0") {
        let cache = engine_cache_path();
        if let Some(body) = fresh(&cache) {
            return (Some(body), Source::EngineDocumentCached);
        }
        // No fallback to `cache_path()` (the old AndroidApp cache) on
        // failure here, on purpose: a warm `clientsettings.json` from before
        // this switch flipped holds the *other* document, and serving it as
        // though it were the engine's own would silently reintroduce the
        // exact confusion this default exists to end. The failure mode when
        // both this fetch and `engine_cache_path()` are empty (a fresh
        // install, network down) is the same one `AndroidApp` already had in
        // the equivalent case: `Source::Nothing`, an empty document, and the
        // engine resolving almost every flag to its own compiled default --
        // not a new regression, the pre-existing "nothing is coming" path.
        return match fetch_and_decompress(ENGINE_URL) {
            // Same reasoning as `fetch()`: a bad application name or an
            // outage can answer with a well-formed body that is not a
            // settings document, and caching that would look like a flag
            // problem rather than a fetch problem for MAX_AGE afterwards.
            Ok(body) if plausible(&body) => {
                if let Some(parent) = cache.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::write(&cache, &body);
                (Some(body), Source::EngineDocumentFetched)
            }
            Ok(_) => {
                let why = format!("{ENGINE_URL} answered with something that is not a settings document");
                match std::fs::read_to_string(&cache) {
                    Ok(body) => (Some(body), Source::StaleCache),
                    Err(_) => (None, Source::Nothing(why)),
                }
            }
            Err(why) => match std::fs::read_to_string(&cache) {
                Ok(body) => (Some(body), Source::StaleCache),
                Err(_) => (None, Source::Nothing(why)),
            },
        };
    }

    // CORDIAL_ENGINE_SETTINGS=0: the old AndroidApp endpoint, kept as the
    // escape hatch. Its own cache (`cache_path()`) is likewise never
    // consulted by the branch above, for the same reason in reverse.
    let cache = cache_path();
    if let Some(body) = fresh(&cache) {
        return (Some(body), Source::FreshCache);
    }
    match fetch() {
        Ok(body) => {
            if let Some(parent) = cache.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&cache, &body);
            (Some(body), Source::Fetched)
        }
        // A stale copy beats nothing when the network is down.
        Err(why) => match std::fs::read_to_string(&cache) {
            Ok(body) => (Some(body), Source::StaleCache),
            Err(_) => (None, Source::Nothing(why)),
        },
    }
}

/// Keys in the flag layers that belong to Cordial rather than to Roblox.
///
/// One prefix rather than a list, so a second Cordial-owned key does not need
/// this file edited to stay out of the engine's settings.
const CORDIAL_KEY_PREFIX: &str = "Cordial";

/// Whether a resolved key is Roblox's to receive.
fn is_roblox_flag(key: &str) -> bool {
    !key.starts_with(CORDIAL_KEY_PREFIX)
}

/// Merge every layer of flag overrides into the settings document.
///
/// The layering, precedence and provenance live in [`crate::flags`]; this only
/// applies the result. Splitting them is what lets a plugin contribute flags
/// without writing into the user's file.
///
/// This is the mechanism that demonstrably works. Verified with a control:
/// `DFFlagRbxTransportUseRtcioRna=false` removes
/// `Initialized RtcIoRna with 1 event loop threads` from the engine's own log,
/// and the same run without it has that line. `nativePreloadFlagOverrides` is
/// *not* the mechanism despite the name — it was tried with several document
/// shapes and changed nothing observable.
///
/// **It works for about two seconds, and that control only held because the
/// line it watches is printed at 0.375 s.** The engine's own settings fetch
/// (see the module note) lands at 1.6–2.3 s on this machine and *reapplies
/// Roblox's document over the top*, so every `DF*` value Cordial merged in is
/// reverted to Roblox's for any key the fetched document contains. Keys the
/// document does not contain keep Cordial's value for the whole run.
///
/// Measured both directions inside one run (2026-08-20, 2.734.0.917, signed
/// in), which is what makes it a control rather than an anecdote:
///
/// ```text
/// override                     Roblox's value   observed
/// DFLogHttpTraceLight = "7"    "0"              logs 0.65 s .. 2.25 s, then silent
/// DFLogHttpTraceError = "0"    "12"             silent .. 2.25 s, logging again from 4.86 s
/// DFLogHttpTrace      = "7"    absent           logs 0.86 s .. 120.1 s
/// ```
///
/// The middle row is the one that settles it: an override asking for *silence*
/// on a key Roblox sets went loud again, at Roblox's own level, part-way
/// through the run. Nothing Cordial does can be the cause of that.
///
/// Two consequences worth having in mind before trusting a flag experiment
/// here. A `DFFlag`/`DFInt`/`DFString`/`DFLog` override only reliably governs
/// the first couple of seconds, so a claim resting on one needs the effect to
/// be visible in that window or it is measuring Roblox's value. And an
/// `FFlag`/`FInt`/`FString` override is read once at startup and is *not*
/// reverted — the family this file's own module doc, and [`crate::flags`],
/// present as the harder one to influence is in fact the durable one.
///
/// **A second call mid-run works, measured 2026-09-01.** The engine takes
/// another `nativeInitClientSettings` while it is running, answers `0`, and
/// the new values take effect -- so a flag edited on disk *while the client is
/// open* can reach the engine, which had never been tested here.
///
/// Three arms, `DFLogHttpTrace`/`DFLogHttpTraceLight` as the probe, counting
/// the engine's own HTTP trace lines per ten seconds of a 75 s run:
///
/// ```text
/// arm                                    lines   after t=35s
/// A  flags present at launch, no re-call   2050   the instrument works at all
/// B  flags written at 35 s, re-call at 45 s 338   ~330
/// C  flags written at 35 s, no re-call       16   ~6
/// ```
///
/// A is the arm that makes the other two mean anything: without it a null
/// result in B is indistinguishable from a probe that does nothing. B and C
/// differ *only* in whether the re-call happened -- same document, same write,
/// same moment -- and B logged some fifty times as much after it. Both were
/// repeated and reproduced to the line, bucket for bucket.
///
/// Two things this does **not** establish. The probe is a `DFLog*` flag, and
/// whether `FFlag`/`FInt`/`FString` -- the durable family -- also update on a
/// second call is untested; the startup path reads them once, and a re-call
/// may or may not be the same path. And it changes nothing about the reverting
/// above: a `DF*` key Roblox's own document contains still goes back to
/// Roblox's value at the next fetch, whenever it was set. So "apply dynamic
/// flags to the running client" remains the wrong shape for a feature even
/// though the mechanism works -- the family that survives a run is the static
/// one, and the family that can be pushed live is the one that gets reverted.
///
/// `CORDIAL_EXPERIMENT_RESETTLE_MS` in `bin/load.rs` is the hook this was run
/// with. It is deliberately not wired to anything a user can press.
///
/// **The later fetch is periodic, every 120 s, and appears to replace rather
/// than merge** -- it drops overrides for keys Roblox's document does not even
/// contain. [`reassert_after_reload`] hands the document back after each one
/// and records the evidence.
fn apply_overrides(doc: String) -> String {
    let resolved = crate::flags::resolve(crate::flags::collect());
    if resolved.is_empty() {
        return doc;
    }
    // Cordial's own keys ride the flag layering for its precedence and
    // provenance, and are not Roblox flags — `CordialGraphicsBackend` asks
    // Cordial whether to offer the engine a Vulkan loader, which is a question
    // the engine has no idea it is being asked. Handing them over would put
    // invented names in Roblox's settings document; the engine ignores what it
    // does not know, but a flag it silently ignores is exactly the thing this
    // project keeps mistaking for a flag that works.
    let overrides: serde_json::Map<String, serde_json::Value> = resolved
        .iter()
        .filter(|(k, _)| is_roblox_flag(k))
        .map(|(k, r)| (k.clone(), serde_json::Value::String(r.value.clone())))
        .collect();

    match merge(&doc, overrides) {
        Ok((merged, _)) => {
            crate::flags::report(&resolved);
            merged
        }
        Err(why) => {
            println!("  flags: {why}; ignoring overrides");
            doc
        }
    }
}

/// Merge overrides into `applicationSettings`, returning the document and how
/// many were applied. Split out from layer resolution so it can be tested.
fn merge(
    doc: &str,
    overrides: serde_json::Map<String, serde_json::Value>,
) -> Result<(String, usize), &'static str> {
    let mut parsed: serde_json::Value =
        serde_json::from_str(doc).map_err(|_| "the settings document did not parse")?;
    let app = parsed
        .get_mut("applicationSettings")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or("no applicationSettings object")?;

    let mut applied = 0usize;
    for (k, v) in overrides {
        let as_string = match v {
            serde_json::Value::String(s) => s,
            other => other.to_string(),
        };
        app.insert(k, serde_json::Value::String(as_string));
        applied += 1;
    }
    let out = serde_json::to_string(&parsed).map_err(|_| "the merged document did not serialise")?;
    Ok((out, applied))
}

/// Returns the reason rather than `None` on failure, because `load_base` now
/// has somewhere to put it (`Source::Nothing`) and "why" is exactly what was
/// missing from the report this exists to answer.
///
/// Used to be a bare `ureq::get(URL).call()` with no connect or read timeout
/// configured at all -- everything `None` except the read-size limit below --
/// so a CDN that accepted the TCP connection and then said nothing could hold
/// a launch open indefinitely with no way to tell, from the log, that this was
/// what had happened. `cordial_update::http::get_text` is the "house" client
/// this project already uses for its own version/changelog/APK metadata
/// fetches, built on the timeouts `cordial-update/src/http.rs` picked for
/// exactly this shape of request -- ten seconds to connect, twenty total
/// (`http::CONNECT`, `http::TIMEOUT`). Reused rather than re-derived, so the
/// two numbers cannot drift apart, and it comes with `url_policy`'s
/// host-locked redirect handling for free, which the bare call did not have.
fn fetch() -> Result<String, String> {
    // The document is ~1.2 MB; `get_text`'s own limit is 8 MB, which is a
    // metadata-sized bound rather than the APK-sized one `crate::download`
    // needs, but comfortably above anything this endpoint has ever answered.
    let body = cordial_update::http::get_text(URL).map_err(|e| e.to_string())?;
    if plausible(&body) {
        Ok(body)
    } else {
        Err(format!("{URL} answered with something that is not a settings document"))
    }
}

/// `CORDIAL_SETTINGS_HISTORY=<dir>`: keep every settings document this
/// process sees, instead of the single overwritten copy `cache_path()` keeps.
///
/// Added to chase a live finding, 2026-09-24: Cordial's default AGDK startup
/// intermittently collapses from its usual two-cycle placeholder-swap into a
/// single cycle with no `Forcing finalize` and a blank render, and the one
/// thing observed to track it across otherwise-identical runs is the byte
/// size the engine itself logs after its own fetch (`getFlags: success =
/// true, payload's size = N`). That size is not one this file's own fetch
/// produces -- `URL` above asks for `AndroidApp`, uncompressed, over
/// `/v2/settings/`; the engine asks for `GoogleAndroidApp`, compressed, over
/// `/v2/settings-compressed/` (see this module's top doc comment) -- so
/// answering "what changed" needs a copy of *that* document, not just
/// Cordial's own.
///
/// This does not read the engine's traffic. ADR-001 rules out anything that
/// would need hooking the engine's process to get it, and nothing here does
/// that: `fetch_engine_copy` is an independent request Cordial's own process
/// makes, moments apart from the engine's, to the same public CDN URL the
/// engine is about to ask for itself. The two are not guaranteed to be
/// byte-identical to what the engine received -- a live rollout could in
/// principle answer differently a few seconds later, to a second connection
/// -- but it is the closest thing to that document obtainable without
/// touching the engine at all, and is intended to be checked against the
/// engine's own logged size after the fact, not assumed to match it.
mod history {
    use super::Source;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    pub fn dir() -> Option<PathBuf> {
        std::env::var_os("CORDIAL_SETTINGS_HISTORY").map(PathBuf::from)
    }

    fn stamp_millis() -> u128 {
        SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
    }

    /// Writes `bytes` under `dir` as `<millis>-<size>-<tag>.json`. Best-effort
    /// and silent on failure beyond a printed line: a diagnostic that could
    /// abort a launch would be worse than the blank screen it exists to
    /// explain, and `println!` rather than a logger for the same reason the
    /// rest of this launch path uses it -- `appData/logs/*.log` is the
    /// engine's, this process's own stdout is Cordial's.
    fn save(dir: &Path, tag: &str, bytes: &[u8]) {
        if let Err(e) = std::fs::create_dir_all(dir) {
            println!("  settings history: {}: {e}", dir.display());
            return;
        }
        let name = format!("{}-{}-{tag}.json", stamp_millis(), bytes.len());
        let path = dir.join(&name);
        match std::fs::write(&path, bytes) {
            Ok(()) => println!("  settings history: wrote {name}"),
            Err(e) => println!("  settings history: {}: {e}", path.display()),
        }
    }

    /// Called once per [`super::load_reporting`], whatever `dir()` names.
    /// Keeps the document this process actually used (tagged with which
    /// [`Source`] it came from, since a cache hit and a live fetch answering
    /// the same generation are not the same evidence), and separately fetches
    /// and decompresses the engine's own URL, saved under its own tag.
    pub fn record(dir: &Path, body: Option<&str>, source: &Source) {
        record_with(dir, body, source, fetch_engine_copy)
    }

    /// Split from [`record`] so the engine-side fetch can be swapped out in a
    /// test: this is a unit test, not an integration test, and it should not
    /// depend on this host's network reaching a real Roblox CDN to pass.
    fn record_with(
        dir: &Path,
        body: Option<&str>,
        source: &Source,
        fetch: impl FnOnce() -> Result<String, String>,
    ) {
        if let Some(body) = body {
            let tag = match source {
                Source::Explicit => "cordial-androidapp-explicit",
                Source::FreshCache => "cordial-androidapp-freshcache",
                Source::Fetched => "cordial-androidapp-fetched",
                Source::StaleCache => "cordial-androidapp-stalecache",
                Source::Nothing(_) => "cordial-androidapp-nothing",
                Source::EngineDocumentCached => "cordial-enginedoc-cached",
                Source::EngineDocumentFetched => "cordial-enginedoc-fetched",
            };
            save(dir, tag, body.as_bytes());
        }
        match fetch() {
            Ok(decompressed) => save(dir, "engine-googleandroidapp", decompressed.as_bytes()),
            Err(why) => println!("  settings history: engine copy: {why}"),
        }
    }

    fn fetch_engine_copy() -> Result<String, String> {
        super::fetch_and_decompress(super::ENGINE_URL)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn save_names_the_file_with_timestamp_and_byte_size() {
            let dir = std::env::temp_dir().join("cordial-settings-history-test");
            let _ = std::fs::remove_dir_all(&dir);
            save(&dir, "sometag", b"12345");
            let entries: Vec<_> = std::fs::read_dir(&dir)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            assert_eq!(entries.len(), 1, "{entries:?}");
            let name = &entries[0];
            assert!(name.ends_with("-5-sometag.json"), "{name}");
            // The prefix before the byte count is a millisecond timestamp:
            // digits only, non-empty, so a reader can sort the directory by
            // filename and get chronological order for free.
            let millis = name.split('-').next().unwrap();
            assert!(!millis.is_empty() && millis.chars().all(|c| c.is_ascii_digit()), "{name}");
        }

        #[test]
        fn dir_reads_the_env_switch() {
            std::env::remove_var("CORDIAL_SETTINGS_HISTORY");
            assert_eq!(dir(), None);
            std::env::set_var("CORDIAL_SETTINGS_HISTORY", "/tmp/somewhere");
            assert_eq!(dir(), Some(PathBuf::from("/tmp/somewhere")));
            std::env::remove_var("CORDIAL_SETTINGS_HISTORY");
        }

        #[test]
        fn record_with_no_body_writes_nothing_for_the_cordial_side() {
            let dir = std::env::temp_dir().join("cordial-settings-history-test-nobody");
            let _ = std::fs::remove_dir_all(&dir);
            record_with(&dir, None, &Source::Fetched, || Err("no network in a unit test".into()));
            // No body and a failed engine fetch: nothing to save on either
            // side, so `save` is never called and the directory is never
            // even created -- a missing directory here is the pass case, not
            // an error to unwrap past.
            let entries: Vec<_> = std::fs::read_dir(&dir)
                .into_iter()
                .flatten()
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            assert!(entries.is_empty(), "{entries:?}");
        }

        #[test]
        fn record_tags_the_cordial_side_document_with_its_source() {
            let dir = std::env::temp_dir().join("cordial-settings-history-test-tagged");
            let _ = std::fs::remove_dir_all(&dir);
            record_with(&dir, Some("{}"), &Source::StaleCache, || {
                Err("no network in a unit test".into())
            });
            let entries: Vec<_> = std::fs::read_dir(&dir)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            assert_eq!(entries.len(), 1, "{entries:?}");
            assert!(entries[0].ends_with("-2-cordial-androidapp-stalecache.json"), "{}", entries[0]);
        }

        #[test]
        fn record_saves_the_engine_copy_under_its_own_tag_when_the_fetch_succeeds() {
            let dir = std::env::temp_dir().join("cordial-settings-history-test-enginecopy");
            let _ = std::fs::remove_dir_all(&dir);
            record_with(&dir, None, &Source::Fetched, || Ok(r#"{"applicationSettings":{}}"#.into()));
            let entries: Vec<_> = std::fs::read_dir(&dir)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            assert_eq!(entries.len(), 1, "{entries:?}");
            assert!(entries[0].ends_with("-engine-googleandroidapp.json"), "{}", entries[0]);
        }
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn cordial_owned_keys_do_not_reach_roblox_settings() {
        // `CordialGraphicsBackend` asks Cordial whether to offer the engine a
        // Vulkan loader, which is a question the engine has no idea it is being
        // asked. It rides the flag layering for that machinery's precedence and
        // provenance, not because it is a FastFlag.
        assert!(!is_roblox_flag("CordialGraphicsBackend"));
        assert!(!is_roblox_flag(crate::graphics::KEY));
        // Roblox's own prefixes are untouched. `DFFlag...` matters most: it is
        // the one this file has a measured control for.
        for key in ["DFFlagRbxTransportUseRtcioRna", "FFlagDebugDisplayFPS", "FStringTest"] {
            assert!(is_roblox_flag(key), "{key}");
        }
    }
    use super::*;

    fn map(pairs: &[(&str, serde_json::Value)]) -> serde_json::Map<String, serde_json::Value> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    }

    #[test]
    fn an_override_replaces_an_existing_flag() {
        let doc = r#"{"applicationSettings":{"DFFlagX":"True","FFlagY":"False"}}"#;
        let (out, n) = merge(doc, map(&[("DFFlagX", serde_json::json!(false))])).unwrap();
        assert_eq!(n, 1);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["applicationSettings"]["DFFlagX"], "false");
        // untouched flags survive
        assert_eq!(v["applicationSettings"]["FFlagY"], "False");
    }

    #[test]
    fn non_string_values_are_converted_rather_than_rejected() {
        // Roblox stores every value as a string, so a config file written with
        // a bare `7` or `true` has to work rather than be a silent no-op.
        let doc = r#"{"applicationSettings":{}}"#;
        let (out, _) = merge(
            doc,
            map(&[("FIntA", serde_json::json!(7)), ("FFlagB", serde_json::json!(true))]),
        )
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["applicationSettings"]["FIntA"], "7");
        assert_eq!(v["applicationSettings"]["FFlagB"], "true");
    }

    #[test]
    fn a_document_without_application_settings_is_refused() {
        assert!(merge(r#"{"nope":{}}"#, map(&[("FFlagX", serde_json::json!(1))])).is_err());
    }

    #[test]
    fn an_error_body_is_not_mistaken_for_settings() {
        // What the CDN actually returns for a bad application name. It is valid
        // JSON, so only the shape check distinguishes it.
        let err = r#"{"errors":[{"code":1,"message":"The application name is invalid."}]}"#;
        assert!(!plausible(err));
        assert!(plausible(r#"{"applicationSettings":{"FFlagX":"True"}}"#));
    }

    /// Exercises `load_base` rather than `load` on purpose. `load` merges the
    /// user's own overrides — since ADR-013 that is `<profile>/flags.json`, and
    /// before it `~/.config/cordial/flags.json` — so going through it would
    /// make this test read the developer's real profile and fail for anyone who
    /// has overrides, which is exactly what it did once one existed. The
    /// behaviour under test is path-versus-network, and that is `load_base`.
    #[test]
    fn engine_cache_path_never_collides_with_the_androidapp_cache_path() {
        // The two endpoints are now known to sometimes disagree (2026-09-24
        // finding), so a shared cache file would let one silently answer for
        // the other -- exactly the kind of "looks like a flag problem" bug
        // this module already warns about for a mismatched application name.
        assert_ne!(cache_path(), engine_cache_path());
    }

    #[test]
    fn an_explicit_path_bypasses_the_network() {
        let dir = std::env::temp_dir().join("cordial-cs-test");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("settings.json");
        std::fs::write(&p, r#"{"applicationSettings":{}}"#).unwrap();
        let (body, source) = load_base(Some(p.to_str().unwrap()));
        assert_eq!(body.as_deref(), Some(r#"{"applicationSettings":{}}"#));
        assert!(matches!(source, Source::Explicit));
    }

    /// GitHub issue #21, reporter A: an unreadable `--client-settings` path
    /// used to come back as a bare `None`, identical to "the CDN was
    /// unreachable and there was no cache either" -- the two failures a
    /// launch log could not tell apart. `Source::Nothing` exists so it can.
    #[test]
    fn an_unreadable_explicit_path_says_why_rather_than_just_no() {
        let (body, source) = load_base(Some("/nonexistent/cordial-cs-test-path.json"));
        assert_eq!(body, None);
        match source {
            Source::Nothing(why) => assert!(why.contains("/nonexistent/cordial-cs-test-path.json")),
            _ => panic!("expected Source::Nothing, got a different source"),
        }
    }
}

/// `nativeInitClientSettings`, kept so the settings can be handed over again
/// after the engine's own reloader has thrown Cordial's away. Zero until the
/// launch path has delivered them once. An address rather than a pointer
/// because a raw pointer is not `Send`; it names an export of a library that
/// stays loaded for the life of the process.
static SETTINGS_NATIVE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// When the settings were first delivered. The log watcher starts on whichever
/// Player log is newest, which before this session has written its own is the
/// previous session's, read from the top -- so a reload line inside the first
/// minute is a replay, the real first one arriving at 120 s.
static DELIVERED_AT: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// Remember the native that took the settings at launch, for
/// [`reassert_after_reload`].
pub fn remember_native(native: *mut std::ffi::c_void) {
    let _ = DELIVERED_AT.set(std::time::Instant::now());
    SETTINGS_NATIVE.store(native as usize, std::sync::atomic::Ordering::Release);
}

/// Hand the engine Cordial's settings document again, because its
/// `DynamicFastVariableReloader` has just replaced it.
///
/// **The reloader runs every 120 s and drops every `DF*` override, including
/// ones for keys Roblox's document does not contain.** That last part is what
/// [`apply_overrides`] left unestablished. Two readings point the same way:
/// `DFLogHttpTrace`, absent from Roblox's document, logged until 120.1 s and
/// stopped; and `DFIntTaskSchedulerTargetFps`, also absent, set to a 165 Hz
/// display's rate at launch, gave 165 fps for about two minutes and the
/// engine's own 60 afterwards (reported on a real session, not yet measured
/// with a control). The engine's logs here show `finished flag fetch` at
/// 120.2 s and 240.8 s, and 88 of them across a 10,668 s session, so it is
/// periodic and not a one-off.
///
/// The whole document is sent rather than only the overrides because the
/// reloader looks to replace rather than merge (`INFERRED`, from the above): a
/// document of overrides alone would then take Roblox's own values with it. A second call mid-run is the
/// mechanism measured to work in [`apply_overrides`]'s three-arm experiment.
///
/// Off its own thread because [`load`] may fetch over HTTP, and this is called
/// from the main thread's pump. Skipped when no `DF*` override is in force,
/// since then there is nothing the reloader took away.
pub fn reassert_after_reload() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static BUSY: AtomicBool = AtomicBool::new(false);

    let native = SETTINGS_NATIVE.load(Ordering::Acquire);
    if native == 0 {
        return;
    }
    if DELIVERED_AT.get().is_none_or(|t| t.elapsed() < Duration::from_secs(60)) {
        return;
    }
    // The control for the measurement above: with it off, the reloader's
    // revert is what you get.
    if std::env::var("CORDIAL_REASSERT_FLAGS").is_ok_and(|v| v == "0") {
        println!("  flags: engine reloaded its settings; CORDIAL_REASSERT_FLAGS=0, leaving them");
        return;
    }
    let resolved = crate::flags::resolve(crate::flags::collect());
    if !resolved.keys().any(|k| k.starts_with("DF") && is_roblox_flag(k)) {
        return;
    }
    if BUSY.swap(true, Ordering::AcqRel) {
        return;
    }
    std::thread::spawn(move || {
        // Not straight away. Sent as soon as the line appeared, the document
        // reached the engine 50 ms after the fetch finished and still lost to
        // it on one reload in two: a `CORDIAL_FPS_CAP=5` run went to the
        // uncapped idle rate after the 120 s reload and back to 5 after the
        // 240 s one, both re-sends answering 0. The reloader evidently applies
        // what it fetched a little after saying it finished.
        std::thread::sleep(Duration::from_secs(5));
        if let Some(settings) = load(None) {
            // SAFETY: `native` is the address of `nativeInitClientSettings`,
            // resolved against the loaded libroblox.so, which is never unloaded.
            match unsafe {
                cordial_linker_sys::game_activity::init_client_settings(
                    native as *mut std::ffi::c_void,
                    &settings,
                    "",
                    "",
                )
            } {
                Ok(code) => println!(
                    "  flags: engine reloaded its settings; overrides handed back -> {code}"
                ),
                Err(e) => println!("  flags: engine reloaded its settings; handing overrides back failed: {e}"),
            }
        }
        BUSY.store(false, Ordering::Release);
    });
}
