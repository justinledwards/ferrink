//! Exact ARM Linux submission boundary for the reviewed KOA3 lightbox ABI.

use std::fs::{File, OpenOptions};
use std::num::NonZeroI32;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};

use ferrink_platform::{DisplayUpdateAbiKind, ResolvedRuntimeDevice};

use crate::{KOA3_APPLY_HALFTONE, Koa3LightboxRequest, Koa3LightboxSubmission, L0DisplayCore};

use super::linux::query_framebuffer_capability;

/// Sends one clear request through a freshly revalidated KOA3 descriptor.
///
/// This is the guardian's crash-recovery boundary. It does not map or access
/// pixels and does not repaint; the foreground restoration transaction owns
/// the required subsequent stock repaint.
///
/// # Errors
///
/// Returns before submission for any profile, ABI, path, file-type, or live
/// capability drift, and reports every unexpected ioctl result.
pub fn clear_koa3_lightbox(
    device: &ResolvedRuntimeDevice,
) -> Result<Koa3LightboxSubmission, LinuxKoa3LightboxError> {
    let file = open_revalidated_lightbox_framebuffer(device)?;
    submit_koa3_lightbox(
        &file,
        Koa3LightboxRequest::encode(crate::Koa3LightboxState::Clear),
    )
}

pub(crate) fn submit_koa3_lightbox(
    framebuffer: &File,
    request: Koa3LightboxRequest,
) -> Result<Koa3LightboxSubmission, LinuxKoa3LightboxError> {
    let ioctl_number = libc::Ioctl::try_from(KOA3_APPLY_HALFTONE)
        .map_err(|_| LinuxKoa3LightboxError::InvalidIoctlNumber)?;
    // SAFETY: the descriptor is the exact revalidated framebuffer and
    // `request` is a fully initialized, compile-time-checked 36-byte payload
    // borrowed only for this synchronous call.
    let result = unsafe {
        libc::ioctl(
            framebuffer.as_raw_fd(),
            ioctl_number,
            &request as *const Koa3LightboxRequest,
        )
    };
    if result >= 0 {
        return Ok(Koa3LightboxSubmission::Accepted);
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::EINVAL) {
        return Ok(Koa3LightboxSubmission::ExpectedInvalidArgument);
    }
    Err(lightbox_io_error(
        LinuxKoa3LightboxOperation::Submit,
        &error,
    ))
}

fn open_revalidated_lightbox_framebuffer(
    device: &ResolvedRuntimeDevice,
) -> Result<File, LinuxKoa3LightboxError> {
    if device.profile_id() != "kindle-oasis-3-0wm" {
        return Err(LinuxKoa3LightboxError::WrongProfile);
    }
    let display = L0DisplayCore::try_from_runtime(device)
        .map_err(|_| LinuxKoa3LightboxError::RefreshUnavailable)?;
    if display.update_abi() != DisplayUpdateAbiKind::Zelda88 {
        return Err(LinuxKoa3LightboxError::WrongRefreshAbi);
    }
    let path = device.framebuffer_path();
    if path != "/dev/fb0" {
        return Err(LinuxKoa3LightboxError::InvalidPath);
    }
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| lightbox_io_error(LinuxKoa3LightboxOperation::Inspect, &error))?;
    if !metadata.file_type().is_char_device() {
        return Err(LinuxKoa3LightboxError::NotCharacterDevice);
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| lightbox_io_error(LinuxKoa3LightboxOperation::Open, &error))?;
    if !file
        .metadata()
        .map_err(|error| lightbox_io_error(LinuxKoa3LightboxOperation::Inspect, &error))?
        .file_type()
        .is_char_device()
    {
        return Err(LinuxKoa3LightboxError::NotCharacterDevice);
    }
    let observed =
        query_framebuffer_capability(&file, path).map_err(LinuxKoa3LightboxError::Query)?;
    if &observed != device.framebuffer_capability() {
        return Err(LinuxKoa3LightboxError::CapabilityMismatch);
    }
    Ok(file)
}

/// Linux lightbox operation associated with a bounded I/O failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinuxKoa3LightboxOperation {
    /// Inspect the configured path or opened descriptor.
    Inspect,
    /// Open the exact framebuffer read/write without mapping it.
    Open,
    /// Submit the exact 36-byte lightbox request.
    Submit,
}

/// Failure while opening or submitting the exact KOA3 lightbox request.
#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LinuxKoa3LightboxError {
    /// The resolved profile was not the exact reviewed KOA3.
    WrongProfile,
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
    /// The ioctl number did not fit the target libc request type.
    InvalidIoctlNumber,
    /// A bounded system operation failed.
    Io {
        /// Exact failed operation.
        operation: LinuxKoa3LightboxOperation,
        /// Positive errno when the platform supplied one.
        errno: Option<NonZeroI32>,
    },
}

impl std::fmt::Display for LinuxKoa3LightboxError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WrongProfile => formatter.write_str("lightbox requires exact KOA3 profile"),
            Self::RefreshUnavailable => formatter.write_str("lightbox refresh is unavailable"),
            Self::WrongRefreshAbi => formatter.write_str("lightbox requires Zelda-88 refresh ABI"),
            Self::InvalidPath => formatter.write_str("lightbox framebuffer path is invalid"),
            Self::NotCharacterDevice => {
                formatter.write_str("lightbox framebuffer is not a character device")
            }
            Self::Query(error) => write!(formatter, "lightbox capability query failed: {error}"),
            Self::CapabilityMismatch => {
                formatter.write_str("lightbox framebuffer capability changed")
            }
            Self::InvalidIoctlNumber => formatter.write_str("lightbox ioctl number is invalid"),
            Self::Io { operation, errno } => {
                write!(formatter, "lightbox {operation:?} failed")?;
                if let Some(errno) = errno {
                    write!(formatter, " with errno {errno}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for LinuxKoa3LightboxError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Query(error) => Some(error),
            Self::WrongProfile
            | Self::RefreshUnavailable
            | Self::WrongRefreshAbi
            | Self::InvalidPath
            | Self::NotCharacterDevice
            | Self::CapabilityMismatch
            | Self::InvalidIoctlNumber
            | Self::Io { .. } => None,
        }
    }
}

fn lightbox_io_error(
    operation: LinuxKoa3LightboxOperation,
    error: &std::io::Error,
) -> LinuxKoa3LightboxError {
    LinuxKoa3LightboxError::Io {
        operation,
        errno: error.raw_os_error().and_then(NonZeroI32::new),
    }
}
