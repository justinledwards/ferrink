use std::collections::BTreeMap;
use std::ffi::{CStr, CString};
use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ferrink_platform::{
    DeviceIdentity, FilesystemSpace, FramebufferCapability, FrontLightCapability,
    InputAxisCapability, InputDeviceCapability, InputDeviceId, InputEventAbi, KernelNodeCapability,
    LipcPropertyCapability, MountCapability, PowerCapability, PowerSupplyCapability, ProbeReport,
    ProbeWarning, ProcessCapability, REDACTION_POLICY, RedactionMetadata, RtcCapability,
    ServiceCapability, SourcedValue, StorageCapability, SuspendCapability, SystemCapability,
    UpstartJobCapability, UserlandFloatAbi, UserstoreCapability, redact_serial, redact_text,
};

#[cfg(target_os = "linux")]
use ferrink_platform::{PixelBitfield, PixelLayout};

use crate::elf::read_elf_abi;

const MAX_TEXT_FILE: u64 = 256 * 1024;

pub(crate) fn collect(root: &Path) -> ProbeReport {
    let files = RootedFiles::new(root);
    let mut warnings = Vec::new();
    let system = collect_system(&files, &mut warnings);
    let identity = collect_identity(&files, &system, &mut warnings);
    let framebuffers = collect_framebuffers(&files, &mut warnings);
    let inputs = collect_inputs(&files, &mut warnings);
    let power = collect_power(&files);
    let services = collect_services(&files);
    let storage = collect_storage(&files, &mut warnings);
    let processes = collect_processes(&files);

    if framebuffers.is_empty() {
        warn(
            &mut warnings,
            "framebuffer",
            "not_found",
            "no readable framebuffer capability was found",
        );
    }
    if inputs.is_empty() {
        warn(
            &mut warnings,
            "input",
            "not_found",
            "no input event capability was found",
        );
    }

    ProbeReport {
        schema_version: ferrink_platform::PROBE_SCHEMA_VERSION,
        probe_version: env!("CARGO_PKG_VERSION").to_owned(),
        captured_at_unix_seconds: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        redaction: RedactionMetadata {
            enabled: true,
            policy: REDACTION_POLICY.to_owned(),
            excluded_categories: vec![
                "full_serial_numbers".to_owned(),
                "network_credentials_and_ssids".to_owned(),
                "tokens_and_account_data".to_owned(),
                "ssh_keys".to_owned(),
                "document_names".to_owned(),
                "process_command_lines".to_owned(),
            ],
        },
        identity,
        system,
        framebuffers,
        inputs,
        power,
        services,
        storage,
        processes,
        warnings,
    }
}

struct RootedFiles {
    root: PathBuf,
}

impl RootedFiles {
    fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
        }
    }

    fn is_live_root(&self) -> bool {
        self.root == Path::new("/")
    }

    fn host_path(&self, absolute: &str) -> PathBuf {
        self.root.join(absolute.trim_start_matches('/'))
    }

    fn exists(&self, absolute: &str) -> bool {
        self.host_path(absolute).exists()
    }

    fn read(&self, absolute: &str) -> std::io::Result<String> {
        let path = self.host_path(absolute);
        let metadata = std::fs::metadata(&path)?;
        if metadata.len() > MAX_TEXT_FILE {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "file exceeds probe read limit",
            ));
        }
        let mut content = String::new();
        std::fs::File::open(path)?
            .take(MAX_TEXT_FILE)
            .read_to_string(&mut content)?;
        Ok(content.trim_matches('\0').to_owned())
    }

    fn read_trimmed(&self, absolute: &str) -> Option<String> {
        self.read(absolute)
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    }

    fn entries(&self, absolute: &str) -> Vec<String> {
        let mut entries = std::fs::read_dir(self.host_path(absolute))
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect::<Vec<_>>();
        entries.sort();
        entries
    }
}

fn collect_identity(
    files: &RootedFiles,
    system: &SystemCapability,
    warnings: &mut Vec<ProbeWarning>,
) -> DeviceIdentity {
    let serial_source = ["/proc/usid", "/proc/serial"]
        .into_iter()
        .find_map(|path| files.read_trimmed(path).map(|value| (path, value)));
    let (serial_prefix, serial_redacted, source) = match serial_source {
        Some((path, value)) => match redact_serial(&value) {
            Some(serial) => (
                Some(serial.prefix),
                Some(serial.display),
                Some(path.to_owned()),
            ),
            None => {
                warn(
                    warnings,
                    "identity",
                    "serial_malformed",
                    "identity source did not contain a recognized serial shape",
                );
                (None, None, Some(path.to_owned()))
            }
        },
        None => {
            warn(
                warnings,
                "identity",
                "serial_unavailable",
                "no supported read-only serial source was present",
            );
            (None, None, None)
        }
    };
    let model_hint = files
        .read_trimmed("/proc/device-tree/model")
        .map(|value| redact_text(&value))
        .or_else(|| {
            system
                .cpu_fields
                .get("hardware")
                .and_then(|values| values.first().cloned())
        });
    DeviceIdentity {
        serial_prefix,
        serial_redacted,
        serial_source: source,
        model_hint,
    }
}

fn collect_system(files: &RootedFiles, warnings: &mut Vec<ProbeWarning>) -> SystemCapability {
    let firmware = [
        "/etc/prettyversion.txt",
        "/etc/version.txt",
        "/etc/uks/version.txt",
    ]
    .into_iter()
    .filter_map(|source| {
        files.read_trimmed(source).map(|value| SourcedValue {
            source: source.to_owned(),
            value: redact_text(value.lines().next().unwrap_or_default()),
        })
    })
    .collect();
    let kernel_release = files
        .read_trimmed("/proc/sys/kernel/osrelease")
        .map(|value| redact_text(&value));
    let machine = if files.is_live_root() {
        uname_machine()
    } else {
        None
    };
    let cpu_fields = files
        .read("/proc/cpuinfo")
        .ok()
        .map(|contents| parse_cpuinfo(&contents))
        .unwrap_or_default();
    let available_dynamic_loaders = [
        "/lib/ld-linux-armhf.so.3",
        "/lib/ld-linux.so.3",
        "/lib/ld-musl-armhf.so.1",
        "/lib/ld-musl-arm.so.1",
        "/lib64/ld-linux-aarch64.so.1",
        "/lib/ld-musl-aarch64.so.1",
    ]
    .into_iter()
    .filter(|path| files.exists(path))
    .map(str::to_owned)
    .collect::<Vec<_>>();
    let userland_float_abi = infer_userland_float_abi(
        machine.as_deref(),
        std::env::consts::ARCH,
        &available_dynamic_loaders,
    );
    let executable_path = files.host_path("/proc/self/exe");
    let executable_abi = match read_elf_abi(&executable_path) {
        Ok(value) => value,
        Err(error) => {
            warn(
                warnings,
                "system",
                "elf_unavailable",
                &format!("could not inspect the probe executable ELF header: {error}"),
            );
            None
        }
    };
    SystemCapability {
        firmware,
        kernel_release,
        machine,
        compiled_architecture: std::env::consts::ARCH.to_owned(),
        available_dynamic_loaders,
        userland_float_abi,
        input_event_abi: compiled_input_event_abi(),
        cpu_fields,
        executable_abi,
    }
}

fn infer_userland_float_abi(
    machine: Option<&str>,
    compiled_architecture: &str,
    loaders: &[String],
) -> UserlandFloatAbi {
    let looks_arm32 = compiled_architecture == "arm"
        || machine.is_some_and(|machine| {
            let machine = machine.to_ascii_lowercase();
            machine.starts_with("arm") && !machine.contains("aarch64")
        });
    if !looks_arm32 {
        return UserlandFloatAbi::NotApplicable;
    }
    let hard = loaders
        .iter()
        .any(|loader| loader.contains("armhf") || loader.contains("gnueabihf"));
    let soft = loaders
        .iter()
        .any(|loader| loader.ends_with("ld-linux.so.3") || loader.ends_with("ld-musl-arm.so.1"));
    match (hard, soft) {
        (true, false) => UserlandFloatAbi::Hard,
        (false, true) => UserlandFloatAbi::Soft,
        (true, true) => UserlandFloatAbi::Mixed,
        (false, false) => UserlandFloatAbi::Unknown,
    }
}

fn compiled_input_event_abi() -> InputEventAbi {
    #[repr(C)]
    struct LibcInputEvent {
        time: libc::timeval,
        kind: u16,
        code: u16,
        value: i32,
    }
    InputEventAbi {
        pointer_width_bits: usize::BITS as u16,
        libc_timeval_bytes: std::mem::size_of::<libc::timeval>() as u16,
        libc_input_event_bytes: std::mem::size_of::<LibcInputEvent>() as u16,
    }
}

fn parse_cpuinfo(contents: &str) -> BTreeMap<String, Vec<String>> {
    const SAFE_FIELDS: &[&str] = &[
        "processor",
        "model name",
        "cpu architecture",
        "features",
        "cpu implementer",
        "cpu variant",
        "cpu part",
        "cpu revision",
        "hardware",
        "revision",
    ];
    let mut fields = BTreeMap::<String, Vec<String>>::new();
    for line in contents.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        if !SAFE_FIELDS.contains(&key.as_str()) {
            continue;
        }
        let value = redact_text(value.trim());
        let values = fields.entry(key).or_default();
        if !value.is_empty() && !values.contains(&value) {
            values.push(value);
        }
    }
    fields
}

fn uname_machine() -> Option<String> {
    // SAFETY: `libc::utsname` is a C POD structure whose all-zero state is a
    // valid output buffer for `uname`.
    let mut name = unsafe { std::mem::zeroed::<libc::utsname>() };
    // SAFETY: `name` points to a valid, writable `libc::utsname` for the
    // duration of the call.
    if unsafe { libc::uname(&mut name) } != 0 {
        return None;
    }
    // SAFETY: a successful POSIX `uname` call NUL-terminates every returned
    // character array, including `machine`.
    let machine = unsafe { CStr::from_ptr(name.machine.as_ptr()) };
    Some(redact_text(&machine.to_string_lossy()))
}

fn collect_inputs(
    files: &RootedFiles,
    warnings: &mut Vec<ProbeWarning>,
) -> Vec<InputDeviceCapability> {
    let mut devices = Vec::new();
    for event in files
        .entries("/sys/class/input")
        .into_iter()
        .filter(|entry| is_numbered_name(entry, "event"))
    {
        let base = format!("/sys/class/input/{event}/device");
        let capabilities = collect_input_capabilities(files, &base);
        let id = InputDeviceId {
            bus: read_hex_u16(files, &format!("{base}/id/bustype")),
            vendor: read_hex_u16(files, &format!("{base}/id/vendor")),
            product: read_hex_u16(files, &format!("{base}/id/product")),
            version: read_hex_u16(files, &format!("{base}/id/version")),
        };
        let device = format!("/dev/input/{event}");
        let advertised_axes = advertised_input_axis_codes(&capabilities);
        let axes = query_input_axes(&files.host_path(&device), &advertised_axes);
        devices.push(InputDeviceCapability {
            device,
            name: files
                .read_trimmed(&format!("{base}/name"))
                .map(|value| redact_text(&value)),
            id,
            capabilities,
            axes,
        });
    }
    if devices.is_empty()
        && let Some(contents) = files.read_trimmed("/proc/bus/input/devices")
    {
        devices = parse_proc_input_devices(&contents, files);
    }
    if devices.iter().any(|device| {
        let advertised_axes = advertised_input_axis_codes(&device.capabilities);
        !advertised_axes.is_empty() && device.axes.len() != advertised_axes.len()
    }) {
        warn(
            warnings,
            "input",
            "axis_ranges_incomplete",
            "one or more input devices could not answer read-only EVIOCGABS queries",
        );
    }
    devices
}

fn collect_input_capabilities(files: &RootedFiles, base: &str) -> BTreeMap<String, String> {
    let capability_path = format!("{base}/capabilities");
    files
        .entries(&capability_path)
        .into_iter()
        .filter_map(|name| {
            let value = files.read_trimmed(&format!("{capability_path}/{name}"))?;
            if value
                .chars()
                .all(|character| character.is_ascii_hexdigit() || character.is_ascii_whitespace())
            {
                Some((name, value.split_whitespace().collect::<Vec<_>>().join(" ")))
            } else {
                None
            }
        })
        .collect()
}

fn read_hex_u16(files: &RootedFiles, path: &str) -> Option<u16> {
    u16::from_str_radix(files.read_trimmed(path)?.trim_start_matches("0x"), 16).ok()
}

fn parse_proc_input_devices(contents: &str, files: &RootedFiles) -> Vec<InputDeviceCapability> {
    let mut devices = Vec::new();
    for block in contents.split("\n\n") {
        let mut name = None;
        let mut id = InputDeviceId::default();
        let mut event = None;
        let mut capabilities = BTreeMap::new();
        for line in block.lines() {
            if let Some(value) = line.strip_prefix("N: Name=") {
                name = Some(redact_text(value.trim_matches('"')));
            } else if let Some(value) = line.strip_prefix("I: ") {
                for item in value.split_whitespace() {
                    let Some((key, value)) = item.split_once('=') else {
                        continue;
                    };
                    let parsed = u16::from_str_radix(value, 16).ok();
                    match key {
                        "Bus" => id.bus = parsed,
                        "Vendor" => id.vendor = parsed,
                        "Product" => id.product = parsed,
                        "Version" => id.version = parsed,
                        _ => {}
                    }
                }
            } else if let Some(value) = line.strip_prefix("H: Handlers=") {
                event = value
                    .split_whitespace()
                    .find(|handler| is_numbered_name(handler, "event"))
                    .map(str::to_owned);
            } else if let Some(value) = line.strip_prefix("B: ")
                && let Some((kind, bits)) = value.split_once('=')
                && bits.chars().all(|character| {
                    character.is_ascii_hexdigit() || character.is_ascii_whitespace()
                })
            {
                capabilities.insert(
                    kind.to_ascii_lowercase(),
                    bits.split_whitespace().collect::<Vec<_>>().join(" "),
                );
            }
        }
        if let Some(event) = event {
            let device = format!("/dev/input/{event}");
            let advertised_axes = advertised_input_axis_codes(&capabilities);
            devices.push(InputDeviceCapability {
                axes: query_input_axes(&files.host_path(&device), &advertised_axes),
                device,
                name,
                id,
                capabilities,
            });
        }
    }
    devices
}

fn advertised_input_axis_codes(capabilities: &BTreeMap<String, String>) -> Vec<u16> {
    capabilities
        .get("abs")
        .map(|bitmap| input_axis_codes(bitmap, usize::BITS))
        .unwrap_or_default()
}

fn input_axis_codes(bitmap: &str, word_bits: u32) -> Vec<u16> {
    if !(1..=64).contains(&word_bits) {
        return Vec::new();
    }
    let words = bitmap.split_whitespace().rev().collect::<Vec<_>>();
    (0u16..=0x3f)
        .filter(|code| {
            let code = u32::from(*code);
            let word_index = usize::try_from(code / word_bits).ok();
            let bit = code % word_bits;
            word_index
                .and_then(|index| words.get(index))
                .and_then(|word| u64::from_str_radix(word, 16).ok())
                .is_some_and(|word| word & (1_u64 << bit) != 0)
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn query_input_axes(path: &Path, advertised_axes: &[u16]) -> Vec<InputAxisCapability> {
    use std::os::fd::AsRawFd;

    #[repr(C)]
    #[derive(Default)]
    struct InputAbsInfo {
        value: i32,
        minimum: i32,
        maximum: i32,
        fuzz: i32,
        flat: i32,
        resolution: i32,
    }

    let Ok(file) = OpenOptions::new().read(true).open(path) else {
        return Vec::new();
    };
    let mut axes = Vec::new();
    for &code in advertised_axes {
        let mut info = InputAbsInfo::default();
        let request = ioc(2, b'E', 0x40 + code as u32, 24);
        // SAFETY: `file` owns a valid descriptor opened read-only, `request` is
        // an EVIOCGABS read request for `InputAbsInfo`, and `info` is a valid
        // writable output buffer for the duration of the call.
        if unsafe {
            libc::ioctl(
                file.as_raw_fd(),
                request as _,
                &mut info as *mut InputAbsInfo,
            )
        } >= 0
        {
            axes.push(InputAxisCapability {
                code,
                name: axis_name(code).map(str::to_owned),
                minimum: info.minimum,
                maximum: info.maximum,
                fuzz: info.fuzz,
                flat: info.flat,
                resolution: info.resolution,
            });
        }
    }
    axes
}

#[cfg(not(target_os = "linux"))]
fn query_input_axes(_path: &Path, _advertised_axes: &[u16]) -> Vec<InputAxisCapability> {
    Vec::new()
}

#[cfg(target_os = "linux")]
fn ioc(direction: u32, kind: u8, number: u32, size: u32) -> libc::c_ulong {
    ((direction << 30) | (size << 16) | ((kind as u32) << 8) | number) as libc::c_ulong
}

#[cfg(target_os = "linux")]
fn axis_name(code: u16) -> Option<&'static str> {
    match code {
        0x00 => Some("abs_x"),
        0x01 => Some("abs_y"),
        0x2f => Some("abs_mt_slot"),
        0x35 => Some("abs_mt_position_x"),
        0x36 => Some("abs_mt_position_y"),
        0x39 => Some("abs_mt_tracking_id"),
        _ => None,
    }
}

fn collect_power(files: &RootedFiles) -> PowerCapability {
    let rtcs = files
        .entries("/sys/class/rtc")
        .into_iter()
        .filter(|name| is_numbered_name(name, "rtc"))
        .map(|name| {
            let wakealarm = format!("/sys/class/rtc/{name}/wakealarm");
            let host_wakealarm = files.host_path(&wakealarm);
            let exists = host_wakealarm.exists();
            RtcCapability {
                name,
                wakealarm: exists.then_some(wakealarm),
                wakealarm_readable: exists
                    && OpenOptions::new().read(true).open(&host_wakealarm).is_ok(),
                wakealarm_has_write_permission_bits: exists
                    && has_write_permission_bits(&host_wakealarm),
            }
        })
        .collect();
    let suspend = SuspendCapability {
        state_path: files
            .exists("/sys/power/state")
            .then(|| "/sys/power/state".to_owned()),
        states: files
            .read_trimmed("/sys/power/state")
            .map(|value| split_modes(&value))
            .unwrap_or_default(),
        mem_sleep_path: files
            .exists("/sys/power/mem_sleep")
            .then(|| "/sys/power/mem_sleep".to_owned()),
        mem_sleep_modes: files
            .read_trimmed("/sys/power/mem_sleep")
            .map(|value| split_modes(&value))
            .unwrap_or_default(),
    };
    let power_supplies = files
        .entries("/sys/class/power_supply")
        .into_iter()
        .map(|name| {
            let path = format!("/sys/class/power_supply/{name}");
            PowerSupplyCapability {
                supply_type: files
                    .read_trimmed(&format!("{path}/type"))
                    .map(|value| redact_text(&value)),
                properties: files
                    .entries(&path)
                    .into_iter()
                    .filter(|property| safe_kernel_name(property))
                    .collect(),
                name,
            }
        })
        .collect();
    let front_lights = files
        .entries("/sys/class/backlight")
        .into_iter()
        .map(|name| {
            let path = format!("/sys/class/backlight/{name}");
            FrontLightCapability {
                max_brightness: files
                    .read_trimmed(&format!("{path}/max_brightness"))
                    .and_then(|value| value.parse().ok()),
                brightness_property_available: files.exists(&format!("{path}/brightness")),
                actual_brightness_property_available: files
                    .exists(&format!("{path}/actual_brightness")),
                name,
                path,
            }
        })
        .collect();
    PowerCapability {
        rtcs,
        suspend,
        power_supplies,
        front_lights,
        legacy_battery_nodes: collect_kernel_nodes(
            files,
            &[
                "/sys/devices/system/yoshi_battery/yoshi_battery0/battery_capacity",
                "/sys/devices/platform/fsl-usb2-udc/charging",
                "/sys/devices/platform/aplite_charger.0/charging",
                "/sys/devices/system/wario_battery/wario_battery0/battery_capacity",
                "/sys/devices/system/wario_charger/wario_charger0/charging",
            ],
        ),
        hall_sensor_nodes: collect_kernel_nodes(
            files,
            &[
                "/sys/devices/platform/eink_hall/hall_enable",
                "/sys/devices/system/wario_hall/wario_hall0/hall_enable",
                "/sys/devices/system/heisenberg_hall/heisenberg_hall0/hall_enable",
                "/sys/bus/platform/drivers/hall_sensor/rex_hall/hall_enable",
            ],
        ),
        deep_sleep_nodes: collect_kernel_nodes(
            files,
            &[
                "/sys/devices/platform/falconblk/uevent",
                "/var/local/system/powerd/hibernate_session_tracker",
            ],
        ),
    }
}

fn collect_kernel_nodes(files: &RootedFiles, paths: &[&str]) -> Vec<KernelNodeCapability> {
    paths
        .iter()
        .filter(|path| files.exists(path))
        .map(|path| {
            let host_path = files.host_path(path);
            KernelNodeCapability {
                path: (*path).to_owned(),
                readable: OpenOptions::new().read(true).open(&host_path).is_ok(),
                has_write_permission_bits: has_write_permission_bits(&host_path),
            }
        })
        .collect()
}

fn split_modes(value: &str) -> Vec<String> {
    value
        .split_whitespace()
        .map(|mode| mode.trim_matches(['[', ']']).to_owned())
        .filter(|mode| !mode.is_empty())
        .collect()
}

fn has_write_permission_bits(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|metadata| metadata.permissions().mode() & 0o222 != 0)
        .unwrap_or(false)
}

fn collect_services(files: &RootedFiles) -> ServiceCapability {
    const LIPC_PROPERTIES: &[(&str, &str)] = &[
        ("com.lab126.powerd", "battLevel"),
        ("com.lab126.powerd", "isCharging"),
        ("com.lab126.powerd", "status"),
        ("com.lab126.powerd", "flIntensity"),
        ("com.lab126.powerd", "currentAmberLevel"),
        ("com.lab126.powerd", "alsLux"),
        ("com.lab126.powerd", "state"),
        ("com.lab126.powerd", "rtcWakeup"),
        ("com.lab126.powerd", "powerButton"),
        ("com.lab126.powerd", "touchScreenSaverTimeout"),
        ("com.lab126.powerd", "preventScreenSaver"),
        ("com.lab126.cmd", "wirelessEnable"),
        ("com.lab126.pillow", "disableEnablePillow"),
        ("com.lab126.wifid", "cmState"),
        ("com.lab126.wifid", "enable"),
        ("com.lab126.wifid", "signalStrength"),
        ("com.lab126.winmgr", "accelerometer"),
    ];
    let getter = find_executable(files, "lipc-get-prop");
    let lipc_properties = LIPC_PROPERTIES
        .iter()
        .map(|(service, property)| {
            let result = getter
                .as_deref()
                .filter(|_| files.is_live_root())
                .map(|getter| query_lipc(getter, service, property))
                .unwrap_or_else(|| "getter_unavailable".to_owned());
            LipcPropertyCapability {
                service: (*service).to_owned(),
                property: (*property).to_owned(),
                readable: result == "ok",
                result,
            }
        })
        .collect();
    ServiceCapability {
        lipc_getter: getter,
        lipc_properties,
        upstart_jobs: collect_upstart_jobs(files),
        sysv_scripts: files
            .entries("/etc/init.d")
            .into_iter()
            .filter(|name| safe_kernel_name(name))
            .collect(),
    }
}

fn find_executable(files: &RootedFiles, name: &str) -> Option<String> {
    ["/usr/bin", "/usr/sbin", "/bin", "/sbin"]
        .into_iter()
        .map(|directory| format!("{directory}/{name}"))
        .find(|path| files.host_path(path).is_file())
}

fn query_lipc(getter: &str, service: &str, property: &str) -> String {
    let Ok(mut child) = Command::new(getter)
        .args([service, property])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return "spawn_error".to_owned();
    };
    for _ in 0..50 {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return "ok".to_owned(),
            Ok(Some(status)) => {
                return status
                    .code()
                    .map(|code| format!("exit_{code}"))
                    .unwrap_or_else(|| "terminated_by_signal".to_owned());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(_) => return "wait_error".to_owned(),
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    "timeout".to_owned()
}

fn collect_upstart_jobs(files: &RootedFiles) -> Vec<UpstartJobCapability> {
    let mut jobs = BTreeMap::new();
    for directory in ["/etc/init", "/etc/upstart"] {
        for filename in files
            .entries(directory)
            .into_iter()
            .filter(|filename| filename.ends_with(".conf"))
        {
            let name = filename.trim_end_matches(".conf").to_owned();
            if !safe_kernel_name(&name) {
                continue;
            }
            let Some(contents) = files.read_trimmed(&format!("{directory}/{filename}")) else {
                continue;
            };
            let mut job = UpstartJobCapability {
                name: name.clone(),
                start_on: Vec::new(),
                stop_on: Vec::new(),
                respawn: false,
                task: false,
            };
            for line in contents.lines().map(str::trim) {
                if let Some(expression) = line.strip_prefix("start on ") {
                    job.start_on.push(redact_text(expression));
                } else if let Some(expression) = line.strip_prefix("stop on ") {
                    job.stop_on.push(redact_text(expression));
                } else if line == "respawn" {
                    job.respawn = true;
                } else if line == "task" {
                    job.task = true;
                }
            }
            jobs.entry(name).or_insert(job);
        }
    }
    jobs.into_values().collect()
}

fn collect_processes(files: &RootedFiles) -> Vec<ProcessCapability> {
    let mut processes = Vec::new();
    for pid in files
        .entries("/proc")
        .into_iter()
        .filter_map(|entry| entry.parse::<u32>().ok())
    {
        let status = files
            .read(&format!("/proc/{pid}/status"))
            .unwrap_or_default();
        let name = files
            .read_trimmed(&format!("/proc/{pid}/comm"))
            .or_else(|| process_status_field(&status, "Name:").map(str::to_owned));
        let Some(name) = name else {
            continue;
        };
        if !is_relevant_process(&name) {
            continue;
        }
        let parent_pid =
            process_status_field(&status, "PPid:").and_then(|value| value.parse().ok());
        let state = process_status_field(&status, "State:").map(redact_text);
        processes.push(ProcessCapability {
            pid,
            parent_pid,
            name: redact_text(&name),
            state,
        });
    }
    processes.sort_by_key(|process| process.pid);
    processes
}

fn process_status_field<'a>(status: &'a str, field: &str) -> Option<&'a str> {
    status
        .lines()
        .find_map(|line| line.strip_prefix(field))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn is_relevant_process(name: &str) -> bool {
    const MARKERS: &[&str] = &[
        "init",
        "upstart",
        "powerd",
        "cvm",
        "awesome",
        "xorg",
        "framework",
        "lipc",
        "wifid",
        "volumd",
        "otav3",
        "otaupd",
        "usb",
        "watchdog",
        "thermal",
        "pillow",
        "webreader",
        "kfxreader",
        "kfxview",
        "mesquite",
        "browserd",
        "stored",
        "scanner",
        "pmond",
        "phd",
        "blanket",
        "lab126_gui",
        "appmgrd",
        "deviced",
    ];
    let name = name.to_ascii_lowercase();
    MARKERS.iter().any(|marker| name.contains(marker))
}

fn collect_storage(files: &RootedFiles, warnings: &mut Vec<ProbeWarning>) -> StorageCapability {
    let mounts = files
        .read("/proc/self/mountinfo")
        .ok()
        .map(|value| parse_mountinfo(&value))
        .or_else(|| {
            files
                .read("/proc/mounts")
                .ok()
                .map(|value| parse_proc_mounts(&value))
        })
        .unwrap_or_else(|| {
            warn(
                warnings,
                "storage",
                "mounts_unavailable",
                "neither /proc/self/mountinfo nor /proc/mounts was readable",
            );
            Vec::new()
        });
    let userstore_path = ["/mnt/us", "/mnt/base-us"]
        .into_iter()
        .find(|path| files.exists(path))
        .unwrap_or("/mnt/us")
        .to_owned();
    let userstore_mount = mounts
        .iter()
        .find(|mount| mount.mount_point == userstore_path);
    let mut filesystems = Vec::new();
    if let Some(space) = filesystem_space(files, "/") {
        filesystems.push(space);
    }
    if files.exists(&userstore_path)
        && let Some(space) = filesystem_space(files, &userstore_path)
    {
        filesystems.push(space);
    }
    StorageCapability {
        userstore: UserstoreCapability {
            path: userstore_path.clone(),
            exists: files.exists(&userstore_path),
            mounted: userstore_mount.is_some(),
            read_only: userstore_mount.map(|mount| mount.read_only),
        },
        mounts,
        filesystems,
    }
}

fn parse_mountinfo(contents: &str) -> Vec<MountCapability> {
    let mut mounts = Vec::new();
    for line in contents.lines() {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        let Some(separator) = fields.iter().position(|field| *field == "-") else {
            continue;
        };
        if fields.len() <= separator + 2 || fields.len() < 6 {
            continue;
        }
        mounts.push(MountCapability {
            mount_point: sanitize_mount_path(&decode_mount_escape(fields[4])),
            filesystem_type: safe_filesystem_type(fields[separator + 1]),
            read_only: fields[5].split(',').any(|option| option == "ro"),
        });
    }
    mounts
}

fn parse_proc_mounts(contents: &str) -> Vec<MountCapability> {
    contents
        .lines()
        .filter_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            (fields.len() >= 4).then(|| MountCapability {
                mount_point: sanitize_mount_path(&decode_mount_escape(fields[1])),
                filesystem_type: safe_filesystem_type(fields[2]),
                read_only: fields[3].split(',').any(|option| option == "ro"),
            })
        })
        .collect()
}

fn decode_mount_escape(value: &str) -> String {
    value
        .replace("\\040", " ")
        .replace("\\011", "\t")
        .replace("\\012", "\n")
        .replace("\\134", "\\")
}

fn sanitize_mount_path(path: &str) -> String {
    if let Some((prefix, _)) = path.split_once("/documents/") {
        format!("{prefix}/documents/<redacted>")
    } else {
        redact_text(path)
    }
}

fn safe_filesystem_type(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || "._-".contains(*character))
        .take(32)
        .collect()
}

fn filesystem_space(files: &RootedFiles, virtual_path: &str) -> Option<FilesystemSpace> {
    let host_path = files.host_path(virtual_path);
    let c_path = CString::new(host_path.as_os_str().as_bytes()).ok()?;
    // SAFETY: `libc::statvfs` is a C POD structure whose all-zero state is a
    // valid output buffer for `statvfs`.
    let mut stats = unsafe { std::mem::zeroed::<libc::statvfs>() };
    // SAFETY: `c_path` is NUL-terminated and both it and the writable `stats`
    // buffer remain valid for the duration of the call.
    if unsafe { libc::statvfs(c_path.as_ptr(), &mut stats) } != 0 {
        return None;
    }
    let block_size = if stats.f_frsize == 0 {
        stats.f_bsize as u64
    } else {
        stats.f_frsize as u64
    };
    Some(FilesystemSpace {
        path: virtual_path.to_owned(),
        block_size,
        total_bytes: block_size.saturating_mul(stats.f_blocks as u64),
        available_bytes: block_size.saturating_mul(stats.f_bavail as u64),
    })
}

fn is_numbered_name(value: &str, prefix: &str) -> bool {
    value
        .strip_prefix(prefix)
        .is_some_and(|suffix| !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()))
}

fn safe_kernel_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._:-@".contains(character))
}

fn warn(warnings: &mut Vec<ProbeWarning>, subsystem: &str, code: &str, message: &str) {
    warnings.push(ProbeWarning {
        subsystem: subsystem.to_owned(),
        code: code.to_owned(),
        message: redact_text(message),
    });
}

#[cfg(target_os = "linux")]
fn collect_framebuffers(
    files: &RootedFiles,
    warnings: &mut Vec<ProbeWarning>,
) -> Vec<FramebufferCapability> {
    use std::os::fd::AsRawFd;

    const FBIOGET_VSCREENINFO: libc::c_ulong = 0x4600;
    const FBIOGET_FSCREENINFO: libc::c_ulong = 0x4602;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct FbBitfield {
        offset: u32,
        length: u32,
        msb_right: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct FbVarScreeninfo {
        xres: u32,
        yres: u32,
        xres_virtual: u32,
        yres_virtual: u32,
        xoffset: u32,
        yoffset: u32,
        bits_per_pixel: u32,
        grayscale: u32,
        red: FbBitfield,
        green: FbBitfield,
        blue: FbBitfield,
        transp: FbBitfield,
        nonstd: u32,
        activate: u32,
        height: u32,
        width: u32,
        accel_flags: u32,
        pixclock: u32,
        left_margin: u32,
        right_margin: u32,
        upper_margin: u32,
        lower_margin: u32,
        hsync_len: u32,
        vsync_len: u32,
        sync: u32,
        vmode: u32,
        rotate: u32,
        colorspace: u32,
        reserved: [u32; 4],
    }

    #[repr(C)]
    #[derive(Default)]
    struct FbFixScreeninfo {
        id: [u8; 16],
        smem_start: libc::c_ulong,
        smem_len: u32,
        type_: u32,
        type_aux: u32,
        visual: u32,
        xpanstep: u16,
        ypanstep: u16,
        ywrapstep: u16,
        line_length: u32,
        mmio_start: libc::c_ulong,
        mmio_len: u32,
        accel: u32,
        capabilities: u16,
        reserved: [u16; 2],
    }

    fn convert(bitfield: FbBitfield) -> PixelBitfield {
        PixelBitfield {
            offset: bitfield.offset,
            length: bitfield.length,
            msb_right: bitfield.msb_right,
        }
    }

    let mut capabilities = Vec::new();
    for name in files
        .entries("/dev")
        .into_iter()
        .filter(|name| is_numbered_name(name, "fb"))
    {
        let device = format!("/dev/{name}");
        let path = files.host_path(&device);
        let Ok(file) = OpenOptions::new().read(true).open(path) else {
            warn(
                warnings,
                "framebuffer",
                "open_failed",
                &format!("could not open {device} read-only"),
            );
            continue;
        };
        let mut variable = FbVarScreeninfo::default();
        let mut fixed = FbFixScreeninfo::default();
        // SAFETY: `file` owns a valid descriptor opened read-only,
        // `FBIOGET_VSCREENINFO` is a read-only metadata request, and `variable`
        // is a valid writable output buffer matching the kernel ABI structure.
        if unsafe {
            libc::ioctl(
                file.as_raw_fd(),
                FBIOGET_VSCREENINFO as _,
                &mut variable as *mut FbVarScreeninfo,
            )
        } < 0
            // SAFETY: the descriptor remains valid, `FBIOGET_FSCREENINFO` is a
            // read-only metadata request, and `fixed` is the matching writable
            // output buffer.
            || unsafe {
                libc::ioctl(
                    file.as_raw_fd(),
                    FBIOGET_FSCREENINFO as _,
                    &mut fixed as *mut FbFixScreeninfo,
                )
            } < 0
        {
            warn(
                warnings,
                "framebuffer",
                "query_failed",
                &format!("read-only framebuffer information ioctl failed for {device}"),
            );
            continue;
        }
        let driver_id = String::from_utf8_lossy(&fixed.id)
            .trim_matches('\0')
            .to_owned();
        let pixel_layout = if variable.bits_per_pixel == 8
            && (variable.grayscale != 0
                || (variable.red.length == 0
                    && variable.green.length == 0
                    && variable.blue.length == 0))
        {
            PixelLayout::Grayscale8
        } else if variable.red.length > 0 && variable.green.length > 0 && variable.blue.length > 0 {
            PixelLayout::PackedRgb
        } else {
            PixelLayout::Unknown
        };
        capabilities.push(FramebufferCapability {
            device,
            driver_id: redact_text(&driver_id),
            visible_width: variable.xres,
            visible_height: variable.yres,
            virtual_width: variable.xres_virtual,
            virtual_height: variable.yres_virtual,
            x_offset: variable.xoffset,
            y_offset: variable.yoffset,
            line_length: fixed.line_length,
            memory_length: fixed.smem_len,
            bits_per_pixel: variable.bits_per_pixel,
            grayscale: variable.grayscale,
            pixel_layout,
            rotation: variable.rotate,
            red: convert(variable.red),
            green: convert(variable.green),
            blue: convert(variable.blue),
            transparency: convert(variable.transp),
        });
    }
    capabilities
}

#[cfg(not(target_os = "linux"))]
fn collect_framebuffers(
    _files: &RootedFiles,
    _warnings: &mut Vec<ProbeWarning>,
) -> Vec<FramebufferCapability> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use ferrink_platform::UserlandFloatAbi;

    use super::{
        RootedFiles, compiled_input_event_abi, infer_userland_float_abi, input_axis_codes,
        parse_cpuinfo, parse_mountinfo, parse_proc_input_devices, process_status_field,
    };

    #[test]
    fn dynamic_loader_inventory_distinguishes_arm_float_abis() {
        let hard = vec!["/lib/ld-linux-armhf.so.3".to_owned()];
        let soft = vec!["/lib/ld-linux.so.3".to_owned()];
        let mixed = vec![hard[0].clone(), soft[0].clone()];

        assert_eq!(
            infer_userland_float_abi(Some("armv7l"), "arm", &hard),
            UserlandFloatAbi::Hard
        );
        assert_eq!(
            infer_userland_float_abi(Some("armv7l"), "arm", &soft),
            UserlandFloatAbi::Soft
        );
        assert_eq!(
            infer_userland_float_abi(Some("armv7l"), "arm", &mixed),
            UserlandFloatAbi::Mixed
        );
        assert_eq!(
            infer_userland_float_abi(Some("x86_64"), "x86_64", &hard),
            UserlandFloatAbi::NotApplicable
        );
    }

    #[test]
    fn input_event_abi_is_derived_from_compiled_libc() {
        let abi = compiled_input_event_abi();
        assert_eq!(abi.pointer_width_bits, usize::BITS as u16);
        assert_eq!(
            abi.libc_timeval_bytes,
            std::mem::size_of::<libc::timeval>() as u16
        );
        assert!(abi.libc_input_event_bytes >= abi.libc_timeval_bytes + 8);
    }

    #[test]
    fn input_axis_codes_decode_32_bit_kernel_words_most_significant_first() {
        assert_eq!(input_axis_codes("6608000 0", 32), vec![47, 53, 54, 57, 58]);
        assert_eq!(input_axis_codes("100 0", 32), vec![40]);
    }

    #[test]
    fn input_axis_codes_reject_invalid_word_widths_and_words() {
        assert!(input_axis_codes("1", 0).is_empty());
        assert!(input_axis_codes("1", 65).is_empty());
        assert!(input_axis_codes("not-hex", 32).is_empty());
    }

    #[test]
    fn cpuinfo_allowlist_excludes_serial() {
        let fields = parse_cpuinfo(
            "Processor: ARMv7 Processor rev 10\nHardware: Freescale i.MX\nSerial: 123456789\n",
        );
        assert!(fields.contains_key("processor"));
        assert!(!fields.contains_key("serial"));
    }

    #[test]
    fn mount_parser_omits_sources_and_redacts_document_paths() {
        let mounts = parse_mountinfo(
            "24 1 8:1 / / rw - ext3 /dev/mmcblk0p1 rw\n\
             25 24 8:2 / /mnt/us/documents/private rw - vfat /dev/mmcblk0p2 rw\n",
        );
        assert_eq!(mounts[0].mount_point, "/");
        assert_eq!(mounts[1].mount_point, "/mnt/us/documents/<redacted>");
    }

    #[test]
    fn proc_input_parser_ignores_phys_and_unique_identifiers() {
        let fixture = "I: Bus=0018 Vendor=0000 Product=0000 Version=0000\n\
                       N: Name=\"cyttsp\"\n\
                       P: Phys=secret/path\n\
                       U: Uniq=full-identifier\n\
                       H: Handlers=event1\n\
                       B: ABS=2608000 0\n\n";
        let devices =
            parse_proc_input_devices(fixture, &RootedFiles::new(std::path::Path::new("/")));
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].device, "/dev/input/event1");
        assert_eq!(devices[0].name.as_deref(), Some("cyttsp"));
        assert_eq!(
            devices[0].capabilities.get("abs").map(String::as_str),
            Some("2608000 0")
        );
    }

    #[test]
    fn process_status_fields_support_kernels_without_proc_comm() {
        let status = "Name:\tpowerd\nState:\tS (sleeping)\nPPid:\t1\n";

        assert_eq!(process_status_field(status, "Name:"), Some("powerd"));
        assert_eq!(process_status_field(status, "State:"), Some("S (sleeping)"));
        assert_eq!(process_status_field(status, "PPid:"), Some("1"));
        assert_eq!(process_status_field(status, "Uid:"), None);
    }
}
