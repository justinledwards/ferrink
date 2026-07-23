//! Shared, host-tested encoder for the reviewed Kindle Zelda display ABI.

#![deny(unsafe_code)]

use ferrink_platform::{RefreshMode, RefreshRequest, UpdateMarker};

/// Oasis 2/3 request number with the exact 88-byte Zelda payload size.
pub(crate) const MXCFB_SEND_UPDATE_ZELDA: u64 = 0x4058_462e;

const WAVEFORM_MODE_GC16: u32 = 2;
const WAVEFORM_MODE_AUTO: u32 = 257;
const UPDATE_MODE_PARTIAL: u32 = 0;
const UPDATE_MODE_FULL: u32 = 1;
const TEMP_USE_AMBIENT: i32 = 0x1000;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UpdateRect {
    top: u32,
    left: u32,
    width: u32,
    height: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AlternateBuffer {
    physical_address: u32,
    width: u32,
    height: u32,
    update_region: UpdateRect,
}

/// Fully initialized request matching the KOA2/KOA3 Zelda kernel ABI.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ZeldaUpdateRequest {
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

impl ZeldaUpdateRequest {
    /// Encodes one already validated generic request using the constants from
    /// the known-working local Slint Kindle backend.
    pub(crate) const fn encode(request: RefreshRequest, marker: UpdateMarker) -> Self {
        let region = request.region();
        let (waveform_mode, update_mode) = match request.mode() {
            RefreshMode::Partial => (WAVEFORM_MODE_AUTO, UPDATE_MODE_PARTIAL),
            RefreshMode::Full => (WAVEFORM_MODE_GC16, UPDATE_MODE_FULL),
        };
        Self {
            update_region: UpdateRect {
                top: region.y(),
                left: region.x(),
                width: region.width(),
                height: region.height(),
            },
            waveform_mode,
            update_mode,
            update_marker: marker.get(),
            temperature: TEMP_USE_AMBIENT,
            flags: 0,
            dither_mode: 0,
            quant_bit: 0,
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
            hist_bw_waveform_mode: 0,
            hist_gray_waveform_mode: 0,
            ts_pxp: 0,
            ts_epdc: 0,
        }
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use ferrink_platform::{
        DisplayExtent, RefreshCompletionPolicy, RefreshRegion, RefreshRequest, UpdateMarkerSequence,
    };
    use std::num::NonZeroU32;

    fn request(mode: RefreshMode) -> RefreshRequest {
        let visible = DisplayExtent::try_new(1264, 1680).unwrap();
        RefreshRequest::new(
            RefreshRegion::try_new(600, 808, 64, 64, visible).unwrap(),
            mode,
            RefreshCompletionPolicy::DoNotWait,
        )
    }

    #[test]
    fn partial_request_matches_the_working_backend_constants() {
        assert_eq!(MXCFB_SEND_UPDATE_ZELDA, 0x4058_462e);
        let update = ZeldaUpdateRequest::encode(
            request(RefreshMode::Partial),
            UpdateMarkerSequence::starting_at(NonZeroU32::new(0x464b_0002).unwrap()).allocate(),
        );

        assert_eq!(update.update_region.top, 808);
        assert_eq!(update.update_region.left, 600);
        assert_eq!(update.update_region.width, 64);
        assert_eq!(update.update_region.height, 64);
        assert_eq!(update.waveform_mode, WAVEFORM_MODE_AUTO);
        assert_eq!(update.update_mode, UPDATE_MODE_PARTIAL);
        assert_eq!(update.update_marker, 0x464b_0002);
        assert_eq!(update.temperature, TEMP_USE_AMBIENT);
    }

    #[test]
    fn full_request_uses_the_working_cleaning_constants() {
        let update = ZeldaUpdateRequest::encode(
            request(RefreshMode::Full),
            UpdateMarkerSequence::starting_at(NonZeroU32::new(7).unwrap()).allocate(),
        );

        assert_eq!(update.waveform_mode, WAVEFORM_MODE_GC16);
        assert_eq!(update.update_mode, UPDATE_MODE_FULL);
        assert_eq!(update.update_marker, 7);
    }
}
