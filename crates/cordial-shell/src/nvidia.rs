//! What Cordial does only when the GPU is NVIDIA's, and why each thing is here.
//!
//! **No NVIDIA hardware exists on the development machine, so nothing in this
//! file has run against a real NVIDIA driver.** Every behaviour below is one of
//! two kinds and says which. Some are *observations* about the host that are
//! true whatever the hardware is (parsing `/proc/driver/nvidia/version`). The
//! rest are *reactions to a
//! reported failure*, and are labelled `INFERRED` because what is established
//! is that the failure was reported and what people did about it, never that
//! the reaction fixes it here. Upstream Cordial's
//! `docs/analysis/nvidia-support.md` holds the catalogue with issue numbers;
//! this fork carries the runtime half only, without upstream's diagnostics row,
//! crash-page hints and Flatpak extension check.
//!
//! ## The gate
//!
//! The runtime reads the physical device's `vendorID` out of
//! `vkGetPhysicalDeviceProperties` and everything NVIDIA-shaped keys off that,
//! never off "an NVIDIA kernel module is loaded". A hybrid laptop has the module
//! loaded and renders on the Intel or AMD part half the time; a workaround
//! applied there is a workaround applied to the wrong GPU. The one place this
//! file does look at the kernel module is where the *question* is about the
//! module: which driver series it is, for the advisory.
//!
//! `CORDIAL_FORCE_GPU_VENDOR` exists so the NVIDIA path can be exercised on a
//! machine that has none. It changes what the gate says and nothing the engine
//! sees, in the same sense ADR-042's texture mask is a test-only lever: off by
//! default, named in `--help`, and set by no packaging script.
//!
//! This module is pure on purpose. The three unsafe Vulkan calls that feed it
//! live in `cordial-runtime`, where the rest of that boundary is.

/// NVIDIA's PCI vendor id, which is also what `VkPhysicalDeviceProperties.vendorID`
/// carries for their proprietary driver.
pub const VENDOR_ID: u32 = 0x10DE;

/// The environment variable that overrides the gate. Test-only.
pub const FORCE_VENDOR_ENV: &str = "CORDIAL_FORCE_GPU_VENDOR";

/// Test-only: make the first N calls to the host's
/// `vkGetPhysicalDeviceSurfacePresentModesKHR` fail with `VK_ERROR_UNKNOWN`, so
/// the retry in [`retry`] can be watched recovering on a machine whose driver
/// never fails it. It proves the retry works. It proves nothing about whether a
/// retry cures the real failure.
pub const FAIL_PRESENT_MODES_ENV: &str = "CORDIAL_TEST_FAIL_PRESENT_MODES";

pub fn is_nvidia(vendor_id: u32) -> bool {
    vendor_id == VENDOR_ID
}

// ------------------------------------------------------------ driver version

/// A driver version as NVIDIA packs it into `VkPhysicalDeviceProperties.driverVersion`.
///
/// **Not `VK_MAKE_VERSION`.** NVIDIA uses 10 bits of major, 8 of minor, 8 of
/// patch and 6 of tail (Khronos' own `vulkaninfo` decodes it this way, in
/// `AppGpu::GetDriverVersionString`). Roblox's engine log prints it with the
/// standard 10/10/12 split, which is why a reporter's `550.652.64` is really
/// `550.163.01`: the same bits, read with the wrong ruler. Those pairs were
/// checked against five reporter-stated versions in the Sober corpus; three of
/// them are the tests below.
///
/// **The minor number wraps at 256.** A driver such as 535.309.01 has a minor
/// that does not fit its eight bits, so the packed value cannot be trusted below
/// the major. The major is safe -- it is the top ten bits and the engine's own
/// log agrees with the reporter's -- which is why the advisory keys on it alone
/// and why the kernel module's version, read from `/proc`, is printed beside
/// this one as the authoritative figure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DriverVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl DriverVersion {
    pub fn from_packed(packed: u32) -> Self {
        DriverVersion { major: packed >> 22, minor: (packed >> 14) & 0xff, patch: (packed >> 6) & 0xff }
    }

    /// `major.minor` alone when there is no patch, as NVIDIA prints `550.120`,
    /// and `major.minor.NN` with the patch zero-padded to two digits otherwise,
    /// as it prints `550.163.01` and `580.82.09`.
    pub fn text(&self) -> String {
        if self.patch == 0 {
            format!("{}.{}", self.major, self.minor)
        } else {
            format!("{}.{}.{:02}", self.major, self.minor, self.patch)
        }
    }

    /// Parses the same text back, for the override.
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts = text.trim().split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = match parts.next() {
            Some(p) => p.parse().ok()?,
            None => 0,
        };
        if parts.next().is_some() {
            return None;
        }
        Some(DriverVersion { major, minor, patch })
    }
}

/// Driver series reported to crash Roblox's Vulkan renderer at the first
/// render-target resize.
///
/// **Evidence, so it can be judged rather than trusted.** Sober #2180 (labelled
/// `nvidia`, still open), #2162, #2181, #2182, #2189, #2192, #2201, #2206, #2338
/// and #2351 all end their log at `SceneManager: resizing main targets` and exit
/// 139 (or `ud2`) on 535.x and 550.x drivers, on Roblox builds from June 2026.
/// A Sober maintainer confirmed it as a driver incompatibility ("not a fix other
/// than by changing driver versions"), and reporters on a GTX 1070 and 1070 Ti
/// went from 535/550 to 580 and were running afterwards. OpenGL avoided it.
///
/// **What is not established.** That Cordial's own renderer hits the same
/// crash: it drives the same engine through the same Vulkan calls, but nobody
/// has run it on an NVIDIA 535 or 550. That every 535 and 550 point release
/// crashes: 550.107.02 and 550.120 were reported working on earlier Roblox
/// builds, so the trigger is the engine build as much as the driver. So this is
/// a warning that says "reported", not a refusal, and it names the evidence.
/// `INFERRED` that it applies here.
pub fn driver_advisory(major: u32) -> Option<&'static str> {
    match major {
        535 | 550 => Some(
            "NVIDIA driver series 535 and 550 have been reported to crash Roblox's Vulkan \
             renderer as the window is first resized, on Roblox builds from June 2026. \
             Reporters fixed it by moving to driver 580 or newer, or by switching to \
             OpenGL ES with `stacked config set graphics gles`.",
        ),
        _ => None,
    }
}

// ----------------------------------------------------------------- the gate

/// What the override asked for: a vendor id to gate as, and optionally a driver
/// version to gate the advisory as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VendorOverride {
    pub vendor_id: u32,
    pub driver: Option<DriverVersion>,
}

/// `CORDIAL_FORCE_GPU_VENDOR=0x10de`, `10de`, `4318` or `nvidia`, optionally
/// followed by `@550.163.01`. Anything else is ignored rather than guessed at,
/// so a typo leaves the gate reading the real device.
pub fn parse_override(text: &str) -> Option<VendorOverride> {
    let (vendor, driver) = match text.split_once('@') {
        Some((v, d)) => (v, Some(DriverVersion::parse(d)?)),
        None => (text, None),
    };
    let v = vendor.trim().to_ascii_lowercase();
    let vendor_id = if v == "nvidia" {
        VENDOR_ID
    } else if let Some(hex) = v.strip_prefix("0x") {
        u32::from_str_radix(hex, 16).ok()?
    } else if v.len() == 4 && v.chars().all(|c| c.is_ascii_hexdigit()) && v.chars().any(|c| c.is_ascii_alphabetic()) {
        // `10de` has letters in it; `4318` does not and is decimal, which is why
        // this arm requires one, or a four-digit decimal would be read as hex.
        u32::from_str_radix(&v, 16).ok()?
    } else {
        v.parse::<u32>().ok()?
    };
    Some(VendorOverride { vendor_id, driver })
}

/// The override for this process, read once by the caller.
pub fn override_from_env() -> Option<VendorOverride> {
    parse_override(&std::env::var(FORCE_VENDOR_ENV).ok()?)
}

/// What the runtime gates on for one physical device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gate {
    pub vendor_id: u32,
    pub driver: DriverVersion,
    /// Whether the override changed either value. The identity line says so,
    /// because a log that read as a real NVIDIA device when it was a test
    /// would be the broken instrument this project keeps retracting.
    pub overridden: bool,
}

impl Gate {
    pub fn nvidia(&self) -> bool {
        is_nvidia(self.vendor_id)
    }
}

pub fn gate(real_vendor_id: u32, real_packed_driver: u32, over: Option<VendorOverride>) -> Gate {
    let real = DriverVersion::from_packed(real_packed_driver);
    match over {
        None => Gate { vendor_id: real_vendor_id, driver: real, overridden: false },
        Some(o) => Gate {
            vendor_id: o.vendor_id,
            driver: o.driver.unwrap_or(real),
            overridden: o.vendor_id != real_vendor_id || o.driver.is_some_and(|d| d != real),
        },
    }
}

/// The one line the client prints for the device it renders on.
///
/// Upstream's launcher reads it back out of the client's output (through
/// [`vendor_from_identity_line`]) to decide whether a crash is worth an NVIDIA
/// hint, so anything that reads a Stacked log for the same purpose -- the
/// Roblox manager, a bug report -- is gated on the same fact the runtime gated
/// on, the device's vendor id, rather than on a guess about which GPU the
/// client ended up using.
pub fn identity_line(name: &str, real_vendor_id: u32, device_id: u32, g: &Gate) -> String {
    let mut line = format!(
        "[android] vulkan: physical device \"{name}\" vendor 0x{real_vendor_id:04x} device 0x{device_id:04x}"
    );
    if g.overridden {
        line.push_str(&format!(
            " -- gating as vendor 0x{:04x} driver {} ({FORCE_VENDOR_ENV}, test only)",
            g.vendor_id,
            g.driver.text()
        ));
    } else if is_nvidia(real_vendor_id) {
        line.push_str(&format!(" driver {} (packed; see the kernel module's version for the exact one)", g.driver.text()));
    }
    line
}

/// The vendor id the gate used, read back out of an [`identity_line`].
pub fn vendor_from_identity_line(line: &str) -> Option<u32> {
    let rest = line.split("[android] vulkan: physical device ").nth(1)?;
    // A test override, when there is one, is what the runtime gated on.
    if let Some(after) = rest.split(" -- gating as vendor 0x").nth(1) {
        let hex: String = after.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
        return u32::from_str_radix(&hex, 16).ok();
    }
    let after = rest.split(" vendor 0x").nth(1)?;
    let hex: String = after.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
    u32::from_str_radix(&hex, 16).ok()
}

/// The line the client prints when the advisory applies. Its own marker so
/// whoever reads the log does not have to re-derive the driver series.
pub const ADVISORY_MARKER: &str = "[android] vulkan: NVIDIA advisory:";

/// The line the client prints when the present-modes retry gave up.
pub const PRESENT_MODES_GAVE_UP_MARKER: &str =
    "[android] vulkan: NVIDIA present-modes query still failing";

// ------------------------------------------------------------------- retry

/// `VK_ERROR_UNKNOWN`, `VK_ERROR_INITIALIZATION_FAILED`, `VK_ERROR_SURFACE_LOST_KHR`.
///
/// The three codes reported for `vkGetPhysicalDeviceSurfacePresentModesKHR`
/// failing on hybrid NVIDIA laptops: `-13` in the 2024 reports (Sober #289),
/// `VK_ERROR_UNKNOWN` in 2025-26 (#1040, #1041, #1332, #1955), and
/// `INITIALIZATION_FAILED`/`SURFACE_LOST` on the neighbouring WSI calls (#95,
/// #315). Out-of-memory and device-lost are not retried: a retry cannot help
/// either and the second would only delay a real failure.
pub fn retryable_present_modes_error(rc: i32) -> bool {
    matches!(rc, -13 | -3 | -1_000_000_000)
}

/// Delays before each retry, in milliseconds. About 1.85 seconds in the worst
/// case, spent only when the engine is about to exit on a fatal error anyway.
pub const RETRY_DELAYS_MS: [u64; 4] = [100, 250, 500, 1000];

/// What [`retry`] did.
#[derive(Debug, PartialEq, Eq)]
pub struct Retried {
    pub rc: i32,
    /// Calls made, including the first.
    pub attempts: u32,
}

/// Call `call`; while it returns a retryable error, wait and call again.
///
/// **Why a retry is here at all, and how far to trust it.** On a hybrid laptop
/// under Wayland, the first Vulkan present-modes query of a boot fails for some
/// people and everything after it works. Four independent reporters in the Sober
/// corpus (#699, #1041, #1101, and one more in #1041) got past it by running
/// `vulkaninfo`, `vkcube` or `switcherooctl glxgears` first: "you only have to
/// run something on your GPU once per boot". Sober's own maintainers pointed at
/// a Mesa issue and closed it; nobody established the mechanism, and the
/// research behind this file could not read that Mesa issue. A dGPU that is
/// still waking is the obvious candidate and is `INFERRED`, not known.
///
/// So the claim is narrow. If the failure is a device that is not ready yet, a
/// pause and a second ask is what "run something first" amounts to. If it is
/// anything else, this costs under two seconds on a path that was going to end
/// in `FATAL: Crash` regardless. It never runs when the call succeeds, and the
/// caller only uses it when the gate says NVIDIA.
///
/// Generic over the sleep so a test can watch the schedule without waiting.
pub fn retry(
    mut call: impl FnMut() -> i32,
    mut sleep: impl FnMut(u64),
    retryable: impl Fn(i32) -> bool,
) -> Retried {
    let mut rc = call();
    let mut attempts = 1;
    for delay in RETRY_DELAYS_MS {
        if !retryable(rc) {
            break;
        }
        sleep(delay);
        rc = call();
        attempts += 1;
    }
    Retried { rc, attempts }
}

// ------------------------------------------------ the host's NVIDIA driver

/// The version out of `/proc/driver/nvidia/version`.
///
/// The first line is `NVRM version: NVIDIA UNIX x86_64 Kernel Module  550.163.01  Tue ...`,
/// and `NVIDIA UNIX Open Kernel Module for x86_64  580.65.06  Release Build ...`
/// for the open modules. The version is the first token that has the shape of
/// one, which is more robust than counting words across both spellings.
pub fn parse_kernel_module_version(text: &str) -> Option<String> {
    for token in text.lines().next()?.split_whitespace() {
        let mut parts = token.split('.');
        let (Some(a), Some(b)) = (parts.next(), parts.next()) else { continue };
        let rest: Vec<&str> = parts.collect();
        let numeric = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
        if numeric(a) && a.len() == 3 && numeric(b) && rest.len() <= 1 && rest.iter().all(|r| numeric(r)) {
            return Some(token.to_string());
        }
    }
    None
}

/// The kernel module's version, or `None` when no NVIDIA module is loaded.
///
/// `/proc/driver/nvidia/version` first because it is what is visible inside a
/// Flatpak sandbox's own `/proc`; `/sys/module/nvidia/version` second because it
/// is what Flatpak itself reads to choose the extension, and a plain version
/// string with no prefix.
pub fn host_driver_version() -> Option<String> {
    if let Ok(text) = std::fs::read_to_string("/proc/driver/nvidia/version") {
        if let Some(v) = parse_kernel_module_version(&text) {
            return Some(v);
        }
    }
    let text = std::fs::read_to_string("/sys/module/nvidia/version").ok()?;
    parse_kernel_module_version(text.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack(major: u32, minor: u32, patch: u32) -> u32 {
        (major << 22) | (minor << 14) | (patch << 6)
    }

    /// The layout NVIDIA uses, checked against the one line of `vulkaninfo`
    /// this project has for it: `driverVersion = 580.82.9.0`.
    #[test]
    fn nvidia_packs_ten_eight_eight_six_and_not_the_vulkan_ruler() {
        let v = DriverVersion::from_packed(pack(580, 82, 9));
        assert_eq!(v, DriverVersion { major: 580, minor: 82, patch: 9 });
        assert_eq!(v.text(), "580.82.09");
        assert_eq!(DriverVersion::from_packed(pack(550, 120, 0)).text(), "550.120");
    }

    /// The engine log prints NVIDIA's bits with the standard 10/10/12 ruler.
    /// These three pairs are reporter-stated in the Sober corpus (the log's
    /// figure, then the version the reporter named), and recovering the second
    /// from the first is the whole reason this decode exists.
    #[test]
    fn the_engine_logs_misread_versions_decode_to_what_reporters_named() {
        // The engine's split: major<<22 | minor<<12 | patch.
        let engine = |a: u32, b: u32, c: u32| (a << 22) | (b << 12) | c;
        assert_eq!(DriverVersion::from_packed(engine(550, 652, 64)).text(), "550.163.01");
        assert_eq!(DriverVersion::from_packed(engine(580, 328, 576)).text(), "580.82.09");
        assert_eq!(DriverVersion::from_packed(engine(610, 172, 128)).text(), "610.43.02");
    }

    #[test]
    fn a_version_round_trips_through_its_text() {
        assert_eq!(DriverVersion::parse("550.163.01"), Some(DriverVersion { major: 550, minor: 163, patch: 1 }));
        assert_eq!(DriverVersion::parse("550.120").map(|v| v.text()), Some("550.120".into()));
        assert_eq!(DriverVersion::parse("550"), None);
        assert_eq!(DriverVersion::parse("550.1.2.3"), None);
        assert_eq!(DriverVersion::parse("abc.def"), None);
    }

    #[test]
    fn only_the_two_reported_series_carry_an_advisory() {
        assert!(driver_advisory(535).is_some());
        assert!(driver_advisory(550).is_some());
        for major in [470, 525, 545, 555, 560, 565, 570, 575, 580, 590, 595, 610] {
            assert!(driver_advisory(major).is_none(), "{major} has no reports behind it");
        }
    }

    #[test]
    fn the_override_reads_every_spelling_and_ignores_rubbish() {
        let nv = |d| Some(VendorOverride { vendor_id: 0x10de, driver: d });
        assert_eq!(parse_override("0x10de"), nv(None));
        assert_eq!(parse_override("0X10DE"), nv(None));
        assert_eq!(parse_override("10de"), nv(None));
        assert_eq!(parse_override("4318"), nv(None), "four digits and no letter is decimal");
        assert_eq!(parse_override("nvidia"), nv(None));
        assert_eq!(parse_override("0x10de@550.163.01"), nv(Some(DriverVersion { major: 550, minor: 163, patch: 1 })));
        assert_eq!(parse_override("0x1002").map(|o| o.vendor_id), Some(0x1002));
        assert_eq!(parse_override("banana"), None);
        assert_eq!(parse_override("0x10de@x"), None);
        assert_eq!(parse_override(""), None);
    }

    /// The gate follows the device unless told otherwise, and says when it was
    /// told.
    #[test]
    fn the_gate_reads_the_real_device_and_marks_an_override() {
        let intel = gate(0x8086, pack(25, 0, 0), None);
        assert!(!intel.nvidia() && !intel.overridden);
        let real_nv = gate(0x10de, pack(580, 82, 9), None);
        assert!(real_nv.nvidia() && !real_nv.overridden);
        let forced = gate(0x8086, pack(25, 0, 0), parse_override("nvidia"));
        assert!(forced.nvidia() && forced.overridden);
        let forced_v = gate(0x8086, pack(25, 0, 0), parse_override("nvidia@550.163.01"));
        assert_eq!(forced_v.driver.major, 550);
        // An override that names the vendor the device already has changes
        // nothing and does not claim to.
        assert!(!gate(0x10de, pack(580, 82, 9), parse_override("nvidia")).overridden);
    }

    #[test]
    fn the_identity_line_round_trips_the_vendor_the_gate_used() {
        let intel = gate(0x8086, pack(25, 0, 0), None);
        let line = identity_line("Intel(R) Graphics", 0x8086, 0xa7a0, &intel);
        assert_eq!(vendor_from_identity_line(&line), Some(0x8086));
        assert!(!line.contains("driver"), "no NVIDIA-packed version is claimed for other vendors: {line}");

        let nv = gate(0x10de, pack(550, 163, 1), None);
        let line = identity_line("NVIDIA GeForce RTX 4060", 0x10de, 0x2882, &nv);
        assert_eq!(vendor_from_identity_line(&line), Some(0x10de));
        assert!(line.contains("driver 550.163.01"), "{line}");

        // A forced one reads back as the forced vendor, and says it is a test.
        let forced = gate(0x8086, pack(25, 0, 0), parse_override("nvidia@550.163.01"));
        let line = identity_line("Intel(R) Graphics", 0x8086, 0xa7a0, &forced);
        assert_eq!(vendor_from_identity_line(&line), Some(0x10de));
        assert!(line.contains("test only") && line.contains("vendor 0x8086"), "{line}");
        assert_eq!(vendor_from_identity_line("nothing to see"), None);
    }

    #[test]
    fn only_the_reported_codes_are_retried() {
        for rc in [-13, -3, -1_000_000_000] {
            assert!(retryable_present_modes_error(rc), "{rc}");
        }
        // success, incomplete, out of host memory, out of device memory,
        // device lost, out of date.
        for rc in [0, 5, -1, -2, -4, -1_000_001_004] {
            assert!(!retryable_present_modes_error(rc), "{rc}");
        }
    }

    /// The schedule, watched without waiting for it.
    #[test]
    fn a_failure_is_asked_again_after_each_delay_until_it_clears() {
        let mut results = vec![-13, -13, 0].into_iter();
        let mut slept = Vec::new();
        let r = retry(|| results.next().unwrap(), |ms| slept.push(ms), retryable_present_modes_error);
        assert_eq!(r, Retried { rc: 0, attempts: 3 });
        assert_eq!(slept, vec![100, 250]);
    }

    #[test]
    fn success_on_the_first_call_never_sleeps() {
        let mut slept = 0;
        let r = retry(|| 0, |_| slept += 1, retryable_present_modes_error);
        assert_eq!(r, Retried { rc: 0, attempts: 1 });
        assert_eq!(slept, 0);
    }

    #[test]
    fn a_failure_that_will_not_clear_stops_after_the_schedule_and_reports_it() {
        let mut calls = 0;
        let mut total = 0;
        let r = retry(
            || {
                calls += 1;
                -13
            },
            |ms| total += ms,
            retryable_present_modes_error,
        );
        assert_eq!(r, Retried { rc: -13, attempts: 5 });
        assert_eq!(calls, 5);
        assert_eq!(total, 1850);
    }

    #[test]
    fn an_error_a_retry_cannot_help_is_returned_at_once() {
        let mut slept = 0;
        let r = retry(|| -4, |_| slept += 1, retryable_present_modes_error);
        assert_eq!(r, Retried { rc: -4, attempts: 1 });
        assert_eq!(slept, 0);
    }

    #[test]
    fn the_kernel_module_version_reads_from_both_spellings() {
        let closed = "NVRM version: NVIDIA UNIX x86_64 Kernel Module  550.163.01  Tue Jul  1 12:00:00 UTC 2025\nGCC version:  gcc";
        assert_eq!(parse_kernel_module_version(closed).as_deref(), Some("550.163.01"));
        let open = "NVRM version: NVIDIA UNIX Open Kernel Module for x86_64  580.65.06  Release Build  (dvs-builder@U22-I3-B12-04-2)  Tue Jul";
        assert_eq!(parse_kernel_module_version(open).as_deref(), Some("580.65.06"));
        let two = "NVRM version: NVIDIA UNIX x86_64 Kernel Module  550.120  Fri Sep 13 10:10:01 UTC 2024";
        assert_eq!(parse_kernel_module_version(two).as_deref(), Some("550.120"));
        assert_eq!(parse_kernel_module_version("garbage"), None);
        assert_eq!(parse_kernel_module_version(""), None);
    }
}
