use std::ffi::OsString;
use std::path::PathBuf;

#[cfg(any(
    test,
    all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")
))]
use std::cell::Cell;
#[cfg(any(
    test,
    all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")
))]
use std::rc::Rc;

#[cfg(any(
    test,
    all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")
))]
use ferrink_shell::{ShellCommand, ShellCommandOutcome, ShellCommandPort};

#[cfg(all(target_os = "linux", target_arch = "arm", target_pointer_width = "32"))]
const MAX_REPORT_AGE_SECONDS: u64 = 15 * 60;

#[cfg(any(
    test,
    all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")
))]
const COMMAND_RETURN_STOCK: u8 = b'S';
#[cfg(any(
    test,
    all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")
))]
const COMMAND_LAUNCH_APPLICATION: u8 = b'A';
#[cfg(any(
    test,
    all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")
))]
const COMMAND_REBOOT: u8 = b'R';
#[cfg(any(
    test,
    all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")
))]
const COMMAND_POWER_OFF: u8 = b'P';

#[derive(Debug, PartialEq, Eq)]
struct Arguments {
    profile: PathBuf,
    report: PathBuf,
    application_manifests: Vec<PathBuf>,
    ready_fd: Option<i32>,
    command_fd: Option<i32>,
}

#[derive(Debug, PartialEq, Eq)]
enum ParseResult {
    Run(Arguments),
    Help,
}

#[cfg(any(
    test,
    all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")
))]
struct GuardianCommandPort {
    exit_requested: Rc<Cell<bool>>,
    request: Rc<Cell<Option<ShellCommand>>>,
    guardian_attached: bool,
}

#[cfg(any(
    test,
    all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")
))]
impl ShellCommandPort for GuardianCommandPort {
    fn submit(&mut self, command: ShellCommand) -> ShellCommandOutcome {
        match command {
            ShellCommand::ReturnToStock
            | ShellCommand::LaunchApplication(_)
            | ShellCommand::Reboot
            | ShellCommand::PowerOff
                if self.guardian_attached && self.request.get().is_none() =>
            {
                self.request.set(Some(command));
                self.exit_requested.set(true);
                ShellCommandOutcome::Accepted
            }
            ShellCommand::ReturnToStock
            | ShellCommand::LaunchApplication(_)
            | ShellCommand::Reboot
            | ShellCommand::PowerOff => ShellCommandOutcome::Unavailable,
        }
    }
}

#[cfg(any(
    test,
    all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")
))]
fn encode_guardian_command(command: ShellCommand) -> Option<([u8; 2], usize)> {
    match command {
        ShellCommand::ReturnToStock => Some(([COMMAND_RETURN_STOCK, 0], 1)),
        ShellCommand::LaunchApplication(index) => {
            Some(([COMMAND_LAUNCH_APPLICATION, index.value()], 2))
        }
        ShellCommand::Reboot => Some(([COMMAND_REBOOT, 0], 1)),
        ShellCommand::PowerOff => Some(([COMMAND_POWER_OFF, 0], 1)),
    }
}

fn main() {
    match run(std::env::args_os().skip(1)) {
        Ok(()) => {}
        Err(error) => {
            eprintln!("ferrink-shell-kindle: {error}");
            std::process::exit(2);
        }
    }
}

fn run(arguments: impl Iterator<Item = OsString>) -> Result<(), Box<dyn std::error::Error>> {
    let arguments = match parse_arguments(arguments)? {
        ParseResult::Run(arguments) => arguments,
        ParseResult::Help => {
            print_help();
            return Ok(());
        }
    };
    run_on_target(arguments)
}

#[cfg(all(target_os = "linux", target_arch = "arm", target_pointer_width = "32"))]
fn run_on_target(arguments: Arguments) -> Result<(), Box<dyn std::error::Error>> {
    use std::cell::RefCell;
    use std::fs::File;
    use std::io::{Read, Write};
    use std::num::NonZeroU32;
    use std::os::fd::FromRawFd;
    use std::path::Path;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use ferrink_manifest::{ApplicationCatalog, MAX_APPLICATIONS, ValidatedApplicationManifest};
    use ferrink_platform::{
        DeviceProfile, Gray8Conversion, LogicalTouchPhase, ProbeReport, RefreshCompletionPolicy,
        RefreshMode, ResolvedRuntimeDevice,
    };
    use ferrink_platform_kindle::{
        BoundedInputPump, ExclusiveInputSession, InputLoopLimits, InputPumpOutcome, L0DisplayCore,
        L0InputCore, LinuxForegroundDisplayTarget, LinuxReadOnlyDeviceIo, LinuxReadOnlyFramebuffer,
        LinuxReadOnlyInput, SlintFrameBuffer, SlintPointerBridge, new_slint_window,
        revalidate_read_only,
    };
    use ferrink_shell::{
        KindleShellDevicePort, ShellController, ShellProfile, ShellWindow,
        configure_application_catalog, configure_shell_window, install_device_handlers,
        install_shell_font, install_shell_handlers, sync_shell_ui,
    };
    use slint::ComponentHandle;
    use slint::platform::{Platform, PlatformError, WindowAdapter};

    const MAX_INPUT_FILE_BYTES: u64 = 1_048_576;
    const MAX_RUNTIME_SECONDS: u64 = 365 * 24 * 60 * 60;
    const INPUT_POLL_MILLIS: u64 = 100;
    const READ_BUFFER_BYTES: u32 = 4_096;
    const INPUT_RECORD_BUDGET: u32 = 10_000;
    const INPUT_RECORD_RENEW_AT: u32 = 8_000;
    const GUARDIAN_READY_FD: i32 = 3;
    const GUARDIAN_READY_BYTE: u8 = b'R';
    const GUARDIAN_COMMAND_FD: i32 = 4;
    const MAX_INPUT_DIAGNOSTIC_READS: u32 = 32;
    const MAX_INPUT_DIAGNOSTIC_CONTACTS: u32 = 16;

    fn read_regular_file(label: &str, path: &Path) -> Result<String, Box<dyn std::error::Error>> {
        let canonical = path
            .canonicalize()
            .map_err(|error| format!("cannot resolve {label} {}: {error}", path.display()))?;
        if ["/dev", "/proc", "/sys"]
            .iter()
            .any(|root| canonical.starts_with(root))
        {
            return Err(format!(
                "refusing {label} path in a device or kernel tree: {}",
                canonical.display()
            )
            .into());
        }
        let file = std::fs::File::open(&canonical)
            .map_err(|error| format!("cannot open {label} {}: {error}", canonical.display()))?;
        let metadata = file
            .metadata()
            .map_err(|error| format!("cannot inspect {label} {}: {error}", canonical.display()))?;
        if !metadata.is_file() || metadata.len() > MAX_INPUT_FILE_BYTES {
            return Err(format!("refusing invalid or oversized {label} file").into());
        }
        let mut input = String::new();
        file.take(MAX_INPUT_FILE_BYTES + 1)
            .read_to_string(&mut input)
            .map_err(|error| format!("cannot read UTF-8 {label}: {error}"))?;
        if input.len() as u64 > MAX_INPUT_FILE_BYTES {
            return Err(format!("{label} grew beyond {MAX_INPUT_FILE_BYTES} bytes").into());
        }
        Ok(input)
    }

    fn read_application_catalog(
        manifest_paths: &[PathBuf],
    ) -> Result<ApplicationCatalog, Box<dyn std::error::Error>> {
        if manifest_paths.len() > MAX_APPLICATIONS {
            return Err("too many application manifests".into());
        }
        let mut catalog = ApplicationCatalog::default();
        for path in manifest_paths {
            let input = read_regular_file("application manifest", path)?;
            catalog.register(ValidatedApplicationManifest::from_toml(&input)?)?;
        }
        Ok(catalog)
    }

    fn take_readiness_writer(
        ready_fd: Option<i32>,
    ) -> Result<Option<File>, Box<dyn std::error::Error>> {
        let Some(ready_fd) = ready_fd else {
            return Ok(None);
        };
        if ready_fd != GUARDIAN_READY_FD {
            return Err("guardian readiness descriptor is not the fixed descriptor".into());
        }
        // SAFETY: F_GETFD validates that the guardian-provided descriptor is
        // open in this process before File assumes ownership of that exact fd.
        if unsafe { libc::fcntl(ready_fd, libc::F_GETFD) } < 0 {
            return Err(format!(
                "guardian readiness descriptor is unavailable: {}",
                std::io::Error::last_os_error()
            )
            .into());
        }
        // SAFETY: the descriptor was just validated, is uniquely transferred
        // by the guardian for this child, and is consumed exactly once here.
        Ok(Some(unsafe { File::from_raw_fd(ready_fd) }))
    }

    fn take_command_writer(
        command_fd: Option<i32>,
    ) -> Result<Option<File>, Box<dyn std::error::Error>> {
        let Some(command_fd) = command_fd else {
            return Ok(None);
        };
        if command_fd != GUARDIAN_COMMAND_FD {
            return Err("guardian command descriptor is not the fixed descriptor".into());
        }
        // SAFETY: F_GETFD validates that the guardian-provided descriptor is
        // open before File assumes ownership of the unique child-side copy.
        if unsafe { libc::fcntl(command_fd, libc::F_GETFD) } < 0 {
            return Err(format!(
                "guardian command descriptor is unavailable: {}",
                std::io::Error::last_os_error()
            )
            .into());
        }
        // SAFETY: the descriptor was just validated and is consumed once.
        Ok(Some(unsafe { File::from_raw_fd(command_fd) }))
    }

    struct RuntimeState {
        exclusive: ExclusiveInputSession<LinuxReadOnlyInput, LinuxReadOnlyFramebuffer>,
        target: LinuxForegroundDisplayTarget,
        display: L0DisplayCore,
        input: L0InputCore,
        pump: BoundedInputPump,
        frame: SlintFrameBuffer,
        pointer: SlintPointerBridge,
        readiness: Option<File>,
        diagnostics: InputDiagnostics,
    }

    #[derive(Debug, Default)]
    struct InputDiagnostics {
        reads: u32,
        contacts: u32,
    }

    impl InputDiagnostics {
        fn read(&mut self, bytes: u32, contacts: usize) {
            self.reads = self.reads.saturating_add(1);
            if self.reads <= MAX_INPUT_DIAGNOSTIC_READS {
                eprintln!(
                    "ferrink-shell-kindle: input read {} accepted {bytes} bytes and produced {contacts} contacts",
                    self.reads
                );
            } else if self.reads == MAX_INPUT_DIAGNOSTIC_READS + 1 {
                eprintln!("ferrink-shell-kindle: further input-read diagnostics suppressed");
            }
        }

        fn dispatch(
            &mut self,
            phase: LogicalTouchPhase,
            result: &slint::WindowEventDispatchResult,
        ) {
            self.contacts = self.contacts.saturating_add(1);
            if self.contacts <= MAX_INPUT_DIAGNOSTIC_CONTACTS {
                eprintln!(
                    "ferrink-shell-kindle: contact {} {phase:?} dispatch was {result:?}",
                    self.contacts
                );
            } else if self.contacts == MAX_INPUT_DIAGNOSTIC_CONTACTS + 1 {
                eprintln!("ferrink-shell-kindle: further contact diagnostics suppressed");
            }
        }
    }

    impl RuntimeState {
        fn run(
            mut self,
            window: &slint::platform::software_renderer::MinimalSoftwareWindow,
            exit_requested: &Cell<bool>,
        ) -> Result<(), PlatformError> {
            let loop_result = self.run_loop(window, exit_requested);
            self.pump.stop();
            let pointer_result = self.pointer.stop(window.window()).map_err(platform_error);

            let Self {
                exclusive, target, ..
            } = self;
            let release_result = exclusive.release().map_err(platform_error);
            let close_result = target.close().map_err(platform_error);

            loop_result?;
            pointer_result?;
            release_result?;
            close_result?;
            Ok(())
        }

        fn run_loop(
            &mut self,
            window: &slint::platform::software_renderer::MinimalSoftwareWindow,
            exit_requested: &Cell<bool>,
        ) -> Result<(), PlatformError> {
            let mut first_frame = true;
            slint::platform::update_timers_and_animations();
            self.present(window, &mut first_frame)?;
            if first_frame {
                return Err(PlatformError::Other(
                    "initial Slint frame was not presented".to_owned(),
                ));
            }
            self.signal_ready()?;

            while !exit_requested.get() {
                if self.input.decoded_records() >= INPUT_RECORD_RENEW_AT {
                    self.input.renew_record_budget();
                }
                match self
                    .exclusive
                    .pump_input_at(&mut self.pump, Instant::now(), &mut self.input)
                {
                    Ok(
                        InputPumpOutcome::TimedOut
                        | InputPumpOutcome::Interrupted
                        | InputPumpOutcome::WouldBlock,
                    ) => {}
                    Ok(InputPumpOutcome::Read { bytes, contacts }) => {
                        self.diagnostics.read(bytes, contacts.len());
                        for contact in contacts {
                            let phase = contact.phase;
                            let result = self
                                .pointer
                                .dispatch(window.window(), contact)
                                .map_err(platform_error)?;
                            self.diagnostics.dispatch(phase, &result);
                        }
                    }
                    Err(error) => return Err(platform_error(error)),
                }

                if exit_requested.get() {
                    break;
                }
                slint::platform::update_timers_and_animations();
                self.present(window, &mut first_frame)?;
            }
            Ok(())
        }

        fn signal_ready(&mut self) -> Result<(), PlatformError> {
            let Some(mut readiness) = self.readiness.take() else {
                return Ok(());
            };
            readiness
                .write_all(&[GUARDIAN_READY_BYTE])
                .map_err(platform_error)
        }

        fn present(
            &mut self,
            window: &slint::platform::software_renderer::MinimalSoftwareWindow,
            first_frame: &mut bool,
        ) -> Result<(), PlatformError> {
            let mode = if *first_frame {
                RefreshMode::Full
            } else {
                RefreshMode::Partial
            };
            let outcome = self
                .frame
                .present_if_needed(
                    window,
                    &mut self.display,
                    &mut self.target,
                    mode,
                    RefreshCompletionPolicy::DoNotWait,
                    Gray8Conversion::Grayscale,
                )
                .map_err(platform_error)?;
            if matches!(
                outcome,
                ferrink_platform_kindle::SlintPresentOutcome::Presented { .. }
            ) {
                *first_frame = false;
            }
            Ok(())
        }
    }

    struct KindlePlatform {
        window: Rc<slint::platform::software_renderer::MinimalSoftwareWindow>,
        runtime: RefCell<Option<RuntimeState>>,
        exit_requested: Rc<Cell<bool>>,
        started: Instant,
    }

    impl Platform for KindlePlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
            Ok(self.window.clone())
        }

        fn duration_since_start(&self) -> Duration {
            self.started.elapsed()
        }

        fn run_event_loop(&self) -> Result<(), PlatformError> {
            let runtime =
                self.runtime.borrow_mut().take().ok_or_else(|| {
                    PlatformError::Other("Kindle event loop already ran".to_owned())
                })?;
            runtime.run(&self.window, &self.exit_requested)
        }
    }

    fn platform_error(error: impl std::fmt::Display) -> PlatformError {
        PlatformError::Other(error.to_string())
    }

    let profile_input = read_regular_file("profile", &arguments.profile)?;
    let report_input = read_regular_file("report", &arguments.report)?;
    let application_catalog = read_application_catalog(&arguments.application_manifests)?;
    let guardian_session = arguments.ready_fd.is_some() && arguments.command_fd.is_some();
    let readiness = take_readiness_writer(arguments.ready_fd)?;
    let mut command_writer = take_command_writer(arguments.command_fd)?;
    let profile = DeviceProfile::from_toml(&profile_input)?;
    let report = ProbeReport::from_json(&report_input)?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let age = now
        .checked_sub(report.captured_at_unix_seconds)
        .ok_or("probe report timestamp is in the future")?;
    // A long-lived guardian validated this report before its first lease and
    // retains the resolved device for app round trips. Its two private
    // descriptors attest subsequent shell children in that same session.
    if age > MAX_REPORT_AGE_SECONDS && !guardian_session {
        return Err(format!(
            "probe report is {age} seconds old; maximum is {MAX_REPORT_AGE_SECONDS}"
        )
        .into());
    }
    let runtime = ResolvedRuntimeDevice::resolve(&profile, &report)?;

    let mut device_io = LinuxReadOnlyDeviceIo;
    let session = revalidate_read_only(&runtime, &mut device_io)?;
    let display = L0DisplayCore::try_from_runtime(&runtime)?;
    let frame = SlintFrameBuffer::try_for_display(&display)?;
    let input = L0InputCore::try_from_runtime(
        &runtime,
        NonZeroU32::new(INPUT_RECORD_BUDGET).expect("fixed input record budget is non-zero"),
    )?;
    let pump = BoundedInputPump::try_new(
        Instant::now(),
        InputLoopLimits {
            maximum_duration: Duration::from_secs(MAX_RUNTIME_SECONDS),
            maximum_poll_slice: Duration::from_millis(INPUT_POLL_MILLIS),
            maximum_steps: NonZeroU32::MAX,
            maximum_bytes: NonZeroU32::MAX,
            read_buffer_bytes: NonZeroU32::new(READ_BUFFER_BYTES)
                .expect("fixed input buffer is non-zero"),
        },
    )?;
    let target = LinuxForegroundDisplayTarget::open(&runtime)?;
    let exclusive = match ExclusiveInputSession::acquire(session) {
        Ok(exclusive) => exclusive,
        Err(error) => {
            target.close()?;
            return Err(error.into());
        }
    };

    let window = new_slint_window();
    let exit_requested = Rc::new(Cell::new(false));
    let command_request = Rc::new(Cell::new(None));
    slint::platform::set_platform(Box::new(KindlePlatform {
        window,
        runtime: RefCell::new(Some(RuntimeState {
            exclusive,
            target,
            display,
            input,
            pump,
            frame,
            pointer: SlintPointerBridge::default(),
            readiness,
            diagnostics: InputDiagnostics::default(),
        })),
        exit_requested: Rc::clone(&exit_requested),
        started: Instant::now(),
    }))?;

    install_shell_font()?;
    let ui = ShellWindow::new()?;
    configure_shell_window(&ui, ShellProfile::Oasis3)?;
    configure_application_catalog(&ui, &application_catalog)?;
    let controller = Rc::new(RefCell::new(ShellController::for_device()));
    let command_port = Rc::new(RefCell::new(GuardianCommandPort {
        exit_requested,
        request: Rc::clone(&command_request),
        guardian_attached: command_writer.is_some(),
    }));
    sync_shell_ui(&ui, &controller.borrow());
    install_shell_handlers(&ui, &controller, &command_port);
    let _device_binding = install_device_handlers(&ui, KindleShellDevicePort::open());
    ui.run()?;
    let request = command_request
        .get()
        .ok_or("Kindle event loop exited without a guardian command")?;
    let (command_bytes, command_length) =
        encode_guardian_command(request).ok_or("unsupported command escaped shell policy")?;
    command_writer
        .as_mut()
        .ok_or("accepted command has no guardian channel")?
        .write_all(&command_bytes[..command_length])?;
    Ok(())
}

#[cfg(not(all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")))]
fn run_on_target(_arguments: Arguments) -> Result<(), Box<dyn std::error::Error>> {
    Err("Kindle shell requires 32-bit ARM Linux".into())
}

fn parse_arguments(mut arguments: impl Iterator<Item = OsString>) -> Result<ParseResult, String> {
    let mut profile = None;
    let mut report = None;
    let mut application_manifests = Vec::new();
    let mut ready_fd = None;
    let mut command_fd = None;
    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--profile") => set_path(&mut profile, "--profile", &mut arguments)?,
            Some("--report") => set_path(&mut report, "--report", &mut arguments)?,
            Some("--application-manifest") => {
                if application_manifests.len() == ferrink_manifest::MAX_APPLICATIONS {
                    return Err("too many --application-manifest values".to_owned());
                }
                application_manifests.push(PathBuf::from(
                    arguments
                        .next()
                        .ok_or_else(|| "--application-manifest requires a path".to_owned())?,
                ));
            }
            Some("--ready-fd") => {
                if ready_fd.is_some() {
                    return Err("--ready-fd may be supplied only once".to_owned());
                }
                let value = arguments
                    .next()
                    .ok_or_else(|| "--ready-fd requires a value".to_owned())?;
                if value.to_str() != Some("3") {
                    return Err("--ready-fd accepts only the guardian descriptor 3".to_owned());
                }
                ready_fd = Some(3);
            }
            Some("--command-fd") => {
                if command_fd.is_some() {
                    return Err("--command-fd may be supplied only once".to_owned());
                }
                let value = arguments
                    .next()
                    .ok_or_else(|| "--command-fd requires a value".to_owned())?;
                if value.to_str() != Some("4") {
                    return Err("--command-fd accepts only the guardian descriptor 4".to_owned());
                }
                command_fd = Some(4);
            }
            Some("-h" | "--help") => return Ok(ParseResult::Help),
            Some(other) => return Err(format!("unknown argument: {other}")),
            None => return Err("arguments must be valid Unicode".to_owned()),
        }
    }
    if ready_fd.is_some() != command_fd.is_some() {
        return Err("--ready-fd and --command-fd must be supplied together".to_owned());
    }
    Ok(ParseResult::Run(Arguments {
        profile: profile.ok_or_else(|| "--profile is required".to_owned())?,
        report: report.ok_or_else(|| "--report is required".to_owned())?,
        application_manifests,
        ready_fd,
        command_fd,
    }))
}

fn set_path(
    slot: &mut Option<PathBuf>,
    name: &str,
    arguments: &mut impl Iterator<Item = OsString>,
) -> Result<(), String> {
    if slot.is_some() {
        return Err(format!("{name} may be supplied only once"));
    }
    let value = arguments
        .next()
        .ok_or_else(|| format!("{name} requires a path"))?;
    *slot = Some(PathBuf::from(value));
    Ok(())
}

fn print_help() {
    println!(
        "ferrink-shell-kindle {}\n\n\
         Usage:\n  ferrink-shell-kindle --profile FILE --report FILE \\\n         [--application-manifest FILE]... [--ready-fd 3 --command-fd 4]\n\n\
         Runs the long-lived Ferrink shell on an exact, freshly resolved Kindle\n\
         display/input profile. Stock ownership and recovery belong to the\n\
         external supervisor; this process never changes boot configuration.",
        env!("CARGO_PKG_VERSION")
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args<'a>(values: &'a [&'a str]) -> impl Iterator<Item = OsString> + 'a {
        values.iter().map(OsString::from)
    }

    #[test]
    fn exact_profile_and_report_are_required_once() {
        let parsed = parse_arguments(args(&[
            "--profile",
            "profile.toml",
            "--report",
            "report.json",
        ]))
        .unwrap();
        assert!(matches!(parsed, ParseResult::Run(_)));
        let parsed = parse_arguments(args(&[
            "--profile",
            "profile.toml",
            "--report",
            "report.json",
            "--ready-fd",
            "3",
            "--command-fd",
            "4",
        ]))
        .unwrap();
        assert!(matches!(
            parsed,
            ParseResult::Run(arguments)
                if arguments.ready_fd == Some(3) && arguments.command_fd == Some(4)
        ));
        assert!(
            parse_arguments(args(&[
                "--profile",
                "profile.toml",
                "--report",
                "report.json",
                "--ready-fd",
                "4",
            ]))
            .is_err()
        );
        assert!(parse_arguments(args(&[])).is_err());
        assert!(parse_arguments(args(&["--profile", "one", "--profile", "two"])).is_err());
        assert!(parse_arguments(args(&["--command-fd", "3"])).is_err());
        assert!(
            parse_arguments(args(&[
                "--profile",
                "profile.toml",
                "--report",
                "report.json",
                "--ready-fd",
                "3",
            ]))
            .is_err()
        );
    }

    #[test]
    fn hardware_and_authority_overrides_do_not_exist() {
        for flag in [
            "--device",
            "--duration",
            "--waveform",
            "--marker",
            "--retry",
            "--force",
            "--install",
            "--reboot",
            "--power-off",
        ] {
            assert!(parse_arguments(args(&[flag])).is_err(), "accepted {flag}");
        }
    }

    #[test]
    fn guardian_accepts_one_confirmed_shell_request() {
        let exit_requested = Rc::new(Cell::new(false));
        let request = Rc::new(Cell::new(None));
        let mut port = GuardianCommandPort {
            exit_requested: Rc::clone(&exit_requested),
            request: Rc::clone(&request),
            guardian_attached: true,
        };

        assert_eq!(
            port.submit(ShellCommand::Reboot),
            ShellCommandOutcome::Accepted
        );
        assert!(exit_requested.get());
        assert_eq!(request.get(), Some(ShellCommand::Reboot));
        assert_eq!(
            port.submit(ShellCommand::PowerOff),
            ShellCommandOutcome::Unavailable
        );
        assert!(exit_requested.get());

        let exit_requested = Rc::new(Cell::new(false));
        let request = Rc::new(Cell::new(None));
        let mut port = GuardianCommandPort {
            exit_requested: Rc::clone(&exit_requested),
            request: Rc::clone(&request),
            guardian_attached: true,
        };
        let first_application = ferrink_shell::ApplicationIndex::try_from(0_usize).unwrap();
        assert_eq!(
            port.submit(ShellCommand::LaunchApplication(first_application)),
            ShellCommandOutcome::Accepted
        );
        assert!(exit_requested.get());
        assert_eq!(
            request.get(),
            Some(ShellCommand::LaunchApplication(first_application))
        );
        assert_eq!(
            port.submit(ShellCommand::ReturnToStock),
            ShellCommandOutcome::Unavailable
        );
    }

    #[test]
    fn detached_shell_cannot_claim_a_handoff() {
        let exit_requested = Rc::new(Cell::new(false));
        let mut port = GuardianCommandPort {
            exit_requested: Rc::clone(&exit_requested),
            request: Rc::new(Cell::new(None)),
            guardian_attached: false,
        };

        assert_eq!(
            port.submit(ShellCommand::ReturnToStock),
            ShellCommandOutcome::Unavailable
        );
        assert!(!exit_requested.get());
    }

    #[test]
    fn guardian_protocol_carries_the_exact_application_index() {
        let index = ferrink_shell::ApplicationIndex::try_from(37_usize).unwrap();
        assert_eq!(
            encode_guardian_command(ShellCommand::LaunchApplication(index)),
            Some(([b'A', 37], 2))
        );
        assert_eq!(
            encode_guardian_command(ShellCommand::ReturnToStock),
            Some(([b'S', 0], 1))
        );
        assert_eq!(
            encode_guardian_command(ShellCommand::Reboot),
            Some(([b'R', 0], 1))
        );
        assert_eq!(
            encode_guardian_command(ShellCommand::PowerOff),
            Some(([b'P', 0], 1))
        );
    }
}
