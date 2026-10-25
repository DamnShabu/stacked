//! Fetching the build from APKPure, a mirror that is not Roblox.
//!
//! [ADR-025](../../../docs/adr/ADR-025-fetching-from-a-third-party-mirror.md)
//! is the decision and it turns on one sentence:
//! [`crate::apk_signature`] verifies that Roblox's own signing certificate
//! signed these exact bytes, so a mirror cannot alter the file it serves
//! without the alteration being caught. That check is not a formality bolted on
//! afterwards -- **it is the entire reason this file is allowed to exist**, and
//! a caller that fetches through here and skips it has not saved a step, it has
//! removed the only thing that made the source acceptable.
//!
//! What the check does not cover is worth naming, because it is what the user
//! is actually deciding about when they choose this source. A mirror can be
//! down. A mirror can decline to serve a version, or serve an older one while
//! claiming it is current -- **nothing here verifies the version list**, only
//! the archives. And a mirror sees who asked and when. Those are the terms, and
//! they are why [`super::local`] is tried first and why this is not the default
//! on a metered connection.
//!
//! ## The protocol has no specification, and this is a reader for it
//!
//! The endpoint answers with a length-delimited binary encoding that is
//! protocol-buffer-shaped and has no published schema, no `.proto`, and no
//! promise of stability. There is no way to parse it correctly in the sense
//! that word usually means; what there is, is a set of byte patterns that hold
//! on the responses this has been measured against. That makes the reader below
//! **a maintenance surface**, and the only thing that makes a maintenance
//! surface maintainable is a record of what it looked like when it worked.
//!
//! Measured on 2026-08-26 from this machine, `x-abis: x86_64`:
//!
//! ```text
//! HTTP/2 200, 99 073 bytes
//! 15 version markers, newest first
//! newest              2.734.917, version code 2908
//! its download        https://download.pureapk.com/b/APK/...  (a single APK)
//! second marker       2.734.916, code 2904
//! ```
//!
//! The engine already installed on that machine reads `2.734.0.917`, which is
//! the same build from a source that shares no code with this one. That
//! agreement is the control: a reader that mis-parsed the response would have
//! to mis-parse it into the right answer to pass it.
//!
//! ## The download, measured the same day
//!
//! `cargo run --release -p cordial-update --example fetch_probe -- --download`:
//!
//! ```text
//! candidate-0.apk   229 140 095 bytes, one monolithic APK
//! contents          3578 entries, 1835 under assets/,
//!                   lib/{arm64-v8a,armeabi-v7a,x86_64}/libroblox.so
//! verified          44932ea35a17a267372d71b54d1a0cb3da0dca5113e94406ae2fe18090ba1477
//! ```
//!
//! **That fingerprint is the same one the archive Sober downloaded from Google
//! Play carries**, and the two files are not the same file: 229 MB of every
//! ABI against 97 MB of assets plus a 53 MB x86-64 split. Two distribution
//! routes that share nothing, one signing certificate, and this verifier saying
//! so about both.
//!
//! That is the strongest statement available about whether the mirror is
//! serving Roblox's build, and it is the measurement to repeat when anyone asks
//! whether this source is safe. It is also, precisely, the thing ADR-025 said
//! had to be true.
//!
//! ## Three field tags do all the work
//!
//! Naming them makes the scan below legible rather than magical.
//!
//! | Byte | Protobuf meaning |
//! |---|---|
//! | `0x2a` | field 5, length-delimited -- the version code, ASCII decimal |
//! | `0x32` | field 6 -- the version name, ASCII |
//! | `0x3a` | field 7 -- whatever follows the name; its tag is the ASCII colon the scan anchors on |
//! | `0x4a` | field 9 -- a download URL |
//!
//! Lengths are single-byte varints except in the URLs, which is why every
//! length check here is against a value under 128 and the URL length is two
//! bytes.
//!
//! ## Nothing downstream trusts this about architecture
//!
//! The ABI filter is a hint to the service and not a fact about what arrives.
//! The archive is opened and [`crate::apk::LIBRARY_IN_APK`] (the host's own
//! library path -- `lib/x86_64/libroblox.so` on an x86-64 build,
//! `lib/arm64-v8a/libroblox.so` on aarch64) is looked for directly, which is
//! why the broad-filter retry is safe: widening it buys availability and
//! cannot buy a wrong answer.

use super::{Archives, Available, Progress, Provider};
use crate::url_policy;
use crate::Unreachable;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Where the version list is asked for, and which hosts may serve the bytes.
///
/// **The base URL is configurable and the protocol is not**, and that split is
/// the honest version of "an APKPure-compatible endpoint". The response is an
/// undocumented, length-delimited binary blob with no schema and no `.proto`,
/// read by the byte patterns below. Anything answering here has to produce that
/// exact shape. So this is the same bargain as pointing an OpenAI client at a
/// different base URL -- the wire format is fixed and the host is yours, be it
/// a mirror, a caching proxy in front of APKPure, or a copy inside a company
/// network -- and it is not a plugin interface for arbitrary services.
///
/// Saying that plainly matters more than it looks. A setting called "mirror
/// URL" invites somebody to paste an F-Droid or Aptoide address, get a reader
/// that finds no version markers, and read the failure as Cordial being broken.
/// The refusal names the shape for that reason.
///
/// ## Why this is allowed to widen the allow-list
///
/// [`crate::url_policy`] exists to stop the *mirror* sending Cordial somewhere
/// of its own choosing, and it still does: every redirect is re-checked and an
/// unlisted target is refused. What changes is who writes the list, and that is
/// now the user -- who could replace the binary anyway, and who is the one
/// person entitled to decide which third party they talk to.
///
/// **The property that actually protects them is untouched.** Whatever host
/// serves the archive, it installs only if Roblox's own signing certificate
/// signed those exact bytes. A hostile base URL can waste bandwidth, watch who
/// asked, and serve nothing installable. It cannot serve a modified Roblox.
/// That is why a mirror was acceptable at all, and it does not care which one.
#[derive(Debug, Clone)]
pub struct Mirror {
    pub metadata_url: String,
    pub download_hosts: Vec<String>,
    pub headers: Vec<(String, String)>,
}

/// The endpoint measured on 2026-08-26; see the module header for the reading.
const DEFAULT_METADATA: &str =
    "https://api.pureapk.com/m/v3/cms/app_version?hl=en-US&package_name=com.roblox.client";

/// APKPure's own names, plus the CDN it actually serves bytes from.
///
/// `winudf.com` looks out of place beside the other two and is where the
/// download lands, so a list without it refuses every real transfer. It is a
/// third organisation to trust with the transport, and naming it rather than
/// quietly widening the rule is the honest version of that.
const DEFAULT_DOWNLOAD_HOSTS: [&str; 3] = ["pureapk.com", "apkpure.com", "winudf.com"];

/// The APKPure client version code the service keys its response shape on.
///
/// **This is the constant most likely to age out.** When the reader below stops
/// finding markers in a response that is otherwise a healthy 200, this is the
/// first thing to look at.
const X_CV: &str = "3172501";
/// Android 10, the SDK level claimed. Bounds which builds are offered.
const X_SV: &str = "29";
/// An opaque flag the client sends. It must be present; its meaning is not
/// known here and guessing at one would be worse than saying so.
const X_GP: &str = "1";

/// Point Cordial at a different endpoint of the same shape.
///
/// One variable and one URL is the whole of the common case, deliberately --
/// a caching proxy in front of APKPure needs nothing else.
pub const MIRROR_URL_ENV: &str = "CORDIAL_MIRROR_URL";

/// Hosts that may serve the archives, comma separated, when the endpoint above
/// hands back URLs on names it does not itself use.
pub const MIRROR_HOSTS_ENV: &str = "CORDIAL_MIRROR_DOWNLOAD_HOSTS";

impl Default for Mirror {
    fn default() -> Self {
        Mirror {
            metadata_url: DEFAULT_METADATA.to_string(),
            download_hosts: DEFAULT_DOWNLOAD_HOSTS.iter().map(|s| s.to_string()).collect(),
            headers: vec![
                ("x-cv".into(), X_CV.into()),
                ("x-sv".into(), X_SV.into()),
                ("x-gp".into(), X_GP.into()),
            ],
        }
    }
}

impl Mirror {
    /// The configured mirror, or the default one.
    ///
    /// A custom URL with no host list gets **its own host and subdomains, and
    /// nothing else**. That is the conservative reading of what somebody meant
    /// by naming one endpoint, and it is chosen rather than inherited: keeping
    /// APKPure's three CDN names against a URL that has nothing to do with
    /// APKPure would leave three hosts trusted for a reason nobody asked for.
    pub fn configured() -> Result<Self, Unreachable> {
        let Some(url) = std::env::var(MIRROR_URL_ENV).ok().filter(|s| !s.trim().is_empty())
        else {
            return Ok(Mirror::default());
        };
        let url = url.trim().to_string();

        // Checked when it is read, not when it is used, so a typo fails saying
        // which variable was wrong instead of as a transport error four steps
        // later. The host is taken from the URL itself, so this cannot refuse
        // the very endpoint it was just given.
        let host = url_policy::host_of(&url)?;
        url_policy::check(&url, &url_policy::Allowed::exactly(host.clone())).map_err(|e| {
            Unreachable::Malformed {
                url: url.clone(),
                why: format!("{MIRROR_URL_ENV} is not a URL Cordial can fetch from: {e}"),
            }
        })?;

        let hosts = match std::env::var(MIRROR_HOSTS_ENV).ok().filter(|s| !s.trim().is_empty()) {
            Some(list) => list
                .split(',')
                .map(|h| h.trim().to_ascii_lowercase())
                .filter(|h| !h.is_empty())
                .collect(),
            None => vec![host],
        };

        Ok(Mirror { metadata_url: url, download_hosts: hosts, ..Mirror::default() })
    }

    fn metadata_allowed(&self) -> Result<url_policy::Allowed, Unreachable> {
        Ok(url_policy::Allowed::exactly(url_policy::host_of(&self.metadata_url)?))
    }

    fn download_allowed(&self) -> url_policy::Allowed {
        url_policy::Allowed::any_of(self.download_hosts.clone())
    }

    /// The metadata URL with the ABI filter this request wants.
    fn headers_with(&self, abi: &str) -> Vec<(String, String)> {
        let mut h = self.headers.clone();
        h.push(("x-abis".into(), abi.into()));
        h
    }
}

/// The `x-abis` value that gets APKPure's **monolithic** bundle -- not "what
/// Cordial can run" the way it reads, and not `crate::apk::HOST_ABI`.
///
/// **This was briefly made `apk::HOST_ABI` during the aarch64 port, and that
/// was wrong.** The x86_64-only measurement in the module header above (and
/// `docs/analysis/apk-mirrors.md`, "APKPure is not behind. Settled, and the
/// narrow filter is why") recorded that asking with `x-abis: x86_64`
/// specifically returns one 229 MB APK carrying `lib/{arm64-v8a,armeabi-v7a,
/// x86_64}/libroblox.so` -- every ABI, in one file. That was read as "the
/// narrow filter for the host's own ABI", which happened to be true only
/// because the host was x86_64. Asked with `x-abis: arm64-v8a` instead
/// (2.739.691, measured 2026-09-24), APKPure serves something else entirely:
/// two XAPK bundles, `config.armeabi_v7a.apk` in one and `config.arm64_v8a.apk`
/// in the other, neither a plain APK `held()` can look inside without XAPK
/// support this module does not have. The literal string `"x86_64"` is what
/// selects the monolithic response, not "the caller's own ABI" -- an artefact
/// of whatever this mirror indexes its bundles by, not a fact about Android.
///
/// So this stays `"x86_64"` unconditionally, on every host architecture,
/// which is also why x86_64's behaviour is unchanged by this constant existing
/// at all. What makes the result correct for aarch64 is downstream:
/// [`crate::apk::LIBRARY_IN_APK`] in `held()`/`classify()` looks for
/// `lib/arm64-v8a/libroblox.so` inside whatever this filter returns, and the
/// monolithic bundle has always carried it.
///
/// **The known gap, not hidden:** if APKPure ever again lists an ARM-only
/// release with no x86_64 entry at all -- `docs/analysis/apk-mirrors.md`
/// records that this already happened once, 2.735.1138 on 2026-08-26 -- an
/// aarch64 host querying with `"x86_64"` will not see it as the newest
/// version, for the same reason an x86_64 host does not chase an ARM-only
/// release today: `newest()`'s own doc comment below explains why that
/// asymmetry is deliberate on the x86_64 side. On aarch64 it is not deliberate,
/// it is this constant's side effect, and it means an ARM-only Roblox release
/// would be invisible to Cordial on ARM until a matching x86_64 build also
/// ships. Fixing that for real needs XAPK support in `held()`/`classify()`
/// (option (b) in the discussion that produced this comment) -- not attempted
/// here, since it needs synthetic-XAPK tests of its own and this fix's job was
/// to get aarch64 users the build that already exists, not every build that
/// might.
const ABI_EXACT: &str = "x86_64";
/// Every ABI, for the retry. The filtered index is not reliably complete for
/// older versions -- APKPure sometimes omits a bundle that does contain the
/// matching split -- so a download that cannot find its version in the narrow
/// list asks again without one. Left spelled out rather than built from
/// `ABI_EXACT`: this list is APKPure's own vocabulary of Android ABI names, not
/// Cordial's, and only ever needs `arm64-v8a` and `x86_64` added to as Android
/// itself grows a new ABI.
const ABI_BROAD: &str = "arm64-v8a,armeabi-v7a,armeabi,x86,x86_64";

/// Metadata is small. Anything larger than this is not a version list, and
/// reading it into memory would be this fetcher's own denial of service.
const MAX_METADATA: u64 = 4 * 1024 * 1024;
/// One archive. Roblox's is about 150 MB across two files; a gigabyte is
/// generous and still bounded.
const MAX_ARCHIVE: u64 = 1024 * 1024 * 1024;

/// More than this many candidate archives for one version means the response
/// was not understood, and downloading them all to find out would cost
/// gigabytes to answer a question the first one usually answers.
const MAX_CANDIDATES: usize = 4;

const CONNECT: Duration = Duration::from_secs(10);
const METADATA_TRANSFER: Duration = Duration::from_secs(60);
const ARCHIVE_TRANSFER: Duration = Duration::from_secs(900);

#[derive(Debug, Default)]
pub struct ApkPure;

/// Bytes that may appear in a version name.
fn version_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'.' | b'+' | b'_' | b'-')
}

/// Bytes that may appear in a URL, per the shape these URLs actually take.
fn url_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"-@:%._+~#=?&/()".contains(&b)
}

/// Where a version name sits in the response.
#[derive(Debug, Clone)]
struct Marker {
    start: usize,
    /// The offset of the `0x3a` that terminates the name.
    colon: usize,
    name: String,
}

/// Every version name in the response, in file order.
///
/// A marker is a byte run that begins with a digit, is not preceded by another
/// name byte -- so a name is not found inside a longer token -- contains a dot,
/// and is immediately followed by `0x3a`. The four conditions together are what
/// separates a version from every other run of digits in a 99 KB binary blob.
fn markers(d: &[u8]) -> Vec<Marker> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < d.len() {
        if d[i].is_ascii_digit() && (i == 0 || !version_byte(d[i - 1])) {
            let mut j = i;
            while j < d.len() && version_byte(d[j]) {
                j += 1;
            }
            let run = &d[i..j];
            if run.contains(&b'.') && j < d.len() && d[j] == 0x3a {
                if let Ok(name) = std::str::from_utf8(run) {
                    out.push(Marker { start: i, colon: j, name: name.to_string() });
                }
                i = j;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// The version code belonging to `marker`, recovered by scanning backwards.
///
/// The code sits in the field immediately before the name, and the five
/// conditions below are what make "immediately" checkable rather than assumed:
/// the last of them requires the name field's value to begin exactly where the
/// marker begins, with nothing in between. Without it this would happily attach
/// the code of one record to the name of another.
fn code_before(d: &[u8], marker: &Marker) -> Option<u64> {
    let name_len = marker.colon - marker.start;
    if name_len > u8::MAX as usize {
        return None;
    }
    let floor = marker.start.saturating_sub(128);
    for p in (floor..marker.start).rev() {
        if d[p] != 0x2a {
            continue;
        }
        let len = *d.get(p + 1)? as usize;
        if !(1..=20).contains(&len) {
            continue;
        }
        let digits = d.get(p + 2..p + 2 + len)?;
        if !digits.iter().all(u8::is_ascii_digit) {
            continue;
        }
        if *d.get(p + 2 + len)? != 0x32 {
            continue;
        }
        if *d.get(p + 3 + len)? as usize != name_len {
            continue;
        }
        if p + 4 + len != marker.start {
            continue;
        }
        let mut code: u64 = 0;
        for b in digits {
            code = code.checked_mul(10)?.checked_add((b - b'0') as u64)?;
        }
        return (code != 0).then_some(code);
    }
    None
}

/// The download URLs inside one record.
///
/// `APKJ` and `XAPKJ` are the artefact kind followed by the `0x4a` tag; the URL
/// starts two bytes later because the length is a two-byte varint, these URLs
/// being well over 127 bytes.
fn urls_in(record: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at + 4 <= record.len() {
        let Some(found) = record[at..].windows(4).position(|w| w == b"APKJ") else { break };
        let x = at + found;
        let begin = x + 4 + 2;
        let mut j = begin;
        while j < record.len() && url_byte(record[j]) {
            j += 1;
        }
        if let Ok(url) = std::str::from_utf8(&record[begin..j]) {
            if url.starts_with("https://") {
                out.push(url.to_string());
            }
        }
        at = x + 4;
    }
    out
}

/// Ask the service for the version list, with `abi` as the filter.
fn metadata(mirror: &Mirror, abi: &str) -> Result<Vec<u8>, Unreachable> {
    let agent = url_policy::agent(CONNECT, METADATA_TRANSFER);
    let owned = mirror.headers_with(abi);
    let headers: Vec<(&str, &str)> =
        owned.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let (url, mut response) =
        url_policy::walk(&agent, &mirror.metadata_url, &mirror.metadata_allowed()?, &headers)?;

    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        let body = response
            .body_mut()
            .with_config()
            .limit(64 * 1024)
            .read_to_string()
            .unwrap_or_default();
        return Err(Unreachable::Status { url, status, body });
    }

    let mut bytes = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(MAX_METADATA)
        .read_to_end(&mut bytes)
        .map_err(|e| Unreachable::Transport { url: url.clone(), why: e.to_string() })?;
    if bytes.len() as u64 >= MAX_METADATA {
        return Err(Unreachable::Malformed {
            url,
            why: "the version list exceeds its size limit, so it is not a version list".into(),
        });
    }
    Ok(bytes)
}

/// Every version the mirror lists for x86-64, newest first.
///
/// For the Version page, which asks when it is shown and never at startup. The
/// narrow filter only, for the reason `newest` gives: where a version listed
/// only under the broad filter was checked, it had no x86-64 engine in it.
pub fn offered() -> Result<Vec<Available>, Unreachable> {
    let mirror = Mirror::configured()?;
    let body = metadata(&mirror, ABI_EXACT)?;
    let found = versions_in(&body);
    if found.is_empty() {
        return Err(Unreachable::Malformed {
            url: mirror.metadata_url.clone(),
            why: "the version list carries no versions Cordial can read".into(),
        });
    }
    Ok(found)
}

/// The versions in a response, once each, ordered by version code.
///
/// A name with no code beside it is left out rather than listed with a zero:
/// `code_before`'s adjacency check is what says a dotted number is a record's
/// version and not something that happens to precede the tag. So is a name
/// that is not digits and dots, because it becomes a directory name if it is
/// downloaded.
fn versions_in(d: &[u8]) -> Vec<Available> {
    let mut out: Vec<Available> = Vec::new();
    for m in markers(d) {
        if out.iter().any(|a| a.name == m.name) || !crate::store::is_valid_version(&m.name) {
            continue;
        }
        if let Some(code) = code_before(d, &m) {
            out.push(Available { name: m.name.clone(), code });
        }
    }
    out.sort_by_key(|v| std::cmp::Reverse(v.code));
    out
}

/// The download URLs the response offers for exactly `version`.
fn downloads_for(d: &[u8], version: &str) -> Vec<String> {
    let marks = markers(d);
    let mut out: Vec<String> = Vec::new();
    for (i, m) in marks.iter().enumerate() {
        if m.name != version {
            continue;
        }
        let end = marks.get(i + 1).map(|n| n.start).unwrap_or(d.len());
        // One URL per record: the first is the artefact, the rest are the same
        // file behind other names.
        if let Some(url) = urls_in(&d[m.colon + 1..end]).into_iter().next() {
            if !out.contains(&url) {
                out.push(url);
            }
        }
    }
    out
}

/// Stream one URL to `dest`, enforcing the ceiling on bytes that arrive.
///
/// **The ceiling is applied to what arrives, not to `Content-Length`.** A
/// declared length is a claim by the same party supplying the bytes; checking
/// it is a cheap early refusal and must never be the only check.
fn download(
    mirror: &Mirror,
    url: &str,
    cancel: &super::Cancel,
    dest: &Path,
    progress: &mut dyn FnMut(Progress),
) -> Result<(), Unreachable> {
    let agent = url_policy::agent(CONNECT, ARCHIVE_TRANSFER);
    let (final_url, mut response) =
        url_policy::walk(&agent, url, &mirror.download_allowed(), &[])?;

    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(Unreachable::Status { url: final_url, status, body: String::new() });
    }

    let total = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if let Some(t) = total {
        if t > MAX_ARCHIVE {
            return Err(Unreachable::Malformed {
                url: final_url,
                why: format!("the archive declares {t} bytes, which exceeds its size limit"),
            });
        }
    }

    let name = dest.file_name().unwrap_or_default().to_string_lossy().to_string();
    let temporary = dest.with_extension("partial");
    let outcome = (|| -> Result<(), Unreachable> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temporary)
            .map_err(|e| Unreachable::NoSource { why: e.to_string() })?;

        let mut reader = response.body_mut().as_reader();
        let mut buf = vec![0u8; 256 * 1024];
        let mut done: u64 = 0;
        let mut announced: u64 = 0;
        loop {
            let n = reader
                .read(&mut buf)
                .map_err(|e| Unreachable::Transport { url: final_url.clone(), why: e.to_string() })?;
            if n == 0 {
                break;
            }
            done += n as u64;
            // **Between chunks, which is the only place it can be.** A read
            // already in flight is not interruptible, so the granularity here
            // is one 256 KiB read -- imperceptible next to the transfer, and
            // the error unwinds through the same path a failure does, so the
            // partial file is removed rather than left to be mistaken for a
            // resumable download.
            cancel.check()?;
            if done > MAX_ARCHIVE {
                return Err(Unreachable::Malformed {
                    url: final_url.clone(),
                    why: "the archive exceeds its size limit".into(),
                });
            }
            file.write_all(&buf[..n])
                .map_err(|e| Unreachable::NoSource { why: e.to_string() })?;
            // Every megabyte, not every 64. The coarse interval was chosen
            // when the only consumer was a log line; a progress bar that moves
            // four times during a 229 MB download is not a progress bar. The
            // callback is a label update, so the cost of the finer interval is
            // nothing next to the transfer it is reporting.
            if done - announced >= 1024 * 1024 {
                announced = done;
                progress(Progress::Fetching { file: name.clone(), done, total });
            }
        }
        if done == 0 {
            return Err(Unreachable::Malformed {
                url: final_url.clone(),
                why: "the download is empty".into(),
            });
        }
        file.sync_all().map_err(|e| Unreachable::NoSource { why: e.to_string() })?;
        drop(file);

        // A ZIP local header, checked before anything downstream opens it. An
        // HTML error page served with a 200 is caught here rather than three
        // steps later inside a zip reader, where it reads as a corrupt archive.
        let mut head = [0u8; 4];
        let mut f = std::fs::File::open(&temporary)
            .map_err(|e| Unreachable::NoSource { why: e.to_string() })?;
        f.read_exact(&mut head).map_err(|e| Unreachable::NoSource { why: e.to_string() })?;
        if head[0] != b'P'
            || head[1] != b'K'
            || !matches!(head[2], 0x03 | 0x05 | 0x07)
            || !matches!(head[3], 0x04 | 0x06 | 0x08)
        {
            return Err(Unreachable::Malformed {
                url: final_url.clone(),
                why: "what arrived is not a ZIP archive, so it is not an APK".into(),
            });
        }

        std::fs::rename(&temporary, dest)
            .map_err(|e| Unreachable::NoSource { why: e.to_string() })?;
        progress(Progress::Fetching { file: name.clone(), done, total });
        Ok(())
    })();

    if outcome.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    outcome
}

impl Provider for ApkPure {
    fn name(&self) -> &'static str {
        "apkpure"
    }

    fn needs_network(&self) -> bool {
        true
    }

    /// **No retry on the broad filter here, deliberately.** Asking what the
    /// newest version is must be asked about builds Cordial can run; widening
    /// the filter would let an ARM-only release become "the newest version",
    /// and every step after this would then be working towards a build that
    /// cannot start.
    ///
    /// That was written as a hypothetical and it is now a measurement. On
    /// 2026-08-26 the narrow filter answered 2.734.917 while the broad one
    /// answered 2.735.1138, which looked like the filter hiding a newer build.
    /// Reading both of 2.735.1138's XAPK bundles -- by their ZIP central
    /// directories over range requests, 600 kB rather than 276 MB -- shows
    /// `config.armeabi_v7a.apk` in one and `config.arm64_v8a.apk` in the other
    /// and **no `config.x86_64.apk` in either**. The narrow filter was right
    /// and a retry here would have chased a build with nothing in it Cordial
    /// can execute. `docs/analysis/apk-mirrors.md` has the entry lists.
    fn newest(&self, progress: &mut dyn FnMut(Progress)) -> Result<Available, Unreachable> {
        progress(Progress::Asking { provider: self.name() });
        let mirror = Mirror::configured()?;
        let body = metadata(&mirror, ABI_EXACT)?;
        let marks = markers(&body);
        let first = marks.first().ok_or_else(|| Unreachable::Malformed {
            url: mirror.metadata_url.clone(),
            why: "the version list carries no versions. If this endpoint was set with \
                  CORDIAL_MIRROR_URL it is answering in a shape this reader does not \
                  understand; otherwise the service has changed."
                .into(),
        })?;
        let code = code_before(&body, first).ok_or_else(|| Unreachable::Malformed {
            url: mirror.metadata_url.clone(),
            why: format!(
                "the version list names {} and carries no version code for it, so its shape \
                 has changed",
                first.name
            ),
        })?;
        Ok(Available { name: first.name.clone(), code })
    }

    fn fetch(
        &self,
        version: &Available,
        cancel: &super::Cancel,
        into: &Path,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<Archives, Unreachable> {
        if into.read_dir().map(|mut d| d.next().is_some()).unwrap_or(false) {
            return Err(Unreachable::NoSource {
                why: format!(
                    "{} already holds files, and a download writes into an empty directory \
                     so a previous attempt's leftovers cannot be mistaken for this one's",
                    into.display()
                ),
            });
        }

        // The narrow filter, then the broad one. See ABI_BROAD: this buys
        // availability for older versions and cannot buy a wrong answer,
        // because what arrives is opened and checked for the engine.
        let mirror = Mirror::configured()?;
        let mut urls = downloads_for(&metadata(&mirror, ABI_EXACT)?, &version.name);
        if urls.is_empty() {
            urls = downloads_for(&metadata(&mirror, ABI_BROAD)?, &version.name);
        }
        if urls.is_empty() {
            return Err(Unreachable::Malformed {
                url: mirror.metadata_url.clone(),
                why: format!(
                    "the mirror does not offer version {}. It is reachable and it does not \
                     have that build.",
                    version.name
                ),
            });
        }
        if urls.len() > MAX_CANDIDATES {
            return Err(Unreachable::Malformed {
                url: mirror.metadata_url.clone(),
                why: format!(
                    "the mirror offers {} different archives for version {}, which is more \
                     than Cordial will try",
                    urls.len(),
                    version.name
                ),
            });
        }
        let allowed = mirror.download_allowed();
        for url in &urls {
            url_policy::check(url, &allowed)?;
        }

        let mut landed: Vec<PathBuf> = Vec::new();
        let result = (|| -> Result<(), Unreachable> {
            for (i, url) in urls.iter().enumerate() {
                let dest = into.join(format!("candidate-{i}.apk"));
                download(&mirror, url, cancel, &dest, progress)?;
                landed.push(dest);
                // **Stop as soon as what has landed is enough.** The measured
                // ordinary case is one monolithic archive carrying both the
                // engine and the assets -- see the module header, 229 MB in
                // about thirteen seconds -- and downloading the remaining
                // candidates after that one lands would double or quadruple
                // that transfer to answer a question the first archive already
                // answered. `held` is the same check `classify` ends with, run
                // early enough to matter.
                let (engine, assets) = held(&landed);
                if engine.is_some() && assets.is_some() {
                    break;
                }
            }
            Ok(())
        })();
        if let Err(e) = result {
            // No partial candidate set. There is no such thing as most of a
            // build, and leftovers a later run mistakes for a complete download
            // are how assets from one release end up beside an engine from
            // another.
            for p in &landed {
                let _ = std::fs::remove_file(p);
            }
            return Err(e);
        }

        classify(&landed).ok_or_else(|| Unreachable::Malformed {
            url: mirror.metadata_url.clone(),
            why: format!(
                "the mirror served {} archive(s) for version {} and none of them carries \
                 {}, so there is no engine in what arrived",
                landed.len(),
                version.name,
                crate::apk::LIBRARY_IN_APK
            ),
        })
    }
}

/// Which of `files`, if any, holds the engine and which holds the assets.
///
/// Split out of [`classify`] so a caller downloading one candidate at a time
/// can ask, after each one lands, whether it already has enough — a monolithic
/// archive sets both from the one file in a single pass here, which is what
/// lets `fetch` stop after the first download in the ordinary case rather than
/// fetching every candidate before anything is inspected.
fn held(files: &[PathBuf]) -> (Option<PathBuf>, Option<PathBuf>) {
    let mut engine: Option<PathBuf> = None;
    let mut assets: Option<PathBuf> = None;
    for f in files {
        let Ok(file) = std::fs::File::open(f) else { continue };
        let Ok(mut zip) = zip::ZipArchive::new(std::io::BufReader::new(file)) else { continue };
        if engine.is_none() && zip.by_name(crate::apk::LIBRARY_IN_APK).is_ok() {
            engine = Some(f.clone());
        }
        if assets.is_none() && (0..zip.len()).any(|i| {
            zip.by_index(i).map(|e| e.name().starts_with("assets/")).unwrap_or(false)
        }) {
            assets = Some(f.clone());
        }
    }
    (engine, assets)
}

/// Which of the downloaded archives is the base and which holds the engine.
///
/// This reads the archives rather than their names, because the names came from
/// the mirror. A monolithic APK holding both is reported as both, which is what
/// [`Archives`] is for.
fn classify(files: &[PathBuf]) -> Option<Archives> {
    let (engine, assets) = held(files);
    let engine = engine?;
    Some(Archives { base: assets.unwrap_or_else(|| engine.clone()), split: engine })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A response captured from the live service. Absent unless somebody has
    /// run the probe, so the tests that need it skip rather than fake: a
    /// synthetic blob written by this file would only prove the reader agrees
    /// with itself.
    fn captured() -> Option<Vec<u8>> {
        let p = std::env::var_os("CORDIAL_APKPURE_FIXTURE")?;
        std::fs::read(p).ok()
    }

    #[test]
    fn a_run_of_digits_with_no_dot_is_not_a_version() {
        assert!(markers(b"\x00123456\x3a\x00").is_empty());
    }

    #[test]
    fn a_version_inside_a_longer_token_is_not_a_marker() {
        // Preceded by a name byte, so it is part of something else.
        assert!(markers(b"build9.9.9\x3a").is_empty());
        assert_eq!(markers(b"\x009.9.9\x3a")[0].name, "9.9.9");
    }

    #[test]
    fn a_version_not_terminated_by_the_colon_tag_is_not_a_marker() {
        assert!(markers(b"\x009.9.9\x40").is_empty());
    }

    /// **The condition that stops a code being attached to the wrong name.**
    /// Everything else about the backward scan can hold while the two fields
    /// belong to different records; only adjacency rules that out.
    #[test]
    fn a_version_code_separated_from_its_name_is_not_accepted() {
        let mut d = Vec::new();
        d.push(0x2a);
        d.push(4);
        d.extend_from_slice(b"2908");
        d.push(0x32);
        d.push(5);
        d.extend_from_slice(&[0, 0, 0, 0]); // something between, so not adjacent
        d.extend_from_slice(b"1.2.3");
        d.push(0x3a);
        let m = markers(&d);
        assert_eq!(m.len(), 1);
        assert_eq!(code_before(&d, &m[0]), None);
    }

    #[test]
    fn a_well_formed_record_yields_its_code() {
        let mut d = vec![0u8];
        d.push(0x2a);
        d.push(4);
        d.extend_from_slice(b"2908");
        d.push(0x32);
        d.push(5);
        d.extend_from_slice(b"1.2.3");
        d.push(0x3a);
        let m = markers(&d);
        assert_eq!(m[0].name, "1.2.3");
        assert_eq!(code_before(&d, &m[0]), Some(2908));
    }

    fn record(code: &str, name: &str) -> Vec<u8> {
        let mut d = vec![0u8, 0x2a, code.len() as u8];
        d.extend_from_slice(code.as_bytes());
        d.push(0x32);
        d.push(name.len() as u8);
        d.extend_from_slice(name.as_bytes());
        d.push(0x3a);
        d
    }

    /// Listed once each and newest first, whatever order the records came in.
    #[test]
    fn the_version_list_is_deduplicated_and_ordered_by_code() {
        let mut d = record("2908", "2.734.917");
        d.extend(record("3101", "2.738.1397"));
        d.extend(record("2908", "2.734.917"));
        // A dotted number with no code beside it is not a version.
        d.extend_from_slice(b"\x001.2.3\x3a");
        let names: Vec<String> = versions_in(&d).into_iter().map(|a| a.name).collect();
        assert_eq!(names, ["2.738.1397", "2.734.917"]);
    }

    #[test]
    fn a_url_that_is_not_https_is_not_taken_from_a_record() {
        let mut r = b"XAPKJ".to_vec();
        r.extend_from_slice(&[0x00, 0x00]);
        r.extend_from_slice(b"http://evil.example/x.apk");
        assert!(urls_in(&r).is_empty());
    }

    /// The reader against a real response, if one has been captured. This is
    /// the only test here that says anything about the actual service.
    #[test]
    fn the_captured_response_reads_the_way_the_module_header_records() {
        let Some(d) = captured() else {
            eprintln!("skipped: no captured response (set CORDIAL_APKPURE_FIXTURE)");
            return;
        };
        let marks = markers(&d);
        assert!(marks.len() > 3, "expected several versions, found {}", marks.len());
        let code = code_before(&d, &marks[0]).expect("the newest version must have a code");
        assert!(code > 0);
        let urls = downloads_for(&d, &marks[0].name);
        assert!(!urls.is_empty(), "the newest version must offer a download");
        for u in &urls {
            url_policy::check(u, &Mirror::default().download_allowed())
                .unwrap_or_else(|e| panic!("a real download URL must pass the allow-list: {e}"));
        }
        eprintln!("captured: {} at code {code}, {} download(s)", marks[0].name, urls.len());
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cordial-update-mirror-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A zip holding whatever entries are named. Same shape as `apk`'s and
    /// `install`'s own fixture: the entry path is what `held` acts on, and
    /// nothing here is a real Roblox byte.
    fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, body) in entries {
            w.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            std::io::Write::write_all(&mut w, body).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    /// **The common case, and the reason `fetch`'s download loop can stop
    /// after one candidate.** A monolithic archive sets both `engine` and
    /// `assets` from the same file in one pass, because the two conditions in
    /// `held`'s loop are independent checks against the same open zip rather
    /// than a match on which one it "is".
    #[test]
    fn held_finds_both_halves_in_one_monolithic_archive() {
        let dir = scratch("monolithic");
        let one = dir.join("one.apk");
        std::fs::write(
            &one,
            zip_of(&[("assets/x.json", b"{}" as &[u8]), (crate::apk::LIBRARY_IN_APK, b"\x7fELF")]),
        )
        .unwrap();
        let (engine, assets) = held(std::slice::from_ref(&one));
        assert_eq!(engine.as_deref(), Some(one.as_path()));
        assert_eq!(assets.as_deref(), Some(one.as_path()));
    }

    /// **The case that must not stop the loop early.** A real split build
    /// needs both files; asking after only the first has landed must say so
    /// plainly rather than reporting the archive that has arrived as enough.
    #[test]
    fn held_needs_both_files_when_the_build_is_genuinely_split() {
        let dir = scratch("split");
        let base = dir.join("base.apk");
        let split = dir.join("split.apk");
        std::fs::write(&base, zip_of(&[("assets/x.json", b"{}")])).unwrap();
        std::fs::write(&split, zip_of(&[(crate::apk::LIBRARY_IN_APK, b"\x7fELF")])).unwrap();

        let (engine, assets) = held(std::slice::from_ref(&base));
        assert!(engine.is_none(), "the base half alone carries no engine");
        assert!(assets.is_some(), "and it does carry the assets it was checked for");

        let (engine, assets) = held(&[base.clone(), split.clone()]);
        assert_eq!(engine.as_deref(), Some(split.as_path()));
        assert_eq!(assets.as_deref(), Some(base.as_path()));
    }

    #[test]
    fn held_finds_neither_in_an_archive_with_no_engine_and_no_assets() {
        let dir = scratch("neither");
        let apk = dir.join("nothing.apk");
        std::fs::write(&apk, zip_of(&[("AndroidManifest.xml", b"x")])).unwrap();
        let (engine, assets) = held(&[apk]);
        assert!(engine.is_none() && assets.is_none());
    }

    /// `classify` still has to agree with `held` once both halves are in:
    /// this is the seam between the two, kept as a test now that `classify`
    /// is a thin wrapper rather than its own copy of the scan.
    #[test]
    fn classify_still_pairs_a_split_build_correctly() {
        let dir = scratch("classify-split");
        let base = dir.join("base.apk");
        let split = dir.join("split.apk");
        std::fs::write(&base, zip_of(&[("assets/x.json", b"{}")])).unwrap();
        std::fs::write(&split, zip_of(&[(crate::apk::LIBRARY_IN_APK, b"\x7fELF")])).unwrap();
        let archives = classify(&[base.clone(), split.clone()]).expect("both halves are present");
        assert_eq!(archives.base, base);
        assert_eq!(archives.split, split);
    }

    /// Against the real mirror, so run by hand: `cargo test -p cordial-update
    /// -- --ignored the_live_mirror_lists`. The fixtures above pin the parser to
    /// a shape; this is the only check that the shape is still the one served.
    #[test]
    #[ignore = "network"]
    fn the_live_mirror_lists_versions() {
        let listed = offered().expect("the mirror answered with a version list");
        for a in &listed {
            println!("{} ({})", a.name, a.code);
        }
        assert!(!listed.is_empty());
        assert!(listed.windows(2).all(|w| w[0].code > w[1].code), "newest first, no repeats");
    }
}
