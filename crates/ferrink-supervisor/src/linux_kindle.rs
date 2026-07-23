//! Opt-in Linux boundary for the KOA3 foreground ownership transaction.

use std::fs::OpenOptions;
use std::io::Read;
use std::num::{NonZeroI32, NonZeroU32};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

use ferrink_platform::ResolvedRuntimeDevice;
use ferrink_platform_kindle::{LinuxStockRepaintProcess, StockRepaintCore, StockRepaintError};

use crate::{
    FOREGROUND_QUIESCENCE, ForegroundSystem, PillowState, ProcessIdentity, ProcessTransition,
    StockProcess, parse_proc_stat_start_time,
};

const LIPC_GET: &str = "/usr/bin/lipc-get-prop";
const LIPC_SET: &str = "/usr/bin/lipc-set-prop";
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const CHILD_POLL_INTERVAL: Duration = Duration::from_millis(25);
const MAX_PROPERTY_OUTPUT: u64 = 64;
const MAX_PROC_COMM: u64 = 64;
const MAX_PROC_STAT: u64 = 4_096;
const MAX_PROC_STATUS: u64 = 8_192;
const CRITICAL_SIGNALS: [libc::c_int; 3] = [libc::SIGHUP, libc::SIGINT, libc::SIGTERM];

static DEFERRED_SIGNAL: AtomicI32 = AtomicI32::new(0);

extern "C" fn defer_critical_signal(signal: libc::c_int) {
    let _ = DEFERRED_SIGNAL.compare_exchange(0, signal, Ordering::SeqCst, Ordering::SeqCst);
}

/// Defers termination signals until foreground restoration completes.
#[derive(Debug)]
pub struct LinuxForegroundSignalGuard {
    previous: [(libc::c_int, libc::sighandler_t); CRITICAL_SIGNALS.len()],
    active: bool,
}

impl LinuxForegroundSignalGuard {
    /// Installs the bounded HUP/INT/TERM deferral boundary.
    ///
    /// # Errors
    ///
    /// Restores every handler installed before a partial failure.
    #[allow(unsafe_code)]
    pub fn install() -> Result<Self, KindleForegroundError> {
        DEFERRED_SIGNAL.store(0, Ordering::SeqCst);
        let mut previous = [(0, libc::SIG_DFL); CRITICAL_SIGNALS.len()];
        for (index, signal) in CRITICAL_SIGNALS.into_iter().enumerate() {
            // SAFETY: the handler has the C signal ABI and performs only one
            // lock-free atomic compare-exchange.
            let handler = defer_critical_signal as *const () as libc::sighandler_t;
            let old = unsafe { libc::signal(signal, handler) };
            if old == libc::SIG_ERR {
                for (installed_signal, installed_handler) in previous[..index].iter().rev() {
                    // SAFETY: each pair came from a successful installation.
                    let _ = unsafe { libc::signal(*installed_signal, *installed_handler) };
                }
                return Err(io_error(
                    KindleForegroundOperation::InstallSignalHandlers,
                    &std::io::Error::last_os_error(),
                ));
            }
            previous[index] = (signal, old);
        }
        Ok(Self {
            previous,
            active: true,
        })
    }

    /// Returns the first deferred signal without clearing it.
    #[must_use]
    pub fn pending_signal(&self) -> Option<NonZeroI32> {
        NonZeroI32::new(DEFERRED_SIGNAL.load(Ordering::SeqCst))
    }

    /// Restores the previous handlers exactly once and returns any signal.
    ///
    /// # Errors
    ///
    /// Returns a bounded restoration failure after attempting every handler.
    #[allow(unsafe_code)]
    pub fn finish(mut self) -> Result<Option<NonZeroI32>, KindleForegroundError> {
        self.active = false;
        let mut failed = false;
        for (signal, handler) in self.previous.iter().rev() {
            // SAFETY: each handler was returned by its matching installation.
            if unsafe { libc::signal(*signal, *handler) } == libc::SIG_ERR {
                failed = true;
            }
        }
        if failed {
            return Err(io_error(
                KindleForegroundOperation::RestoreSignalHandlers,
                &std::io::Error::last_os_error(),
            ));
        }
        Ok(NonZeroI32::new(DEFERRED_SIGNAL.swap(0, Ordering::SeqCst)))
    }
}

impl Drop for LinuxForegroundSignalGuard {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        for (signal, handler) in self.previous.iter().rev() {
            // SAFETY: best-effort restoration of each captured handler.
            let _ = unsafe { libc::signal(*signal, *handler) };
        }
    }
}

/// Opt-in KOA3 adapter for exact process, LIPC, and stock-repaint operations.
///
/// Pillow exposes no reliable readable enabled/disabled property on the
/// reviewed firmware. The constructor therefore requires a separately reviewed
/// preflight baseline and returns that exact baseline to the policy core.
#[derive(Debug)]
pub struct LinuxKindleForegroundSystem {
    confirmed_pillow: PillowState,
    repaint: StockRepaintCore,
}

impl LinuxKindleForegroundSystem {
    /// Creates the adapter for one exact resolved device and confirmed Pillow
    /// baseline. This performs no mutation.
    ///
    /// # Errors
    ///
    /// Returns if the reviewed stock repaint is unavailable.
    pub fn try_new(
        device: &ResolvedRuntimeDevice,
        confirmed_pillow: PillowState,
    ) -> Result<Self, KindleForegroundError> {
        let repaint = StockRepaintCore::try_from_runtime(device)
            .map_err(KindleForegroundError::StockRepaint)?;
        Ok(Self {
            confirmed_pillow,
            repaint,
        })
    }
}

impl ForegroundSystem for LinuxKindleForegroundSystem {
    type Error = KindleForegroundError;

    fn prevent_screen_saver(&mut self) -> Result<bool, Self::Error> {
        let output = run_command(
            LIPC_GET,
            &["com.lab126.powerd", "preventScreenSaver"],
            KindleCommand::GetPreventScreenSaver,
            true,
        )?;
        match trim_ascii(&output) {
            b"0" => Ok(false),
            b"1" => Ok(true),
            _ => Err(KindleForegroundError::InvalidCommandOutput(
                KindleCommand::GetPreventScreenSaver,
            )),
        }
    }

    fn pillow_state(&mut self) -> Result<PillowState, Self::Error> {
        Ok(self.confirmed_pillow)
    }

    fn running_processes(
        &mut self,
        process: StockProcess,
    ) -> Result<Vec<ProcessIdentity>, Self::Error> {
        let mut identities = Vec::new();
        for entry in std::fs::read_dir("/proc")
            .map_err(|error| io_error(KindleForegroundOperation::ReadProcessDirectory, &error))?
        {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    return Err(io_error(
                        KindleForegroundOperation::ReadProcessDirectory,
                        &error,
                    ));
                }
            };
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<u32>().ok())
                .and_then(NonZeroU32::new)
            else {
                continue;
            };
            let proc_dir = entry.path();
            let command = match read_bounded_utf8(
                &proc_dir.join("comm"),
                MAX_PROC_COMM,
                KindleForegroundOperation::ReadProcessIdentity,
            ) {
                Ok(command) => command,
                Err(error) if error.is_not_found() => continue,
                Err(error) => return Err(error),
            };
            if command.trim_end_matches(['\r', '\n']) != process_name(process) {
                continue;
            }
            let identity = read_process_identity(proc_dir.as_path(), process, pid)?;
            let state = read_process_state(proc_dir.as_path())?;
            if matches!(state, b'T' | b't' | b'Z' | b'X' | b'x') {
                return Err(KindleForegroundError::UnexpectedProcessState { identity, state });
            }
            identities.push(identity);
        }
        identities.sort_unstable_by_key(|identity| identity.pid());
        Ok(identities)
    }

    fn set_prevent_screen_saver(&mut self, prevent: bool) -> Result<(), Self::Error> {
        let value = if prevent { "1" } else { "0" };
        run_command(
            LIPC_SET,
            &["com.lab126.powerd", "preventScreenSaver", value],
            KindleCommand::SetPreventScreenSaver(prevent),
            false,
        )?;
        Ok(())
    }

    fn set_pillow(&mut self, state: PillowState) -> Result<(), Self::Error> {
        let value = match state {
            PillowState::Enabled => "enable",
            PillowState::Disabled => "disable",
        };
        run_command(
            LIPC_SET,
            &["com.lab126.pillow", "disableEnablePillow", value],
            KindleCommand::SetPillow(state),
            false,
        )?;
        Ok(())
    }

    #[allow(unsafe_code)]
    fn transition_process(
        &mut self,
        identity: ProcessIdentity,
        transition: ProcessTransition,
    ) -> Result<(), Self::Error> {
        revalidate_process_identity(identity)?;
        let signal = match transition {
            ProcessTransition::Stop => libc::SIGSTOP,
            ProcessTransition::Continue => libc::SIGCONT,
        };
        let pid = i32::try_from(identity.pid().get())
            .map_err(|_| KindleForegroundError::ProcessIdentityChanged(identity))?;
        // SAFETY: `pid` was captured from `/proc`, revalidated by command name
        // and start time immediately above, and `signal` is SIGSTOP or SIGCONT.
        if unsafe { libc::kill(pid, signal) } < 0 {
            return Err(io_error(
                KindleForegroundOperation::SignalProcess(identity, transition),
                &std::io::Error::last_os_error(),
            ));
        }
        Ok(())
    }

    fn quiesce(&mut self, duration: Duration) -> Result<(), Self::Error> {
        if duration != FOREGROUND_QUIESCENCE {
            return Err(KindleForegroundError::InvalidQuiescence(duration));
        }
        std::thread::sleep(duration);
        Ok(())
    }

    fn repaint_stock(&mut self) -> Result<(), Self::Error> {
        self.repaint
            .repaint(&mut LinuxStockRepaintProcess)
            .map_err(KindleForegroundError::StockRepaint)
    }
}

fn process_name(process: StockProcess) -> &'static str {
    match process {
        StockProcess::Awesome => "awesome",
        StockProcess::Cvm => "cvm",
    }
}

fn read_process_identity(
    proc_dir: &Path,
    process: StockProcess,
    pid: NonZeroU32,
) -> Result<ProcessIdentity, KindleForegroundError> {
    let stat = read_bounded_utf8(
        &proc_dir.join("stat"),
        MAX_PROC_STAT,
        KindleForegroundOperation::ReadProcessIdentity,
    )?;
    let start_time_ticks = parse_proc_stat_start_time(&stat)
        .ok_or(KindleForegroundError::InvalidProcessMetadata { process, pid })?;
    Ok(ProcessIdentity::new(process, pid, start_time_ticks))
}

fn read_process_state(proc_dir: &Path) -> Result<u8, KindleForegroundError> {
    let status = read_bounded_utf8(
        &proc_dir.join("status"),
        MAX_PROC_STATUS,
        KindleForegroundOperation::ReadProcessIdentity,
    )?;
    status
        .lines()
        .find_map(|line| line.strip_prefix("State:").map(str::trim_start))
        .and_then(|state| state.as_bytes().first().copied())
        .ok_or_else(|| KindleForegroundError::InvalidProcessStatus(proc_dir.to_path_buf()))
}

fn revalidate_process_identity(identity: ProcessIdentity) -> Result<(), KindleForegroundError> {
    let proc_dir = PathBuf::from("/proc").join(identity.pid().get().to_string());
    let command = read_bounded_utf8(
        &proc_dir.join("comm"),
        MAX_PROC_COMM,
        KindleForegroundOperation::ReadProcessIdentity,
    )?;
    if command.trim_end_matches(['\r', '\n']) != process_name(identity.process()) {
        return Err(KindleForegroundError::ProcessIdentityChanged(identity));
    }
    let observed = read_process_identity(proc_dir.as_path(), identity.process(), identity.pid())?;
    if observed != identity {
        return Err(KindleForegroundError::ProcessIdentityChanged(identity));
    }
    Ok(())
}

fn run_command(
    executable: &str,
    arguments: &[&str],
    command: KindleCommand,
    capture_stdout: bool,
) -> Result<Vec<u8>, KindleForegroundError> {
    verify_executable(executable)?;
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
        .map_err(|error| io_error(KindleForegroundOperation::SpawnCommand(command), &error))?;
    let mut child = ChildGuard::new(child, command);
    let mut stdout = if capture_stdout {
        child.take_stdout()?
    } else {
        None
    };
    let status = child.wait_bounded(COMMAND_TIMEOUT)?;
    if !status.success() {
        return Err(KindleForegroundError::CommandFailed {
            command,
            code: status.code(),
        });
    }
    let Some(stdout) = stdout.as_mut() else {
        return Ok(Vec::new());
    };
    let mut output = Vec::new();
    stdout
        .take(MAX_PROPERTY_OUTPUT + 1)
        .read_to_end(&mut output)
        .map_err(|error| {
            io_error(
                KindleForegroundOperation::ReadCommandOutput(command),
                &error,
            )
        })?;
    if output.len() as u64 > MAX_PROPERTY_OUTPUT {
        return Err(KindleForegroundError::InvalidCommandOutput(command));
    }
    Ok(output)
}

fn verify_executable(executable: &str) -> Result<(), KindleForegroundError> {
    let path = Path::new(executable);
    let metadata = path
        .symlink_metadata()
        .map_err(|error| io_error(KindleForegroundOperation::VerifyExecutable, &error))?;
    if !metadata.file_type().is_file() || metadata.permissions().mode() & 0o111 == 0 {
        return Err(KindleForegroundError::InvalidExecutable);
    }
    if path
        .canonicalize()
        .map_err(|error| io_error(KindleForegroundOperation::VerifyExecutable, &error))?
        != path
    {
        return Err(KindleForegroundError::InvalidExecutable);
    }
    Ok(())
}

fn read_bounded_utf8(
    path: &Path,
    maximum: u64,
    operation: KindleForegroundOperation,
) -> Result<String, KindleForegroundError> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| io_error(operation, &error))?;
    let mut input = String::new();
    file.take(maximum + 1)
        .read_to_string(&mut input)
        .map_err(|error| io_error(operation, &error))?;
    if input.len() as u64 > maximum {
        return Err(KindleForegroundError::InputTooLarge(operation));
    }
    Ok(input)
}

fn trim_ascii(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map_or(start, |index| index + 1);
    &bytes[start..end]
}

#[derive(Debug)]
struct ChildGuard {
    child: Option<Child>,
    command: KindleCommand,
}

impl ChildGuard {
    fn new(child: Child, command: KindleCommand) -> Self {
        Self {
            child: Some(child),
            command,
        }
    }

    fn take_stdout(&mut self) -> Result<Option<std::process::ChildStdout>, KindleForegroundError> {
        self.child
            .as_mut()
            .ok_or(KindleForegroundError::MissingChild(self.command))
            .map(|child| child.stdout.take())
    }

    fn wait_bounded(&mut self, timeout: Duration) -> Result<ExitStatus, KindleForegroundError> {
        let started = Instant::now();
        loop {
            let child = self
                .child
                .as_mut()
                .ok_or(KindleForegroundError::MissingChild(self.command))?;
            if let Some(status) = child.try_wait().map_err(|error| {
                io_error(KindleForegroundOperation::WaitCommand(self.command), &error)
            })? {
                self.child = None;
                return Ok(status);
            }
            let elapsed = started.elapsed();
            if elapsed >= timeout {
                self.terminate_and_reap()?;
                return Err(KindleForegroundError::CommandTimedOut(self.command));
            }
            std::thread::sleep(CHILD_POLL_INTERVAL.min(timeout - elapsed));
        }
    }

    fn terminate_and_reap(&mut self) -> Result<(), KindleForegroundError> {
        let mut child = self
            .child
            .take()
            .ok_or(KindleForegroundError::MissingChild(self.command))?;
        child.kill().map_err(|error| {
            io_error(
                KindleForegroundOperation::TerminateCommand(self.command),
                &error,
            )
        })?;
        child.wait().map_err(|error| {
            io_error(KindleForegroundOperation::WaitCommand(self.command), &error)
        })?;
        Ok(())
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Exact no-shell LIPC child operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KindleCommand {
    /// Read `preventScreenSaver`.
    GetPreventScreenSaver,
    /// Write `preventScreenSaver`.
    SetPreventScreenSaver(bool),
    /// Write the Pillow command property.
    SetPillow(PillowState),
}

/// Bounded Linux operation associated with a system error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KindleForegroundOperation {
    /// Install the termination-signal deferral boundary.
    InstallSignalHandlers,
    /// Restore the previous signal handlers.
    RestoreSignalHandlers,
    /// Enumerate `/proc`.
    ReadProcessDirectory,
    /// Read and parse one process identity.
    ReadProcessIdentity,
    /// Validate an exact command path.
    VerifyExecutable,
    /// Spawn one exact LIPC command.
    SpawnCommand(KindleCommand),
    /// Wait for one exact LIPC command.
    WaitCommand(KindleCommand),
    /// Terminate one timed-out LIPC command.
    TerminateCommand(KindleCommand),
    /// Read bounded command output.
    ReadCommandOutput(KindleCommand),
    /// Signal one revalidated process identity.
    SignalProcess(ProcessIdentity, ProcessTransition),
}

/// Bounded, redacted KOA3 foreground adapter failure.
#[derive(Debug)]
#[non_exhaustive]
pub enum KindleForegroundError {
    /// A bounded system operation failed.
    Io {
        /// Failed operation.
        operation: KindleForegroundOperation,
        /// Positive errno when available.
        errno: Option<NonZeroI32>,
    },
    /// A procfs input exceeded its strict bound.
    InputTooLarge(KindleForegroundOperation),
    /// Process stat data could not produce a stable identity.
    InvalidProcessMetadata {
        /// Expected process kind.
        process: StockProcess,
        /// Captured PID.
        pid: NonZeroU32,
    },
    /// Process status data did not contain a state byte.
    InvalidProcessStatus(PathBuf),
    /// A required process was already stopped, dead, or a zombie at preflight.
    UnexpectedProcessState {
        /// Captured identity.
        identity: ProcessIdentity,
        /// Linux state byte.
        state: u8,
    },
    /// Command name or start time changed before the signal.
    ProcessIdentityChanged(ProcessIdentity),
    /// An exact executable was absent, non-executable, or redirected.
    InvalidExecutable,
    /// A child exited unsuccessfully.
    CommandFailed {
        /// Exact command kind.
        command: KindleCommand,
        /// Exit status code when available.
        code: Option<i32>,
    },
    /// A child exceeded its five-second deadline and was reaped.
    CommandTimedOut(KindleCommand),
    /// Bounded command output was absent or invalid.
    InvalidCommandOutput(KindleCommand),
    /// Internal child ownership was unexpectedly absent.
    MissingChild(KindleCommand),
    /// The policy requested a different quiescence interval.
    InvalidQuiescence(Duration),
    /// The promoted stock repaint failed.
    StockRepaint(StockRepaintError),
}

impl KindleForegroundError {
    fn is_not_found(&self) -> bool {
        matches!(
            self,
            Self::Io {
                errno: Some(errno),
                ..
            } if errno.get() == libc::ENOENT
        )
    }
}

impl std::fmt::Display for KindleForegroundError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { operation, errno } => {
                write!(
                    formatter,
                    "Kindle foreground {operation:?} failed{}",
                    ErrnoSuffix(*errno)
                )
            }
            Self::InputTooLarge(operation) => {
                write!(
                    formatter,
                    "Kindle foreground {operation:?} input exceeded its bound"
                )
            }
            Self::InvalidProcessMetadata { process, pid } => {
                write!(
                    formatter,
                    "invalid {process:?} process metadata for PID {pid}"
                )
            }
            Self::InvalidProcessStatus(path) => {
                write!(formatter, "invalid process status at {}", path.display())
            }
            Self::UnexpectedProcessState { identity, state } => write!(
                formatter,
                "process {:?} PID {} had disallowed state {:?}",
                identity.process(),
                identity.pid(),
                char::from(*state)
            ),
            Self::ProcessIdentityChanged(identity) => write!(
                formatter,
                "process {:?} PID {} changed identity",
                identity.process(),
                identity.pid()
            ),
            Self::InvalidExecutable => formatter.write_str("exact LIPC executable is invalid"),
            Self::CommandFailed { command, code } => {
                write!(
                    formatter,
                    "exact {command:?} command failed with status {code:?}"
                )
            }
            Self::CommandTimedOut(command) => {
                write!(formatter, "exact {command:?} command timed out")
            }
            Self::InvalidCommandOutput(command) => {
                write!(
                    formatter,
                    "exact {command:?} command returned invalid output"
                )
            }
            Self::MissingChild(command) => {
                write!(formatter, "exact {command:?} child ownership was missing")
            }
            Self::InvalidQuiescence(duration) => {
                write!(formatter, "invalid foreground quiescence {duration:?}")
            }
            Self::StockRepaint(error) => write!(formatter, "stock repaint failed: {error}"),
        }
    }
}

impl std::error::Error for KindleForegroundError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::StockRepaint(error) => Some(error),
            _ => None,
        }
    }
}

fn io_error(operation: KindleForegroundOperation, error: &std::io::Error) -> KindleForegroundError {
    KindleForegroundError::Io {
        operation,
        errno: error
            .raw_os_error()
            .filter(|errno| *errno > 0)
            .and_then(NonZeroI32::new),
    }
}

struct ErrnoSuffix(Option<NonZeroI32>);

impl std::fmt::Display for ErrnoSuffix {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            Some(errno) => write!(formatter, " with errno {}", errno.get()),
            None => Ok(()),
        }
    }
}
