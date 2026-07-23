//! Linux implementation limited to read-only metadata and nonblocking input.

use std::fs::{File, OpenOptions};
use std::num::NonZeroI32;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::time::Duration;

use ferrink_platform::{
    FramebufferCapability, InputAxisCapability, InputDeviceId, PixelBitfield, PixelLayout,
    redact_text,
};

#[cfg(feature = "linux-input-grab-card")]
use crate::{ExclusiveInputSource, InputGrabIoError, InputGrabOperation};
use crate::{
    InputPollStatus, InputReadStatus, InputStreamIoError, InputStreamOperation,
    NonBlockingInputSource, ReadOnlyDeviceIo, ReadOnlyFramebuffer, ReadOnlyInput,
    ReadOnlyInputSnapshot, ReadOnlyIoError, ReadOnlyOperation,
};

const FBIOGET_VSCREENINFO: libc::c_ulong = 0x4600;
const FBIOGET_FSCREENINFO: libc::c_ulong = 0x4602;
const INPUT_NAME_BYTES: usize = 129;
#[cfg(feature = "linux-input-grab-card")]
const EVIOCGRAB: libc::c_ulong = 0x4004_4590;

/// Linux opener that grants only read-only descriptor authority.
#[derive(Debug, Default)]
pub struct LinuxReadOnlyDeviceIo;

impl ReadOnlyDeviceIo for LinuxReadOnlyDeviceIo {
    type Framebuffer = LinuxReadOnlyFramebuffer;
    type Input = LinuxReadOnlyInput;

    fn open_framebuffer_read_only(
        &mut self,
        path: &str,
    ) -> Result<Self::Framebuffer, ReadOnlyIoError> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(path)
            .map_err(|error| io_error(ReadOnlyOperation::OpenFramebuffer, &error))?;
        Ok(LinuxReadOnlyFramebuffer {
            file,
            device: path.to_owned(),
        })
    }

    fn open_input_read_only_nonblocking(
        &mut self,
        path: &str,
    ) -> Result<Self::Input, ReadOnlyIoError> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
            .map_err(|error| io_error(ReadOnlyOperation::OpenInput, &error))?;
        Ok(LinuxReadOnlyInput {
            file,
            device: path.to_owned(),
        })
    }
}

/// Owned Linux framebuffer descriptor opened without write access.
#[derive(Debug)]
pub struct LinuxReadOnlyFramebuffer {
    file: File,
    device: String,
}

impl ReadOnlyFramebuffer for LinuxReadOnlyFramebuffer {
    fn query_capability(&mut self) -> Result<FramebufferCapability, ReadOnlyIoError> {
        query_framebuffer_capability(&self.file, &self.device)
    }
}

pub(crate) fn query_framebuffer_capability(
    file: &File,
    device: &str,
) -> Result<FramebufferCapability, ReadOnlyIoError> {
    let mut variable = FbVarScreeninfo::default();
    let mut fixed = FbFixScreeninfo::default();
    // SAFETY: `file` owns a live descriptor, this request only writes metadata
    // into the correctly sized C-compatible output structure, and `variable`
    // remains exclusively borrowed for the duration of the call.
    if unsafe {
        libc::ioctl(
            file.as_raw_fd(),
            FBIOGET_VSCREENINFO as _,
            &mut variable as *mut FbVarScreeninfo,
        )
    } < 0
    {
        return Err(last_io_error(ReadOnlyOperation::QueryFramebuffer));
    }
    // SAFETY: the same owned descriptor remains live, this request only writes
    // metadata into the matching C-compatible output structure, and `fixed` is
    // exclusively borrowed for the call.
    if unsafe {
        libc::ioctl(
            file.as_raw_fd(),
            FBIOGET_FSCREENINFO as _,
            &mut fixed as *mut FbFixScreeninfo,
        )
    } < 0
    {
        return Err(last_io_error(ReadOnlyOperation::QueryFramebuffer));
    }

    let driver_id = String::from_utf8_lossy(&fixed.id)
        .trim_matches('\0')
        .to_owned();
    let pixel_layout = if variable.bits_per_pixel == 8
        && (variable.grayscale != 0
            || (variable.red.length == 0
                && variable.green.length == 0
                && variable.blue.length == 0))
    {
        PixelLayout::Grayscale8
    } else if variable.red.length > 0 && variable.green.length > 0 && variable.blue.length > 0 {
        PixelLayout::PackedRgb
    } else {
        PixelLayout::Unknown
    };
    Ok(FramebufferCapability {
        device: device.to_owned(),
        driver_id: redact_text(&driver_id),
        visible_width: variable.xres,
        visible_height: variable.yres,
        virtual_width: variable.xres_virtual,
        virtual_height: variable.yres_virtual,
        x_offset: variable.xoffset,
        y_offset: variable.yoffset,
        line_length: fixed.line_length,
        memory_length: fixed.smem_len,
        bits_per_pixel: variable.bits_per_pixel,
        grayscale: variable.grayscale,
        pixel_layout,
        rotation: variable.rotate,
        red: variable.red.into(),
        green: variable.green.into(),
        blue: variable.blue.into(),
        transparency: variable.transp.into(),
    })
}

/// Owned Linux input descriptor opened read-only and nonblocking.
#[derive(Debug)]
pub struct LinuxReadOnlyInput {
    file: File,
    device: String,
}

impl ReadOnlyInput for LinuxReadOnlyInput {
    fn query_snapshot(
        &mut self,
        expected: &ReadOnlyInputSnapshot,
    ) -> Result<ReadOnlyInputSnapshot, ReadOnlyIoError> {
        let mut name = [0_u8; INPUT_NAME_BYTES];
        let name_request = ioc(2, b'E', 0x06, u32::try_from(name.len()).unwrap_or(u32::MAX));
        // SAFETY: `self.file` owns a live input descriptor, `name_request` is
        // EVIOCGNAME for exactly `name.len()` writable bytes, and the array is
        // exclusively borrowed for the duration of the call.
        if unsafe {
            libc::ioctl(
                self.file.as_raw_fd(),
                name_request as _,
                name.as_mut_ptr().cast::<libc::c_void>(),
            )
        } < 0
        {
            return Err(last_io_error(ReadOnlyOperation::QueryInput));
        }
        let name_end = name
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(name.len());
        let name = redact_text(&String::from_utf8_lossy(&name[..name_end]));

        let mut id = InputId::default();
        let id_request = ioc(
            2,
            b'E',
            0x02,
            u32::try_from(std::mem::size_of::<InputId>()).unwrap_or(u32::MAX),
        );
        // SAFETY: `self.file` owns a live input descriptor, `id_request` is
        // EVIOCGID for `InputId`, and `id` is a valid exclusive output buffer.
        if unsafe {
            libc::ioctl(
                self.file.as_raw_fd(),
                id_request as _,
                &mut id as *mut InputId,
            )
        } < 0
        {
            return Err(last_io_error(ReadOnlyOperation::QueryInput));
        }

        let mut axes = Vec::new();
        axes.try_reserve_exact(expected.axes().len())
            .map_err(|_| ReadOnlyIoError::new(ReadOnlyOperation::QueryInput, None))?;
        for expected_axis in expected.axes() {
            let mut info = InputAbsInfo::default();
            let request = ioc(2, b'E', 0x40 + u32::from(expected_axis.code), 24);
            // SAFETY: `self.file` owns a live input descriptor, `request` is an
            // EVIOCGABS read for the reviewed axis and 24-byte `InputAbsInfo`,
            // and `info` is an exclusive output buffer for the call.
            if unsafe {
                libc::ioctl(
                    self.file.as_raw_fd(),
                    request as _,
                    &mut info as *mut InputAbsInfo,
                )
            } < 0
            {
                return Err(last_io_error(ReadOnlyOperation::QueryInput));
            }
            axes.push(InputAxisCapability {
                code: expected_axis.code,
                name: axis_name(expected_axis.code).map(str::to_owned),
                minimum: info.minimum,
                maximum: info.maximum,
                fuzz: info.fuzz,
                flat: info.flat,
                resolution: info.resolution,
            });
        }

        Ok(ReadOnlyInputSnapshot::new(
            self.device.clone(),
            (!name.is_empty()).then_some(name),
            InputDeviceId {
                bus: Some(id.bustype),
                vendor: Some(id.vendor),
                product: Some(id.product),
                version: Some(id.version),
            },
            axes,
        ))
    }
}

impl NonBlockingInputSource for LinuxReadOnlyInput {
    fn poll_readable(&mut self, timeout: Duration) -> Result<InputPollStatus, InputStreamIoError> {
        let mut descriptor = libc::pollfd {
            fd: self.file.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let timeout_millis = timeout.as_millis().clamp(1, libc::c_int::MAX as u128);
        let timeout_millis = libc::c_int::try_from(timeout_millis).unwrap_or(libc::c_int::MAX);
        // SAFETY: `descriptor` is a valid one-element pollfd array for the call,
        // its file descriptor remains owned by `self`, and the timeout is finite.
        let result = unsafe { libc::poll(&mut descriptor, 1, timeout_millis) };
        if result < 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EINTR) {
                return Ok(InputPollStatus::Interrupted);
            }
            return Err(stream_io_error(InputStreamOperation::Poll, &error));
        }
        if result == 0 {
            return Ok(InputPollStatus::TimedOut);
        }
        let fatal = libc::POLLERR | libc::POLLHUP | libc::POLLNVAL;
        if descriptor.revents & fatal != 0 {
            return Err(InputStreamIoError::new(InputStreamOperation::Poll, None));
        }
        if descriptor.revents & libc::POLLIN != 0 {
            Ok(InputPollStatus::Readable)
        } else {
            Err(InputStreamIoError::new(InputStreamOperation::Poll, None))
        }
    }

    fn read_nonblocking(
        &mut self,
        buffer: &mut [u8],
    ) -> Result<InputReadStatus, InputStreamIoError> {
        // SAFETY: `buffer` is a valid writable slice for exactly `buffer.len()`
        // bytes, and `self.file` owns the nonblocking descriptor for the call.
        let result = unsafe {
            libc::read(
                self.file.as_raw_fd(),
                buffer.as_mut_ptr().cast::<libc::c_void>(),
                buffer.len(),
            )
        };
        if result < 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EINTR) {
                return Ok(InputReadStatus::Interrupted);
            }
            if error.raw_os_error() == Some(libc::EAGAIN)
                || error.raw_os_error() == Some(libc::EWOULDBLOCK)
            {
                return Ok(InputReadStatus::WouldBlock);
            }
            return Err(stream_io_error(InputStreamOperation::Read, &error));
        }
        if result == 0 {
            return Ok(InputReadStatus::EndOfFile);
        }
        usize::try_from(result)
            .map(InputReadStatus::Bytes)
            .map_err(|_| InputStreamIoError::new(InputStreamOperation::Read, None))
    }
}

#[cfg(feature = "linux-input-grab-card")]
impl ExclusiveInputSource for LinuxReadOnlyInput {
    fn set_exclusive(&mut self, exclusive: bool) -> Result<(), InputGrabIoError> {
        let value: libc::c_int = i32::from(exclusive);
        // SAFETY: `self.file` owns the exact revalidated input descriptor,
        // EVIOCGRAB takes one integer flag by value, and the call is attempted
        // exactly once by the explicit transaction boundary.
        if unsafe { libc::ioctl(self.file.as_raw_fd(), EVIOCGRAB as _, value) } < 0 {
            return Err(InputGrabIoError::new(
                if exclusive {
                    InputGrabOperation::Acquire
                } else {
                    InputGrabOperation::Release
                },
                positive_errno(&std::io::Error::last_os_error()),
            ));
        }
        Ok(())
    }
}

fn io_error(operation: ReadOnlyOperation, error: &std::io::Error) -> ReadOnlyIoError {
    ReadOnlyIoError::new(operation, positive_errno(error))
}

fn last_io_error(operation: ReadOnlyOperation) -> ReadOnlyIoError {
    io_error(operation, &std::io::Error::last_os_error())
}

fn stream_io_error(operation: InputStreamOperation, error: &std::io::Error) -> InputStreamIoError {
    InputStreamIoError::new(operation, positive_errno(error))
}

fn positive_errno(error: &std::io::Error) -> Option<NonZeroI32> {
    error
        .raw_os_error()
        .filter(|errno| *errno > 0)
        .and_then(NonZeroI32::new)
}

const fn ioc(direction: u32, kind: u8, number: u32, size: u32) -> libc::c_ulong {
    ((direction << 30) | (size << 16) | ((kind as u32) << 8) | number) as libc::c_ulong
}

const fn axis_name(code: u16) -> Option<&'static str> {
    match code {
        0x00 => Some("abs_x"),
        0x01 => Some("abs_y"),
        0x2f => Some("abs_mt_slot"),
        0x35 => Some("abs_mt_position_x"),
        0x36 => Some("abs_mt_position_y"),
        0x39 => Some("abs_mt_tracking_id"),
        _ => None,
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FbBitfield {
    offset: u32,
    length: u32,
    msb_right: u32,
}

impl From<FbBitfield> for PixelBitfield {
    fn from(value: FbBitfield) -> Self {
        Self {
            offset: value.offset,
            length: value.length,
            msb_right: value.msb_right,
        }
    }
}

#[repr(C)]
#[derive(Default)]
struct FbVarScreeninfo {
    xres: u32,
    yres: u32,
    xres_virtual: u32,
    yres_virtual: u32,
    xoffset: u32,
    yoffset: u32,
    bits_per_pixel: u32,
    grayscale: u32,
    red: FbBitfield,
    green: FbBitfield,
    blue: FbBitfield,
    transp: FbBitfield,
    nonstd: u32,
    activate: u32,
    height: u32,
    width: u32,
    accel_flags: u32,
    pixclock: u32,
    left_margin: u32,
    right_margin: u32,
    upper_margin: u32,
    lower_margin: u32,
    hsync_len: u32,
    vsync_len: u32,
    sync: u32,
    vmode: u32,
    rotate: u32,
    colorspace: u32,
    reserved: [u32; 4],
}

#[repr(C)]
#[derive(Default)]
struct FbFixScreeninfo {
    id: [u8; 16],
    smem_start: libc::c_ulong,
    smem_len: u32,
    type_: u32,
    type_aux: u32,
    visual: u32,
    xpanstep: u16,
    ypanstep: u16,
    ywrapstep: u16,
    line_length: u32,
    mmio_start: libc::c_ulong,
    mmio_len: u32,
    accel: u32,
    capabilities: u16,
    reserved: [u16; 2],
}

#[repr(C)]
#[derive(Default)]
struct InputId {
    bustype: u16,
    vendor: u16,
    product: u16,
    version: u16,
}

#[repr(C)]
#[derive(Default)]
struct InputAbsInfo {
    value: i32,
    minimum: i32,
    maximum: i32,
    fuzz: i32,
    flat: i32,
    resolution: i32,
}

const _: () = {
    assert!(std::mem::size_of::<FbBitfield>() == 12);
    assert!(std::mem::size_of::<FbVarScreeninfo>() == 160);
    assert!(std::mem::size_of::<InputId>() == 8);
    assert!(std::mem::size_of::<InputAbsInfo>() == 24);
    assert!(ioc(2, b'E', 0x02, 8) == 0x8008_4502);
    assert!(ioc(2, b'E', 0x06, 129) == 0x8081_4506);
    assert!(ioc(2, b'E', 0x40 + 53, 24) == 0x8018_4575);
};

#[cfg(target_pointer_width = "32")]
const _: () = assert!(std::mem::size_of::<FbFixScreeninfo>() == 68);

#[cfg(target_pointer_width = "64")]
const _: () = assert!(std::mem::size_of::<FbFixScreeninfo>() == 80);
