//! Minimal Zelda display-update ABI boundary.

#[cfg(all(target_os = "linux", target_arch = "arm", target_pointer_width = "32"))]
use std::os::fd::AsRawFd;

use ferrink_platform::ActiveDisplayRequestPlan;

#[cfg(any(
    test,
    all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")
))]
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UpdateRect {
    top: u32,
    left: u32,
    width: u32,
    height: u32,
}

#[cfg(any(
    test,
    all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")
))]
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AlternateBuffer {
    physical_address: u32,
    width: u32,
    height: u32,
    update_region: UpdateRect,
}

#[cfg(any(
    test,
    all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")
))]
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ZeldaUpdateRequest {
    update_region: UpdateRect,
    waveform_mode: u32,
    update_mode: u32,
    update_marker: u32,
    temperature: i32,
    flags: u32,
    dither_mode: i32,
    quant_bit: i32,
    alternate_buffer: AlternateBuffer,
    hist_bw_waveform_mode: u32,
    hist_gray_waveform_mode: u32,
    ts_pxp: u32,
    ts_epdc: u32,
}

#[cfg(any(
    test,
    all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")
))]
const _: () = {
    assert!(std::mem::size_of::<UpdateRect>() == 16);
    assert!(std::mem::size_of::<AlternateBuffer>() == 28);
    assert!(std::mem::size_of::<ZeldaUpdateRequest>() == 88);
    assert!(std::mem::offset_of!(ZeldaUpdateRequest, update_region) == 0);
    assert!(std::mem::offset_of!(ZeldaUpdateRequest, waveform_mode) == 16);
    assert!(std::mem::offset_of!(ZeldaUpdateRequest, update_mode) == 20);
    assert!(std::mem::offset_of!(ZeldaUpdateRequest, update_marker) == 24);
    assert!(std::mem::offset_of!(ZeldaUpdateRequest, temperature) == 28);
    assert!(std::mem::offset_of!(ZeldaUpdateRequest, flags) == 32);
    assert!(std::mem::offset_of!(ZeldaUpdateRequest, dither_mode) == 36);
    assert!(std::mem::offset_of!(ZeldaUpdateRequest, quant_bit) == 40);
    assert!(std::mem::offset_of!(ZeldaUpdateRequest, alternate_buffer) == 44);
    assert!(std::mem::offset_of!(ZeldaUpdateRequest, hist_bw_waveform_mode) == 72);
    assert!(std::mem::offset_of!(ZeldaUpdateRequest, hist_gray_waveform_mode) == 76);
    assert!(std::mem::offset_of!(ZeldaUpdateRequest, ts_pxp) == 80);
    assert!(std::mem::offset_of!(ZeldaUpdateRequest, ts_epdc) == 84);
};

#[cfg(any(
    test,
    all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")
))]
impl ZeldaUpdateRequest {
    fn from_plan(plan: &ActiveDisplayRequestPlan) -> Self {
        Self {
            update_region: UpdateRect {
                top: plan.region.y,
                left: plan.region.x,
                width: plan.region.width.get(),
                height: plan.region.height.get(),
            },
            waveform_mode: plan.waveform_mode,
            update_mode: plan.update_mode,
            update_marker: plan.marker.get(),
            temperature: plan.temperature,
            flags: plan.flags,
            dither_mode: plan.dither_mode,
            quant_bit: plan.quant_bit,
            alternate_buffer: AlternateBuffer {
                physical_address: 0,
                width: 0,
                height: 0,
                update_region: UpdateRect {
                    top: 0,
                    left: 0,
                    width: 0,
                    height: 0,
                },
            },
            hist_bw_waveform_mode: plan.histogram_modes[0],
            hist_gray_waveform_mode: plan.histogram_modes[1],
            ts_pxp: plan.timestamps[0],
            ts_epdc: plan.timestamps[1],
        }
    }
}

/// Submits exactly one Zelda update request through the supplied framebuffer.
///
/// # Errors
///
/// Returns an operating-system error if the ioctl request number cannot be
/// represented on this target or if the kernel rejects the request.
#[cfg(all(target_os = "linux", target_arch = "arm", target_pointer_width = "32"))]
pub(super) fn submit(
    framebuffer: &std::fs::File,
    plan: &ActiveDisplayRequestPlan,
) -> std::io::Result<()> {
    let request = ZeldaUpdateRequest::from_plan(plan);
    let ioctl_number = libc::Ioctl::try_from(plan.request_ioctl.get()).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "display ioctl number is not representable as libc::Ioctl",
        )
    })?;

    // SAFETY: `framebuffer` is an open `/dev/fb0` descriptor, `request` is a
    // live 88-byte `repr(C)` Zelda request whose complete layout is asserted
    // above, and the ioctl only borrows its pointer for this synchronous call.
    let result = unsafe { libc::ioctl(framebuffer.as_raw_fd(), ioctl_number, &request) };
    if result == -1 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(all(target_os = "linux", target_arch = "arm", target_pointer_width = "32")))]
pub(super) fn submit(
    _framebuffer: &std::fs::File,
    _plan: &ActiveDisplayRequestPlan,
) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "active display submission requires 32-bit ARM Linux",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrink_platform::ActiveDisplayPlan;

    const PLAN: &str = include_str!(
        "../../ferrink-platform/tests/fixtures/reference-portrait-display-mechanism-plan-v1.json"
    );

    #[test]
    fn zelda_request_bytes_match_every_pinned_field() {
        let plan = ActiveDisplayPlan::from_json(PLAN).unwrap();
        let request = ZeldaUpdateRequest::from_plan(&plan.request);

        assert_eq!(request.update_region.top, 808);
        assert_eq!(request.update_region.left, 600);
        assert_eq!(request.update_region.width, 64);
        assert_eq!(request.update_region.height, 64);
        assert_eq!(request.waveform_mode, 257);
        assert_eq!(request.update_mode, 0);
        assert_eq!(request.update_marker, 0x464b_0001);
        assert_eq!(request.temperature, 0x1000);
        assert_eq!(request.flags, 0);
        assert_eq!(request.dither_mode, 0);
        assert_eq!(request.quant_bit, 0);
        assert_eq!(
            request.alternate_buffer,
            AlternateBuffer {
                physical_address: 0,
                width: 0,
                height: 0,
                update_region: UpdateRect {
                    top: 0,
                    left: 0,
                    width: 0,
                    height: 0,
                },
            }
        );
        assert_eq!(request.hist_bw_waveform_mode, 0);
        assert_eq!(request.hist_gray_waveform_mode, 0);
        assert_eq!(request.ts_pxp, 0);
        assert_eq!(request.ts_epdc, 0);
    }
}
