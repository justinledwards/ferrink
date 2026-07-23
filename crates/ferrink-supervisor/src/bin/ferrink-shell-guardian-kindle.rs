use std::ffi::OsString;
use std::path::PathBuf;

#[cfg(all(target_os = "linux", target_arch = "arm", target_pointer_width = "32"))]
const MAX_REPORT_AGE_SECONDS: u64 = 15 * 60;

#[derive(Debug, PartialEq, Eq)]
struct Arguments {
    profile: PathBuf,
    report: PathBuf,
    shell: PathBuf,
    application_manifests: Vec<PathBuf>,
    crash_state: Option<PathBuf>,
    startup_mode: StartupMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartupMode {
    StockRunning,
    StockStarting,
}

#[derive(Debug, PartialEq, Eq)]
enum ParseResult {
    Run(Arguments),
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(any(
    test,
    all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")
))]
enum GuardianDisposition {
    CleanExit,
    NonzeroExit,
    SpawnFailed,
    ReadinessFailed,
    ReadinessTimedOut,
    SignalRequested,
    ForcedTermination,
    ChildIoFailed,
    CommandMissing,
    CommandInvalid,
    CrashStateFailed,
}

#[cfg(any(
    test,
    all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")
))]
impl GuardianDisposition {
    const fn is_success(self) -> bool {
        matches!(self, Self::CleanExit)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(any(
    test,
    all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")
))]
enum ShellRequest {
    ReturnToStock,
    LaunchApplication(u8),
    Reboot,
    PowerOff,
}

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

#[cfg(any(
    test,
    all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")
))]
fn decode_shell_request(bytes: &[u8]) -> Result<ShellRequest, GuardianDisposition> {
    match bytes {
        [COMMAND_RETURN_STOCK] => Ok(ShellRequest::ReturnToStock),
        [COMMAND_LAUNCH_APPLICATION, index]
            if usize::from(*index) < ferrink_manifest::MAX_APPLICATIONS =>
        {
            Ok(ShellRequest::LaunchApplication(*index))
        }
        [COMMAND_REBOOT] => Ok(ShellRequest::Reboot),
        [COMMAND_POWER_OFF] => Ok(ShellRequest::PowerOff),
        [] => Err(GuardianDisposition::CommandMissing),
        _ => Err(GuardianDisposition::CommandInvalid),
    }
}

fn main() {
    match run(std::env::args_os().skip(1)) {
        Ok(()) => {}
        Err(error) => {
            eprintln!("ferrink-shell-guardian-kindle: {error}");
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
    use std::collections::BTreeMap;
    use std::fs::File;
    use std::io::Read;
    use std::num::NonZeroUsize;
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixStream;
    use std::os::unix::process::CommandExt;
    use std::path::Path;
    use std::process::{Child, Command, ExitStatus, Stdio};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use ferrink_manifest::{
        ApplicationCatalog, ApplicationManifest, DisplayHandoff, DisplayMode, MAX_APPLICATIONS,
        ValidatedApplicationManifest,
    };
    use ferrink_platform::{DeviceProfile, ProbeReport, ResolvedRuntimeDevice};
    use ferrink_supervisor::{
        CrashBudgetPolicy, EarlyBootForegroundLease, ForegroundLease, LinuxForegroundSignalGuard,
        LinuxKindleForegroundSystem, PersistentCrashBudget, PersistentCrashBudgetStart,
        PillowState, acquire_early_boot_foreground, acquire_foreground,
    };

    const MAX_INPUT_FILE_BYTES: u64 = 1_048_576;
    const MAX_ICON_FILE_BYTES: u64 = 1_048_576;
    const POLL_INTERVAL: Duration = Duration::from_millis(25);
    const TERMINATION_DEADLINE: Duration = Duration::from_secs(5);
    const READINESS_DEADLINE: Duration = Duration::from_secs(10);
    const STABLE_SUPERVISION: Duration = Duration::from_secs(2 * 60);
    const CRASH_BUDGET_WINDOW: Duration = Duration::from_secs(10 * 60);
    const CRASH_BUDGET_STARTS: usize = 3;
    const READY_FD: libc::c_int = 3;
    const READY_BYTE: u8 = b'R';
    const COMMAND_FD: libc::c_int = 4;

    struct RegisteredApplication {
        manifest: ApplicationManifest,
        executable: PathBuf,
    }

    enum ActiveForegroundLease {
        StockRunning(ForegroundLease),
        StockStarting(EarlyBootForegroundLease),
    }

    impl ActiveForegroundLease {
        fn restore(self, system: &mut LinuxKindleForegroundSystem) -> Result<(), String> {
            match self {
                Self::StockRunning(lease) => lease
                    .restore(system)
                    .map_err(|error| format!("stock-running restoration failed: {error:?}")),
                Self::StockStarting(lease) => lease
                    .restore(system)
                    .map_err(|error| format!("stock-starting restoration failed: {error:?}")),
            }
        }
    }

    fn read_regular_file(
        label: &str,
        path: &Path,
    ) -> Result<(PathBuf, String), Box<dyn std::error::Error>> {
        let canonical = exact_absolute_file(label, path, false)?;
        let file = File::open(&canonical)
            .map_err(|error| format!("cannot open {label} {}: {error}", canonical.display()))?;
        let metadata = file
            .metadata()
            .map_err(|error| format!("cannot inspect {label} {}: {error}", canonical.display()))?;
        if metadata.len() > MAX_INPUT_FILE_BYTES {
            return Err(format!("refusing oversized {label} file").into());
        }
        let mut input = String::new();
        file.take(MAX_INPUT_FILE_BYTES + 1)
            .read_to_string(&mut input)
            .map_err(|error| format!("cannot read UTF-8 {label}: {error}"))?;
        if input.len() as u64 > MAX_INPUT_FILE_BYTES {
            return Err(format!("{label} grew beyond {MAX_INPUT_FILE_BYTES} bytes").into());
        }
        Ok((canonical, input))
    }

    fn exact_absolute_file(
        label: &str,
        path: &Path,
        executable: bool,
    ) -> Result<PathBuf, Box<dyn std::error::Error>> {
        if !path.is_absolute() {
            return Err(format!("{label} path must be absolute").into());
        }
        let metadata = path
            .symlink_metadata()
            .map_err(|error| format!("cannot inspect {label} {}: {error}", path.display()))?;
        if !metadata.file_type().is_file() {
            return Err(format!("{label} must be a regular file").into());
        }
        if executable && metadata.permissions().mode() & 0o111 == 0 {
            return Err(format!("{label} is not executable").into());
        }
        let canonical = path
            .canonicalize()
            .map_err(|error| format!("cannot resolve {label} {}: {error}", path.display()))?;
        if canonical != path {
            return Err(format!("{label} path must be canonical and must not be a symlink").into());
        }
        if ["/dev", "/proc", "/sys"]
            .iter()
            .any(|root| canonical.starts_with(root))
        {
            return Err(format!("refusing {label} path in a device or kernel tree").into());
        }
        Ok(canonical)
    }

    struct SupervisedChild {
        child: Option<Child>,
        process_group: i32,
        readiness: UnixStream,
        commands: UnixStream,
    }

    impl SupervisedChild {
        fn spawn(
            shell: &Path,
            profile: &Path,
            report: &Path,
            application_manifests: &[PathBuf],
        ) -> Result<Self, std::io::Error> {
            let (readiness, child_readiness) = UnixStream::pair()?;
            let (commands, child_commands) = UnixStream::pair()?;
            readiness.set_nonblocking(true)?;
            let child_readiness_fd = child_readiness.as_raw_fd();
            let child_commands_fd = child_commands.as_raw_fd();
            let mut command = Command::new(shell);
            command
                .arg("--profile")
                .arg(profile)
                .arg("--report")
                .arg(report)
                .arg("--ready-fd")
                .arg(READY_FD.to_string())
                .arg("--command-fd")
                .arg(COMMAND_FD.to_string())
                .env_clear()
                .current_dir(shell.parent().unwrap_or(Path::new("/")))
                .stdin(Stdio::null())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .process_group(0);
            for manifest in application_manifests {
                command.arg("--application-manifest").arg(manifest);
            }
            // SAFETY: the closure calls only async-signal-safe descriptor
            // operations between fork and exec. Temporary copies above both
            // fixed targets prevent descriptor aliasing while the two unique
            // child socket ends are installed.
            unsafe {
                command.pre_exec(move || {
                    let readiness_source =
                        libc::fcntl(child_readiness_fd, libc::F_DUPFD_CLOEXEC, COMMAND_FD + 1);
                    if readiness_source < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    let command_source =
                        libc::fcntl(child_commands_fd, libc::F_DUPFD_CLOEXEC, COMMAND_FD + 1);
                    if command_source < 0 {
                        libc::close(readiness_source);
                        return Err(std::io::Error::last_os_error());
                    }
                    if libc::dup2(readiness_source, READY_FD) < 0
                        || libc::dup2(command_source, COMMAND_FD) < 0
                    {
                        let error = std::io::Error::last_os_error();
                        libc::close(readiness_source);
                        libc::close(command_source);
                        return Err(error);
                    }
                    libc::close(readiness_source);
                    libc::close(command_source);
                    Ok(())
                });
            }
            let mut child = command.spawn()?;
            drop(child_readiness);
            drop(child_commands);
            let process_group = match i32::try_from(child.id()) {
                Ok(process_group) => process_group,
                Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "child PID does not fit a process-group identifier",
                    ));
                }
            };
            Ok(Self {
                child: Some(child),
                process_group,
                readiness,
                commands,
            })
        }

        fn request(&mut self) -> Result<ShellRequest, GuardianDisposition> {
            let mut bytes = [0_u8; 3];
            let mut length = 0_usize;
            loop {
                if length == bytes.len() {
                    return Err(GuardianDisposition::CommandInvalid);
                }
                match self.commands.read(&mut bytes[length..]) {
                    Ok(0) => break,
                    Ok(count) => length += count,
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => return Err(GuardianDisposition::ChildIoFailed),
                }
            }
            decode_shell_request(&bytes[..length])
        }

        fn poll(&mut self) -> Result<Option<ExitStatus>, std::io::Error> {
            let child = self
                .child
                .as_mut()
                .ok_or_else(|| std::io::Error::other("child already reaped"))?;
            let status = child.try_wait()?;
            if status.is_some() {
                self.child = None;
            }
            Ok(status)
        }

        fn await_readiness(
            &mut self,
            signal_guard: &LinuxForegroundSignalGuard,
        ) -> Result<(), GuardianDisposition> {
            let started = Instant::now();
            let mut byte = [0_u8; 1];
            loop {
                match self.readiness.read(&mut byte) {
                    Ok(1) if byte[0] == READY_BYTE => return Ok(()),
                    Ok(1) | Ok(0) => {
                        let _ = self.terminate_bounded();
                        return Err(GuardianDisposition::ReadinessFailed);
                    }
                    Ok(_) => unreachable!("one-byte readiness read returned too many bytes"),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => {
                        let _ = self.terminate_bounded();
                        return Err(GuardianDisposition::ReadinessFailed);
                    }
                }

                match self.poll() {
                    Ok(Some(_)) | Err(_) => return Err(GuardianDisposition::ReadinessFailed),
                    Ok(None) => {}
                }
                if signal_guard.pending_signal().is_some() {
                    return Err(match self.terminate_bounded() {
                        Ok(false) => GuardianDisposition::SignalRequested,
                        Ok(true) => GuardianDisposition::ForcedTermination,
                        Err(_) => GuardianDisposition::ChildIoFailed,
                    });
                }
                if started.elapsed() >= READINESS_DEADLINE {
                    let _ = self.terminate_bounded();
                    return Err(GuardianDisposition::ReadinessTimedOut);
                }
                std::thread::sleep(POLL_INTERVAL);
            }
        }

        fn terminate_bounded(&mut self) -> Result<bool, std::io::Error> {
            self.signal_group(libc::SIGTERM)?;
            let started = Instant::now();
            while started.elapsed() < TERMINATION_DEADLINE {
                if self.poll()?.is_some() {
                    return Ok(false);
                }
                std::thread::sleep(POLL_INTERVAL);
            }

            self.signal_group(libc::SIGKILL)?;
            let mut child = self
                .child
                .take()
                .ok_or_else(|| std::io::Error::other("child disappeared before reap"))?;
            child.wait()?;
            Ok(true)
        }

        #[allow(unsafe_code)]
        fn signal_group(&self, signal: libc::c_int) -> Result<(), std::io::Error> {
            // SAFETY: spawn created a new process group whose positive ID is
            // the exact child PID. Negating it targets that group only.
            if unsafe { libc::kill(-self.process_group, signal) } < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        }
    }

    impl Drop for SupervisedChild {
        fn drop(&mut self) {
            let Some(mut child) = self.child.take() else {
                return;
            };
            let _ = self.signal_group(libc::SIGKILL);
            let _ = child.wait();
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum ApplicationDisposition {
        CleanExit,
        NonzeroExit,
        SpawnFailed,
        SignalRequested,
        ForcedTermination,
        ChildIoFailed,
        CrashStateFailed,
    }

    impl ApplicationDisposition {
        const fn was_interrupted(self) -> bool {
            matches!(
                self,
                Self::SignalRequested
                    | Self::ForcedTermination
                    | Self::ChildIoFailed
                    | Self::CrashStateFailed
            )
        }
    }

    struct SupervisedApplication {
        child: Option<Child>,
        process_group: i32,
        exit_deadline: Duration,
    }

    impl SupervisedApplication {
        fn spawn(
            executable: &Path,
            application: &ferrink_manifest::ApplicationManifest,
        ) -> Result<Self, std::io::Error> {
            let mut command = Command::new(executable);
            command
                .args(application.command.iter().skip(1))
                .env_clear()
                .envs(&application.environment)
                .current_dir(executable.parent().unwrap_or(Path::new("/")))
                .stdin(Stdio::null())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .process_group(0);
            let mut child = command.spawn()?;
            let process_group = match i32::try_from(child.id()) {
                Ok(process_group) => process_group,
                Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "application PID does not fit a process-group identifier",
                    ));
                }
            };
            Ok(Self {
                child: Some(child),
                process_group,
                exit_deadline: Duration::from_secs(u64::from(
                    application.lifecycle.exit_timeout_seconds,
                )),
            })
        }

        fn poll(&mut self) -> Result<Option<ExitStatus>, std::io::Error> {
            let child = self
                .child
                .as_mut()
                .ok_or_else(|| std::io::Error::other("application already reaped"))?;
            let status = child.try_wait()?;
            if status.is_some() {
                self.child = None;
            }
            Ok(status)
        }

        fn terminate_bounded(&mut self) -> Result<bool, std::io::Error> {
            self.signal_group(libc::SIGTERM)?;
            let started = Instant::now();
            while started.elapsed() < self.exit_deadline {
                if self.poll()?.is_some() {
                    return Ok(false);
                }
                std::thread::sleep(POLL_INTERVAL);
            }
            self.signal_group(libc::SIGKILL)?;
            let mut child = self
                .child
                .take()
                .ok_or_else(|| std::io::Error::other("application disappeared before reap"))?;
            child.wait()?;
            Ok(true)
        }

        #[allow(unsafe_code)]
        fn signal_group(&self, signal: libc::c_int) -> Result<(), std::io::Error> {
            // SAFETY: spawn created a new process group whose positive ID is
            // the exact launcher PID. Its children inherit that group.
            if unsafe { libc::kill(-self.process_group, signal) } < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        }
    }

    impl Drop for SupervisedApplication {
        fn drop(&mut self) {
            let Some(mut child) = self.child.take() else {
                return;
            };
            let _ = self.signal_group(libc::SIGKILL);
            let _ = child.wait();
        }
    }

    fn mark_stable_if_due(
        crash_budget: &mut Option<PersistentCrashBudget>,
        guardian_started: Instant,
        force: bool,
    ) -> Result<(), ferrink_supervisor::PersistentCrashBudgetError> {
        if crash_budget.is_none() || (!force && guardian_started.elapsed() < STABLE_SUPERVISION) {
            return Ok(());
        }
        crash_budget
            .as_mut()
            .expect("crash budget was checked above")
            .mark_stable()?;
        *crash_budget = None;
        Ok(())
    }

    fn supervise(
        child: &mut SupervisedChild,
        signal_guard: &LinuxForegroundSignalGuard,
        crash_budget: &mut Option<PersistentCrashBudget>,
        guardian_started: Instant,
    ) -> GuardianDisposition {
        loop {
            if mark_stable_if_due(crash_budget, guardian_started, false).is_err() {
                let _ = child.terminate_bounded();
                return GuardianDisposition::CrashStateFailed;
            }
            match child.poll() {
                Ok(Some(status)) => {
                    return if status.success() {
                        GuardianDisposition::CleanExit
                    } else {
                        GuardianDisposition::NonzeroExit
                    };
                }
                Ok(None) => {}
                Err(_) => {
                    let _ = child.terminate_bounded();
                    return GuardianDisposition::ChildIoFailed;
                }
            }

            if signal_guard.pending_signal().is_some() {
                return match child.terminate_bounded() {
                    Ok(false) => GuardianDisposition::SignalRequested,
                    Ok(true) => GuardianDisposition::ForcedTermination,
                    Err(_) => GuardianDisposition::ChildIoFailed,
                };
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    fn supervise_application(
        child: &mut SupervisedApplication,
        signal_guard: &LinuxForegroundSignalGuard,
        crash_budget: &mut Option<PersistentCrashBudget>,
        guardian_started: Instant,
    ) -> ApplicationDisposition {
        loop {
            if mark_stable_if_due(crash_budget, guardian_started, false).is_err() {
                let _ = child.terminate_bounded();
                return ApplicationDisposition::CrashStateFailed;
            }
            match child.poll() {
                Ok(Some(status)) => {
                    return if status.success() {
                        ApplicationDisposition::CleanExit
                    } else {
                        ApplicationDisposition::NonzeroExit
                    };
                }
                Ok(None) => {}
                Err(_) => {
                    let _ = child.terminate_bounded();
                    return ApplicationDisposition::ChildIoFailed;
                }
            }
            if signal_guard.pending_signal().is_some() {
                return match child.terminate_bounded() {
                    Ok(false) => ApplicationDisposition::SignalRequested,
                    Ok(true) => ApplicationDisposition::ForcedTermination,
                    Err(_) => ApplicationDisposition::ChildIoFailed,
                };
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let guardian_started = Instant::now();
    let mut crash_budget = if let Some(path) = arguments.crash_state.as_deref() {
        let policy = CrashBudgetPolicy::new(
            NonZeroUsize::new(CRASH_BUDGET_STARTS).expect("crash budget is nonzero"),
            CRASH_BUDGET_WINDOW,
        )?;
        match PersistentCrashBudget::begin(path, policy, now)? {
            PersistentCrashBudgetStart::Permit(budget) => Some(budget),
            PersistentCrashBudgetStart::Fallback {
                retry_at_unix_seconds,
            } => {
                return Err(format!(
                    "startup crash budget exhausted; select stock until unix second {retry_at_unix_seconds}"
                )
                .into());
            }
        }
    } else {
        None
    };

    let (profile_path, profile_input) = read_regular_file("profile", &arguments.profile)?;
    let (report_path, report_input) = read_regular_file("report", &arguments.report)?;
    let shell_path = exact_absolute_file("shell", &arguments.shell, true)?;
    if arguments.application_manifests.is_empty() {
        return Err("shell guardian requires at least one application manifest".into());
    }
    if arguments.application_manifests.len() > MAX_APPLICATIONS {
        return Err("too many application manifests".into());
    }
    let mut catalog = ApplicationCatalog::default();
    let mut manifest_paths_by_id = BTreeMap::new();
    for path in &arguments.application_manifests {
        let (canonical_path, input) = read_regular_file("application manifest", path)?;
        let application = ValidatedApplicationManifest::from_toml(&input)?;
        let id = application.manifest().id.clone();
        catalog.register(application)?;
        manifest_paths_by_id.insert(id, canonical_path);
    }
    let application_manifest_paths: Vec<_> = manifest_paths_by_id.into_values().collect();
    let mut applications = Vec::with_capacity(catalog.len());
    for application in catalog.iter() {
        let manifest = application.manifest().clone();
        if manifest.display.mode != DisplayMode::Framebuffer {
            return Err("shell guardian accepts only framebuffer application manifests".into());
        }
        let executable = exact_absolute_file(
            "application executable",
            Path::new(&manifest.command[0]),
            true,
        )?;
        let icon = exact_absolute_file("application icon", Path::new(&manifest.icon), false)?;
        let icon_size = icon
            .metadata()
            .map_err(|error| format!("cannot inspect application icon: {error}"))?
            .len();
        if icon_size == 0 || icon_size > MAX_ICON_FILE_BYTES {
            return Err("application icon must be a non-empty PNG no larger than 1 MiB".into());
        }
        let mut png_signature = [0_u8; 8];
        File::open(&icon)
            .and_then(|mut file| file.read_exact(&mut png_signature))
            .map_err(|error| format!("cannot read application icon signature: {error}"))?;
        if png_signature != *b"\x89PNG\r\n\x1a\n" {
            return Err("application icon does not have a PNG signature".into());
        }
        applications.push(RegisteredApplication {
            manifest,
            executable,
        });
    }
    if arguments.startup_mode == StartupMode::StockStarting
        && applications.iter().any(|application| {
            application.manifest.display.handoff == DisplayHandoff::StockMediated
        })
    {
        return Err(
            "stock-starting startup accepts only supervisor-owned application handoffs".into(),
        );
    }
    let profile = DeviceProfile::from_toml(&profile_input)?;
    let report = ProbeReport::from_json(&report_input)?;
    let age = now
        .checked_sub(report.captured_at_unix_seconds)
        .ok_or("probe report timestamp is in the future")?;
    if age > MAX_REPORT_AGE_SECONDS {
        return Err(format!(
            "probe report is {age} seconds old; maximum is {MAX_REPORT_AGE_SECONDS}"
        )
        .into());
    }
    let runtime = ResolvedRuntimeDevice::resolve(&profile, &report)?;

    let confirmed_pillow = match arguments.startup_mode {
        StartupMode::StockRunning => PillowState::Enabled,
        StartupMode::StockStarting => PillowState::Disabled,
    };
    let mut system = LinuxKindleForegroundSystem::try_new(&runtime, confirmed_pillow)?;
    let signal_guard = LinuxForegroundSignalGuard::install()?;
    loop {
        let acquired = match arguments.startup_mode {
            StartupMode::StockRunning => acquire_foreground(&mut system)
                .map(ActiveForegroundLease::StockRunning)
                .map_err(|error| format!("stock-running acquisition failed: {error:?}")),
            StartupMode::StockStarting => acquire_early_boot_foreground(&mut system)
                .map(ActiveForegroundLease::StockStarting)
                .map_err(|error| format!("stock-starting acquisition failed: {error:?}")),
        };
        let mut lease = Some(match acquired {
            Ok(lease) => lease,
            Err(error) => {
                let signal_restore = signal_guard.finish();
                return match signal_restore {
                    Ok(_) => Err(error.into()),
                    Err(signal_error) => Err(format!(
                        "{error}; signal restoration also failed: {signal_error}"
                    )
                    .into()),
                };
            }
        });

        loop {
            let (disposition, request) = if signal_guard.pending_signal().is_some() {
                (GuardianDisposition::SignalRequested, None)
            } else {
                match SupervisedChild::spawn(
                    &shell_path,
                    &profile_path,
                    &report_path,
                    &application_manifest_paths,
                ) {
                    Ok(mut child) => match child.await_readiness(&signal_guard) {
                        Ok(()) => {
                            let disposition = supervise(
                                &mut child,
                                &signal_guard,
                                &mut crash_budget,
                                guardian_started,
                            );
                            if disposition.is_success() {
                                match child.request() {
                                    Ok(request) => (disposition, Some(request)),
                                    Err(error) => (error, None),
                                }
                            } else {
                                (disposition, None)
                            }
                        }
                        Err(disposition) => (disposition, None),
                    },
                    Err(_) => (GuardianDisposition::SpawnFailed, None),
                }
            };

            if signal_guard.pending_signal().is_some() || !disposition.is_success() {
                let active_lease = lease
                    .take()
                    .expect("foreground lease exists until an explicit handoff");
                if let Err(error) = active_lease.restore(&mut system) {
                    let signal_restore = signal_guard.finish();
                    return match signal_restore {
                        Ok(_) => Err(format!(
                            "stock restoration failed after {disposition:?}: {error:?}"
                        )
                        .into()),
                        Err(signal_error) => Err(format!(
                            "stock restoration and signal restoration failed after {disposition:?}: {error:?}; {signal_error}"
                        )
                        .into()),
                    };
                }
                let deferred_signal = signal_guard.finish()?;
                return Err(format!(
                    "guardian restored stock after shell outcome {disposition:?}; deferred signal {deferred_signal:?}"
                )
                .into());
            }

            match request.ok_or("clean shell exit omitted its private command")? {
                ShellRequest::ReturnToStock => {
                    let active_lease = lease
                        .take()
                        .expect("foreground lease exists until stock handoff");
                    active_lease.restore(&mut system).map_err(|error| {
                        format!("stock restoration failed after clean shell exit: {error:?}")
                    })?;
                    mark_stable_if_due(&mut crash_budget, guardian_started, true)?;
                    let deferred_signal = signal_guard.finish()?;
                    if deferred_signal.is_some() {
                        return Err("guardian restored stock after a deferred signal".into());
                    }
                    return Ok(());
                }
                ShellRequest::LaunchApplication(index) => {
                    let application = applications
                        .get(usize::from(index))
                        .ok_or("shell selected an application outside the registered catalog")?;
                    if application.manifest.display.handoff == DisplayHandoff::StockMediated {
                        let active_lease = lease
                            .take()
                            .expect("foreground lease exists until stock-mediated handoff");
                        active_lease.restore(&mut system).map_err(|error| {
                            format!(
                                "stock restoration failed before application handoff: {error:?}"
                            )
                        })?;
                    }
                    let handoff_label = match application.manifest.display.handoff {
                        DisplayHandoff::Supervisor => "inside the Ferrink foreground lease",
                        DisplayHandoff::StockMediated => "from restored stock",
                    };
                    eprintln!(
                        "ferrink-shell-guardian-kindle: launching {} ({}) {handoff_label}",
                        application.manifest.name, application.manifest.id
                    );
                    let application_disposition = match SupervisedApplication::spawn(
                        &application.executable,
                        &application.manifest,
                    ) {
                        Ok(mut child) => supervise_application(
                            &mut child,
                            &signal_guard,
                            &mut crash_budget,
                            guardian_started,
                        ),
                        Err(error) => {
                            eprintln!(
                                "ferrink-shell-guardian-kindle: application spawn failed: {error}"
                            );
                            ApplicationDisposition::SpawnFailed
                        }
                    };
                    eprintln!(
                        "ferrink-shell-guardian-kindle: application outcome {application_disposition:?}; returning to ferrink"
                    );
                    if application_disposition.was_interrupted()
                        || signal_guard.pending_signal().is_some()
                    {
                        if let Some(active_lease) = lease.take() {
                            active_lease.restore(&mut system).map_err(|error| {
                                format!(
                                    "stock restoration failed after interrupted application: {error:?}"
                                )
                            })?;
                        }
                        let deferred_signal = signal_guard.finish()?;
                        return Err(format!(
                            "application handoff ended after deferred signal {deferred_signal:?}: {application_disposition:?}"
                        )
                        .into());
                    }
                    if application.manifest.display.handoff == DisplayHandoff::StockMediated {
                        break;
                    }
                }
                action @ (ShellRequest::Reboot | ShellRequest::PowerOff) => {
                    let active_lease = lease
                        .take()
                        .expect("foreground lease exists until a system action");
                    mark_stable_if_due(&mut crash_budget, guardian_started, true)?;
                    let deferred_signal = signal_guard.finish()?;
                    if deferred_signal.is_some() {
                        active_lease.restore(&mut system).map_err(|error| {
                            format!(
                                "stock restoration failed after deferred system action: {error:?}"
                            )
                        })?;
                        return Err("system action rejected after a deferred signal".into());
                    }

                    let (label, executable) = match action {
                        ShellRequest::Reboot => ("restart", "/sbin/reboot"),
                        ShellRequest::PowerOff => ("power off", "/sbin/poweroff"),
                        ShellRequest::ReturnToStock | ShellRequest::LaunchApplication(_) => {
                            unreachable!("system-action match accepts only restart or power off")
                        }
                    };
                    eprintln!("ferrink-shell-guardian-kindle: requesting {label}");
                    let error = Command::new(executable)
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::inherit())
                        .exec();
                    active_lease.restore(&mut system).map_err(|restore_error| {
                        format!(
                            "{label} execution failed ({error}); stock restoration also failed: {restore_error:?}"
                        )
                    })?;
                    return Err(format!("{label} execution failed: {error}").into());
                }
            }
        }
    }
}

#[cfg(not(all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")))]
fn run_on_target(_arguments: Arguments) -> Result<(), Box<dyn std::error::Error>> {
    Err("shell guardian requires 32-bit ARM Linux".into())
}

fn parse_arguments(mut arguments: impl Iterator<Item = OsString>) -> Result<ParseResult, String> {
    let mut profile = None;
    let mut report = None;
    let mut shell = None;
    let mut application_manifests = Vec::new();
    let mut crash_state = None;
    let mut startup_mode = None;
    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--profile") => set_path(&mut profile, "--profile", &mut arguments)?,
            Some("--report") => set_path(&mut report, "--report", &mut arguments)?,
            Some("--shell") => set_path(&mut shell, "--shell", &mut arguments)?,
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
            Some("--crash-state") => set_path(&mut crash_state, "--crash-state", &mut arguments)?,
            Some("--startup-mode") => {
                if startup_mode.is_some() {
                    return Err("--startup-mode may be supplied only once".to_owned());
                }
                startup_mode = Some(
                    match arguments.next().and_then(|value| value.into_string().ok()) {
                        Some(value) if value == "stock-running" => StartupMode::StockRunning,
                        Some(value) if value == "stock-starting" => StartupMode::StockStarting,
                        Some(value) => return Err(format!("unknown startup mode: {value}")),
                        None => return Err("--startup-mode requires a value".to_owned()),
                    },
                );
            }
            Some("-h" | "--help") => return Ok(ParseResult::Help),
            Some(other) => return Err(format!("unknown argument: {other}")),
            None => return Err("arguments must be valid Unicode".to_owned()),
        }
    }
    Ok(ParseResult::Run(Arguments {
        profile: profile.ok_or_else(|| "--profile is required".to_owned())?,
        report: report.ok_or_else(|| "--report is required".to_owned())?,
        shell: shell.ok_or_else(|| "--shell is required".to_owned())?,
        application_manifests,
        crash_state,
        startup_mode: startup_mode.unwrap_or(StartupMode::StockRunning),
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
        "ferrink-shell-guardian-kindle {}\n\n\
         Usage:\n  ferrink-shell-guardian-kindle --profile FILE --report FILE \\\n         --shell FILE --application-manifest FILE... [--crash-state FILE] \\\n         [--startup-mode stock-running|stock-starting]\n\n\
         Runs the Ferrink shell under a reviewed foreground lease. A\n\
         registered framebuffer application is launched only after restoring\n\
         stock ownership; Ferrink acquires a new lease when the application\n\
         exits. Failures and deferred HUP/INT/TERM restore stock. An optional\n\
         rootfs crash-state file records unproven boot starts atomically and\n\
         fails closed before userstore access when its budget is exhausted.\n\
         Stock-starting mode requires disabled Pillow, an early Awesome, and\n\
         absent CVM; it owns Awesome plus the sleep inhibitor until restore.\n\
         Boot is never modified.",
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
    fn exact_paths_are_required_once() {
        let parsed = parse_arguments(args(&[
            "--profile",
            "/mnt/us/profile.toml",
            "--report",
            "/mnt/us/report.json",
            "--shell",
            "/mnt/us/ferrink-shell-kindle",
            "--application-manifest",
            "/mnt/us/org.koreader.reader.toml",
        ]))
        .unwrap();
        assert!(matches!(parsed, ParseResult::Run(_)));
        let parsed = parse_arguments(args(&[
            "--profile",
            "/mnt/us/profile.toml",
            "--report",
            "/mnt/us/report.json",
            "--shell",
            "/mnt/us/ferrink-shell-kindle",
            "--application-manifest",
            "/mnt/us/org.koreader.reader.toml",
            "--startup-mode",
            "stock-starting",
        ]))
        .unwrap();
        assert!(matches!(
            parsed,
            ParseResult::Run(Arguments {
                startup_mode: StartupMode::StockStarting,
                ..
            })
        ));
        let parsed_with_crash_state = parse_arguments(args(&[
            "--profile",
            "/mnt/us/profile.toml",
            "--report",
            "/mnt/us/report.json",
            "--shell",
            "/mnt/us/ferrink-shell-kindle",
            "--application-manifest",
            "/mnt/us/org.koreader.reader.toml",
            "--crash-state",
            "/var/local/ferrink/crash-state-v1",
        ]))
        .unwrap();
        assert!(matches!(
            parsed_with_crash_state,
            ParseResult::Run(Arguments {
                crash_state: Some(_),
                ..
            })
        ));
        assert!(parse_arguments(args(&[])).is_err());
        assert!(parse_arguments(args(&["--shell", "one", "--shell", "two"])).is_err());
        let multiple = parse_arguments(args(&[
            "--profile",
            "/mnt/us/profile.toml",
            "--report",
            "/mnt/us/report.json",
            "--shell",
            "/mnt/us/ferrink-shell-kindle",
            "--application-manifest",
            "/mnt/us/io.home-assistant.dashboard.toml",
            "--application-manifest",
            "/mnt/us/org.koreader.reader.toml",
        ]))
        .unwrap();
        assert!(matches!(
            multiple,
            ParseResult::Run(Arguments {
                application_manifests,
                ..
            }) if application_manifests.len() == 2
        ));
        assert!(parse_arguments(args(&["--crash-state", "one", "--crash-state", "two",])).is_err());
        assert!(parse_arguments(args(&["--startup-mode", "unknown"])).is_err());
        assert!(
            parse_arguments(args(&[
                "--startup-mode",
                "stock-running",
                "--startup-mode",
                "stock-starting",
            ]))
            .is_err()
        );
    }

    #[test]
    fn authority_and_substitution_overrides_do_not_exist() {
        for flag in [
            "--device",
            "--process",
            "--duration",
            "--retry",
            "--force",
            "--install",
            "--boot",
            "--reboot",
            "--power-off",
        ] {
            assert!(parse_arguments(args(&[flag])).is_err(), "accepted {flag}");
        }
    }

    #[test]
    fn only_clean_child_exit_is_success() {
        assert!(GuardianDisposition::CleanExit.is_success());
        for disposition in [
            GuardianDisposition::NonzeroExit,
            GuardianDisposition::SpawnFailed,
            GuardianDisposition::ReadinessFailed,
            GuardianDisposition::ReadinessTimedOut,
            GuardianDisposition::SignalRequested,
            GuardianDisposition::ForcedTermination,
            GuardianDisposition::ChildIoFailed,
            GuardianDisposition::CommandMissing,
            GuardianDisposition::CommandInvalid,
            GuardianDisposition::CrashStateFailed,
        ] {
            assert!(!disposition.is_success());
        }
    }

    #[test]
    fn shell_protocol_rejects_ambiguous_or_out_of_range_requests() {
        assert_eq!(decode_shell_request(b"S"), Ok(ShellRequest::ReturnToStock));
        assert_eq!(decode_shell_request(b"R"), Ok(ShellRequest::Reboot));
        assert_eq!(decode_shell_request(b"P"), Ok(ShellRequest::PowerOff));
        assert_eq!(
            decode_shell_request(&[b'A', 37]),
            Ok(ShellRequest::LaunchApplication(37))
        );
        assert_eq!(
            decode_shell_request(&[]),
            Err(GuardianDisposition::CommandMissing)
        );
        for bytes in [
            b"A".as_slice(),
            &[b'A', 64][..],
            &[b'S', 0][..],
            &[b'R', 0][..],
            &[b'P', 0][..],
            &[b'A', 1, 2][..],
        ] {
            assert_eq!(
                decode_shell_request(bytes),
                Err(GuardianDisposition::CommandInvalid)
            );
        }
    }
}
