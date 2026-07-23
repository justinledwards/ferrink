//! ARM Linux mmap and Zelda boundary for the exact KOA3 pixel card.

use std::fs::{File, OpenOptions};
use std::num::{NonZeroI32, NonZeroU16};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Duration;

use ferrink_platform::{
    DisplayUpdateAbiKind, RefreshCompletionPolicy, RefreshError, RefreshMode, RefreshRequest,
    ResolvedRuntimeDevice, UpdateMarker,
};

use crate::{
    DisplayTarget, Koa3PixelCard, PixelCardError, PixelCardIoError, PixelCardTarget,
    ReadOnlyIoError,
};

use super::linux::query_framebuffer_capability;
use super::zelda::{MXCFB_SEND_UPDATE_ZELDA, ZeldaUpdateRequest};

const EXACT_MARKER: u32 = 0x464b_0002;
const EXACT_X: u32 = 600;
const EXACT_Y: u32 = 808;
const EXACT_WIDTH: u32 = 64;
const EXACT_HEIGHT: u32 = 64;
const CRITICAL_SIGNALS: [libc::c_int; 3] = [libc::SIGHUP, libc::SIGINT, libc::SIGTERM];

static DEFERRED_SIGNAL: AtomicI32 = AtomicI32::new(0);

extern "C" fn defer_critical_signal(signal: libc::c_int) {
    let _ = DEFERRED_SIGNAL.compare_exchange(0, signal, Ordering::SeqCst, Ordering::SeqCst);
}

/// Temporary signal boundary that lets the exact mapped bytes restore before
/// an operator-requested abort is reported.
#[derive(Debug)]
pub struct LinuxPixelSignalGuard {
    previous: [(libc::c_int, libc::sighandler_t); CRITICAL_SIGNALS.len()],
    active: bool,
}

impl LinuxPixelSignalGuard {
    /// Defers `SIGHUP`, `SIGINT`, and `SIGTERM` into one bounded status flag.
    ///
    /// # Errors
    ///
    /// Returns a signal-installation error after restoring every handler that
    /// was changed before the failure. No framebuffer has been touched yet.
    pub fn install() -> Result<Self, LinuxPixelTargetError> {
        DEFERRED_SIGNAL.store(0, Ordering::SeqCst);
        let mut previous = [(0, libc::SIG_DFL); CRITICAL_SIGNALS.len()];
        for (index, signal) in CRITICAL_SIGNALS.into_iter().enumerate() {
            // SAFETY: `defer_critical_signal` has the required C signal-handler
            // ABI and performs only one lock-free atomic compare-exchange. The
            // returned previous handler is retained for exact restoration.
            let handler = defer_critical_signal as *const () as libc::sighandler_t;
            let old = unsafe { libc::signal(signal, handler) };
            if old == libc::SIG_ERR {
                for (installed_signal, installed_handler) in previous[..index].iter().rev() {
                    // SAFETY: each pair was returned by the successful signal
                    // installation above and is restored at most once here.
                    let _ = unsafe { libc::signal(*installed_signal, *installed_handler) };
                }
                return Err(last_target_io_error(LinuxPixelOperation::SignalInstall));
            }
            previous[index] = (signal, old);
        }
        Ok(Self {
            previous,
            active: true,
        })
    }

    /// Restores all three previous handlers exactly once and returns the first
    /// deferred signal, if any.
    ///
    /// # Errors
    ///
    /// Returns a bounded restoration error without attempting another handler
    /// replacement from [`Drop`].
    pub fn finish(mut self) -> Result<Option<NonZeroI32>, LinuxPixelTargetError> {
        self.active = false;
        restore_signal_handlers(&self.previous)?;
        Ok(NonZeroI32::new(DEFERRED_SIGNAL.swap(0, Ordering::SeqCst)))
    }
}

impl Drop for LinuxPixelSignalGuard {
    fn drop(&mut self) {
        if self.active {
            self.active = false;
            let _ = restore_signal_handlers(&self.previous);
        }
    }
}

fn restore_signal_handlers(
    previous: &[(libc::c_int, libc::sighandler_t); CRITICAL_SIGNALS.len()],
) -> Result<(), LinuxPixelTargetError> {
    let mut failed = false;
    for (signal, handler) in previous.iter().rev() {
        // SAFETY: every handler was returned by the matching successful signal
        // installation and this function is called at most once per guard path.
        if unsafe { libc::signal(*signal, *handler) } == libc::SIG_ERR {
            failed = true;
        }
    }
    if failed {
        Err(last_target_io_error(LinuxPixelOperation::SignalRestore))
    } else {
        Ok(())
    }
}

/// Exact KOA3 framebuffer mapping owned for one pixel-card attempt.
#[derive(Debug)]
pub struct LinuxKoa3PixelTarget {
    mapping: Option<NonNull<u8>>,
    length: usize,
    file: File,
    submitted: bool,
}

impl LinuxKoa3PixelTarget {
    /// Opens, re-queries, and maps only the exact resolved KOA3 framebuffer.
    ///
    /// # Errors
    ///
    /// Returns [`LinuxPixelTargetError`] before mapping when profile/card
    /// prerequisites, path type, open mode, or live metadata differ. Mapping
    /// failure closes the descriptor without an update attempt.
    pub fn open(device: &ResolvedRuntimeDevice) -> Result<Self, LinuxPixelTargetError> {
        Koa3PixelCard::try_from_runtime(device).map_err(LinuxPixelTargetError::Prerequisite)?;
        let path = device.framebuffer_path();
        if path != "/dev/fb0" {
            return Err(LinuxPixelTargetError::InvalidPath);
        }
        let metadata = std::fs::symlink_metadata(path)
            .map_err(|error| target_io_error(LinuxPixelOperation::Inspect, &error))?;
        if !metadata.file_type().is_char_device() {
            return Err(LinuxPixelTargetError::NotCharacterDevice);
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(path)
            .map_err(|error| target_io_error(LinuxPixelOperation::Open, &error))?;
        if !file
            .metadata()
            .map_err(|error| target_io_error(LinuxPixelOperation::Inspect, &error))?
            .file_type()
            .is_char_device()
        {
            return Err(LinuxPixelTargetError::NotCharacterDevice);
        }
        let observed =
            query_framebuffer_capability(&file, path).map_err(LinuxPixelTargetError::Query)?;
        if &observed != device.framebuffer_capability() {
            return Err(LinuxPixelTargetError::CapabilityMismatch);
        }
        let length = usize::try_from(device.framebuffer_layout().memory_length())
            .map_err(|_| LinuxPixelTargetError::LengthUnrepresentable)?;
        // SAFETY: `file` is the exact revalidated framebuffer opened read/write,
        // `length` is its non-zero kernel-reported mapping length, offset zero is
        // page aligned, and the returned pointer is checked against MAP_FAILED.
        let raw = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                length,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        if raw == libc::MAP_FAILED {
            return Err(last_target_io_error(LinuxPixelOperation::Map));
        }
        let Some(mapping) = NonNull::new(raw.cast::<u8>()) else {
            // SAFETY: mmap succeeded for this exact pointer and length, but a
            // null address cannot form a Rust slice. Release it once before
            // returning the fail-closed representation error.
            let _ = unsafe { libc::munmap(raw, length) };
            return Err(LinuxPixelTargetError::NullMapping);
        };
        Ok(Self {
            mapping: Some(mapping),
            length,
            file,
            submitted: false,
        })
    }

    /// Unmaps the framebuffer exactly once and then closes its descriptor.
    ///
    /// # Errors
    ///
    /// Returns a bounded `munmap` error. It does not retry; process exit remains
    /// the final kernel cleanup boundary.
    pub fn close(mut self) -> Result<(), LinuxPixelTargetError> {
        self.unmap_once()
    }

    fn unmap_once(&mut self) -> Result<(), LinuxPixelTargetError> {
        let Some(mapping) = self.mapping.take() else {
            return Ok(());
        };
        // SAFETY: `mapping` is the live pointer returned by the one successful
        // mmap call above, `length` is unchanged, and taking the Option prevents
        // a second munmap attempt from this owner.
        if unsafe { libc::munmap(mapping.as_ptr().cast(), self.length) } < 0 {
            return Err(last_target_io_error(LinuxPixelOperation::Unmap));
        }
        Ok(())
    }
}

impl DisplayTarget for LinuxKoa3PixelTarget {
    fn framebuffer_memory(&mut self) -> &mut [u8] {
        let mapping = self
            .mapping
            .expect("BUG: framebuffer memory requested after explicit close");
        // SAFETY: the mapping is live for exactly `self.length` bytes and &mut
        // self prevents this owner from producing overlapping mutable slices.
        unsafe { std::slice::from_raw_parts_mut(mapping.as_ptr(), self.length) }
    }

    fn submit_refresh(
        &mut self,
        update_abi: DisplayUpdateAbiKind,
        request: RefreshRequest,
        marker: UpdateMarker,
    ) -> Result<(), RefreshError> {
        if self.submitted {
            return Err(RefreshError::SubmissionFailed {
                marker,
                errno: None,
            });
        }
        self.submitted = true;
        let region = request.region();
        if update_abi != DisplayUpdateAbiKind::Zelda88
            || region.x() != EXACT_X
            || region.y() != EXACT_Y
            || region.width() != EXACT_WIDTH
            || region.height() != EXACT_HEIGHT
            || request.mode() != RefreshMode::Partial
            || request.completion() != RefreshCompletionPolicy::DoNotWait
            || marker.get() != EXACT_MARKER
        {
            return Err(RefreshError::SubmissionFailed {
                marker,
                errno: None,
            });
        }
        let update = ZeldaUpdateRequest::encode(request, marker);
        // SAFETY: `self.file` remains the exact live KOA3 framebuffer, `update`
        // is a fully initialized 88-byte repr(C) Zelda request whose layout is
        // asserted below, and ioctl borrows it only for this synchronous call.
        if unsafe {
            libc::ioctl(
                self.file.as_raw_fd(),
                MXCFB_SEND_UPDATE_ZELDA as _,
                &update as *const ZeldaUpdateRequest,
            )
        } < 0
        {
            return Err(RefreshError::SubmissionFailed {
                marker,
                errno: positive_errno(&std::io::Error::last_os_error())
                    .and_then(|errno| u16::try_from(errno.get()).ok())
                    .and_then(NonZeroU16::new),
            });
        }
        Ok(())
    }
}

impl PixelCardTarget for LinuxKoa3PixelTarget {
    fn dwell(&mut self, duration: Duration) -> Result<(), PixelCardIoError> {
        if duration != Duration::from_secs(3) {
            return Err(PixelCardIoError::new(None));
        }
        std::thread::sleep(duration);
        Ok(())
    }
}

impl Drop for LinuxKoa3PixelTarget {
    fn drop(&mut self) {
        let _ = self.unmap_once();
    }
}

/// Exact Linux framebuffer stage that failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinuxPixelOperation {
    /// Critical-window signal handlers could not be installed.
    SignalInstall,
    /// Critical-window signal handlers could not be restored.
    SignalRestore,
    /// The resolved path could not be inspected.
    Inspect,
    /// The exact framebuffer could not be opened read/write.
    Open,
    /// The exact reported memory could not be mapped.
    Map,
    /// The owned mapping could not be released.
    Unmap,
}

/// Failure before, during, or after owning the exact framebuffer mapping.
#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LinuxPixelTargetError {
    /// The profile has not passed every prerequisite host/live gate.
    Prerequisite(PixelCardError),
    /// The resolved device was not the exact `/dev/fb0` card target.
    InvalidPath,
    /// The path or opened file was not a character device.
    NotCharacterDevice,
    /// A bounded operating-system operation failed.
    Io {
        operation: LinuxPixelOperation,
        errno: Option<NonZeroI32>,
    },
    /// Metadata ioctls failed after the read/write open.
    Query(ReadOnlyIoError),
    /// Metadata from the read/write descriptor differed from the fresh report.
    CapabilityMismatch,
    /// The kernel mapping length could not be represented by this process.
    LengthUnrepresentable,
    /// `mmap` unexpectedly returned a null pointer rather than MAP_FAILED.
    NullMapping,
}

impl std::fmt::Display for LinuxPixelTargetError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Prerequisite(error) => write!(formatter, "pixel prerequisite failed: {error}"),
            Self::InvalidPath => formatter.write_str("pixel card requires exact /dev/fb0"),
            Self::NotCharacterDevice => {
                formatter.write_str("pixel card framebuffer is not a character device")
            }
            Self::Io { operation, errno } => {
                write!(
                    formatter,
                    "pixel target {operation:?} failed{}",
                    ErrnoSuffix(*errno)
                )
            }
            Self::Query(error) => write!(formatter, "pixel target metadata query failed: {error}"),
            Self::CapabilityMismatch => {
                formatter.write_str("read/write framebuffer metadata changed from fresh report")
            }
            Self::LengthUnrepresentable => {
                formatter.write_str("framebuffer mapping length is unrepresentable")
            }
            Self::NullMapping => formatter.write_str("framebuffer mmap returned null"),
        }
    }
}

impl std::error::Error for LinuxPixelTargetError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Prerequisite(error) => Some(error),
            Self::Query(error) => Some(error),
            Self::InvalidPath
            | Self::NotCharacterDevice
            | Self::Io { .. }
            | Self::CapabilityMismatch
            | Self::LengthUnrepresentable
            | Self::NullMapping => None,
        }
    }
}

fn target_io_error(
    operation: LinuxPixelOperation,
    error: &std::io::Error,
) -> LinuxPixelTargetError {
    LinuxPixelTargetError::Io {
        operation,
        errno: positive_errno(error),
    }
}

fn last_target_io_error(operation: LinuxPixelOperation) -> LinuxPixelTargetError {
    target_io_error(operation, &std::io::Error::last_os_error())
}

fn positive_errno(error: &std::io::Error) -> Option<NonZeroI32> {
    error
        .raw_os_error()
        .filter(|errno| *errno > 0)
        .and_then(NonZeroI32::new)
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
