use std::cell::RefCell;
use std::error::Error;
use std::fmt;
use std::rc::Rc;

use ferrink_shell::{
    ShellCommand, ShellCommandOutcome, ShellCommandPort, ShellController, ShellProfile,
    ShellWindow, configure_bundled_preview_catalog, configure_shell_window, install_shell_font,
    install_shell_handlers, sync_shell_ui,
};
use slint::ComponentHandle;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Options {
    profile: ShellProfile,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            profile: ShellProfile::Paperwhite1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ArgumentError(String);

impl fmt::Display for ArgumentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for ArgumentError {}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct PreviewCommandPort;

impl ShellCommandPort for PreviewCommandPort {
    fn submit(&mut self, _command: ShellCommand) -> ShellCommandOutcome {
        ShellCommandOutcome::Unavailable
    }
}

fn parse_options(arguments: impl IntoIterator<Item = String>) -> Result<Options, ArgumentError> {
    let mut options = Options::default();
    let mut arguments = arguments.into_iter();

    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--profile" => {
                let value = arguments
                    .next()
                    .ok_or_else(|| ArgumentError("--profile requires pw1 or koa3".into()))?;
                options.profile = match value.as_str() {
                    "pw1" => ShellProfile::Paperwhite1,
                    "koa3" => ShellProfile::Oasis3,
                    _ => {
                        return Err(ArgumentError(format!(
                            "unsupported preview profile {value:?}; expected pw1 or koa3"
                        )));
                    }
                };
            }
            _ => {
                return Err(ArgumentError(format!(
                    "unknown argument {argument:?}; expected --profile pw1|koa3"
                )));
            }
        }
    }

    Ok(options)
}

fn main() -> Result<(), Box<dyn Error>> {
    let options = parse_options(std::env::args().skip(1))?;
    let ui = ShellWindow::new()?;
    install_shell_font()?;
    let controller = Rc::new(RefCell::new(ShellController::default()));
    let command_port = Rc::new(RefCell::new(PreviewCommandPort));

    configure_shell_window(&ui, options.profile)?;
    configure_bundled_preview_catalog(&ui)?;
    sync_shell_ui(&ui, &controller.borrow());
    install_shell_handlers(&ui, &controller, &command_port);

    ui.run()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrink_shell::ShellAction;

    #[test]
    fn preview_defaults_to_paperwhite_geometry() {
        let options = parse_options(Vec::<String>::new()).expect("default arguments should parse");

        assert_eq!(options.profile, ShellProfile::Paperwhite1);
        assert_eq!(options.profile.dimensions(), (758, 1024));
    }

    #[test]
    fn exact_oasis_profile_is_accepted() {
        let options =
            parse_options(["--profile".into(), "koa3".into()]).expect("known profile should parse");

        assert_eq!(options.profile, ShellProfile::Oasis3);
        assert_eq!(options.profile.dimensions(), (1264, 1680));
    }

    #[test]
    fn unknown_arguments_and_profile_values_fail_closed() {
        assert!(parse_options(["--device".into()]).is_err());
        assert!(parse_options(["--profile".into(), "pw5".into()]).is_err());
        assert!(parse_options(["--profile".into()]).is_err());
    }

    #[test]
    fn emitted_commands_are_only_recorded_as_inert_preview_notices() {
        let mut controller = ShellController::default();
        let mut command_port = PreviewCommandPort;
        let command = controller
            .dispatch(ShellAction::RequestStock)
            .expect("stock action should produce a typed request");

        assert_eq!(command, ShellCommand::ReturnToStock);
        let outcome = command_port.submit(command);
        assert_eq!(outcome, ShellCommandOutcome::Unavailable);
        controller.note_command_outcome(command, outcome);
        assert_eq!(
            controller.notice().text(),
            "Preview only · stock handoff was not executed"
        );
    }
}
