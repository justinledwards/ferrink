//! Typed shell boundary for device status and reversible quick controls.

use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;
use std::time::Duration;

use slint::{ComponentHandle, ModelRc, Timer, TimerMode, VecModel};

use crate::{LauncherBackgroundOption, ShellActions, ShellData, ShellWindow};

const STATUS_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
const LIGHT_MAXIMUM: u8 = 24;
const MAX_LAUNCHER_BACKGROUND_CHOICES: u8 = 16;

/// One bounded launcher-background option ready for presentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LauncherBackgroundOptionSnapshot {
    /// Human-readable basename-derived title.
    pub title: String,
    /// Short source description.
    pub detail: String,
    /// Whether this is the active choice.
    pub selected: bool,
}

/// One sanitized device-state sample ready for presentation.
#[derive(Debug, Clone, PartialEq)]
pub struct ShellDeviceSnapshot {
    /// Local wall-clock time for the compact header.
    pub time: String,
    /// Short device-local timezone label.
    pub timezone: String,
    /// Battery state of charge, clamped to `0..=100`.
    pub battery_percent: u8,
    /// Whether external power is charging the battery.
    pub charging: bool,
    /// Human-readable Wi-Fi state.
    pub wifi: String,
    /// Front-light intensity, clamped to the reviewed `0..=24` range.
    pub frontlight: u8,
    /// Warm-light intensity, clamped to the reviewed `0..=24` range.
    pub warmth: u8,
    /// Whether the stock ambient-light controller is enabled.
    pub auto_brightness: bool,
    /// Human-readable Bluetooth state.
    pub bluetooth: String,
    /// Human-readable SSH state.
    pub ssh: String,
    /// Human-readable USB networking state.
    pub usbnet: String,
    /// Short reviewed adapter identity.
    pub adapter: String,
    /// Whether a bounded literary corpus is installed.
    pub literary_clock_available: bool,
    /// Selected excerpt-update interval; zero disables the feature.
    pub literary_clock_interval_minutes: u16,
    /// Presentation-ready excerpt with only its time phrase emphasized.
    pub literary_excerpt: slint::StyledText,
    /// Short work and author attribution for the current excerpt.
    pub literary_credit: String,
    /// Bounded logical font size selected for the current excerpt length.
    pub literary_excerpt_font_size: u8,
    /// Current launcher-background settings label.
    pub launcher_background_label: String,
    /// Generated pattern plus valid images from the one reviewed directory.
    pub launcher_background_options: Vec<LauncherBackgroundOptionSnapshot>,
    /// Replacement image when the device preference changes the configured default.
    pub launcher_background: Option<slint::Image>,
}

/// A reversible quick-control request emitted by the shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellDeviceCommand {
    /// Change front-light intensity by one signed step.
    AdjustFrontlight(i8),
    /// Change warm-light intensity by one signed step.
    AdjustWarmth(i8),
    /// Set front-light intensity to one absolute reviewed level.
    SetFrontlight(u8),
    /// Set warm-light intensity to one absolute reviewed level.
    SetWarmth(u8),
    /// Toggle the reviewed Wi-Fi service pair.
    ToggleWifi,
    /// Advance the optional persisted literary-clock interval.
    CycleLiteraryClockInterval,
    /// Select one entry from the last authoritative background snapshot.
    SelectLauncherBackground(u8),
}

/// Device status and control boundary used by Slint callbacks.
pub trait ShellDevicePort {
    /// Adapter-specific error type. Detailed diagnostics remain outside Slint.
    type Error: fmt::Display;

    /// Reads one coherent, sanitized snapshot.
    fn snapshot(&mut self) -> Result<ShellDeviceSnapshot, Self::Error>;

    /// Applies one typed reversible command and rereads authoritative state.
    fn apply(&mut self, command: ShellDeviceCommand) -> Result<ShellDeviceSnapshot, Self::Error>;
}

/// Keeps a device port and its periodic Slint timer alive.
pub struct ShellDeviceBinding<P> {
    _port: Rc<RefCell<P>>,
    _timer: Timer,
}

/// Copies one sanitized device snapshot into the Slint view model.
pub fn sync_device_ui(ui: &ShellWindow, snapshot: &ShellDeviceSnapshot) {
    let data = ui.global::<ShellData>();
    data.set_time_status(snapshot.time.as_str().into());
    data.set_timezone_status(snapshot.timezone.as_str().into());
    data.set_battery_percent(i32::from(snapshot.battery_percent));
    data.set_battery_charging(snapshot.charging);
    data.set_battery_status(format!("{}%", snapshot.battery_percent).into());
    data.set_network_status(snapshot.wifi.as_str().into());
    data.set_frontlight_level(i32::from(snapshot.frontlight));
    data.set_warmth_level(i32::from(snapshot.warmth));
    data.set_auto_brightness(snapshot.auto_brightness);
    data.set_display_controls_available(true);
    data.set_bluetooth_status(snapshot.bluetooth.as_str().into());
    data.set_ssh_status(snapshot.ssh.as_str().into());
    data.set_usbnet_status(snapshot.usbnet.as_str().into());
    data.set_device_adapter_status(snapshot.adapter.as_str().into());
    data.set_literary_clock_available(snapshot.literary_clock_available);
    data.set_literary_clock_enabled(snapshot.literary_clock_interval_minutes > 0);
    data.set_literary_clock_interval_label(
        literary_clock_interval_label(snapshot.literary_clock_interval_minutes).into(),
    );
    data.set_literary_clock_has_excerpt(
        snapshot.literary_clock_interval_minutes > 0 && !snapshot.literary_credit.is_empty(),
    );
    data.set_literary_excerpt(snapshot.literary_excerpt.clone());
    data.set_literary_credit(snapshot.literary_credit.as_str().into());
    data.set_literary_excerpt_font_size(i32::from(snapshot.literary_excerpt_font_size));
    data.set_launcher_background_label(snapshot.launcher_background_label.as_str().into());
    let options: Vec<_> = snapshot
        .launcher_background_options
        .iter()
        .map(|option| LauncherBackgroundOption {
            title: option.title.as_str().into(),
            detail: option.detail.as_str().into(),
            selected: option.selected,
        })
        .collect();
    data.set_launcher_background_options(ModelRc::new(VecModel::from(options)));
    if let Some(background) = snapshot.launcher_background.as_ref() {
        data.set_launcher_background(background.clone());
    }
}

fn note_device_error(ui: &ShellWindow, error: &impl fmt::Display) {
    eprintln!("ferrink-shell: device adapter error: {error}");
    let data = ui.global::<ShellData>();
    data.set_display_controls_available(false);
    data.set_device_adapter_status("ERROR".into());
}

fn apply_result<E: fmt::Display>(
    weak_ui: &slint::Weak<ShellWindow>,
    result: Result<ShellDeviceSnapshot, E>,
) {
    let Some(ui) = weak_ui.upgrade() else {
        return;
    };
    match result {
        Ok(snapshot) => sync_device_ui(&ui, &snapshot),
        Err(error) => note_device_error(&ui, &error),
    }
}

fn checked_step(value: i32) -> Option<i8> {
    match value {
        -1 => Some(-1),
        1 => Some(1),
        _ => None,
    }
}

fn checked_light_level(value: i32) -> Option<u8> {
    u8::try_from(value)
        .ok()
        .filter(|value| *value <= LIGHT_MAXIMUM)
}

fn checked_background_index(value: i32) -> Option<u8> {
    u8::try_from(value)
        .ok()
        .filter(|value| *value < MAX_LAUNCHER_BACKGROUND_CHOICES)
}

/// Returns the next supported interval in the settings cycle.
#[must_use]
pub const fn next_literary_clock_interval(current: u16) -> u16 {
    match current {
        0 => 1,
        1 => 5,
        5 => 15,
        15 => 30,
        30 => 60,
        60 => 0,
        _ => 0,
    }
}

fn literary_clock_interval_label(interval: u16) -> &'static str {
    match interval {
        0 => "Off",
        1 => "Every minute",
        5 => "Every 5 minutes",
        15 => "Every 15 minutes",
        30 => "Every 30 minutes",
        60 => "Every hour",
        _ => "Unavailable",
    }
}

/// Installs live device status and reversible quick-control callbacks.
///
/// The returned binding must live at least as long as the UI event loop.
pub fn install_device_handlers<P: ShellDevicePort + 'static>(
    ui: &ShellWindow,
    port: P,
) -> ShellDeviceBinding<P> {
    let port = Rc::new(RefCell::new(port));
    let initial = port.borrow_mut().snapshot();
    apply_result(&ui.as_weak(), initial);

    let actions = ui.global::<ShellActions>();

    let weak_ui = ui.as_weak();
    let frontlight_port = Rc::clone(&port);
    actions.on_adjust_frontlight(move |step| {
        let Some(step) = checked_step(step) else {
            return;
        };
        let result = frontlight_port
            .borrow_mut()
            .apply(ShellDeviceCommand::AdjustFrontlight(step));
        apply_result(&weak_ui, result);
    });

    let weak_ui = ui.as_weak();
    let warmth_port = Rc::clone(&port);
    actions.on_adjust_warmth(move |step| {
        let Some(step) = checked_step(step) else {
            return;
        };
        let result = warmth_port
            .borrow_mut()
            .apply(ShellDeviceCommand::AdjustWarmth(step));
        apply_result(&weak_ui, result);
    });

    let weak_ui = ui.as_weak();
    let frontlight_port = Rc::clone(&port);
    actions.on_set_frontlight(move |value| {
        let Some(value) = checked_light_level(value) else {
            return;
        };
        let result = frontlight_port
            .borrow_mut()
            .apply(ShellDeviceCommand::SetFrontlight(value));
        apply_result(&weak_ui, result);
    });

    let weak_ui = ui.as_weak();
    let warmth_port = Rc::clone(&port);
    actions.on_set_warmth(move |value| {
        let Some(value) = checked_light_level(value) else {
            return;
        };
        let result = warmth_port
            .borrow_mut()
            .apply(ShellDeviceCommand::SetWarmth(value));
        apply_result(&weak_ui, result);
    });

    let weak_ui = ui.as_weak();
    let wifi_port = Rc::clone(&port);
    actions.on_toggle_wifi(move || {
        let result = wifi_port.borrow_mut().apply(ShellDeviceCommand::ToggleWifi);
        apply_result(&weak_ui, result);
    });

    let weak_ui = ui.as_weak();
    let literary_clock_port = Rc::clone(&port);
    actions.on_cycle_literary_clock_interval(move || {
        let result = literary_clock_port
            .borrow_mut()
            .apply(ShellDeviceCommand::CycleLiteraryClockInterval);
        apply_result(&weak_ui, result);
    });

    let weak_ui = ui.as_weak();
    let background_port = Rc::clone(&port);
    actions.on_select_launcher_background(move |index| {
        let Some(index) = checked_background_index(index) else {
            return;
        };
        let result = background_port
            .borrow_mut()
            .apply(ShellDeviceCommand::SelectLauncherBackground(index));
        apply_result(&weak_ui, result);
    });

    let timer = Timer::default();
    let weak_ui = ui.as_weak();
    let timer_port = Rc::clone(&port);
    timer.start(TimerMode::Repeated, STATUS_REFRESH_INTERVAL, move || {
        let result = timer_port.borrow_mut().snapshot();
        apply_result(&weak_ui, result);
    });

    ShellDeviceBinding {
        _port: port,
        _timer: timer,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_single_steps_cross_the_ui_boundary() {
        assert_eq!(checked_step(-1), Some(-1));
        assert_eq!(checked_step(1), Some(1));
        assert_eq!(checked_step(0), None);
        assert_eq!(checked_step(2), None);
        assert_eq!(checked_step(i32::MAX), None);
    }

    #[test]
    fn absolute_light_levels_are_bounded_to_the_reviewed_range() {
        assert_eq!(checked_light_level(0), Some(0));
        assert_eq!(checked_light_level(24), Some(24));
        assert_eq!(checked_light_level(-1), None);
        assert_eq!(checked_light_level(25), None);
        assert_eq!(checked_light_level(i32::MAX), None);
    }

    #[test]
    fn background_indices_are_bounded_before_reaching_the_device_port() {
        assert_eq!(checked_background_index(0), Some(0));
        assert_eq!(checked_background_index(15), Some(15));
        assert_eq!(checked_background_index(-1), None);
        assert_eq!(checked_background_index(16), None);
        assert_eq!(checked_background_index(i32::MAX), None);
    }

    #[test]
    fn literary_clock_interval_cycle_is_closed_and_readable() {
        let cycle = [0, 1, 5, 15, 30, 60, 0];
        for pair in cycle.windows(2) {
            assert_eq!(next_literary_clock_interval(pair[0]), pair[1]);
        }
        assert_eq!(next_literary_clock_interval(2), 0);
        assert_eq!(literary_clock_interval_label(0), "Off");
        assert_eq!(literary_clock_interval_label(60), "Every hour");
    }
}
