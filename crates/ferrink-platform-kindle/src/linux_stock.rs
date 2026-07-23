//! Linux process boundary for the exact reviewed stock repaint command.

use std::num::NonZeroI32;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use crate::{
    StockRepaintCommand, StockRepaintIoError, StockRepaintOperation, StockRepaintOutcome,
    StockRepaintProcess,
};

const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Linux runner for `/usr/bin/xrefresh -d :0.0` with no shell or inherited
/// environment.
#[derive(Debug, Default)]
pub struct LinuxStockRepaintProcess;

impl StockRepaintProcess for LinuxStockRepaintProcess {
    fn run_exact(
        &mut self,
        command: StockRepaintCommand,
    ) -> Result<StockRepaintOutcome, StockRepaintIoError> {
        verify_command(command)?;
        verify_executable(command.executable())?;

        let child = Command::new(command.executable())
            .args(command.arguments())
            .env_clear()
            .current_dir("/")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| process_error(StockRepaintOperation::Spawn, &error))?;
        let mut child = ChildGuard::new(child);
        let started = Instant::now();

        loop {
            if let Some(status) = child.try_wait()? {
                return Ok(outcome_from_status(status));
            }
            let elapsed = started.elapsed();
            if elapsed >= command.timeout() {
                child.terminate_and_reap()?;
                return Ok(StockRepaintOutcome::TimedOut);
            }
            std::thread::sleep(POLL_INTERVAL.min(command.timeout() - elapsed));
        }
    }
}

fn verify_command(command: StockRepaintCommand) -> Result<(), StockRepaintIoError> {
    if command.executable() != "/usr/bin/xrefresh"
        || command.arguments() != ["-d", ":0.0"]
        || command.timeout() != Duration::from_secs(5)
    {
        return Err(StockRepaintIoError::new(
            StockRepaintOperation::Verify,
            None,
        ));
    }
    Ok(())
}

fn verify_executable(executable: &str) -> Result<(), StockRepaintIoError> {
    let path = Path::new(executable);
    let metadata = path
        .symlink_metadata()
        .map_err(|error| process_error(StockRepaintOperation::Verify, &error))?;
    if !metadata.file_type().is_file() || metadata.permissions().mode() & 0o111 == 0 {
        return Err(StockRepaintIoError::new(
            StockRepaintOperation::Verify,
            None,
        ));
    }
    let canonical = path
        .canonicalize()
        .map_err(|error| process_error(StockRepaintOperation::Verify, &error))?;
    if canonical != path {
        return Err(StockRepaintIoError::new(
            StockRepaintOperation::Verify,
            None,
        ));
    }
    Ok(())
}

fn outcome_from_status(status: ExitStatus) -> StockRepaintOutcome {
    if status.success() {
        StockRepaintOutcome::Succeeded
    } else {
        StockRepaintOutcome::ExitedFailure {
            code: status.code(),
        }
    }
}

#[derive(Debug)]
struct ChildGuard {
    child: Option<Child>,
}

impl ChildGuard {
    fn new(child: Child) -> Self {
        Self { child: Some(child) }
    }

    fn try_wait(&mut self) -> Result<Option<ExitStatus>, StockRepaintIoError> {
        let child = self
            .child
            .as_mut()
            .ok_or_else(|| StockRepaintIoError::new(StockRepaintOperation::Wait, None))?;
        match child
            .try_wait()
            .map_err(|error| process_error(StockRepaintOperation::Wait, &error))?
        {
            Some(status) => {
                self.child = None;
                Ok(Some(status))
            }
            None => Ok(None),
        }
    }

    fn terminate_and_reap(&mut self) -> Result<(), StockRepaintIoError> {
        let mut child = self
            .child
            .take()
            .ok_or_else(|| StockRepaintIoError::new(StockRepaintOperation::Terminate, None))?;
        child
            .kill()
            .map_err(|error| process_error(StockRepaintOperation::Terminate, &error))?;
        child
            .wait()
            .map_err(|error| process_error(StockRepaintOperation::Wait, &error))?;
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

fn process_error(operation: StockRepaintOperation, error: &std::io::Error) -> StockRepaintIoError {
    StockRepaintIoError::new(
        operation,
        error
            .raw_os_error()
            .filter(|errno| *errno > 0)
            .and_then(NonZeroI32::new),
    )
}
