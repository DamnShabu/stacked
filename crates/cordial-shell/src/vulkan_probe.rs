//! Ask the Vulkan loader what GPUs it can actually drive.
//!
//! **Why this exists.** Finding `libvulkan.so.1` and a driver manifest on disk
//! says a driver is installed, not that it works: a broken NVIDIA driver, a
//! manifest that names a library which is not there, and a session with no GPU
//! access all read as healthy from the files alone, and the engine then falls
//! back to OpenGL ES without a word. Creating an instance and listing the
//! physical devices is what the engine does first, so it is the smallest
//! question with the real answer -- and it is the only place a CPU renderer
//! (llvmpipe) shows up as what it is. [`crate::gtk_renderer`] uses the answer
//! to choose GTK's renderer before the game window exists. (Upstream Cordial's
//! doctor uses it as well; `stacked doctor` still checks files only.)
//!
//! **It runs in a child process.** Enumerating devices loads every installed
//! driver into the caller, and a driver that faults or hangs at load would
//! take the client with it before it had drawn anything. [`probe`] runs
//! `--vulkan-probe` on this same binary with a deadline, and the child's crash
//! or hang is an answer ([`Failure::Crashed`], [`Failure::TimedOut`]), not a
//! fault in the client.
//!
//! The FFI is hand-written and small on purpose: `vkCreateInstance`,
//! `vkEnumeratePhysicalDevices`, `vkGetPhysicalDeviceProperties2` and
//! `vkDestroyInstance`, through `vkGetInstanceProcAddr` from a `dlopen`ed
//! loader. Nothing links against Vulkan, so a machine without it still runs
//! the shell. Structures whose layout is long and irrelevant here
//! (`VkPhysicalDeviceLimits`) are an opaque, over-sized, 8-aligned block; the
//! fields that are read sit at the start, at offsets fixed by the Vulkan
//! specification.

use std::ffi::{c_char, c_void, CStr, CString};
use std::io::Read;
use std::time::{Duration, Instant};

/// `VkPhysicalDeviceType`, the four the specification names besides "other".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Other,
    Integrated,
    Discrete,
    Virtual,
    /// A software renderer: llvmpipe, SwiftShader, lavapipe. Vulkan works and
    /// the GPU is not being used.
    Cpu,
}

impl Kind {
    fn from_raw(raw: u32) -> Kind {
        match raw {
            1 => Kind::Integrated,
            2 => Kind::Discrete,
            3 => Kind::Virtual,
            4 => Kind::Cpu,
            _ => Kind::Other,
        }
    }

    fn word(self) -> &'static str {
        match self {
            Kind::Other => "other",
            Kind::Integrated => "integrated",
            Kind::Discrete => "discrete",
            Kind::Virtual => "virtual",
            Kind::Cpu => "cpu",
        }
    }

    fn from_word(word: &str) -> Kind {
        [Kind::Integrated, Kind::Discrete, Kind::Virtual, Kind::Cpu]
            .into_iter()
            .find(|k| k.word() == word)
            .unwrap_or(Kind::Other)
    }
}

/// One physical device, as Vulkan describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub vendor_id: u32,
    pub device_id: u32,
    pub kind: Kind,
    pub name: String,
    /// `VkPhysicalDeviceDriverProperties::driverName`, when the driver
    /// answers it (Vulkan 1.2 or `VK_KHR_driver_properties`). `None` is "the
    /// driver did not say", not "unknown driver".
    pub driver_name: Option<String>,
    pub driver_info: Option<String>,
    /// The raw `driverVersion`, which each vendor packs its own way; use
    /// [`Device::driver_version_text`].
    pub driver_version: u32,
    pub api_version: u32,
}

impl Device {
    /// The vendor by name, from the PCI ids the specification says
    /// `vendorID` carries. Anything unlisted is its hex id rather than a guess.
    pub fn vendor_name(&self) -> String {
        match self.vendor_id {
            0x10de => "NVIDIA".into(),
            0x1002 | 0x1022 => "AMD".into(),
            0x8086 => "Intel".into(),
            0x13b5 => "ARM".into(),
            0x5143 => "Qualcomm".into(),
            0x106b => "Apple".into(),
            // The Khronos-assigned ids `VkVendorId` reserves for drivers that
            // are not a PCI vendor's: Mesa's software rasterisers and friends.
            0x10001 => "Vivante".into(),
            0x10002 => "VeriSilicon".into(),
            0x10003 => "Kazan".into(),
            0x10004 => "Codeplay".into(),
            0x10005 => "Mesa".into(),
            other => format!("vendor {other:#06x}"),
        }
    }

    /// The driver version in the way its vendor writes it. NVIDIA packs
    /// 10.8.8.6 bits (the number the driver's own installer prints); every
    /// other vendor here uses Vulkan's own 7.10.12 encoding, which is what
    /// Mesa reports.
    pub fn driver_version_text(&self) -> String {
        let v = self.driver_version;
        if self.vendor_id == 0x10de {
            format!("{}.{}.{}", v >> 22, (v >> 14) & 0xff, (v >> 6) & 0xff)
        } else {
            format!("{}.{}.{}", v >> 22, (v >> 12) & 0x3ff, v & 0xfff)
        }
    }

    /// `1.3.296`-style text for `apiVersion`.
    pub fn api_version_text(&self) -> String {
        format!("{}.{}.{}", (self.api_version >> 22) & 0x7f, (self.api_version >> 12) & 0x3ff, self.api_version & 0xfff)
    }

    /// One line, for the doctor: `NVIDIA GeForce RTX 4060, NVIDIA 555.58.02`.
    pub fn summary(&self) -> String {
        let driver = match &self.driver_name {
            Some(name) => format!("{name} {}", self.driver_version_text()),
            None => format!("driver {}", self.driver_version_text()),
        };
        format!("{} ({}), {driver}, Vulkan {}", self.name, self.vendor_name(), self.api_version_text())
    }
}

/// Why no device list came back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// `libvulkan.so.1` could not be loaded.
    NoLoader,
    /// The loader is there and `vkCreateInstance` failed. `-9` is
    /// `VK_ERROR_INCOMPATIBLE_DRIVER`: the loader found no working driver.
    Instance(i32),
    /// The loader created an instance and listed no device.
    NoDevices,
    /// The probe process did not answer in time.
    TimedOut,
    /// The probe process died, or could not be started. Carries what it said.
    Crashed(String),
}

// ---------------------------------------------------------------------------
// The probe itself. Runs in the child.

type VkInstance = *mut c_void;
type VkPhysicalDevice = *mut c_void;
type VkResult = i32;
type PfnVoid = Option<unsafe extern "C" fn()>;

#[repr(C)]
struct ApplicationInfo {
    s_type: u32,
    p_next: *const c_void,
    application_name: *const c_char,
    application_version: u32,
    engine_name: *const c_char,
    engine_version: u32,
    api_version: u32,
}

#[repr(C)]
struct InstanceCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    application_info: *const ApplicationInfo,
    enabled_layer_count: u32,
    enabled_layer_names: *const *const c_char,
    enabled_extension_count: u32,
    enabled_extension_names: *const *const c_char,
}

/// `VkPhysicalDeviceProperties` is 824 bytes on every 64-bit target and has
/// `u64` members, so 8-aligned. Read by offset; see the module comment.
#[repr(C)]
struct Properties {
    raw: [u64; 103],
}

#[repr(C)]
struct Properties2 {
    s_type: u32,
    p_next: *mut c_void,
    properties: Properties,
}

#[repr(C)]
struct DriverProperties {
    s_type: u32,
    p_next: *mut c_void,
    driver_id: u32,
    driver_name: [c_char; 256],
    driver_info: [c_char; 256],
    conformance: [u8; 4],
}

const STRUCTURE_TYPE_APPLICATION_INFO: u32 = 0;
const STRUCTURE_TYPE_INSTANCE_CREATE_INFO: u32 = 1;
const STRUCTURE_TYPE_PHYSICAL_DEVICE_PROPERTIES_2: u32 = 1_000_059_001;
const STRUCTURE_TYPE_PHYSICAL_DEVICE_DRIVER_PROPERTIES: u32 = 1_000_196_000;
/// `VK_MAKE_API_VERSION(0, 1, 1, 0)`: the version that made
/// `vkGetPhysicalDeviceProperties2` core.
const API_1_1: u32 = (1 << 22) | (1 << 12);

fn text_of(buf: &[c_char]) -> Option<String> {
    // SAFETY: the buffers are zero-initialised and the driver writes at most
    // their length; `from_ptr` stops at the first NUL, which is inside them.
    let s = unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// Create an instance and list the devices, in this process.
///
/// Public only for the `--vulkan-probe` mode of the shell binary; everything
/// else should call [`probe`], which does not risk the caller on a bad driver.
pub fn enumerate_here() -> Result<Vec<Device>, Failure> {
    let name = CString::new("libvulkan.so.1").expect("no NUL in a literal");
    // SAFETY: `name` is NUL-terminated and outlives the call. The handle is
    // deliberately never closed: the child exits straight afterwards, and
    // `dlclose` on a Vulkan loader with drivers loaded is a known way to fault.
    let lib = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
    if lib.is_null() {
        return Err(Failure::NoLoader);
    }
    let sym = |lib: *mut c_void, name: &str| -> *mut c_void {
        let c = CString::new(name).expect("no NUL in a literal");
        // SAFETY: `lib` is a live handle and `c` is NUL-terminated.
        unsafe { libc::dlsym(lib, c.as_ptr()) }
    };
    let get_proc = sym(lib, "vkGetInstanceProcAddr");
    if get_proc.is_null() {
        return Err(Failure::NoLoader);
    }
    // SAFETY: `vkGetInstanceProcAddr` has exactly this signature.
    let get_proc: unsafe extern "C" fn(VkInstance, *const c_char) -> PfnVoid = unsafe { std::mem::transmute(get_proc) };
    let load = |instance: VkInstance, name: &str| -> PfnVoid {
        let c = CString::new(name).expect("no NUL in a literal");
        // SAFETY: `c` is NUL-terminated; a null instance is valid for the
        // three global commands, and a real one for the rest.
        unsafe { get_proc(instance, c.as_ptr()) }
    };

    // `c"..."` needs Rust 1.77 and the workspace floor is 1.75.
    let app_name = CString::new("cordial doctor").expect("no NUL in a literal");
    let app = ApplicationInfo {
        s_type: STRUCTURE_TYPE_APPLICATION_INFO,
        p_next: std::ptr::null(),
        application_name: app_name.as_ptr(),
        application_version: 1,
        engine_name: std::ptr::null(),
        engine_version: 0,
        api_version: API_1_1,
    };
    let info = InstanceCreateInfo {
        s_type: STRUCTURE_TYPE_INSTANCE_CREATE_INFO,
        p_next: std::ptr::null(),
        flags: 0,
        application_info: &app,
        enabled_layer_count: 0,
        enabled_layer_names: std::ptr::null(),
        enabled_extension_count: 0,
        enabled_extension_names: std::ptr::null(),
    };
    let Some(create) = load(std::ptr::null_mut(), "vkCreateInstance") else {
        return Err(Failure::NoLoader);
    };
    // SAFETY: `vkCreateInstance` has this signature; `info` and `app` live
    // through the call; the out-pointer is a valid local.
    let create: unsafe extern "C" fn(*const InstanceCreateInfo, *const c_void, *mut VkInstance) -> VkResult =
        unsafe { std::mem::transmute(create) };
    let mut instance: VkInstance = std::ptr::null_mut();
    // SAFETY: as above.
    let made = unsafe { create(&info, std::ptr::null(), &mut instance) };
    if made != 0 || instance.is_null() {
        return Err(Failure::Instance(made));
    }

    let result = list_devices(instance, &load);

    if let Some(destroy) = load(instance, "vkDestroyInstance") {
        // SAFETY: `vkDestroyInstance(instance, allocator)`; `instance` is the
        // one created above and is not used again.
        let destroy: unsafe extern "C" fn(VkInstance, *const c_void) = unsafe { std::mem::transmute(destroy) };
        // SAFETY: as above; a null allocator is the specified default.
        unsafe { destroy(instance, std::ptr::null()) };
    }
    result
}

fn list_devices(instance: VkInstance, load: &dyn Fn(VkInstance, &str) -> PfnVoid) -> Result<Vec<Device>, Failure> {
    let Some(enumerate) = load(instance, "vkEnumeratePhysicalDevices") else {
        return Err(Failure::NoLoader);
    };
    // SAFETY: the documented signature of `vkEnumeratePhysicalDevices`.
    let enumerate: unsafe extern "C" fn(VkInstance, *mut u32, *mut VkPhysicalDevice) -> VkResult =
        unsafe { std::mem::transmute(enumerate) };
    let mut count = 0u32;
    // SAFETY: a null device array with a count out-parameter is the
    // specified way to ask how many there are.
    let r = unsafe { enumerate(instance, &mut count, std::ptr::null_mut()) };
    if r != 0 {
        return Err(Failure::Instance(r));
    }
    if count == 0 {
        return Err(Failure::NoDevices);
    }
    let mut devices: Vec<VkPhysicalDevice> = vec![std::ptr::null_mut(); count as usize];
    // SAFETY: `devices` holds `count` slots, which is what `count` says.
    let r = unsafe { enumerate(instance, &mut count, devices.as_mut_ptr()) };
    // VK_INCOMPLETE (5) is a list that grew between the two calls; what fit
    // is still real devices.
    if r != 0 && r != 5 {
        return Err(Failure::Instance(r));
    }
    devices.truncate(count as usize);

    let props2 = load(instance, "vkGetPhysicalDeviceProperties2");
    let props1 = load(instance, "vkGetPhysicalDeviceProperties");
    let mut out = Vec::new();
    for device in devices {
        let mut driver = DriverProperties {
            s_type: STRUCTURE_TYPE_PHYSICAL_DEVICE_DRIVER_PROPERTIES,
            p_next: std::ptr::null_mut(),
            driver_id: 0,
            driver_name: [0; 256],
            driver_info: [0; 256],
            conformance: [0; 4],
        };
        let mut props = Properties2 {
            s_type: STRUCTURE_TYPE_PHYSICAL_DEVICE_PROPERTIES_2,
            p_next: (&mut driver as *mut DriverProperties).cast(),
            properties: Properties { raw: [0; 103] },
        };
        if let Some(f) = props2 {
            // SAFETY: the documented signature; `props` is an oversized,
            // correctly typed out-structure whose `pNext` chain is `driver`.
            let f: unsafe extern "C" fn(VkPhysicalDevice, *mut Properties2) = unsafe { std::mem::transmute(f) };
            // SAFETY: `device` came from the enumeration above and `props` is
            // valid for the call.
            unsafe { f(device, &mut props) };
        } else if let Some(f) = props1 {
            // SAFETY: as above, for the Vulkan 1.0 command.
            let f: unsafe extern "C" fn(VkPhysicalDevice, *mut Properties) = unsafe { std::mem::transmute(f) };
            // SAFETY: as above.
            unsafe { f(device, &mut props.properties) };
        } else {
            continue;
        }
        let bytes: &[u8] = {
            // SAFETY: `raw` is 824 initialised bytes, and any bit pattern is a
            // valid `u8`.
            unsafe { std::slice::from_raw_parts(props.properties.raw.as_ptr().cast::<u8>(), 824) }
        };
        let word = |at: usize| u32::from_ne_bytes(bytes[at..at + 4].try_into().expect("four bytes"));
        // Offsets from `VkPhysicalDeviceProperties`: apiVersion, driverVersion,
        // vendorID, deviceID, deviceType, then deviceName[256].
        let name_bytes = &bytes[20..20 + 256];
        let name = name_bytes.split(|&b| b == 0).next().map(|b| String::from_utf8_lossy(b).trim().to_string());
        let answered_driver = props2.is_some() && driver.driver_name[0] != 0;
        out.push(Device {
            api_version: word(0),
            driver_version: word(4),
            vendor_id: word(8),
            device_id: word(12),
            kind: Kind::from_raw(word(16)),
            name: name.filter(|n| !n.is_empty()).unwrap_or_else(|| "unnamed device".into()),
            driver_name: answered_driver.then(|| text_of(&driver.driver_name)).flatten(),
            driver_info: answered_driver.then(|| text_of(&driver.driver_info)).flatten(),
        });
    }
    if out.is_empty() {
        return Err(Failure::NoDevices);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// The wire between the child and the caller: one tab-separated line per device.

/// What the child prints for one device. Tabs and newlines in a driver's own
/// strings are replaced, so a device is always exactly one line.
pub fn to_line(d: &Device) -> String {
    let clean = |s: &str| s.replace(['\t', '\n', '\r'], " ");
    format!(
        "device\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
        d.vendor_id,
        d.device_id,
        d.kind.word(),
        d.api_version,
        d.driver_version,
        clean(&d.name),
        clean(d.driver_name.as_deref().unwrap_or("")),
        clean(d.driver_info.as_deref().unwrap_or("")),
    )
}

pub fn from_line(line: &str) -> Option<Device> {
    let mut f = line.split('\t');
    if f.next()? != "device" {
        return None;
    }
    let mut num = || f.next()?.parse::<u32>().ok();
    let vendor_id = num()?;
    let device_id = num()?;
    let kind = Kind::from_word(f.next()?);
    let mut num = || f.next()?.parse::<u32>().ok();
    let api_version = num()?;
    let driver_version = num()?;
    let name = f.next()?.to_string();
    let some = |s: Option<&str>| s.map(str::to_string).filter(|s| !s.is_empty());
    let driver_name = some(f.next());
    let driver_info = some(f.next());
    Some(Device { vendor_id, device_id, kind, name, driver_name, driver_info, driver_version, api_version })
}

/// The whole of `--vulkan-probe`: print one line per device, or one `failure`
/// line. Returns the exit status.
pub fn run_probe_mode() -> u8 {
    match enumerate_here() {
        Ok(devices) => {
            for d in &devices {
                println!("{}", to_line(d));
            }
            0
        }
        Err(Failure::NoLoader) => {
            println!("failure\tno-loader");
            0
        }
        Err(Failure::Instance(code)) => {
            println!("failure\tinstance\t{code}");
            0
        }
        Err(Failure::NoDevices) => {
            println!("failure\tno-devices");
            0
        }
        Err(other) => {
            println!("failure\tother\t{other:?}");
            0
        }
    }
}

/// Parse what the child printed.
pub fn parse_output(text: &str) -> Result<Vec<Device>, Failure> {
    let mut devices = Vec::new();
    for line in text.lines() {
        if let Some(d) = from_line(line) {
            devices.push(d);
            continue;
        }
        if let Some(rest) = line.strip_prefix("failure\t") {
            let mut parts = rest.split('\t');
            return Err(match parts.next() {
                Some("no-loader") => Failure::NoLoader,
                Some("no-devices") => Failure::NoDevices,
                Some("instance") => Failure::Instance(parts.next().and_then(|c| c.parse().ok()).unwrap_or(-1)),
                _ => Failure::Crashed(rest.to_string()),
            });
        }
    }
    if devices.is_empty() {
        Err(Failure::Crashed("the probe printed nothing".into()))
    } else {
        Ok(devices)
    }
}

/// Run the probe in a child of this binary, giving up after `limit`.
pub fn probe(limit: Duration) -> Result<Vec<Device>, Failure> {
    let exe = std::env::current_exe().map_err(|e| Failure::Crashed(format!("cannot find this program: {e}")))?;
    probe_with(&exe, limit)
}

/// [`probe`] against an explicit program, so the timeout and crash paths are
/// testable with something that misbehaves on purpose.
pub fn probe_with(exe: &std::path::Path, limit: Duration) -> Result<Vec<Device>, Failure> {
    let mut child = std::process::Command::new(exe)
        .arg("--vulkan-probe")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        // A driver's own complaints go to stderr and are not the answer.
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| Failure::Crashed(format!("could not start the probe: {e}")))?;
    let mut stdout = child.stdout.take().expect("stdout was piped");
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        text
    });
    let deadline = Instant::now() + limit;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                // Our own child, by handle: nothing else is touched.
                let _ = child.kill();
                let _ = child.wait();
                return Err(Failure::TimedOut);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(Failure::Crashed(format!("could not wait for the probe: {e}"))),
        }
    };
    let text = reader.join().unwrap_or_default();
    if !status.success() {
        return Err(Failure::Crashed(format!("the probe ended with {status}")));
    }
    parse_output(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nvidia() -> Device {
        Device {
            vendor_id: 0x10de,
            device_id: 0x2882,
            kind: Kind::Discrete,
            name: "NVIDIA GeForce RTX 4060".into(),
            driver_name: Some("NVIDIA".into()),
            driver_info: Some("555.58.02".into()),
            // 555.58.02 packed the way NVIDIA packs it.
            driver_version: (555 << 22) | (58 << 14) | (2 << 6),
            api_version: (1 << 22) | (3 << 12) | 277,
        }
    }

    #[test]
    fn a_device_survives_the_line_it_is_sent_as() {
        let d = nvidia();
        assert_eq!(from_line(&to_line(&d)), Some(d));
        let bare = Device { driver_name: None, driver_info: None, ..nvidia() };
        assert_eq!(from_line(&to_line(&bare)), Some(bare));
    }

    #[test]
    fn a_driver_string_cannot_split_a_device_across_lines() {
        let d = Device { name: "odd\tname\nwith breaks".into(), ..nvidia() };
        let line = to_line(&d);
        assert_eq!(line.lines().count(), 1);
        assert_eq!(from_line(&line).unwrap().name, "odd name with breaks");
    }

    #[test]
    fn nvidia_and_mesa_pack_their_driver_versions_differently() {
        assert_eq!(nvidia().driver_version_text(), "555.58.2");
        let mesa = Device {
            vendor_id: 0x1002,
            driver_version: (25 << 22) | (1 << 12) | 3,
            ..nvidia()
        };
        assert_eq!(mesa.driver_version_text(), "25.1.3");
        assert_eq!(nvidia().api_version_text(), "1.3.277");
    }

    #[test]
    fn a_vendor_with_no_name_here_is_shown_as_its_id() {
        let d = Device { vendor_id: 0xabcd, ..nvidia() };
        assert_eq!(d.vendor_name(), "vendor 0xabcd");
    }

    #[test]
    fn a_probe_that_reports_failure_is_a_failure_not_an_empty_list() {
        assert_eq!(parse_output("failure\tno-loader\n"), Err(Failure::NoLoader));
        assert_eq!(parse_output("failure\tinstance\t-9\n"), Err(Failure::Instance(-9)));
        assert_eq!(parse_output("failure\tno-devices\n"), Err(Failure::NoDevices));
        assert!(matches!(parse_output(""), Err(Failure::Crashed(_))));
        let ok = parse_output(&format!("{}\n", to_line(&nvidia()))).unwrap();
        assert_eq!(ok.len(), 1);
    }

    /// `probe_with` on a script this test process has just written. Another
    /// test forking in parallel can inherit the script's write descriptor for
    /// a moment, and exec then fails with ETXTBSY ("Text file busy"); that
    /// failed CI once on a correct probe. Retry that one error only.
    fn probe_fresh_script(script: &std::path::Path, deadline: Duration) -> Result<Vec<Device>, Failure> {
        for _ in 0..50 {
            match probe_with(script, deadline) {
                Err(Failure::Crashed(why)) if why.contains("Text file busy") => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                other => return other,
            }
        }
        probe_with(script, deadline)
    }

    /// A child that hangs is killed and reported, not waited for.
    #[test]
    fn a_probe_that_hangs_times_out() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("hang.sh");
        std::fs::write(&script, "#!/bin/sh\nexec sleep 30\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let started = Instant::now();
        assert_eq!(probe_fresh_script(&script, Duration::from_millis(300)), Err(Failure::TimedOut));
        assert!(started.elapsed() < Duration::from_secs(5), "the hung child was waited for");
    }

    /// A child that dies is an answer, and the parent is still here to give it.
    #[test]
    fn a_probe_that_crashes_is_reported_as_a_crash() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("crash.sh");
        std::fs::write(&script, "#!/bin/sh\nkill -SEGV $$\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(probe_fresh_script(&script, Duration::from_secs(5)), Err(Failure::Crashed(_))));
    }

    #[test]
    fn the_device_types_are_the_ones_the_specification_numbers() {
        assert_eq!(Kind::from_raw(1), Kind::Integrated);
        assert_eq!(Kind::from_raw(2), Kind::Discrete);
        assert_eq!(Kind::from_raw(3), Kind::Virtual);
        assert_eq!(Kind::from_raw(4), Kind::Cpu);
        assert_eq!(Kind::from_raw(0), Kind::Other);
        assert_eq!(std::mem::size_of::<Properties>(), 824);
    }
}
