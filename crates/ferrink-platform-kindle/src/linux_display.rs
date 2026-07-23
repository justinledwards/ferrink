//! Multi-frame ARM Linux display target for an owned foreground.

use std::fs::{File, OpenOptions};
use std::num::{NonZeroI32, NonZeroU16};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};
use std::ptr::NonNull;

use ferrink_platform::{
    DisplayUpdateAbiKind, RefreshCapabilities, RefreshCompletionPolicy, RefreshError,
    RefreshRequest, ResolvedRuntimeDevice, UpdateMarker,
};

use crate::{DisplayTarget, L0DisplayCore};

use super::linux::query_framebuffer_capability;
use super::zelda::{MXCFB_SEND_UPDATE_ZELDA, ZeldaUpdateRequest};

/// Explicitly owned framebuffer mapping for repeated validated foreground frames.
///
/// This target performs no service handoff, input operation, stock repaint, or
/// process signal. An outer supervisor must own those transitions.
#[derive(Debug)]
pub struct LinuxForegroundDisplayTarget {
    mapping: Option<NonNull<u8>>,
    length: usize,
    file: File,
    update_abi: DisplayUpdateAbiKind,
    capabilities: RefreshCapabilities,
}

impl LinuxForegroundDisplayTarget {
    /// Opens, revalidates, and maps the resolved framebuffer.
    ///
    /// # Errors
    ///
    /// Returns before mapping for any profile, ABI, path, file-type, or live
    /// capability drift. A mapping failure closes the descriptor.
    pub fn open(device: &ResolvedRuntimeDevice) -> Result<Self, LinuxForegroundDisplayError> {
        let display = L0DisplayCore::try_from_runtime(device)
            .map_err(|_| LinuxForegroundDisplayError::RefreshUnavailable)?;
        if display.update_abi() != DisplayUpdateAbiKind::Zelda88 {
            return Err(LinuxForegroundDisplayError::WrongRefreshAbi);
        }
        let path = device.framebuffer_path();
        if path != "/dev/fb0" {
            return Err(LinuxForegroundDisplayError::InvalidPath);
        }
        let metadata = std::fs::symlink_metadata(path)
            .map_err(|error| display_io_error(LinuxForegroundDisplayOperation::Inspect, &error))?;
        if !metadata.file_type().is_char_device() {
            return Err(LinuxForegroundDisplayError::NotCharacterDevice);
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(path)
            .map_err(|error| display_io_error(LinuxForegroundDisplayOperation::Open, &error))?;
        if !file
            .metadata()
            .map_err(|error| display_io_error(LinuxForegroundDisplayOperation::Inspect, &error))?
            .file_type()
            .is_char_device()
        {
            return Err(LinuxForegroundDisplayError::NotCharacterDevice);
        }
        let observed = query_framebuffer_capability(&file, path)
            .map_err(LinuxForegroundDisplayError::Query)?;
        if &observed != device.framebuffer_capability() {
            return Err(LinuxForegroundDisplayError::CapabilityMismatch);
        }
        let length = usize::try_from(display.layout().memory_length())
            .map_err(|_| LinuxForegroundDisplayError::LengthUnrepresentable)?;
        // SAFETY: `file` is the exact revalidated framebuffer opened read/write,
        // `length` is its non-zero kernel-reported mapping length, offset zero is
        // page aligned, and the return value is checked against MAP_FAILED.
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
            return Err(last_display_io_error(LinuxForegroundDisplayOperation::Map));
        }
        let Some(mapping) = NonNull::new(raw.cast::<u8>()) else {
            // SAFETY: mmap succeeded for this exact pointer and length. Release
            // it once because a null address cannot form a Rust slice.
            let _ = unsafe { libc::munmap(raw, length) };
            return Err(LinuxForegroundDisplayError::NullMapping);
        };
        Ok(Self {
            mapping: Some(mapping),
            length,
            file,
            update_abi: display.update_abi(),
            capabilities: device
                .refresh()
                .expect("validated display core requires refresh")
                .capabilities(),
        })
    }

    /// Unmaps the framebuffer exactly once, after which the descriptor closes.
    ///
    /// # Errors
    ///
    /// Returns the bounded `munmap` failure without retrying.
    pub fn close(mut self) -> Result<(), LinuxForegroundDisplayError> {
        self.unmap_once()
    }

    fn unmap_once(&mut self) -> Result<(), LinuxForegroundDisplayError> {
        let Some(mapping) = self.mapping.take() else {
            return Ok(());
        };
        // SAFETY: `mapping` and `length` come from this owner's one successful
        // mmap, and taking the Option prevents a second unmap attempt.
        if unsafe { libc::munmap(mapping.as_ptr().cast(), self.length) } < 0 {
            return Err(last_display_io_error(
                LinuxForegroundDisplayOperation::Unmap,
            ));
        }
        Ok(())
    }
}

impl DisplayTarget for LinuxForegroundDisplayTarget {
    fn framebuffer_memory(&mut self) -> &mut [u8] {
        let mapping = self
            .mapping
            .expect("BUG: framebuffer memory requested after explicit close");
        // SAFETY: the mapping is live for `self.length` bytes and &mut self
        // prevents this owner from producing overlapping mutable slices.
        unsafe { std::slice::from_raw_parts_mut(mapping.as_ptr(), self.length) }
    }

    fn submit_refresh(
        &mut self,
        update_abi: DisplayUpdateAbiKind,
        request: RefreshRequest,
        marker: UpdateMarker,
    ) -> Result<(), RefreshError> {
        if update_abi != self.update_abi
            || update_abi != DisplayUpdateAbiKind::Zelda88
            || request.completion() != RefreshCompletionPolicy::DoNotWait
            || self.capabilities.validate(request).is_err()
        {
            return Err(RefreshError::SubmissionFailed {
                marker,
                errno: None,
            });
        }
        let update = ZeldaUpdateRequest::encode(request, marker);
        // SAFETY: `self.file` remains the exact revalidated framebuffer and
        // `update` is a fully initialized, compile-time-checked 88-byte Zelda
        // payload borrowed only for this synchronous ioctl.
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

impl Drop for LinuxForegroundDisplayTarget {
    fn drop(&mut self) {
        let _ = self.unmap_once();
    }
}

/// Linux display operation associated with a bounded I/O failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinuxForegroundDisplayOperation {
    /// Inspect the configured path or opened descriptor.
    Inspect,
    /// Open the exact framebuffer read/write.
    Open,
    /// Create the shared mapping.
    Map,
    /// Release the shared mapping.
    Unmap,
}

/// Failure before or while owning the multi-frame foreground mapping.
#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LinuxForegroundDisplayError {
    /// No reviewed refresh capability was present.
    RefreshUnavailable,
    /// The reviewed refresh ABI was not Zelda-88.
    WrongRefreshAbi,
    /// The resolved framebuffer path was not exactly `/dev/fb0`.
    InvalidPath,
    /// The path or opened descriptor was not a character device.
    NotCharacterDevice,
    /// Live metadata queries failed.
    Query(crate::ReadOnlyIoError),
    /// Live metadata changed after passive resolution.
    CapabilityMismatch,
    /// The mapping length did not fit the target process.
    LengthUnrepresentable,
    /// mmap returned a null pointer rather than MAP_FAILED.
    NullMapping,
    /// A bounded system operation failed.
    Io {
        /// Exact failed operation.
        operation: LinuxForegroundDisplayOperation,
        /// Positive errno when the platform supplied one.
        errno: Option<NonZeroI32>,
    },
}

impl std::fmt::Display for LinuxForegroundDisplayError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RefreshUnavailable => {
                formatter.write_str("foreground display refresh is unavailable")
            }
            Self::WrongRefreshAbi => formatter.write_str("foreground display requires Zelda-88"),
            Self::InvalidPath => formatter.write_str("foreground framebuffer path changed"),
            Self::NotCharacterDevice => {
                formatter.write_str("foreground framebuffer is not a character device")
            }
            Self::Query(error) => write!(formatter, "foreground metadata query failed: {error}"),
            Self::CapabilityMismatch => {
                formatter.write_str("foreground framebuffer metadata changed")
            }
            Self::LengthUnrepresentable => {
                formatter.write_str("foreground mapping length is unrepresentable")
            }
            Self::NullMapping => formatter.write_str("foreground mmap returned null"),
            Self::Io { operation, errno } => write!(
                formatter,
                "foreground display {operation:?} failed{}",
                ErrnoSuffix(*errno)
            ),
        }
    }
}

impl std::error::Error for LinuxForegroundDisplayError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Query(error) => Some(error),
            _ => None,
        }
    }
}

fn display_io_error(
    operation: LinuxForegroundDisplayOperation,
    error: &std::io::Error,
) -> LinuxForegroundDisplayError {
    LinuxForegroundDisplayError::Io {
        operation,
        errno: positive_errno(error),
    }
}

fn last_display_io_error(
    operation: LinuxForegroundDisplayOperation,
) -> LinuxForegroundDisplayError {
    display_io_error(operation, &std::io::Error::last_os_error())
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
