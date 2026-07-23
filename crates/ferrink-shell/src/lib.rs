//! Toolkit-neutral state and actions for the ferrink reference shell.
//!
//! This crate does not execute commands, open devices, or alter system state.
//! It turns UI actions into typed commands for a future supervisor to review.

#![deny(unsafe_code)]

slint::include_modules!();

mod device;
#[cfg(feature = "kindle-runtime")]
mod kindle_device;
mod literary_clock;
mod procedural_background;

pub use device::*;
#[cfg(feature = "kindle-runtime")]
pub use kindle_device::*;
pub use literary_clock::{LiteraryClockCorpus, LiteraryClockError, LiteraryExcerpt};
pub use procedural_background::{
    JACQUARD_HEIGHT, JACQUARD_WIDTH, JacquardError, JacquardPreset, LauncherBackgroundChoice,
    LauncherBackgroundFileName, LauncherBackgroundImageError, inspect_launcher_background_png,
    load_launcher_background_png, render_jacquard_background,
};

use std::cell::{Cell, RefCell};
use std::fmt;
use std::fs::File;
use std::io::{BufRead, BufReader, Cursor, Seek};
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use ferrink_manifest::{ApplicationCatalog, MAX_APPLICATIONS};
use slint::{ComponentHandle, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};

const MAX_APPLICATION_ICON_BYTES: u64 = 1_048_576;
const MIN_APPLICATION_ICON_EDGE: u32 = 64;
const MAX_APPLICATION_ICON_EDGE: u32 = 512;

/// The embedded font family used by the shell and its screenshot fixtures.
pub const SHELL_FONT_FAMILY: &str = "Inter Variable";

/// Failure to register the bundled shell font with Slint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellFontError;

impl fmt::Display for ShellFontError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("bundled shell font contained no usable faces")
    }
}

impl std::error::Error for ShellFontError {}

/// Registers the bundled Inter variable face and makes it the sans-serif default.
///
/// Slint's platform must be installed before this function is called. Keeping
/// the font bytes in a Cargo dependency makes host fixtures and later device
/// rendering independent of the system font database.
///
/// # Errors
///
/// Returns [`ShellFontError`] when Fontique cannot discover any face in the
/// bundled font.
pub fn install_shell_font() -> Result<(), ShellFontError> {
    use slint::fontique_011::fontique::{Blob, GenericFamily};

    let blob = Blob::new(Arc::new(damascene_fonts_inter::INTER_VARIABLE));
    let mut collection = slint::fontique_011::shared_collection();
    let registered = collection.register_fonts(blob, None);
    let family_ids: Vec<_> = registered.iter().map(|(family_id, _)| *family_id).collect();

    if family_ids.is_empty() {
        return Err(ShellFontError);
    }

    for generic in [
        GenericFamily::SansSerif,
        GenericFamily::SystemUi,
        GenericFamily::UiSansSerif,
    ] {
        collection.set_generic_families(generic, family_ids.iter().copied());
    }

    Ok(())
}

/// An exact reference geometry supported by the shell prototype.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellProfile {
    /// First-generation Kindle Paperwhite at 758×1024.
    Paperwhite1,
    /// Third-generation Kindle Oasis at 1264×1680.
    Oasis3,
}

impl ShellProfile {
    /// Every exact reference profile in stable fixture order.
    pub const ALL: [Self; 2] = [Self::Paperwhite1, Self::Oasis3];

    /// Returns the short filename-safe profile name.
    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Paperwhite1 => "pw1",
            Self::Oasis3 => "koa3",
        }
    }

    /// Returns the exact physical width and height in pixels.
    #[must_use]
    pub const fn dimensions(self) -> (u32, u32) {
        match self {
            Self::Paperwhite1 => (758, 1024),
            Self::Oasis3 => (1264, 1680),
        }
    }

    /// Returns the diagnostic label shown by the host/manual shell.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Paperwhite1 => "PW1 · 758 × 1024 · host preview",
            Self::Oasis3 => "KOA3 · 1264 × 1680 · host preview",
        }
    }

    /// Returns the profile's logical-to-physical UI scale.
    #[must_use]
    pub const fn ui_scale(self) -> f32 {
        match self {
            Self::Paperwhite1 => 1.0,
            Self::Oasis3 => 1.640_625,
        }
    }
}

/// Applies an exact reference profile and generated launcher background.
///
/// # Errors
///
/// Returns [`JacquardError`] if the bounded procedural image cannot be created.
pub fn configure_shell_window(
    ui: &ShellWindow,
    profile: ShellProfile,
) -> Result<(), JacquardError> {
    let (width, height) = profile.dimensions();
    ui.window()
        .set_size(slint::PhysicalSize::new(width, height));
    ui.set_ui_scale(profile.ui_scale());
    let data = ui.global::<ShellData>();
    data.set_profile_label(profile.label().into());
    let background =
        render_jacquard_background(JACQUARD_WIDTH, JACQUARD_HEIGHT, JacquardPreset::EinkCalm)?;
    data.set_launcher_background(slint::Image::from_rgb8(background));
    data.set_launcher_background_options(ModelRc::new(VecModel::from(vec![
        LauncherBackgroundOption {
            title: "Pattern".into(),
            detail: "Generated on device".into(),
            selected: true,
        },
        LauncherBackgroundOption {
            title: "Topography".into(),
            detail: "Image from ferrink/backgrounds".into(),
            selected: false,
        },
    ])));
    Ok(())
}

/// Replaces the launcher rows with one stable-ID-sorted manifest catalog.
pub fn configure_application_catalog(
    ui: &ShellWindow,
    catalog: &ApplicationCatalog,
) -> Result<(), ApplicationIconError> {
    let applications: Result<Vec<_>, _> = catalog
        .iter()
        .map(|application| {
            let manifest = application.manifest();
            let icon = load_application_icon(Path::new(&manifest.icon))?;
            Ok::<RegisteredApplication, ApplicationIconError>(RegisteredApplication {
                title: manifest.name.as_str().into(),
                detail: manifest.description.as_str().into(),
                icon,
                available: true,
            })
        })
        .collect();
    ui.global::<ShellData>()
        .set_applications(ModelRc::new(VecModel::from(applications?)));
    Ok(())
}

/// Replaces the catalog with the two source-tree application bundles used by host previews.
///
/// The images are decoded through the same bounded PNG path as device-provided icons,
/// without enabling Slint's larger general-purpose image loader.
pub fn configure_bundled_preview_catalog(ui: &ShellWindow) -> Result<(), ApplicationIconError> {
    let applications = vec![
        RegisteredApplication {
            title: "Home Assistant".into(),
            detail: "Control your home from the Kindle dashboard".into(),
            icon: decode_application_icon(Cursor::new(include_bytes!(
                "../../../apps/io.home-assistant.dashboard/icon.png"
            )))?,
            available: true,
        },
        RegisteredApplication {
            title: "KOReader".into(),
            detail: "Open your library and continue reading".into(),
            icon: decode_application_icon(Cursor::new(include_bytes!(
                "../../../apps/org.koreader.reader/icon.png"
            )))?,
            available: true,
        },
    ];
    ui.global::<ShellData>()
        .set_applications(ModelRc::new(VecModel::from(applications)));
    Ok(())
}

/// A package icon could not be safely inspected or decoded.
#[derive(Debug)]
pub enum ApplicationIconError {
    /// The path is missing, is not a regular file, or could not be read.
    Io(std::io::Error),
    /// The encoded PNG is malformed or unsupported.
    Decode(png::DecodingError),
    /// The image is not a bounded square icon.
    InvalidDimensions { width: u32, height: u32 },
    /// The normalized decoder output has an unexpected layout.
    InvalidPixelLayout,
}

impl fmt::Display for ApplicationIconError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "cannot read application icon: {error}"),
            Self::Decode(error) => write!(formatter, "cannot decode application icon PNG: {error}"),
            Self::InvalidDimensions { width, height } => write!(
                formatter,
                "application icon is {width}×{height}; expected a square edge from {MIN_APPLICATION_ICON_EDGE} to {MAX_APPLICATION_ICON_EDGE} pixels"
            ),
            Self::InvalidPixelLayout => {
                formatter.write_str("application icon has an unsupported normalized pixel layout")
            }
        }
    }
}

impl std::error::Error for ApplicationIconError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Decode(error) => Some(error),
            Self::InvalidDimensions { .. } | Self::InvalidPixelLayout => None,
        }
    }
}

fn load_application_icon(path: &Path) -> Result<slint::Image, ApplicationIconError> {
    let metadata = path.symlink_metadata().map_err(ApplicationIconError::Io)?;
    if !metadata.file_type().is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_APPLICATION_ICON_BYTES
    {
        return Err(ApplicationIconError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "icon must be a non-empty regular file no larger than 1 MiB",
        )));
    }
    decode_application_icon(BufReader::new(
        File::open(path).map_err(ApplicationIconError::Io)?,
    ))
}

fn decode_application_icon<R: BufRead + Seek>(
    input: R,
) -> Result<slint::Image, ApplicationIconError> {
    let mut decoder = png::Decoder::new(input);
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(ApplicationIconError::Decode)?;
    let (width, height) = (reader.info().width, reader.info().height);
    if width != height || !(MIN_APPLICATION_ICON_EDGE..=MAX_APPLICATION_ICON_EDGE).contains(&width)
    {
        return Err(ApplicationIconError::InvalidDimensions { width, height });
    }
    let output_size = reader
        .output_buffer_size()
        .ok_or(ApplicationIconError::InvalidPixelLayout)?;
    if output_size > usize::try_from(MAX_APPLICATION_ICON_BYTES).unwrap_or(usize::MAX) {
        return Err(ApplicationIconError::InvalidPixelLayout);
    }
    let mut bytes = vec![0_u8; output_size];
    let frame = reader
        .next_frame(bytes.as_mut_slice())
        .map_err(ApplicationIconError::Decode)?;
    let bytes = &bytes[..frame.buffer_size()];
    let pixels: Vec<_> = match frame.color_type {
        png::ColorType::Rgba => bytes
            .chunks_exact(4)
            .map(|pixel| Rgba8Pixel::new(pixel[0], pixel[1], pixel[2], pixel[3]))
            .collect(),
        png::ColorType::Rgb => bytes
            .chunks_exact(3)
            .map(|pixel| Rgba8Pixel::new(pixel[0], pixel[1], pixel[2], u8::MAX))
            .collect(),
        png::ColorType::GrayscaleAlpha => bytes
            .chunks_exact(2)
            .map(|pixel| Rgba8Pixel::new(pixel[0], pixel[0], pixel[0], pixel[1]))
            .collect(),
        png::ColorType::Grayscale => bytes
            .iter()
            .map(|value| Rgba8Pixel::new(*value, *value, *value, u8::MAX))
            .collect(),
        png::ColorType::Indexed => return Err(ApplicationIconError::InvalidPixelLayout),
    };
    let expected = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .ok_or(ApplicationIconError::InvalidPixelLayout)?;
    if pixels.len() != expected {
        return Err(ApplicationIconError::InvalidPixelLayout);
    }
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
    buffer.make_mut_slice().copy_from_slice(pixels.as_slice());
    Ok(slint::Image::from_rgba8(buffer))
}

/// Copies the toolkit-neutral controller state into the Slint view model.
pub fn sync_shell_ui(ui: &ShellWindow, controller: &ShellController) {
    let data = ui.global::<ShellData>();
    data.set_page(match controller.view() {
        ShellView::Home => ShellPage::Home,
        ShellView::Sample => ShellPage::Sample,
        ShellView::Settings => ShellPage::Settings,
        ShellView::Maintenance => ShellPage::Maintenance,
    });
    data.set_confirmation(match controller.pending_power_action() {
        None => ConfirmationKind::None,
        Some(PowerAction::Reboot) => ConfirmationKind::Reboot,
        Some(PowerAction::PowerOff) => ConfirmationKind::PowerOff,
    });
    data.set_alert(match controller.alert() {
        ShellAlert::None => AlertKind::None,
        ShellAlert::ApplicationUnavailable => AlertKind::ApplicationUnavailable,
        ShellAlert::ApplicationStopped => AlertKind::ApplicationStopped,
        ShellAlert::RecoveryCountdown { .. } => AlertKind::RecoveryCountdown,
    });
    data.set_recovery_seconds(match controller.alert() {
        ShellAlert::RecoveryCountdown { seconds_remaining } => i32::from(seconds_remaining),
        ShellAlert::None | ShellAlert::ApplicationUnavailable | ShellAlert::ApplicationStopped => 0,
    });
    data.set_status_text(controller.notice().text().into());
    if controller.pending_power_action().is_some() || controller.alert() != ShellAlert::None {
        ui.set_quick_settings_open(false);
    }
}

/// Moves host keyboard focus to the safe first action on the active surface.
///
/// This does not affect touch geometry or make a keyboard a device
/// requirement. It is used by the desktop preview and input tests after a
/// callback replaces the focused surface.
pub fn focus_shell_ui(ui: &ShellWindow) {
    ui.invoke_focus_keyboard_root();
    let current = ui.get_focus_generation();
    let next = current.checked_add(1).unwrap_or(1);
    ui.set_focus_generation(next);
}

fn dispatch_shell_action<P: ShellCommandPort>(
    weak_ui: &slint::Weak<ShellWindow>,
    controller: &Rc<RefCell<ShellController>>,
    command_port: &Rc<RefCell<P>>,
    action: ShellAction,
) {
    let command = controller.borrow_mut().dispatch(action);
    if let Some(command) = command {
        let outcome = command_port.borrow_mut().submit(command);
        controller
            .borrow_mut()
            .note_command_outcome(command, outcome);
    }

    if let Some(ui) = weak_ui.upgrade() {
        sync_shell_ui(&ui, &controller.borrow());
        let weak_focus_ui = ui.as_weak();
        slint::Timer::single_shot(Duration::ZERO, move || {
            if let Some(ui) = weak_focus_ui.upgrade() {
                focus_shell_ui(&ui);
            }
        });
    }
}

fn action_handler<P: ShellCommandPort + 'static>(
    ui: &ShellWindow,
    controller: &Rc<RefCell<ShellController>>,
    command_port: &Rc<RefCell<P>>,
    action: ShellAction,
) -> impl Fn() + 'static {
    let weak_ui = ui.as_weak();
    let controller = Rc::clone(controller);
    let command_port = Rc::clone(command_port);
    move || dispatch_shell_action(&weak_ui, &controller, &command_port, action)
}

/// Connects every production shell callback to a toolkit-neutral controller.
///
/// The supplied [`ShellCommandPort`] receives only commands already permitted
/// by [`ShellController`] policy. Installing a port does not grant the Slint
/// view any device or process authority.
pub fn install_shell_handlers<P: ShellCommandPort + 'static>(
    ui: &ShellWindow,
    controller: &Rc<RefCell<ShellController>>,
    command_port: &Rc<RefCell<P>>,
) {
    let actions = ui.global::<ShellActions>();
    {
        let weak_ui = ui.as_weak();
        let controller = Rc::clone(controller);
        let command_port = Rc::clone(command_port);
        actions.on_launch_application(move |raw_index| {
            let Ok(index) = ApplicationIndex::try_from(raw_index) else {
                return;
            };
            dispatch_shell_action(
                &weak_ui,
                &controller,
                &command_port,
                ShellAction::LaunchApplication(index),
            );
        });
    }
    actions.on_return_home(action_handler(
        ui,
        controller,
        command_port,
        ShellAction::ReturnHome,
    ));
    actions.on_open_sample(action_handler(
        ui,
        controller,
        command_port,
        ShellAction::OpenSample,
    ));
    actions.on_open_settings(action_handler(
        ui,
        controller,
        command_port,
        ShellAction::OpenSettings,
    ));
    actions.on_open_maintenance(action_handler(
        ui,
        controller,
        command_port,
        ShellAction::OpenMaintenance,
    ));
    actions.on_request_stock(action_handler(
        ui,
        controller,
        command_port,
        ShellAction::RequestStock,
    ));
    actions.on_request_reboot(action_handler(
        ui,
        controller,
        command_port,
        ShellAction::RequestReboot,
    ));
    actions.on_request_power_off(action_handler(
        ui,
        controller,
        command_port,
        ShellAction::RequestPowerOff,
    ));
    actions.on_confirm_power_action(action_handler(
        ui,
        controller,
        command_port,
        ShellAction::ConfirmPowerAction,
    ));
    actions.on_cancel_power_action(action_handler(
        ui,
        controller,
        command_port,
        ShellAction::CancelPowerAction,
    ));
    actions.on_dismiss_alert(action_handler(
        ui,
        controller,
        command_port,
        ShellAction::DismissAlert,
    ));

    let sample_count = Rc::new(Cell::new(0_i32));
    let weak_ui = ui.as_weak();
    actions.on_increment_sample(move || {
        let next = sample_count.get().saturating_add(1);
        sample_count.set(next);
        if let Some(ui) = weak_ui.upgrade() {
            ui.global::<ShellData>().set_sample_count(next);
        }
    });
}

/// The shell surface currently presented to the operator.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ShellView {
    /// The application and system-action launcher.
    #[default]
    Home,
    /// The built-in Slint sample application.
    Sample,
    /// Device settings and maintenance controls.
    Settings,
    /// Read-only maintenance information.
    Maintenance,
}

/// A destructive power action that requires explicit confirmation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerAction {
    /// Request a supervised operating-system reboot.
    Reboot,
    /// Request a supervised operating-system power-off.
    PowerOff,
}

/// A supervisor-owned condition that temporarily replaces ordinary shell input.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ShellAlert {
    /// No modal condition is visible.
    #[default]
    None,
    /// The selected application cannot be started with current capabilities.
    ApplicationUnavailable,
    /// A child application stopped unexpectedly and control returned safely.
    ApplicationStopped,
    /// ferrink is counting down to a stock-interface recovery handoff.
    RecoveryCountdown {
        /// Whole seconds remaining before the supervisor performs the handoff.
        seconds_remaining: u8,
    },
}

/// An operator action reported by the shell view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellAction {
    /// Return from a detail surface to the launcher.
    ReturnHome,
    /// Ask the supervisor to launch the registered foreground application.
    LaunchApplication(ApplicationIndex),
    /// Show the built-in sample application.
    OpenSample,
    /// Show device settings.
    OpenSettings,
    /// Show the read-only maintenance surface.
    OpenMaintenance,
    /// Ask a future supervisor to restore the stock interface.
    RequestStock,
    /// Open the reboot confirmation dialog.
    RequestReboot,
    /// Open the power-off confirmation dialog.
    RequestPowerOff,
    /// Accept the currently visible destructive-action confirmation.
    ConfirmPowerAction,
    /// Dismiss the currently visible destructive-action confirmation.
    CancelPowerAction,
    /// Dismiss a recoverable unavailable/error alert.
    DismissAlert,
}

/// A typed request for a future supervisor.
///
/// Producing a command does not execute it. The current host preview records
/// the request as inert, and device execution remains a later work package.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellCommand {
    /// Restore or reveal the stock interface and exit ferrink.
    ReturnToStock,
    /// Cleanly hand foreground ownership to the registered application.
    LaunchApplication(ApplicationIndex),
    /// Reboot after the supervisor repeats its own safety checks.
    Reboot,
    /// Power off after the supervisor repeats its own safety checks.
    PowerOff,
}

/// Stable position in the supervisor's ID-sorted application catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ApplicationIndex(u8);

impl ApplicationIndex {
    /// Returns the protocol-safe zero-based catalog position.
    #[must_use]
    pub const fn value(self) -> u8 {
        self.0
    }
}

impl TryFrom<i32> for ApplicationIndex {
    type Error = ApplicationIndexError;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        let value = u8::try_from(value).map_err(|_| ApplicationIndexError)?;
        if usize::from(value) >= MAX_APPLICATIONS {
            return Err(ApplicationIndexError);
        }
        Ok(Self(value))
    }
}

impl TryFrom<usize> for ApplicationIndex {
    type Error = ApplicationIndexError;

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        let value = u8::try_from(value).map_err(|_| ApplicationIndexError)?;
        if usize::from(value) >= MAX_APPLICATIONS {
            return Err(ApplicationIndexError);
        }
        Ok(Self(value))
    }
}

/// Application selection was outside the fixed catalog transport range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApplicationIndexError;

impl fmt::Display for ApplicationIndexError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("application index is outside the registered catalog")
    }
}

impl std::error::Error for ApplicationIndexError {}

/// The host or supervisor response to one typed shell command.
///
/// This is deliberately narrower than a transport or device error. A command
/// port owns detailed diagnostics and exposes only what the shell needs to
/// present a truthful status.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellCommandOutcome {
    /// The port accepted responsibility for completing the requested action.
    Accepted,
    /// The current host has no authorized implementation for the action.
    Unavailable,
}

/// Delivery boundary between shell policy and a host or supervisor.
///
/// Implementations run on the Slint event-loop thread and should return
/// promptly. A future IPC implementation may forward the typed request to a
/// separate supervisor; a host preview must return
/// [`ShellCommandOutcome::Unavailable`] without touching the device.
pub trait ShellCommandPort {
    /// Submits one policy-approved system command.
    #[must_use]
    fn submit(&mut self, command: ShellCommand) -> ShellCommandOutcome;
}

/// Status text selected by host policy rather than authored in the view.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ShellNotice {
    /// The shell is a manual host preview with no device authority.
    #[default]
    ManualPreview,
    /// A real device adapter is attached and no transient notice is active.
    DeviceReady,
    /// A host or supervisor accepted responsibility for the request.
    CommandAccepted(ShellCommand),
    /// A typed request was intentionally not executed by the preview host.
    CommandNotExecuted(ShellCommand),
}

impl ShellNotice {
    /// Returns short operator-facing status text.
    #[must_use]
    pub const fn text(self) -> &'static str {
        match self {
            Self::ManualPreview => "L0 manual preview · device actions unavailable",
            Self::DeviceReady => "",
            Self::CommandAccepted(ShellCommand::ReturnToStock) => "Opening Amazon home",
            Self::CommandAccepted(ShellCommand::LaunchApplication(_)) => "Opening application",
            Self::CommandAccepted(ShellCommand::Reboot) => "Restart requested",
            Self::CommandAccepted(ShellCommand::PowerOff) => "Power off requested",
            Self::CommandNotExecuted(ShellCommand::ReturnToStock) => {
                "Preview only · stock handoff was not executed"
            }
            Self::CommandNotExecuted(ShellCommand::LaunchApplication(_)) => {
                "Preview only · application was not opened"
            }
            Self::CommandNotExecuted(ShellCommand::Reboot) => {
                "Preview only · reboot was not executed"
            }
            Self::CommandNotExecuted(ShellCommand::PowerOff) => {
                "Preview only · power off was not executed"
            }
        }
    }
}

/// Pure shell policy that gates commands before they reach a supervisor.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ShellController {
    view: ShellView,
    pending_power_action: Option<PowerAction>,
    alert: ShellAlert,
    notice: ShellNotice,
}

impl ShellController {
    /// Creates a controller for a shell with reviewed device adapters attached.
    #[must_use]
    pub const fn for_device() -> Self {
        Self {
            view: ShellView::Home,
            pending_power_action: None,
            alert: ShellAlert::None,
            notice: ShellNotice::DeviceReady,
        }
    }

    /// Returns the active shell surface.
    #[must_use]
    pub const fn view(&self) -> ShellView {
        self.view
    }

    /// Returns the destructive action awaiting operator confirmation.
    #[must_use]
    pub const fn pending_power_action(&self) -> Option<PowerAction> {
        self.pending_power_action
    }

    /// Returns the supervisor-owned condition currently replacing shell input.
    #[must_use]
    pub const fn alert(&self) -> ShellAlert {
        self.alert
    }

    /// Returns the current host-policy notice.
    #[must_use]
    pub const fn notice(&self) -> ShellNotice {
        self.notice
    }

    /// Presents a supervisor-owned unavailable, failure, or recovery condition.
    ///
    /// Alerts supersede a pending power prompt so two modal decisions can never
    /// compete for input. [`ShellAlert::None`] clears the current condition.
    pub const fn present_alert(&mut self, alert: ShellAlert) {
        self.pending_power_action = None;
        self.alert = alert;
    }

    /// Applies one UI action and returns a command only when policy permits it.
    ///
    /// While a destructive confirmation is visible, unrelated actions are
    /// ignored so background controls cannot bypass or replace the prompt.
    #[must_use]
    pub fn dispatch(&mut self, action: ShellAction) -> Option<ShellCommand> {
        if self.alert != ShellAlert::None {
            return match (self.alert, action) {
                (
                    ShellAlert::ApplicationUnavailable | ShellAlert::ApplicationStopped,
                    ShellAction::DismissAlert,
                ) => {
                    self.alert = ShellAlert::None;
                    self.view = ShellView::Home;
                    None
                }
                (ShellAlert::RecoveryCountdown { .. }, ShellAction::RequestStock) => {
                    self.alert = ShellAlert::None;
                    self.view = ShellView::Home;
                    Some(ShellCommand::ReturnToStock)
                }
                (
                    ShellAlert::None
                    | ShellAlert::ApplicationUnavailable
                    | ShellAlert::ApplicationStopped
                    | ShellAlert::RecoveryCountdown { .. },
                    ShellAction::ReturnHome
                    | ShellAction::LaunchApplication(_)
                    | ShellAction::OpenSample
                    | ShellAction::OpenSettings
                    | ShellAction::OpenMaintenance
                    | ShellAction::RequestStock
                    | ShellAction::RequestReboot
                    | ShellAction::RequestPowerOff
                    | ShellAction::ConfirmPowerAction
                    | ShellAction::CancelPowerAction
                    | ShellAction::DismissAlert,
                ) => None,
            };
        }

        if let Some(pending) = self.pending_power_action {
            return match action {
                ShellAction::ConfirmPowerAction => {
                    self.pending_power_action = None;
                    Some(match pending {
                        PowerAction::Reboot => ShellCommand::Reboot,
                        PowerAction::PowerOff => ShellCommand::PowerOff,
                    })
                }
                ShellAction::CancelPowerAction => {
                    self.pending_power_action = None;
                    None
                }
                ShellAction::ReturnHome
                | ShellAction::LaunchApplication(_)
                | ShellAction::OpenSample
                | ShellAction::OpenSettings
                | ShellAction::OpenMaintenance
                | ShellAction::RequestStock
                | ShellAction::RequestReboot
                | ShellAction::RequestPowerOff
                | ShellAction::DismissAlert => None,
            };
        }

        match action {
            ShellAction::ReturnHome => self.view = ShellView::Home,
            ShellAction::LaunchApplication(index) => {
                return Some(ShellCommand::LaunchApplication(index));
            }
            ShellAction::OpenSample => self.view = ShellView::Sample,
            ShellAction::OpenSettings => self.view = ShellView::Settings,
            ShellAction::OpenMaintenance => self.view = ShellView::Maintenance,
            ShellAction::RequestStock => return Some(ShellCommand::ReturnToStock),
            ShellAction::RequestReboot => {
                self.pending_power_action = Some(PowerAction::Reboot);
            }
            ShellAction::RequestPowerOff => {
                self.pending_power_action = Some(PowerAction::PowerOff);
            }
            ShellAction::ConfirmPowerAction
            | ShellAction::CancelPowerAction
            | ShellAction::DismissAlert => {}
        }

        None
    }

    /// Records the host or supervisor response to one emitted command.
    pub const fn note_command_outcome(
        &mut self,
        command: ShellCommand,
        outcome: ShellCommandOutcome,
    ) {
        self.notice = match outcome {
            ShellCommandOutcome::Accepted => ShellNotice::CommandAccepted(command),
            ShellCommandOutcome::Unavailable => ShellNotice::CommandNotExecuted(command),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_state_is_an_inert_manual_home_preview() {
        let controller = ShellController::default();

        assert_eq!(controller.view(), ShellView::Home);
        assert_eq!(controller.pending_power_action(), None);
        assert_eq!(controller.alert(), ShellAlert::None);
        assert_eq!(controller.notice(), ShellNotice::ManualPreview);
    }

    #[test]
    fn application_indexes_are_bounded_to_the_manifest_catalog() {
        assert_eq!(ApplicationIndex::try_from(0_usize).unwrap().value(), 0);
        assert_eq!(ApplicationIndex::try_from(63_i32).unwrap().value(), 63);
        assert!(ApplicationIndex::try_from(-1_i32).is_err());
        assert!(ApplicationIndex::try_from(64_usize).is_err());
        assert!(ApplicationIndex::try_from(256_usize).is_err());
    }

    #[test]
    fn device_controller_starts_ready_without_preview_chrome() {
        let controller = ShellController::for_device();

        assert_eq!(controller.view(), ShellView::Home);
        assert_eq!(controller.notice(), ShellNotice::DeviceReady);
        assert_eq!(controller.notice().text(), "");
    }

    #[test]
    fn navigation_changes_views_without_emitting_commands() {
        let mut controller = ShellController::default();

        assert_eq!(controller.dispatch(ShellAction::OpenSample), None);
        assert_eq!(controller.view(), ShellView::Sample);
        assert_eq!(controller.dispatch(ShellAction::OpenSettings), None);
        assert_eq!(controller.view(), ShellView::Settings);
        assert_eq!(controller.dispatch(ShellAction::OpenMaintenance), None);
        assert_eq!(controller.view(), ShellView::Maintenance);
        assert_eq!(controller.dispatch(ShellAction::ReturnHome), None);
        assert_eq!(controller.view(), ShellView::Home);
    }

    #[test]
    fn reboot_requires_confirmation_and_is_consumed_once() {
        let mut controller = ShellController::default();

        assert_eq!(controller.dispatch(ShellAction::RequestReboot), None);
        assert_eq!(controller.pending_power_action(), Some(PowerAction::Reboot));
        assert_eq!(
            controller.dispatch(ShellAction::ConfirmPowerAction),
            Some(ShellCommand::Reboot)
        );
        assert_eq!(controller.pending_power_action(), None);
        assert_eq!(controller.dispatch(ShellAction::ConfirmPowerAction), None);
    }

    #[test]
    fn cancel_discards_power_off_without_emitting_a_command() {
        let mut controller = ShellController::default();

        assert_eq!(controller.dispatch(ShellAction::RequestPowerOff), None);
        assert_eq!(controller.dispatch(ShellAction::CancelPowerAction), None);
        assert_eq!(controller.pending_power_action(), None);
    }

    #[test]
    fn confirmation_blocks_background_actions() {
        let mut controller = ShellController::default();

        assert_eq!(controller.dispatch(ShellAction::RequestReboot), None);
        assert_eq!(controller.dispatch(ShellAction::RequestStock), None);
        assert_eq!(controller.dispatch(ShellAction::OpenSample), None);
        assert_eq!(controller.view(), ShellView::Home);
        assert_eq!(controller.pending_power_action(), Some(PowerAction::Reboot));
    }

    #[test]
    fn stock_request_is_typed_but_not_executed_by_the_controller() {
        let mut controller = ShellController::default();

        let command = controller.dispatch(ShellAction::RequestStock);

        assert_eq!(command, Some(ShellCommand::ReturnToStock));
        controller.note_command_outcome(
            ShellCommand::ReturnToStock,
            ShellCommandOutcome::Unavailable,
        );
        assert_eq!(
            controller.notice(),
            ShellNotice::CommandNotExecuted(ShellCommand::ReturnToStock)
        );
    }

    #[test]
    fn accepted_command_outcomes_are_distinct_from_preview_refusals() {
        let mut controller = ShellController::default();

        controller.note_command_outcome(ShellCommand::Reboot, ShellCommandOutcome::Accepted);

        assert_eq!(
            controller.notice(),
            ShellNotice::CommandAccepted(ShellCommand::Reboot)
        );
        assert_eq!(controller.notice().text(), "Restart requested");
    }

    #[test]
    fn recoverable_alerts_block_background_actions_until_dismissed() {
        for alert in [
            ShellAlert::ApplicationUnavailable,
            ShellAlert::ApplicationStopped,
        ] {
            let mut controller = ShellController::default();
            controller.present_alert(alert);

            assert_eq!(controller.dispatch(ShellAction::OpenSample), None);
            assert_eq!(controller.dispatch(ShellAction::RequestStock), None);
            assert_eq!(controller.alert(), alert);
            assert_eq!(controller.dispatch(ShellAction::DismissAlert), None);
            assert_eq!(controller.alert(), ShellAlert::None);
            assert_eq!(controller.view(), ShellView::Home);
        }
    }

    #[test]
    fn recovery_countdown_only_permits_immediate_stock_handoff() {
        let mut controller = ShellController::default();
        controller.present_alert(ShellAlert::RecoveryCountdown {
            seconds_remaining: 8,
        });

        assert_eq!(controller.dispatch(ShellAction::DismissAlert), None);
        assert_eq!(controller.dispatch(ShellAction::RequestPowerOff), None);
        assert_eq!(
            controller.alert(),
            ShellAlert::RecoveryCountdown {
                seconds_remaining: 8
            }
        );
        assert_eq!(
            controller.dispatch(ShellAction::RequestStock),
            Some(ShellCommand::ReturnToStock)
        );
        assert_eq!(controller.alert(), ShellAlert::None);
    }

    #[test]
    fn alert_supersedes_pending_power_confirmation() {
        let mut controller = ShellController::default();
        assert_eq!(controller.dispatch(ShellAction::RequestReboot), None);

        controller.present_alert(ShellAlert::ApplicationStopped);

        assert_eq!(controller.pending_power_action(), None);
        assert_eq!(controller.alert(), ShellAlert::ApplicationStopped);
        assert_eq!(controller.dispatch(ShellAction::ConfirmPowerAction), None);
    }
}
