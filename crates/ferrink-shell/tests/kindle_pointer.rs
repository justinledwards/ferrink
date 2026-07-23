use std::cell::RefCell;
use std::error::Error;
use std::num::NonZeroU32;
use std::rc::Rc;

use ferrink_platform::{
    DeviceProfile, DisplayPoint, LogicalTouchPhase, ProbeReport, ResolvedRuntimeDevice,
    TouchContactEvent,
};
use ferrink_platform_kindle::{L0InputCore, SlintPointerBridge, new_slint_window};
use ferrink_shell::{
    ApplicationIndex, LauncherBackgroundOptionSnapshot, ShellActions, ShellCommand,
    ShellCommandOutcome, ShellCommandPort, ShellController, ShellData, ShellDeviceCommand,
    ShellDevicePort, ShellDeviceSnapshot, ShellProfile, ShellView, ShellWindow,
    configure_bundled_preview_catalog, configure_shell_window, install_device_handlers,
    install_shell_font, install_shell_handlers, sync_shell_ui,
};
use slint::ComponentHandle;
use slint::platform::software_renderer::MinimalSoftwareWindow;
use slint::platform::{Platform, PlatformError, WindowAdapter};

struct PointerPlatform {
    window: Rc<MinimalSoftwareWindow>,
}

impl Platform for PointerPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.window.clone())
    }
}

#[derive(Default)]
struct InertPort {
    commands: Vec<ShellCommand>,
}

impl ShellCommandPort for InertPort {
    fn submit(&mut self, command: ShellCommand) -> ShellCommandOutcome {
        self.commands.push(command);
        ShellCommandOutcome::Unavailable
    }
}

struct FakeDevicePort {
    snapshot: ShellDeviceSnapshot,
}

impl ShellDevicePort for FakeDevicePort {
    type Error = std::convert::Infallible;

    fn snapshot(&mut self) -> Result<ShellDeviceSnapshot, Self::Error> {
        Ok(self.snapshot.clone())
    }

    fn apply(&mut self, command: ShellDeviceCommand) -> Result<ShellDeviceSnapshot, Self::Error> {
        match command {
            ShellDeviceCommand::AdjustFrontlight(step) => {
                self.snapshot.frontlight =
                    self.snapshot.frontlight.saturating_add_signed(step).min(24);
            }
            ShellDeviceCommand::AdjustWarmth(step) => {
                self.snapshot.warmth = self.snapshot.warmth.saturating_add_signed(step).min(24);
            }
            ShellDeviceCommand::SetFrontlight(value) => {
                self.snapshot.frontlight = value.min(24);
            }
            ShellDeviceCommand::SetWarmth(value) => {
                self.snapshot.warmth = value.min(24);
            }
            ShellDeviceCommand::ToggleWifi => {
                self.snapshot.wifi = if self.snapshot.wifi == "Off" {
                    "Wi-Fi".to_owned()
                } else {
                    "Off".to_owned()
                };
            }
            ShellDeviceCommand::CycleLiteraryClockInterval => {
                self.snapshot.literary_clock_interval_minutes =
                    ferrink_shell::next_literary_clock_interval(
                        self.snapshot.literary_clock_interval_minutes,
                    );
            }
            ShellDeviceCommand::SelectLauncherBackground(index) => {
                let selected = usize::from(index);
                if let Some(option) = self.snapshot.launcher_background_options.get(selected) {
                    self.snapshot.launcher_background_label = option.title.clone();
                    for (option_index, option) in self
                        .snapshot
                        .launcher_background_options
                        .iter_mut()
                        .enumerate()
                    {
                        option.selected = option_index == selected;
                    }
                }
            }
        }
        Ok(self.snapshot.clone())
    }
}

fn fake_device_port() -> FakeDevicePort {
    FakeDevicePort {
        snapshot: ShellDeviceSnapshot {
            time: "10:12 AM".to_owned(),
            timezone: "EDT".to_owned(),
            battery_percent: 73,
            charging: false,
            wifi: "Wi-Fi".to_owned(),
            frontlight: 10,
            warmth: 18,
            auto_brightness: false,
            bluetooth: "Off".to_owned(),
            ssh: "On".to_owned(),
            usbnet: "On".to_owned(),
            adapter: "TEST".to_owned(),
            literary_clock_available: true,
            literary_clock_interval_minutes: 1,
            literary_excerpt: slint::StyledText::from_markdown(
                "At **10:12**, the test panel was ready.",
            )
            .expect("fixture markdown should parse"),
            literary_credit: "— ferrink test".to_owned(),
            literary_excerpt_font_size: 30,
            launcher_background_label: "Pattern".to_owned(),
            launcher_background_options: vec![
                LauncherBackgroundOptionSnapshot {
                    title: "Pattern".to_owned(),
                    detail: "Generated on device".to_owned(),
                    selected: true,
                },
                LauncherBackgroundOptionSnapshot {
                    title: "Topography".to_owned(),
                    detail: "Image from ferrink/backgrounds".to_owned(),
                    selected: false,
                },
            ],
            launcher_background: None,
        },
    }
}

fn input_event(timestamp_micros: u32, event_type: u16, code: u16, value: i32) -> [u8; 16] {
    let mut event = [0_u8; 16];
    event[4..8].copy_from_slice(&timestamp_micros.to_le_bytes());
    event[8..10].copy_from_slice(&event_type.to_le_bytes());
    event[10..12].copy_from_slice(&code.to_le_bytes());
    event[12..16].copy_from_slice(&value.to_le_bytes());
    event
}

fn koa3_touch_core() -> Result<L0InputCore, Box<dyn Error>> {
    let profile = DeviceProfile::from_toml(include_str!(
        "../../../device-profiles/reference-portrait.toml"
    ))?;
    let report = ProbeReport::from_json(include_str!(
        "../../ferrink-platform/tests/fixtures/probe-reference-portrait.json"
    ))?;
    let runtime = ResolvedRuntimeDevice::resolve(&profile, &report)?;
    Ok(L0InputCore::try_from_runtime(
        &runtime,
        NonZeroU32::new(8).expect("fixed record budget is non-zero"),
    )?)
}

fn tap(
    pointer: &mut SlintPointerBridge,
    ui: &ShellWindow,
    point: DisplayPoint,
) -> Result<(), Box<dyn Error>> {
    for phase in [LogicalTouchPhase::Pressed, LogicalTouchPhase::Released] {
        pointer.dispatch(ui.window(), TouchContactEvent { phase, point })?;
    }
    slint::platform::update_timers_and_animations();
    Ok(())
}

#[test]
fn koa3_physical_taps_reach_the_real_top_bar_and_application_row() -> Result<(), Box<dyn Error>> {
    let window = new_slint_window();
    slint::platform::set_platform(Box::new(PointerPlatform {
        window: window.clone(),
    }))?;
    install_shell_font()?;

    let ui = ShellWindow::new()?;
    configure_shell_window(&ui, ShellProfile::Oasis3)?;
    configure_bundled_preview_catalog(&ui)?;
    let controller = Rc::new(RefCell::new(ShellController::default()));
    let command_port = Rc::new(RefCell::new(InertPort::default()));
    sync_shell_ui(&ui, &controller.borrow());
    install_shell_handlers(&ui, &controller, &command_port);
    let _device_binding = install_device_handlers(&ui, fake_device_port());
    ui.show()?;
    window.request_redraw();
    slint::platform::update_timers_and_animations();
    window.draw_if_needed(|_| {});

    let data = ui.global::<ShellData>();
    assert_eq!(data.get_time_status().as_str(), "10:12 AM");
    assert_eq!(data.get_battery_percent(), 73);
    assert_eq!(data.get_frontlight_level(), 10);
    ui.global::<ShellActions>().invoke_adjust_frontlight(1);
    assert_eq!(data.get_frontlight_level(), 11);
    ui.global::<ShellActions>().invoke_set_warmth(7);
    assert_eq!(data.get_warmth_level(), 7);
    ui.global::<ShellActions>()
        .invoke_cycle_literary_clock_interval();
    assert_eq!(
        data.get_literary_clock_interval_label().as_str(),
        "Every 5 minutes"
    );

    let mut pointer = SlintPointerBridge::default();

    // Physical center of the stock-style top chevron's generous hit target.
    tap(&mut pointer, &ui, DisplayPoint { x: 632, y: 44 })?;
    assert!(ui.get_quick_settings_open());
    // A direct tap on the brightness track crosses the absolute-level callback;
    // the side minus/plus targets remain available for older touch hardware.
    tap(&mut pointer, &ui, DisplayPoint { x: 950, y: 375 })?;
    assert!(data.get_frontlight_level() >= 18);
    tap(&mut pointer, &ui, DisplayPoint { x: 632, y: 44 })?;
    assert!(!ui.get_quick_settings_open());

    // Feed a native 32-bit KOA3 Protocol-B trace through the same ABI decoder,
    // profile transform, Slint bridge, and production callback as the device.
    let mut trace = Vec::new();
    for event in [
        input_event(1, 3, 0x39, 1),
        input_event(2, 3, 0x35, 500),
        input_event(3, 3, 0x36, 435),
        input_event(4, 0, 0, 0),
        input_event(5, 3, 0x39, -1),
        input_event(6, 0, 0, 0),
    ] {
        trace.extend_from_slice(&event);
    }
    let mut input = koa3_touch_core()?;
    for contact in input.push_bytes(&trace)? {
        pointer.dispatch(ui.window(), contact)?;
    }
    slint::platform::update_timers_and_animations();
    assert_eq!(controller.borrow().view(), ShellView::Home);
    assert_eq!(
        command_port.borrow().commands,
        [ShellCommand::LaunchApplication(
            ApplicationIndex::try_from(0_usize).unwrap()
        )]
    );

    pointer.stop(ui.window())?;
    ui.hide()?;
    Ok(())
}
