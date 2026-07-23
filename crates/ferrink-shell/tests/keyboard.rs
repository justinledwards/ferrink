use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;

use ferrink_shell::{
    ApplicationIndex, RegisteredApplication, ShellActions, ShellAlert, ShellCommand,
    ShellCommandOutcome, ShellCommandPort, ShellController, ShellData, ShellNotice, ShellProfile,
    ShellView, ShellWindow, configure_bundled_preview_catalog, configure_shell_window,
    focus_shell_ui, install_shell_font, install_shell_handlers, sync_shell_ui,
};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Key, Platform, PlatformError, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, Model, ModelRc, VecModel};

thread_local! {
    static WINDOW: Rc<MinimalSoftwareWindow> =
        MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
}

struct KeyboardPlatform;

impl Platform for KeyboardPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(WINDOW.with(Rc::clone))
    }
}

#[derive(Debug, Default)]
struct RecordingPreviewPort {
    commands: Vec<ShellCommand>,
}

impl ShellCommandPort for RecordingPreviewPort {
    fn submit(&mut self, command: ShellCommand) -> ShellCommandOutcome {
        self.commands.push(command);
        ShellCommandOutcome::Unavailable
    }
}

fn send_key(ui: &ShellWindow, key: impl Into<slint::SharedString>) {
    let text = key.into();
    ui.window()
        .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
    ui.window()
        .dispatch_event(WindowEvent::KeyReleased { text });
    slint::platform::update_timers_and_animations();
}

fn application(index: usize) -> ApplicationIndex {
    ApplicationIndex::try_from(index).unwrap()
}

#[test]
fn keyboard_traversal_activation_and_modal_escape_use_production_callbacks()
-> Result<(), Box<dyn Error>> {
    slint::platform::set_platform(Box::new(KeyboardPlatform))
        .map_err(|error| format!("failed to install keyboard-test platform: {error}"))?;
    let ui = ShellWindow::new()?;
    install_shell_font()?;
    configure_shell_window(&ui, ShellProfile::Paperwhite1)?;
    configure_bundled_preview_catalog(&ui)?;
    let applications = ui.global::<ShellData>().get_applications();
    assert_eq!(applications.row_count(), 2);
    assert_eq!(
        applications.row_data(0).unwrap().title.as_str(),
        "Home Assistant"
    );
    assert_eq!(applications.row_data(1).unwrap().title.as_str(), "KOReader");

    let controller = Rc::new(RefCell::new(ShellController::default()));
    let command_port = Rc::new(RefCell::new(RecordingPreviewPort::default()));
    sync_shell_ui(&ui, &controller.borrow());
    install_shell_handlers(&ui, &controller, &command_port);
    ui.show()?;

    // Tab first reaches the top-bar settings segment. Return opens the real
    // quick-settings sheet and Escape closes it through the enclosing scope.
    send_key(&ui, Key::Tab);
    send_key(&ui, Key::Return);
    assert!(ui.get_quick_settings_open());
    send_key(&ui, Key::Escape);
    assert!(!ui.get_quick_settings_open());

    // The next Tab reaches the first application tile and Return invokes it.
    send_key(&ui, Key::Tab);
    send_key(&ui, Key::Return);
    assert_eq!(controller.borrow().view(), ShellView::Home);
    assert_eq!(
        command_port.borrow().commands,
        [ShellCommand::LaunchApplication(application(0))]
    );

    // The next enabled tile emits the second registered-application handoff.
    send_key(&ui, Key::Tab);
    send_key(&ui, Key::Return);
    assert_eq!(controller.borrow().view(), ShellView::Home);
    assert_eq!(
        command_port.borrow().commands,
        [
            ShellCommand::LaunchApplication(application(0)),
            ShellCommand::LaunchApplication(application(1))
        ]
    );

    // Canceling a production confirmation emits nothing; confirming one sends
    // exactly one command through the injected port.
    ui.global::<ShellActions>().invoke_request_reboot();
    focus_shell_ui(&ui);
    send_key(&ui, Key::Escape);
    assert_eq!(controller.borrow().pending_power_action(), None);
    assert_eq!(
        command_port.borrow().commands,
        [
            ShellCommand::LaunchApplication(application(0)),
            ShellCommand::LaunchApplication(application(1))
        ]
    );

    ui.global::<ShellActions>().invoke_request_power_off();
    ui.global::<ShellActions>().invoke_confirm_power_action();
    assert_eq!(
        command_port.borrow().commands,
        [
            ShellCommand::LaunchApplication(application(0)),
            ShellCommand::LaunchApplication(application(1)),
            ShellCommand::PowerOff
        ]
    );

    controller
        .borrow_mut()
        .present_alert(ShellAlert::ApplicationStopped);
    sync_shell_ui(&ui, &controller.borrow());
    focus_shell_ui(&ui);
    send_key(&ui, Key::Escape);
    assert_eq!(controller.borrow().alert(), ShellAlert::None);

    controller
        .borrow_mut()
        .present_alert(ShellAlert::RecoveryCountdown {
            seconds_remaining: 8,
        });
    sync_shell_ui(&ui, &controller.borrow());
    focus_shell_ui(&ui);
    send_key(&ui, Key::Escape);
    assert_eq!(
        controller.borrow().alert(),
        ShellAlert::RecoveryCountdown {
            seconds_remaining: 8
        }
    );

    // Background focus is disabled while recovery is visible. Tab reaches the
    // sole recovery action and Space requests the inert preview handoff.
    send_key(&ui, Key::Tab);
    send_key(&ui, " ");
    assert_eq!(controller.borrow().alert(), ShellAlert::None);
    assert_eq!(
        controller.borrow().notice(),
        ShellNotice::CommandNotExecuted(ShellCommand::ReturnToStock)
    );
    assert_eq!(
        command_port.borrow().commands,
        [
            ShellCommand::LaunchApplication(application(0)),
            ShellCommand::LaunchApplication(application(1)),
            ShellCommand::PowerOff,
            ShellCommand::ReturnToStock
        ]
    );

    // Disabled application rows are omitted from traversal.
    ui.global::<ShellData>()
        .set_applications(ModelRc::new(VecModel::from(vec![RegisteredApplication {
            title: "Unavailable".into(),
            detail: "Unavailable".into(),
            icon: slint::Image::default(),
            available: false,
        }])));
    let commands_before = command_port.borrow().commands.clone();
    focus_shell_ui(&ui);
    send_key(&ui, Key::Tab);
    send_key(&ui, Key::Tab);
    send_key(&ui, Key::Return);
    assert_eq!(command_port.borrow().commands, commands_before);

    ui.hide()?;
    Ok(())
}
