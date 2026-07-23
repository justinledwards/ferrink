//! Reviewed Kindle implementation of the shell device boundary.

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use crate::{
    JACQUARD_HEIGHT, JACQUARD_WIDTH, JacquardPreset, LauncherBackgroundChoice,
    LauncherBackgroundFileName, LauncherBackgroundOptionSnapshot, LiteraryClockCorpus,
    ShellDeviceCommand, ShellDevicePort, ShellDeviceSnapshot, inspect_launcher_background_png,
    load_launcher_background_png, next_literary_clock_interval, render_jacquard_background,
};

const DATE: &str = "/bin/date";
const LIPC_GET: &str = "/usr/bin/lipc-get-prop";
const LIPC_SET: &str = "/usr/bin/lipc-set-prop";
const COMMAND_TIMEOUT: Duration = Duration::from_secs(2);
const COMMAND_POLL_INTERVAL: Duration = Duration::from_millis(20);
const MAX_COMMAND_OUTPUT: u64 = 128;
const MAX_STATUS_FILE: u64 = 64;
const LIGHT_MAXIMUM: i32 = 24;
const LITERARY_CORPUS_PATH: &str = "/mnt/us/ferrink/literary-clock/quotes.psv";
const LITERARY_SETTING_PATH: &str = "/mnt/us/ferrink/literary-clock.settings";
const LITERARY_SETTING_HEADER: &str = "ferrink-literary-clock-v1";
const MAX_LITERARY_SETTING_BYTES: u64 = 64;
const LAUNCHER_BACKGROUND_DIRECTORY: &str = "/mnt/us/ferrink/backgrounds";
const LAUNCHER_BACKGROUND_SETTING_PATH: &str = "/mnt/us/ferrink/background.settings";
const LAUNCHER_BACKGROUND_SETTING_HEADER: &str = "ferrink-launcher-background-v1";
const MAX_LAUNCHER_BACKGROUND_SETTING_BYTES: u64 = 128;
const MAX_BACKGROUND_DIRECTORY_ENTRIES: usize = 64;
const MAX_BACKGROUND_FILE_CHOICES: usize = 15;

/// Reviewed KOA3 status and quick-control adapter.
#[derive(Debug)]
pub struct KindleShellDevicePort {
    literary_corpus: Option<LiteraryClockCorpus>,
    literary_clock_interval_minutes: u16,
    launcher_background_choice: LauncherBackgroundChoice,
    launcher_background_choices: Vec<LauncherBackgroundChoice>,
    launcher_background: Option<slint::Image>,
}

impl KindleShellDevicePort {
    /// Loads the optional corpus and persisted preference without making either
    /// one a shell-startup dependency.
    #[must_use]
    pub fn open() -> Self {
        let literary_corpus =
            match LiteraryClockCorpus::load_optional(Path::new(LITERARY_CORPUS_PATH)) {
                Ok(corpus) => corpus,
                Err(error) => {
                    eprintln!("ferrink-shell: literary corpus disabled: {error}");
                    None
                }
            };
        let literary_clock_interval_minutes = if literary_corpus.is_some() {
            match read_literary_setting(Path::new(LITERARY_SETTING_PATH)) {
                Ok(interval) => interval,
                Err(error) => {
                    eprintln!("ferrink-shell: literary clock preference ignored: {error}");
                    0
                }
            }
        } else {
            0
        };
        let background_directory = Path::new(LAUNCHER_BACKGROUND_DIRECTORY);
        let launcher_background_choices = discover_background_choices(background_directory);
        let requested_background =
            match read_launcher_background_setting(Path::new(LAUNCHER_BACKGROUND_SETTING_PATH)) {
                Ok(choice) => choice,
                Err(error) => {
                    eprintln!("ferrink-shell: background preference ignored: {error}");
                    LauncherBackgroundChoice::Pattern
                }
            };
        let (launcher_background_choice, launcher_background) =
            if launcher_background_choices.contains(&requested_background) {
                match load_optional_background(background_directory, &requested_background) {
                    Ok(background) => (requested_background, background),
                    Err(error) => {
                        eprintln!("ferrink-shell: selected background disabled: {error}");
                        (LauncherBackgroundChoice::Pattern, None)
                    }
                }
            } else {
                if requested_background != LauncherBackgroundChoice::Pattern {
                    eprintln!("ferrink-shell: selected background is no longer installed");
                }
                (LauncherBackgroundChoice::Pattern, None)
            };
        Self {
            literary_corpus,
            literary_clock_interval_minutes,
            launcher_background_choice,
            launcher_background_choices,
            launcher_background,
        }
    }
}

/// Kindle status or reversible-control failure.
#[derive(Debug)]
pub enum KindleShellDeviceError {
    /// A fixed-path command could not be started, waited on, or read.
    CommandIo {
        /// Short non-sensitive operation label.
        operation: &'static str,
        /// Underlying operating-system error.
        source: std::io::Error,
    },
    /// A fixed-path command exceeded its deadline.
    CommandTimedOut(&'static str),
    /// A fixed-path command exited unsuccessfully.
    CommandFailed {
        /// Short non-sensitive operation label.
        operation: &'static str,
        /// Optional process exit code.
        code: Option<i32>,
    },
    /// A property or bounded status file returned an invalid value.
    InvalidValue(&'static str),
    /// A fixed literary-clock preference file could not be read or replaced.
    StateIo {
        /// Short non-sensitive operation label.
        operation: &'static str,
        /// Underlying operating-system error.
        source: std::io::Error,
    },
}

impl fmt::Display for KindleShellDeviceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CommandIo { operation, source } => {
                write!(formatter, "{operation} failed: {source}")
            }
            Self::CommandTimedOut(operation) => write!(formatter, "{operation} timed out"),
            Self::CommandFailed { operation, code } => {
                write!(formatter, "{operation} exited unsuccessfully ({code:?})")
            }
            Self::InvalidValue(operation) => write!(formatter, "{operation} returned invalid data"),
            Self::StateIo { operation, source } => {
                write!(formatter, "{operation} failed: {source}")
            }
        }
    }
}

impl std::error::Error for KindleShellDeviceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::CommandIo { source, .. } | Self::StateIo { source, .. } => Some(source),
            Self::CommandTimedOut(_) | Self::CommandFailed { .. } | Self::InvalidValue(_) => None,
        }
    }
}

impl ShellDevicePort for KindleShellDevicePort {
    type Error = KindleShellDeviceError;

    fn snapshot(&mut self) -> Result<ShellDeviceSnapshot, Self::Error> {
        read_snapshot(self)
    }

    fn apply(&mut self, command: ShellDeviceCommand) -> Result<ShellDeviceSnapshot, Self::Error> {
        match command {
            ShellDeviceCommand::AdjustFrontlight(step) => {
                adjust_light("flIntensity", step, "front-light")?;
            }
            ShellDeviceCommand::AdjustWarmth(step) => {
                adjust_light("currentAmberLevel", step, "warm-light")?;
            }
            ShellDeviceCommand::SetFrontlight(value) => {
                set_light("flIntensity", value, "front-light")?;
            }
            ShellDeviceCommand::SetWarmth(value) => {
                set_light("currentAmberLevel", value, "warm-light")?;
            }
            ShellDeviceCommand::ToggleWifi => toggle_wifi()?,
            ShellDeviceCommand::CycleLiteraryClockInterval => {
                if self.literary_corpus.is_none() {
                    return Err(KindleShellDeviceError::InvalidValue(
                        "literary clock interval",
                    ));
                }
                let interval = next_literary_clock_interval(self.literary_clock_interval_minutes);
                write_literary_setting(Path::new(LITERARY_SETTING_PATH), interval)?;
                self.literary_clock_interval_minutes = interval;
            }
            ShellDeviceCommand::SelectLauncherBackground(index) => {
                let next = self
                    .launcher_background_choices
                    .get(usize::from(index))
                    .cloned()
                    .ok_or(KindleShellDeviceError::InvalidValue(
                        "launcher background choice",
                    ))?;
                let background = match &next {
                    LauncherBackgroundChoice::Pattern => slint::Image::from_rgb8(
                        render_jacquard_background(
                            JACQUARD_WIDTH,
                            JACQUARD_HEIGHT,
                            JacquardPreset::EinkCalm,
                        )
                        .map_err(|_| {
                            KindleShellDeviceError::InvalidValue("generated launcher background")
                        })?,
                    ),
                    LauncherBackgroundChoice::File(filename) => load_launcher_background_png(
                        Path::new(LAUNCHER_BACKGROUND_DIRECTORY)
                            .join(filename.as_str())
                            .as_path(),
                    )
                    .map_err(|_| {
                        KindleShellDeviceError::InvalidValue("image launcher background")
                    })?,
                };
                write_launcher_background_setting(
                    Path::new(LAUNCHER_BACKGROUND_SETTING_PATH),
                    &next,
                )?;
                self.launcher_background_choice = next;
                self.launcher_background = Some(background);
            }
        }
        read_snapshot(self)
    }
}

fn read_snapshot(
    port: &KindleShellDevicePort,
) -> Result<ShellDeviceSnapshot, KindleShellDeviceError> {
    let clock = read_local_clock()?;
    let battery_percent = read_clamped_u8("com.lab126.powerd", "battLevel", 100, "battery")?;
    let charging = read_bool("com.lab126.powerd", "isCharging", "charging")?;
    let frontlight = read_clamped_u8(
        "com.lab126.powerd",
        "flIntensity",
        LIGHT_MAXIMUM,
        "front-light",
    )?;
    let warmth = read_clamped_u8(
        "com.lab126.powerd",
        "currentAmberLevel",
        LIGHT_MAXIMUM,
        "warm-light",
    )?;
    let auto_brightness = read_bool("com.lab126.powerd", "flAuto", "auto-brightness")?;
    let wifi_enabled = read_bool("com.lab126.wifid", "enable", "Wi-Fi enable")?;
    let connection = get_property("com.lab126.wifid", "cmState", "Wi-Fi state")?;
    let wifi = wifi_label(wifi_enabled, connection.as_str()).to_owned();
    let bluetooth = optional_property("com.lab126.btfd", "BTstate", "Bluetooth state")
        .as_deref()
        .map(bluetooth_label)
        .unwrap_or("Unavailable")
        .to_owned();
    let ssh = if process_is_running("dropbear") {
        "On"
    } else {
        "Off"
    }
    .to_owned();
    let usbnet = read_optional_status_file(Path::new("/sys/class/net/usb0/operstate"))
        .as_deref()
        .map(interface_label)
        .unwrap_or("Unavailable")
        .to_owned();
    let excerpt = current_literary_excerpt(port, &clock);

    Ok(ShellDeviceSnapshot {
        time: clock.display,
        timezone: clock.timezone,
        battery_percent,
        charging,
        wifi,
        frontlight,
        warmth,
        auto_brightness,
        bluetooth,
        ssh,
        usbnet,
        adapter: "KOA3".to_owned(),
        literary_clock_available: port.literary_corpus.is_some(),
        literary_clock_interval_minutes: port.literary_clock_interval_minutes,
        literary_excerpt: excerpt
            .map(|excerpt| excerpt.styled_text().clone())
            .unwrap_or_default(),
        literary_credit: excerpt
            .map(|excerpt| excerpt.credit().to_owned())
            .unwrap_or_default(),
        literary_excerpt_font_size: excerpt.map_or(30, crate::LiteraryExcerpt::font_size),
        launcher_background_label: port.launcher_background_choice.label(),
        launcher_background_options: port
            .launcher_background_choices
            .iter()
            .map(|choice| LauncherBackgroundOptionSnapshot {
                title: choice.label(),
                detail: choice.detail().to_owned(),
                selected: choice == &port.launcher_background_choice,
            })
            .collect(),
        launcher_background: port.launcher_background.clone(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LocalClock {
    minute_key: String,
    day_selector: u32,
    display: String,
    timezone: String,
}

fn read_local_clock() -> Result<LocalClock, KindleShellDeviceError> {
    let output = run_command(DATE, &["+%H:%M|%Y%j|%-I:%M %p|%Z"], "local time", true)?;
    let value = String::from_utf8(output)
        .map_err(|_| KindleShellDeviceError::InvalidValue("local time"))?;
    let value = value.trim();
    let mut fields = value.split('|');
    let (Some(minute_key), Some(day_key), Some(display), Some(timezone), None) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return Err(KindleShellDeviceError::InvalidValue("local time"));
    };
    if !valid_minute_key(minute_key)
        || !valid_day_key(day_key)
        || !valid_time_label(display)
        || !valid_timezone_label(timezone)
    {
        return Err(KindleShellDeviceError::InvalidValue("local time"));
    }
    Ok(LocalClock {
        minute_key: minute_key.to_owned(),
        day_selector: day_key
            .parse()
            .map_err(|_| KindleShellDeviceError::InvalidValue("local time"))?,
        display: display.to_owned(),
        timezone: timezone.to_owned(),
    })
}

fn current_literary_excerpt<'a>(
    port: &'a KindleShellDevicePort,
    clock: &LocalClock,
) -> Option<&'a crate::LiteraryExcerpt> {
    let interval = usize::from(port.literary_clock_interval_minutes);
    if interval == 0 {
        return None;
    }
    let minute = minute_index(clock.minute_key.as_str())?;
    let bucket = minute.checked_sub(minute % interval)?;
    let bucket_key = format!("{:02}:{:02}", bucket / 60, bucket % 60);
    port.literary_corpus
        .as_ref()?
        .excerpt_at(bucket_key.as_str(), u64::from(clock.day_selector))
}

fn valid_minute_key(value: &str) -> bool {
    if value.len() != 5 || value.as_bytes().get(2) != Some(&b':') {
        return false;
    }
    value[..2].parse::<u8>().is_ok_and(|hour| hour <= 23)
        && value[3..].parse::<u8>().is_ok_and(|minute| minute <= 59)
}

fn minute_index(value: &str) -> Option<usize> {
    valid_minute_key(value).then(|| {
        let hour = value[..2].parse::<usize>().ok()?;
        let minute = value[3..].parse::<usize>().ok()?;
        hour.checked_mul(60)?.checked_add(minute)
    })?
}

fn valid_day_key(value: &str) -> bool {
    value.len() == 7
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && value[4..]
            .parse::<u16>()
            .is_ok_and(|day| (1..=366).contains(&day))
}

fn valid_time_label(value: &str) -> bool {
    let Some((clock, period)) = value.split_once(' ') else {
        return false;
    };
    let Some((hour, minute)) = clock.split_once(':') else {
        return false;
    };
    matches!(period, "AM" | "PM")
        && hour
            .parse::<u8>()
            .is_ok_and(|hour| (1..=12).contains(&hour))
        && minute.len() == 2
        && minute.bytes().all(|byte| byte.is_ascii_digit())
        && minute.parse::<u8>().is_ok_and(|minute| minute <= 59)
}

fn valid_timezone_label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 16
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'_' | b'/'))
}

fn adjust_light(
    property: &'static str,
    step: i8,
    operation: &'static str,
) -> Result<(), KindleShellDeviceError> {
    if !matches!(step, -1 | 1) {
        return Err(KindleShellDeviceError::InvalidValue(operation));
    }
    let current = read_i32("com.lab126.powerd", property, operation)?;
    let next = current
        .saturating_add(i32::from(step))
        .clamp(0, LIGHT_MAXIMUM);
    if next != current {
        set_property(
            "com.lab126.powerd",
            property,
            next.to_string().as_str(),
            operation,
        )?;
    }
    Ok(())
}

fn set_light(
    property: &'static str,
    value: u8,
    operation: &'static str,
) -> Result<(), KindleShellDeviceError> {
    if i32::from(value) > LIGHT_MAXIMUM {
        return Err(KindleShellDeviceError::InvalidValue(operation));
    }
    set_property(
        "com.lab126.powerd",
        property,
        value.to_string().as_str(),
        operation,
    )
}

fn toggle_wifi() -> Result<(), KindleShellDeviceError> {
    let enabled = read_bool("com.lab126.wifid", "enable", "Wi-Fi enable")?;
    if enabled {
        set_property("com.lab126.wifid", "enable", "0", "disable Wi-Fi service")?;
        set_property(
            "com.lab126.cmd",
            "wirelessEnable",
            "0",
            "disable wireless command",
        )?;
    } else {
        set_property(
            "com.lab126.cmd",
            "wirelessEnable",
            "1",
            "enable wireless command",
        )?;
        set_property("com.lab126.wifid", "enable", "1", "enable Wi-Fi service")?;
    }
    Ok(())
}

fn read_clamped_u8(
    service: &'static str,
    property: &'static str,
    maximum: i32,
    operation: &'static str,
) -> Result<u8, KindleShellDeviceError> {
    let value = read_i32(service, property, operation)?.clamp(0, maximum);
    u8::try_from(value).map_err(|_| KindleShellDeviceError::InvalidValue(operation))
}

fn read_bool(
    service: &'static str,
    property: &'static str,
    operation: &'static str,
) -> Result<bool, KindleShellDeviceError> {
    match read_i32(service, property, operation)? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(KindleShellDeviceError::InvalidValue(operation)),
    }
}

fn read_i32(
    service: &'static str,
    property: &'static str,
    operation: &'static str,
) -> Result<i32, KindleShellDeviceError> {
    get_property(service, property, operation)?
        .parse()
        .map_err(|_| KindleShellDeviceError::InvalidValue(operation))
}

fn optional_property(
    service: &'static str,
    property: &'static str,
    operation: &'static str,
) -> Option<String> {
    get_property(service, property, operation).ok()
}

fn get_property(
    service: &'static str,
    property: &'static str,
    operation: &'static str,
) -> Result<String, KindleShellDeviceError> {
    let output = run_command(LIPC_GET, &[service, property], operation, true)?;
    String::from_utf8(output)
        .map(|value| value.trim().to_owned())
        .map_err(|_| KindleShellDeviceError::InvalidValue(operation))
}

fn set_property(
    service: &'static str,
    property: &'static str,
    value: &str,
    operation: &'static str,
) -> Result<(), KindleShellDeviceError> {
    run_command(LIPC_SET, &[service, property, value], operation, false).map(|_| ())
}

fn run_command(
    executable: &'static str,
    arguments: &[&str],
    operation: &'static str,
    capture_stdout: bool,
) -> Result<Vec<u8>, KindleShellDeviceError> {
    let child = Command::new(executable)
        .args(arguments)
        .env_clear()
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(if capture_stdout {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stderr(Stdio::null())
        .spawn()
        .map_err(|source| KindleShellDeviceError::CommandIo { operation, source })?;
    let mut child = CommandChild::new(child, operation);
    let mut stdout = if capture_stdout {
        child.take_stdout()
    } else {
        None
    };
    let status = child.wait_bounded()?;
    if !status.success() {
        return Err(KindleShellDeviceError::CommandFailed {
            operation,
            code: status.code(),
        });
    }
    let Some(stdout) = stdout.as_mut() else {
        return Ok(Vec::new());
    };
    let mut output = Vec::new();
    stdout
        .take(MAX_COMMAND_OUTPUT + 1)
        .read_to_end(&mut output)
        .map_err(|source| KindleShellDeviceError::CommandIo { operation, source })?;
    if output.len() as u64 > MAX_COMMAND_OUTPUT {
        return Err(KindleShellDeviceError::InvalidValue(operation));
    }
    Ok(output)
}

struct CommandChild {
    child: Option<Child>,
    operation: &'static str,
}

impl CommandChild {
    fn new(child: Child, operation: &'static str) -> Self {
        Self {
            child: Some(child),
            operation,
        }
    }

    fn take_stdout(&mut self) -> Option<std::process::ChildStdout> {
        self.child.as_mut().and_then(|child| child.stdout.take())
    }

    fn wait_bounded(&mut self) -> Result<ExitStatus, KindleShellDeviceError> {
        let started = Instant::now();
        loop {
            let child = self
                .child
                .as_mut()
                .ok_or(KindleShellDeviceError::InvalidValue(self.operation))?;
            if let Some(status) =
                child
                    .try_wait()
                    .map_err(|source| KindleShellDeviceError::CommandIo {
                        operation: self.operation,
                        source,
                    })?
            {
                self.child = None;
                return Ok(status);
            }
            if started.elapsed() >= COMMAND_TIMEOUT {
                self.terminate_and_reap()?;
                return Err(KindleShellDeviceError::CommandTimedOut(self.operation));
            }
            std::thread::sleep(COMMAND_POLL_INTERVAL);
        }
    }

    fn terminate_and_reap(&mut self) -> Result<(), KindleShellDeviceError> {
        let mut child = self
            .child
            .take()
            .ok_or(KindleShellDeviceError::InvalidValue(self.operation))?;
        child
            .kill()
            .and_then(|()| child.wait().map(|_| ()))
            .map_err(|source| KindleShellDeviceError::CommandIo {
                operation: self.operation,
                source,
            })
    }
}

impl Drop for CommandChild {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn discover_background_choices(directory: &Path) -> Vec<LauncherBackgroundChoice> {
    let mut choices = vec![LauncherBackgroundChoice::Pattern];
    let Ok(entries) = std::fs::read_dir(directory) else {
        return choices;
    };
    let mut filenames: Vec<_> = entries
        .take(MAX_BACKGROUND_DIRECTORY_ENTRIES)
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let raw_name = entry.file_name();
            let filename = LauncherBackgroundFileName::parse(raw_name.to_str()?)?;
            inspect_launcher_background_png(entry.path().as_path())
                .is_ok()
                .then_some(filename)
        })
        .collect();
    filenames.sort();
    filenames.dedup();
    filenames.truncate(MAX_BACKGROUND_FILE_CHOICES);
    choices.extend(filenames.into_iter().map(LauncherBackgroundChoice::File));
    choices
}

fn load_optional_background(
    directory: &Path,
    choice: &LauncherBackgroundChoice,
) -> Result<Option<slint::Image>, crate::LauncherBackgroundImageError> {
    match choice.filename() {
        None => Ok(None),
        Some(filename) => {
            load_launcher_background_png(directory.join(filename.as_str()).as_path()).map(Some)
        }
    }
}

fn read_literary_setting(path: &Path) -> Result<u16, KindleShellDeviceError> {
    let Some(value) = read_bounded_preference(
        path,
        MAX_LITERARY_SETTING_BYTES,
        "literary clock preference",
    )?
    else {
        return Ok(0);
    };
    parse_literary_setting(value.as_str()).ok_or(KindleShellDeviceError::InvalidValue(
        "literary clock preference",
    ))
}

fn parse_literary_setting(value: &str) -> Option<u16> {
    let interval = value
        .strip_prefix("ferrink-literary-clock-v1\ninterval_minutes=")?
        .strip_suffix('\n')?
        .parse()
        .ok()?;
    matches!(interval, 0 | 1 | 5 | 15 | 30 | 60).then_some(interval)
}

fn write_literary_setting(path: &Path, interval: u16) -> Result<(), KindleShellDeviceError> {
    if !matches!(interval, 0 | 1 | 5 | 15 | 30 | 60) {
        return Err(KindleShellDeviceError::InvalidValue(
            "literary clock interval",
        ));
    }
    let value = format!("{LITERARY_SETTING_HEADER}\ninterval_minutes={interval}\n");
    write_atomic_preference(
        path,
        "literary-clock",
        value.as_str(),
        "literary clock preference",
    )
}

fn read_launcher_background_setting(
    path: &Path,
) -> Result<LauncherBackgroundChoice, KindleShellDeviceError> {
    let Some(value) = read_bounded_preference(
        path,
        MAX_LAUNCHER_BACKGROUND_SETTING_BYTES,
        "launcher background preference",
    )?
    else {
        return Ok(LauncherBackgroundChoice::Pattern);
    };
    parse_launcher_background_setting(value.as_str()).ok_or(KindleShellDeviceError::InvalidValue(
        "launcher background preference",
    ))
}

fn parse_launcher_background_setting(value: &str) -> Option<LauncherBackgroundChoice> {
    let choice = value
        .strip_prefix("ferrink-launcher-background-v1\nchoice=")?
        .strip_suffix('\n')?;
    LauncherBackgroundChoice::from_setting_value(choice)
}

fn write_launcher_background_setting(
    path: &Path,
    choice: &LauncherBackgroundChoice,
) -> Result<(), KindleShellDeviceError> {
    let setting_value = choice.setting_value();
    let value = format!(
        "{LAUNCHER_BACKGROUND_SETTING_HEADER}\nchoice={}\n",
        setting_value
    );
    write_atomic_preference(
        path,
        "launcher-background",
        value.as_str(),
        "launcher background preference",
    )
}

fn read_bounded_preference(
    path: &Path,
    maximum_bytes: u64,
    operation: &'static str,
) -> Result<Option<String>, KindleShellDeviceError> {
    let metadata = match path.symlink_metadata() {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(KindleShellDeviceError::StateIo { operation, source });
        }
    };
    if !metadata.file_type().is_file() || metadata.len() == 0 || metadata.len() > maximum_bytes {
        return Err(KindleShellDeviceError::InvalidValue(operation));
    }
    let mut value = String::new();
    File::open(path)
        .map_err(|source| KindleShellDeviceError::StateIo { operation, source })?
        .take(maximum_bytes + 1)
        .read_to_string(&mut value)
        .map_err(|source| KindleShellDeviceError::StateIo { operation, source })?;
    if value.len() as u64 > maximum_bytes {
        return Err(KindleShellDeviceError::InvalidValue(operation));
    }
    Ok(Some(value))
}

fn write_atomic_preference(
    path: &Path,
    temporary_stem: &str,
    value: &str,
    operation: &'static str,
) -> Result<(), KindleShellDeviceError> {
    let parent = path
        .parent()
        .ok_or(KindleShellDeviceError::InvalidValue(operation))?;
    let parent_metadata = parent
        .symlink_metadata()
        .map_err(|source| KindleShellDeviceError::StateIo { operation, source })?;
    if !parent_metadata.file_type().is_dir() {
        return Err(KindleShellDeviceError::InvalidValue(operation));
    }
    match path.symlink_metadata() {
        Ok(metadata) if !metadata.file_type().is_file() => {
            return Err(KindleShellDeviceError::InvalidValue(operation));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(KindleShellDeviceError::StateIo { operation, source });
        }
    }

    let temporary_name = format!(".{temporary_stem}.settings.tmp.{}", std::process::id());
    let temporary_path = parent.join(temporary_name);
    let mut temporary = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary_path)
        .map_err(|source| KindleShellDeviceError::StateIo { operation, source })?;
    let mut pending = PendingPreference::new(temporary_path.clone());
    temporary
        .write_all(value.as_bytes())
        .and_then(|()| temporary.sync_all())
        .map_err(|source| KindleShellDeviceError::StateIo { operation, source })?;
    std::fs::rename(&temporary_path, path)
        .map_err(|source| KindleShellDeviceError::StateIo { operation, source })?;
    pending.committed = true;
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| KindleShellDeviceError::StateIo { operation, source })
}

struct PendingPreference {
    path: PathBuf,
    committed: bool,
}

impl PendingPreference {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            committed: false,
        }
    }
}

impl Drop for PendingPreference {
    fn drop(&mut self) {
        if !self.committed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn read_optional_status_file(path: &Path) -> Option<String> {
    let file = File::open(path).ok()?;
    let mut value = String::new();
    file.take(MAX_STATUS_FILE + 1)
        .read_to_string(&mut value)
        .ok()?;
    if value.len() as u64 > MAX_STATUS_FILE {
        return None;
    }
    Some(value.trim().to_owned())
}

fn process_is_running(expected: &str) -> bool {
    std::fs::read_dir("/proc").ok().is_some_and(|entries| {
        entries.filter_map(Result::ok).any(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.bytes().all(|byte| byte.is_ascii_digit()))
                && read_optional_status_file(entry.path().join("comm").as_path())
                    .is_some_and(|command| command == expected)
        })
    })
}

fn wifi_label(enabled: bool, connection: &str) -> &'static str {
    if !enabled {
        "Off"
    } else if connection.eq_ignore_ascii_case("CONNECTED") {
        "Wi-Fi"
    } else {
        "On"
    }
}

fn bluetooth_label(value: &str) -> &'static str {
    match value.trim() {
        "0" => "Off",
        "1" => "On",
        _ => "Unknown",
    }
}

fn interface_label(value: &str) -> &'static str {
    if value.eq_ignore_ascii_case("up") {
        "On"
    } else {
        "Off"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_labels_are_small_and_truthful() {
        assert_eq!(wifi_label(false, "CONNECTED"), "Off");
        assert_eq!(wifi_label(true, "CONNECTED"), "Wi-Fi");
        assert_eq!(wifi_label(true, "PENDING"), "On");
        assert_eq!(bluetooth_label("0"), "Off");
        assert_eq!(bluetooth_label("1\n"), "On");
        assert_eq!(bluetooth_label("unexpected"), "Unknown");
        assert_eq!(interface_label("up"), "On");
        assert_eq!(interface_label("down"), "Off");
    }

    #[test]
    fn local_time_label_accepts_only_the_stock_clock_shape() {
        assert!(valid_minute_key("00:00"));
        assert!(valid_minute_key("23:59"));
        assert!(!valid_minute_key("24:00"));
        assert!(!valid_minute_key("9:41"));
        assert_eq!(minute_index("09:41"), Some(9 * 60 + 41));
        assert!(valid_day_key("2026204"));
        assert!(!valid_day_key("2026000"));
        assert!(!valid_day_key("2026367"));
        assert!(valid_time_label("1:00 AM"));
        assert!(valid_time_label("10:25 AM"));
        assert!(valid_time_label("12:59 PM"));
        assert!(!valid_time_label("0:00 AM"));
        assert!(!valid_time_label("13:00 PM"));
        assert!(!valid_time_label("10:5 AM"));
        assert!(!valid_time_label("10:25 UTC"));
        assert!(!valid_time_label("10:25 AM extra"));
        assert!(valid_timezone_label("ADT"));
        assert!(valid_timezone_label("GMT+4"));
        assert!(!valid_timezone_label(""));
        assert!(!valid_timezone_label("America/Halifax time"));
    }

    #[test]
    fn literary_clock_preference_has_one_strict_versioned_shape() {
        assert_eq!(
            parse_literary_setting("ferrink-literary-clock-v1\ninterval_minutes=0\n"),
            Some(0)
        );
        assert_eq!(
            parse_literary_setting("ferrink-literary-clock-v1\ninterval_minutes=15\n"),
            Some(15)
        );
        assert_eq!(parse_literary_setting("interval_minutes=1\n"), None);
        assert_eq!(
            parse_literary_setting("ferrink-literary-clock-v1\ninterval_minutes=2\n"),
            None
        );
    }

    #[test]
    fn launcher_background_preference_has_one_strict_versioned_shape() {
        assert_eq!(
            parse_launcher_background_setting("ferrink-launcher-background-v1\nchoice=pattern\n")
                .map(|choice| choice.setting_value()),
            Some("pattern".to_owned())
        );
        assert_eq!(
            parse_launcher_background_setting(
                "ferrink-launcher-background-v1\nchoice=topography\n"
            )
            .map(|choice| choice.setting_value()),
            Some("file:topography.png".to_owned())
        );
        assert_eq!(
            parse_launcher_background_setting(
                "ferrink-launcher-background-v1\nchoice=file:family-art.png\n"
            )
            .map(|choice| choice.setting_value()),
            Some("file:family-art.png".to_owned())
        );
        assert_eq!(
            parse_launcher_background_setting("choice=topography\n"),
            None
        );
        assert_eq!(
            parse_launcher_background_setting("ferrink-launcher-background-v1\nchoice=unknown\n"),
            None
        );
    }

    #[test]
    fn background_discovery_is_bounded_to_valid_png_files_in_one_directory() {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("docs")
            .join("design");
        let choices = discover_background_choices(directory.as_path());
        let values: Vec<_> = choices
            .iter()
            .map(LauncherBackgroundChoice::setting_value)
            .collect();
        assert_eq!(values.first().map(String::as_str), Some("pattern"));
        assert!(values.contains(&"file:launcher-background-topography-6shade.png".to_owned()));
        assert!(values.len() <= MAX_BACKGROUND_FILE_CHOICES + 1);
    }

    #[test]
    fn literary_clock_interval_buckets_and_rotates_complete_minute_group() {
        let corpus = LiteraryClockCorpus::from_pipe_separated(
            b"09:40|09:40|First 09:40 excerpt.|A|Author|NO\n09:40|nine forty|Second nine forty excerpt.|B|Author|YES\n09:43|09:43|Exact 09:43 excerpt.|C|Author|NO\n",
        )
        .expect("fixture corpus should parse");
        let mut port = KindleShellDevicePort {
            literary_corpus: Some(corpus),
            literary_clock_interval_minutes: 5,
            launcher_background_choice: LauncherBackgroundChoice::Pattern,
            launcher_background_choices: vec![LauncherBackgroundChoice::Pattern],
            launcher_background: None,
        };
        let clock = LocalClock {
            minute_key: "09:43".to_owned(),
            day_selector: 1,
            display: "9:43 AM".to_owned(),
            timezone: "EDT".to_owned(),
        };

        assert_eq!(
            current_literary_excerpt(&port, &clock).unwrap().credit(),
            "— B, Author"
        );
        port.literary_clock_interval_minutes = 1;
        assert_eq!(
            current_literary_excerpt(&port, &clock).unwrap().credit(),
            "— C, Author"
        );
        port.literary_clock_interval_minutes = 0;
        assert!(current_literary_excerpt(&port, &clock).is_none());
    }
}
