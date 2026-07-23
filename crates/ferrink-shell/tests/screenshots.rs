use std::error::Error;
use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use ferrink_shell::{
    RegisteredApplication, ShellAction, ShellAlert, ShellController, ShellData, ShellProfile,
    ShellWindow, configure_bundled_preview_catalog, configure_shell_window, install_shell_font,
    sync_shell_ui,
};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Key, Platform, PlatformError, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, ModelRc, PhysicalSize, Rgb8Pixel, VecModel};

const UPDATE_ENV: &str = "FERRINK_UPDATE_SCREENSHOTS";

thread_local! {
    static WINDOW: Rc<MinimalSoftwareWindow> =
        MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
}

struct ScreenshotPlatform;

impl Platform for ScreenshotPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(WINDOW.with(Rc::clone))
    }
}

#[derive(Debug, Clone, Copy)]
enum Scenario {
    Home,
    QuickSettings,
    Settings,
    BackgroundPicker,
    SettingsBottom,
    KeyboardFocus,
    LongUnavailable,
    StockNotice,
    Sample,
    SampleIncremented,
    Maintenance,
    MaintenanceWarnings,
    ApplicationUnavailable,
    ApplicationStopped,
    RecoveryCountdown,
    RebootConfirmation,
    RebootNotice,
    PowerOffConfirmation,
    PowerOffNotice,
}

impl Scenario {
    const ALL: [Self; 19] = [
        Self::Home,
        Self::QuickSettings,
        Self::Settings,
        Self::BackgroundPicker,
        Self::SettingsBottom,
        Self::KeyboardFocus,
        Self::LongUnavailable,
        Self::StockNotice,
        Self::Sample,
        Self::SampleIncremented,
        Self::Maintenance,
        Self::MaintenanceWarnings,
        Self::ApplicationUnavailable,
        Self::ApplicationStopped,
        Self::RecoveryCountdown,
        Self::RebootConfirmation,
        Self::RebootNotice,
        Self::PowerOffConfirmation,
        Self::PowerOffNotice,
    ];

    const fn slug(self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::QuickSettings => "quick-settings",
            Self::Settings => "settings",
            Self::BackgroundPicker => "background-picker",
            Self::SettingsBottom => "settings-bottom",
            Self::KeyboardFocus => "keyboard-focus",
            Self::LongUnavailable => "long-unavailable",
            Self::StockNotice => "stock-notice",
            Self::Sample => "sample",
            Self::SampleIncremented => "sample-count-1",
            Self::Maintenance => "maintenance",
            Self::MaintenanceWarnings => "maintenance-warnings",
            Self::ApplicationUnavailable => "application-unavailable",
            Self::ApplicationStopped => "application-stopped",
            Self::RecoveryCountdown => "recovery-countdown",
            Self::RebootConfirmation => "reboot-confirmation",
            Self::RebootNotice => "reboot-notice",
            Self::PowerOffConfirmation => "power-off-confirmation",
            Self::PowerOffNotice => "power-off-notice",
        }
    }

    fn apply(self, ui: &ShellWindow, controller: &mut ShellController) {
        match self {
            Self::Home | Self::KeyboardFocus => {}
            Self::QuickSettings => ui.set_quick_settings_open(true),
            Self::Settings | Self::BackgroundPicker | Self::SettingsBottom => {
                assert_eq!(controller.dispatch(ShellAction::OpenSettings), None);
                if matches!(self, Self::BackgroundPicker) {
                    ui.set_background_picker_open(true);
                }
            }
            Self::LongUnavailable => {
                let data = ui.global::<ShellData>();
                data.set_applications(ModelRc::new(VecModel::from(vec![RegisteredApplication {
                    title: "KOReader with Bionic Reading and the Edwards Family Library".into(),
                    detail: "Unavailable · application storage is not mounted".into(),
                    icon: slint::Image::default(),
                    available: false,
                }])));
            }
            Self::StockNotice => record_inert_command(controller, ShellAction::RequestStock),
            Self::Sample => {
                assert_eq!(controller.dispatch(ShellAction::OpenSample), None);
            }
            Self::SampleIncremented => {
                assert_eq!(controller.dispatch(ShellAction::OpenSample), None);
                ui.global::<ShellData>().set_sample_count(1);
            }
            Self::Maintenance => {
                assert_eq!(controller.dispatch(ShellAction::OpenMaintenance), None);
            }
            Self::MaintenanceWarnings => {
                assert_eq!(controller.dispatch(ShellAction::OpenMaintenance), None);
                let data = ui.global::<ShellData>();
                data.set_battery_status("UNKNOWN".into());
                data.set_network_status("OFFLINE".into());
                data.set_storage_status("LOW · 63 MB FREE".into());
                data.set_storage_warning(true);
            }
            Self::ApplicationUnavailable => {
                ui.global::<ShellData>().set_active_application_title(
                    "KOReader with Bionic Reading and the Edwards Family Library".into(),
                );
                controller.present_alert(ShellAlert::ApplicationUnavailable);
            }
            Self::ApplicationStopped => {
                controller.present_alert(ShellAlert::ApplicationStopped);
            }
            Self::RecoveryCountdown => {
                controller.present_alert(ShellAlert::RecoveryCountdown {
                    seconds_remaining: 8,
                });
            }
            Self::RebootConfirmation => {
                assert_eq!(controller.dispatch(ShellAction::RequestReboot), None);
            }
            Self::RebootNotice => {
                assert_eq!(controller.dispatch(ShellAction::RequestReboot), None);
                record_inert_command(controller, ShellAction::ConfirmPowerAction);
            }
            Self::PowerOffConfirmation => {
                assert_eq!(controller.dispatch(ShellAction::RequestPowerOff), None);
            }
            Self::PowerOffNotice => {
                assert_eq!(controller.dispatch(ShellAction::RequestPowerOff), None);
                record_inert_command(controller, ShellAction::ConfirmPowerAction);
            }
        }
    }

    fn after_show(self, ui: &ShellWindow) {
        if matches!(self, Self::KeyboardFocus) {
            ui.window().dispatch_event(WindowEvent::KeyPressed {
                text: Key::Tab.into(),
            });
            ui.window().dispatch_event(WindowEvent::KeyReleased {
                text: Key::Tab.into(),
            });
        } else if matches!(self, Self::SettingsBottom) {
            let size = ui.window().size().to_logical(1.0);
            ui.window().dispatch_event(WindowEvent::PointerScrolled {
                position: LogicalPosition::new(size.width / 2.0, size.height / 2.0),
                delta_x: 0.0,
                delta_y: -500.0,
            });
        }
    }
}

fn record_inert_command(controller: &mut ShellController, action: ShellAction) {
    let command = controller
        .dispatch(action)
        .expect("fixture action should emit a command");
    controller.note_command_outcome(command, ferrink_shell::ShellCommandOutcome::Unavailable);
}

fn render(
    window: &Rc<MinimalSoftwareWindow>,
    profile: ShellProfile,
    scenario: Scenario,
) -> Result<Vec<Rgb8Pixel>, Box<dyn Error>> {
    let (width, height) = profile.dimensions();
    window.set_size(PhysicalSize::new(width, height));

    let ui = ShellWindow::new()?;
    configure_shell_window(&ui, profile)?;
    configure_bundled_preview_catalog(&ui)?;

    let mut controller = ShellController::default();
    scenario.apply(&ui, &mut controller);
    sync_shell_ui(&ui, &controller);
    ui.show()?;
    scenario.after_show(&ui);

    let pixel_count = usize::try_from(width)?
        .checked_mul(usize::try_from(height)?)
        .ok_or("screenshot pixel count overflowed the host address space")?;
    let mut pixels = vec![Rgb8Pixel::default(); pixel_count];
    window.request_redraw();
    let rendered = window.draw_if_needed(|renderer| {
        renderer.render(
            pixels.as_mut_slice(),
            usize::try_from(width).expect("width fits usize"),
        );
    });
    ui.hide()?;

    if !rendered {
        return Err("software renderer did not draw the requested screenshot".into());
    }

    Ok(pixels)
}

fn baseline_path(profile: ShellProfile, scenario: Scenario) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("screenshots")
        .join(format!("{}-{}.png", profile.slug(), scenario.slug()))
}

fn diff_path(profile: ShellProfile, scenario: Scenario, kind: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("screenshot-diffs")
        .join(format!("{}-{}-{kind}.png", profile.slug(), scenario.slug()))
}

fn rgb_bytes(pixels: &[Rgb8Pixel]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(pixels.len().saturating_mul(3));
    for pixel in pixels {
        bytes.extend([pixel.r, pixel.g, pixel.b]);
    }
    bytes
}

fn write_png(
    path: &Path,
    width: u32,
    height: u32,
    pixels: &[Rgb8Pixel],
) -> Result<(), Box<dyn Error>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let output = BufWriter::new(File::create(path)?);
    let mut encoder = png::Encoder::new(output, width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()?
        .write_image_data(&rgb_bytes(pixels))?;
    Ok(())
}

fn read_png(path: &Path) -> Result<(u32, u32, Vec<Rgb8Pixel>), Box<dyn Error>> {
    let input = BufReader::new(File::open(path)?);
    let mut reader = png::Decoder::new(input).read_info()?;
    let buffer_size = reader
        .output_buffer_size()
        .ok_or("decoded PNG is too large for this host")?;
    let mut bytes = vec![0; buffer_size];
    let info = reader.next_frame(&mut bytes)?;

    if info.color_type != png::ColorType::Rgb || info.bit_depth != png::BitDepth::Eight {
        return Err(format!("baseline {} is not RGB8", path.display()).into());
    }

    let bytes = &bytes[..info.buffer_size()];
    if !bytes.len().is_multiple_of(3) {
        return Err(format!("baseline {} has incomplete RGB data", path.display()).into());
    }

    let pixels = bytes
        .chunks_exact(3)
        .map(|rgb| Rgb8Pixel::new(rgb[0], rgb[1], rgb[2]))
        .collect();
    Ok((info.width, info.height, pixels))
}

fn luma(pixel: Rgb8Pixel) -> u8 {
    let weighted =
        u32::from(pixel.r) * 54 + u32::from(pixel.g) * 183 + u32::from(pixel.b) * 19 + 128;
    u8::try_from(weighted / 256).expect("weighted RGB8 luma remains in RGB8 range")
}

fn e_ink_level(pixel: Rgb8Pixel) -> u8 {
    u8::try_from((u16::from(luma(pixel)) + 8) / 17).expect("four-bit E Ink level remains in range")
}

fn compare(
    profile: ShellProfile,
    scenario: Scenario,
    actual: &[Rgb8Pixel],
) -> Result<(), Box<dyn Error>> {
    let path = baseline_path(profile, scenario);
    let (expected_width, expected_height, expected) = read_png(&path)?;
    let (actual_width, actual_height) = profile.dimensions();

    if (expected_width, expected_height) != (actual_width, actual_height) {
        return Err(format!(
            "baseline {} is {}×{}, expected {}×{}",
            path.display(),
            expected_width,
            expected_height,
            actual_width,
            actual_height
        )
        .into());
    }

    if expected.len() != actual.len() {
        return Err(format!(
            "baseline {} has {} pixels, expected {}",
            path.display(),
            expected.len(),
            actual.len()
        )
        .into());
    }

    let mut changed = 0_usize;
    let mut max_luma_delta = 0_u8;
    let mut first_change = None;
    let mut diff = Vec::with_capacity(actual.len());

    for (index, (expected_pixel, actual_pixel)) in expected.iter().zip(actual).enumerate() {
        let differs = e_ink_level(*expected_pixel) != e_ink_level(*actual_pixel);
        if differs {
            changed = changed.saturating_add(1);
            max_luma_delta =
                max_luma_delta.max(luma(*expected_pixel).abs_diff(luma(*actual_pixel)));
            first_change.get_or_insert(index);
            diff.push(Rgb8Pixel::new(0, 0, 0));
        } else {
            diff.push(Rgb8Pixel::new(255, 255, 255));
        }
    }

    if changed == 0 {
        return Ok(());
    }

    let actual_path = diff_path(profile, scenario, "actual");
    let changed_path = diff_path(profile, scenario, "changed");
    write_png(&actual_path, actual_width, actual_height, actual)?;
    write_png(&changed_path, actual_width, actual_height, &diff)?;

    let first_change = first_change.expect("a changed pixel records its index");
    let width = usize::try_from(actual_width)?;
    let x = first_change % width;
    let y = first_change / width;
    Err(format!(
        "{} differs at {changed} four-bit grayscale pixels (first at {x},{y}; max luma delta {max_luma_delta}); inspect {} and {}",
        path.display(),
        actual_path.display(),
        changed_path.display()
    )
    .into())
}

#[test]
fn exact_profile_screenshots_match_committed_baselines() -> Result<(), Box<dyn Error>> {
    slint::platform::set_platform(Box::new(ScreenshotPlatform))
        .map_err(|error| format!("failed to install screenshot platform: {error}"))?;
    install_shell_font()?;
    let window = WINDOW.with(Rc::clone);
    let update = std::env::var_os(UPDATE_ENV).is_some_and(|value| value == "1");

    for profile in ShellProfile::ALL {
        for scenario in Scenario::ALL {
            let pixels = render(&window, profile, scenario)?;
            if update {
                let (width, height) = profile.dimensions();
                write_png(&baseline_path(profile, scenario), width, height, &pixels)?;
            } else {
                compare(profile, scenario, &pixels)?;
            }
        }
    }

    Ok(())
}
