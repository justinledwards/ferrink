use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const PROBE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeReport {
    pub schema_version: u32,
    pub probe_version: String,
    pub captured_at_unix_seconds: u64,
    pub redaction: RedactionMetadata,
    pub identity: DeviceIdentity,
    pub system: SystemCapability,
    pub framebuffers: Vec<FramebufferCapability>,
    pub inputs: Vec<InputDeviceCapability>,
    pub power: PowerCapability,
    pub services: ServiceCapability,
    pub storage: StorageCapability,
    pub processes: Vec<ProcessCapability>,
    #[serde(default)]
    pub warnings: Vec<ProbeWarning>,
}

impl ProbeReport {
    pub fn from_json(input: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(input)
    }

    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if self.schema_version != PROBE_SCHEMA_VERSION {
            errors.push(format!(
                "unsupported probe schema version {}",
                self.schema_version
            ));
        }
        if !self.redaction.enabled {
            errors.push("probe report redaction must be enabled".to_owned());
        }
        if let Some(prefix) = self.identity.serial_prefix.as_deref()
            && (prefix.is_empty()
                || prefix.len() > 6
                || !prefix
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric()))
        {
            errors.push("serial prefix is not a short alphanumeric prefix".to_owned());
        }
        if let Some(serial) = self.identity.serial_redacted.as_deref() {
            let valid_redaction = serial.split_once('…').is_some_and(|(visible, suffix)| {
                !visible.is_empty() && visible.len() <= 8 && suffix == "REDACTED"
            });
            if !valid_redaction {
                errors.push("serial identity is not in the required redacted form".to_owned());
            }
        }
        for framebuffer in &self.framebuffers {
            if framebuffer.visible_width == 0 || framebuffer.visible_height == 0 {
                errors.push(format!("{} has zero visible geometry", framebuffer.device));
            }
            let minimum_stride = framebuffer
                .visible_width
                .saturating_mul(framebuffer.bits_per_pixel)
                .div_ceil(8);
            if framebuffer.line_length < minimum_stride {
                errors.push(format!(
                    "{} line_length {} is smaller than minimum {}",
                    framebuffer.device, framebuffer.line_length, minimum_stride
                ));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RedactionMetadata {
    pub enabled: bool,
    pub policy: String,
    pub excluded_categories: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceIdentity {
    pub serial_prefix: Option<String>,
    pub serial_redacted: Option<String>,
    pub serial_source: Option<String>,
    pub model_hint: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystemCapability {
    pub firmware: Vec<SourcedValue>,
    pub kernel_release: Option<String>,
    pub machine: Option<String>,
    pub compiled_architecture: String,
    pub available_dynamic_loaders: Vec<String>,
    pub userland_float_abi: UserlandFloatAbi,
    pub input_event_abi: InputEventAbi,
    pub cpu_fields: BTreeMap<String, Vec<String>>,
    pub executable_abi: Option<ElfAbi>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserlandFloatAbi {
    Hard,
    Soft,
    Mixed,
    #[default]
    Unknown,
    NotApplicable,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputEventAbi {
    pub pointer_width_bits: u16,
    pub libc_timeval_bytes: u16,
    pub libc_input_event_bytes: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourcedValue {
    pub source: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElfAbi {
    pub class: ElfClass,
    pub endianness: Endianness,
    pub machine: u16,
    pub flags: u32,
    pub arm_eabi_version: Option<u8>,
    pub arm_float_abi: Option<ArmFloatAbi>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ElfClass {
    Elf32,
    Elf64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Endianness {
    Little,
    Big,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArmFloatAbi {
    Hard,
    Soft,
    Unspecified,
    ConflictingFlags,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FramebufferCapability {
    pub device: String,
    pub driver_id: String,
    pub visible_width: u32,
    pub visible_height: u32,
    pub virtual_width: u32,
    pub virtual_height: u32,
    pub x_offset: u32,
    pub y_offset: u32,
    pub line_length: u32,
    pub memory_length: u32,
    pub bits_per_pixel: u32,
    pub grayscale: u32,
    pub pixel_layout: PixelLayout,
    pub rotation: u32,
    pub red: PixelBitfield,
    pub green: PixelBitfield,
    pub blue: PixelBitfield,
    pub transparency: PixelBitfield,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PixelLayout {
    Grayscale8,
    PackedRgb,
    Unknown,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PixelBitfield {
    pub offset: u32,
    pub length: u32,
    pub msb_right: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputDeviceCapability {
    pub device: String,
    pub name: Option<String>,
    pub id: InputDeviceId,
    pub capabilities: BTreeMap<String, String>,
    pub axes: Vec<InputAxisCapability>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputDeviceId {
    pub bus: Option<u16>,
    pub vendor: Option<u16>,
    pub product: Option<u16>,
    pub version: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputAxisCapability {
    pub code: u16,
    pub name: Option<String>,
    pub minimum: i32,
    pub maximum: i32,
    pub fuzz: i32,
    pub flat: i32,
    pub resolution: i32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PowerCapability {
    pub rtcs: Vec<RtcCapability>,
    pub suspend: SuspendCapability,
    pub power_supplies: Vec<PowerSupplyCapability>,
    pub front_lights: Vec<FrontLightCapability>,
    pub legacy_battery_nodes: Vec<KernelNodeCapability>,
    pub hall_sensor_nodes: Vec<KernelNodeCapability>,
    pub deep_sleep_nodes: Vec<KernelNodeCapability>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KernelNodeCapability {
    pub path: String,
    pub readable: bool,
    pub has_write_permission_bits: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RtcCapability {
    pub name: String,
    pub wakealarm: Option<String>,
    pub wakealarm_readable: bool,
    pub wakealarm_has_write_permission_bits: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuspendCapability {
    pub state_path: Option<String>,
    pub states: Vec<String>,
    pub mem_sleep_path: Option<String>,
    pub mem_sleep_modes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PowerSupplyCapability {
    pub name: String,
    pub supply_type: Option<String>,
    pub properties: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrontLightCapability {
    pub name: String,
    pub path: String,
    pub max_brightness: Option<u32>,
    pub brightness_property_available: bool,
    pub actual_brightness_property_available: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceCapability {
    pub lipc_getter: Option<String>,
    pub lipc_properties: Vec<LipcPropertyCapability>,
    pub upstart_jobs: Vec<UpstartJobCapability>,
    pub sysv_scripts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LipcPropertyCapability {
    pub service: String,
    pub property: String,
    pub readable: bool,
    pub result: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpstartJobCapability {
    pub name: String,
    pub start_on: Vec<String>,
    pub stop_on: Vec<String>,
    pub respawn: bool,
    pub task: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorageCapability {
    pub mounts: Vec<MountCapability>,
    pub filesystems: Vec<FilesystemSpace>,
    pub userstore: UserstoreCapability,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MountCapability {
    pub mount_point: String,
    pub filesystem_type: String,
    pub read_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilesystemSpace {
    pub path: String,
    pub block_size: u64,
    pub total_bytes: u64,
    pub available_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserstoreCapability {
    pub path: String,
    pub exists: bool,
    pub mounted: bool,
    pub read_only: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessCapability {
    pub pid: u32,
    pub parent_pid: Option<u32>,
    pub name: String,
    pub state: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeWarning {
    pub subsystem: String,
    pub code: String,
    pub message: String,
}
