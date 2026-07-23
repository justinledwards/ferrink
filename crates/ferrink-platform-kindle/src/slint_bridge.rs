//! Host-testable bridge between validated Kindle events and Slint.

#![deny(unsafe_code)]

use std::num::NonZeroU32;
use std::rc::Rc;

use ferrink_platform::{
    Gray8Conversion, LogicalTouchPhase, RefreshCompletionPolicy, RefreshMode, RefreshRegion,
    RefreshRequest, Rgb8Pixel, TouchContactEvent, UpdateMarker,
};
use slint::WindowEventDispatchResult;
use slint::platform::software_renderer::{
    MinimalSoftwareWindow, PhysicalRegion, RepaintBufferType,
};
use slint::platform::{PointerEventButton, WindowAdapter, WindowEvent};

use crate::{DisplayTarget, L0DisplayCore, L0DisplayError};

/// Creates the single-window software adapter required by the foreground loop.
///
/// The reused buffer is essential: Slint may render only dirty pixels after the
/// first frame, while [`SlintFrameBuffer`] retains every other pixel.
#[must_use]
pub fn new_slint_window() -> Rc<MinimalSoftwareWindow> {
    MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer)
}

/// Stateful, fail-closed delivery of primary touch contacts to Slint.
#[derive(Debug, Default)]
pub struct SlintPointerBridge {
    stopped: bool,
}

impl SlintPointerBridge {
    /// Delivers one already transformed physical contact to Slint.
    ///
    /// The window's scale factor is the only physical-to-logical conversion.
    /// Device orientation and axis transforms have already been applied by
    /// `TouchTracker`.
    ///
    /// # Errors
    ///
    /// Returns [`SlintPointerError::Stopped`] after teardown, rejects an invalid
    /// Slint scale factor, and preserves any toolkit dispatch failure.
    pub fn dispatch(
        &mut self,
        window: &slint::Window,
        contact: TouchContactEvent,
    ) -> Result<WindowEventDispatchResult, SlintPointerError> {
        if self.stopped {
            return Err(SlintPointerError::Stopped);
        }
        let event = pointer_event(contact, window.scale_factor())?;
        window
            .dispatch_event_with_result(event)
            .map_err(SlintPointerError::Dispatch)
    }

    /// Stops input delivery and clears Slint's pointer hover/grab state.
    ///
    /// Stopping is idempotent. The stopped state is committed before dispatch,
    /// so a toolkit failure cannot accidentally permit later input.
    ///
    /// # Errors
    ///
    /// Preserves a toolkit failure while dispatching the teardown event.
    pub fn stop(
        &mut self,
        window: &slint::Window,
    ) -> Result<WindowEventDispatchResult, SlintPointerError> {
        if self.stopped {
            return Ok(WindowEventDispatchResult::Accepted);
        }
        self.stopped = true;
        window
            .dispatch_event_with_result(WindowEvent::PointerExited)
            .map_err(SlintPointerError::Dispatch)
    }

    /// Returns whether teardown has permanently stopped input delivery.
    #[must_use]
    pub const fn is_stopped(&self) -> bool {
        self.stopped
    }
}

fn pointer_event(
    contact: TouchContactEvent,
    scale_factor: f32,
) -> Result<WindowEvent, SlintPointerError> {
    if !scale_factor.is_finite() || scale_factor <= 0.0 {
        return Err(SlintPointerError::InvalidScaleFactor);
    }
    let position = slint::LogicalPosition::new(
        contact.point.x as f32 / scale_factor,
        contact.point.y as f32 / scale_factor,
    );
    Ok(match contact.phase {
        LogicalTouchPhase::Pressed => WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Left,
        },
        LogicalTouchPhase::Moved => WindowEvent::PointerMoved { position },
        LogicalTouchPhase::Released => WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        },
    })
}

/// Failure while translating or dispatching a validated touch contact.
#[derive(Debug)]
#[non_exhaustive]
pub enum SlintPointerError {
    /// The Slint window reported a zero, negative, infinite, or NaN scale.
    InvalidScaleFactor,
    /// Input teardown has already begun.
    Stopped,
    /// Slint rejected event delivery at the platform boundary.
    Dispatch(slint::PlatformError),
}

impl std::fmt::Display for SlintPointerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidScaleFactor => formatter.write_str("Slint window scale factor is invalid"),
            Self::Stopped => formatter.write_str("Slint pointer delivery has stopped"),
            Self::Dispatch(error) => write!(formatter, "Slint pointer dispatch failed: {error}"),
        }
    }
}

impl std::error::Error for SlintPointerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Dispatch(error) => Some(error),
            Self::InvalidScaleFactor | Self::Stopped => None,
        }
    }
}

/// Persistent software-renderer and conversion storage for one exact display.
#[derive(Debug)]
pub struct SlintFrameBuffer {
    width: NonZeroU32,
    height: NonZeroU32,
    render_pixels: Vec<slint::Rgb8Pixel>,
    staging_pixels: Vec<Rgb8Pixel>,
}

impl SlintFrameBuffer {
    /// Allocates ordinary memory for the exact visible framebuffer extent.
    ///
    /// # Errors
    ///
    /// Returns [`SlintRenderError`] if the pixel count does not fit the host or
    /// the allocation cannot be reserved. No device memory is involved.
    pub fn try_for_display(display: &L0DisplayCore) -> Result<Self, SlintRenderError> {
        let visible = display.layout().visible();
        let width = NonZeroU32::new(visible.width()).expect("validated display width is non-zero");
        let height =
            NonZeroU32::new(visible.height()).expect("validated display height is non-zero");
        let pixel_count = usize::try_from(width.get())
            .ok()
            .and_then(|width| {
                usize::try_from(height.get())
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .ok_or(SlintRenderError::PixelCountOverflow)?;
        let mut render_pixels = Vec::new();
        render_pixels
            .try_reserve_exact(pixel_count)
            .map_err(|_| SlintRenderError::AllocationFailed)?;
        render_pixels.resize(
            pixel_count,
            slint::Rgb8Pixel {
                r: u8::MAX,
                g: u8::MAX,
                b: u8::MAX,
            },
        );
        Ok(Self {
            width,
            height,
            render_pixels,
            staging_pixels: Vec::new(),
        })
    }

    /// Renders and presents at most one coalesced dirty region.
    ///
    /// Window dimensions and renderer buffer policy are checked before target
    /// memory changes. Slint's non-overlapping dirty rectangles are coalesced
    /// into their bounding region so one frame consumes at most one marker and
    /// one exact refresh submission. A full refresh stages the complete retained
    /// frame, even when Slint reports a smaller dirty region, so pixels left by
    /// the previous foreground do not survive Ferrink's first frame. Every RGB
    /// pixel is staged before the validated display core sees target memory.
    ///
    /// # Errors
    ///
    /// Returns [`SlintRenderError`] for configuration drift, invalid dirty
    /// geometry, allocation failure, or the structured display-core failure.
    pub fn present_if_needed<T: DisplayTarget>(
        &mut self,
        window: &MinimalSoftwareWindow,
        display: &mut L0DisplayCore,
        target: &mut T,
        mode: RefreshMode,
        completion: RefreshCompletionPolicy,
        conversion: Gray8Conversion,
    ) -> Result<SlintPresentOutcome, SlintRenderError> {
        let actual = WindowAdapter::size(window);
        if actual.width != self.width.get() || actual.height != self.height.get() {
            return Err(SlintRenderError::WindowSizeMismatch {
                expected_width: self.width.get(),
                expected_height: self.height.get(),
                actual_width: actual.width,
                actual_height: actual.height,
            });
        }

        let mut rendered_region: Option<Result<PhysicalRegion, SlintRenderError>> = None;
        let rendered = window.draw_if_needed(|renderer| {
            if renderer.repaint_buffer_type() != RepaintBufferType::ReusedBuffer {
                rendered_region = Some(Err(SlintRenderError::RendererBufferNotReused));
                return;
            }
            rendered_region = Some(Ok(renderer.render(
                self.render_pixels.as_mut_slice(),
                usize::try_from(self.width.get()).expect("u32 display width fits supported usize"),
            )));
        });
        if !rendered {
            return Ok(SlintPresentOutcome::Idle);
        }
        let dirty = rendered_region.ok_or(SlintRenderError::RendererDidNotReturnRegion)??;
        let size = dirty.bounding_box_size();
        if size.width == 0 || size.height == 0 {
            return Ok(SlintPresentOutcome::RedrawnWithoutPixels);
        }
        let origin = dirty.bounding_box_origin();
        let x = u32::try_from(origin.x).map_err(|_| SlintRenderError::DirtyRegionInvalid)?;
        let y = u32::try_from(origin.y).map_err(|_| SlintRenderError::DirtyRegionInvalid)?;
        let dirty_region =
            RefreshRegion::try_new(x, y, size.width, size.height, display.layout().visible())
                .map_err(|_| SlintRenderError::DirtyRegionInvalid)?;
        let region = if mode == RefreshMode::Full {
            RefreshRegion::full(display.layout().visible())
        } else {
            dirty_region
        };

        self.stage_region(region)?;
        let request = RefreshRequest::new(region, mode, completion);
        let marker = display
            .present_rgb8(
                target,
                request,
                self.staging_pixels.as_slice(),
                region
                    .width()
                    .try_into()
                    .expect("validated region width is non-zero"),
                conversion,
            )
            .map_err(SlintRenderError::Display)?;
        Ok(SlintPresentOutcome::Presented { region, marker })
    }

    fn stage_region(&mut self, region: RefreshRegion) -> Result<(), SlintRenderError> {
        let width =
            usize::try_from(region.width()).map_err(|_| SlintRenderError::PixelCountOverflow)?;
        let height =
            usize::try_from(region.height()).map_err(|_| SlintRenderError::PixelCountOverflow)?;
        let pixel_count = width
            .checked_mul(height)
            .ok_or(SlintRenderError::PixelCountOverflow)?;
        self.staging_pixels.clear();
        self.staging_pixels
            .try_reserve(pixel_count)
            .map_err(|_| SlintRenderError::AllocationFailed)?;
        let render_stride =
            usize::try_from(self.width.get()).map_err(|_| SlintRenderError::PixelCountOverflow)?;
        let x = usize::try_from(region.x()).map_err(|_| SlintRenderError::DirtyRegionInvalid)?;
        let y = usize::try_from(region.y()).map_err(|_| SlintRenderError::DirtyRegionInvalid)?;
        for row in 0..height {
            let start = y
                .checked_add(row)
                .and_then(|row| row.checked_mul(render_stride))
                .and_then(|start| start.checked_add(x))
                .ok_or(SlintRenderError::DirtyRegionInvalid)?;
            let end = start
                .checked_add(width)
                .ok_or(SlintRenderError::DirtyRegionInvalid)?;
            let pixels = self
                .render_pixels
                .get(start..end)
                .ok_or(SlintRenderError::DirtyRegionInvalid)?;
            self.staging_pixels
                .extend(pixels.iter().map(|pixel| Rgb8Pixel {
                    red: pixel.r,
                    green: pixel.g,
                    blue: pixel.b,
                }));
        }
        Ok(())
    }
}

/// Result of one conditional Slint frame presentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlintPresentOutcome {
    /// Slint did not request a redraw.
    Idle,
    /// Slint consumed a redraw request but reported no changed pixels.
    RedrawnWithoutPixels,
    /// One coalesced dirty region was written and submitted.
    Presented {
        /// Exact physical dirty region.
        region: RefreshRegion,
        /// Unique marker allocated by the display core.
        marker: UpdateMarker,
    },
}

/// Failure while rendering or presenting one Slint frame.
#[derive(Debug)]
#[non_exhaustive]
pub enum SlintRenderError {
    /// The visible extent cannot be represented as a host pixel count.
    PixelCountOverflow,
    /// Ordinary render or staging memory could not be reserved.
    AllocationFailed,
    /// The Slint window no longer matches the resolved visible extent.
    WindowSizeMismatch {
        /// Resolved visible width.
        expected_width: u32,
        /// Resolved visible height.
        expected_height: u32,
        /// Current Slint physical width.
        actual_width: u32,
        /// Current Slint physical height.
        actual_height: u32,
    },
    /// The adapter was not created with a persistent reused render buffer.
    RendererBufferNotReused,
    /// Slint claimed a redraw without invoking the renderer callback.
    RendererDidNotReturnRegion,
    /// Slint returned dirty geometry outside the resolved visible extent.
    DirtyRegionInvalid,
    /// Validated framebuffer writing or exact refresh submission failed.
    Display(L0DisplayError),
}

impl std::fmt::Display for SlintRenderError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PixelCountOverflow => formatter.write_str("Slint pixel count overflowed"),
            Self::AllocationFailed => formatter.write_str("Slint pixel allocation failed"),
            Self::WindowSizeMismatch {
                expected_width,
                expected_height,
                actual_width,
                actual_height,
            } => write!(
                formatter,
                "Slint window is {actual_width}x{actual_height}; expected {expected_width}x{expected_height}"
            ),
            Self::RendererBufferNotReused => {
                formatter.write_str("Slint renderer is not using a reused buffer")
            }
            Self::RendererDidNotReturnRegion => {
                formatter.write_str("Slint redraw omitted its renderer callback")
            }
            Self::DirtyRegionInvalid => {
                formatter.write_str("Slint dirty region left the resolved display")
            }
            Self::Display(error) => write!(formatter, "Slint presentation failed: {error}"),
        }
    }
}

impl std::error::Error for SlintRenderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Display(error) => Some(error),
            Self::PixelCountOverflow
            | Self::AllocationFailed
            | Self::WindowSizeMismatch { .. }
            | Self::RendererBufferNotReused
            | Self::RendererDidNotReturnRegion
            | Self::DirtyRegionInvalid => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DisplayTarget;
    use ferrink_platform::{
        DeviceProfile, DisplayPoint, DisplayUpdateAbiKind, ProbeReport, RefreshError,
        ResolvedRuntimeDevice,
    };
    use slint::ComponentHandle;
    use slint::platform::{Platform, PlatformError};

    const KOA3_REPORT: &str =
        include_str!("../../ferrink-platform/tests/fixtures/probe-reference-portrait.json");
    const KOA3_PROFILE: &str = include_str!("../../../device-profiles/reference-portrait.toml");

    slint::slint! {
        export component BridgeCard inherits Window {
            width: 1264px;
            height: 1680px;
            background: white;
            in-out property <bool> activated: false;

            Rectangle {
                x: 10px;
                y: 10px;
                width: 100px;
                height: 100px;
                background: root.activated ? black : #777777;

                TouchArea {
                    clicked => { root.activated = true; }
                }
            }
        }
    }

    struct TestPlatform {
        window: Rc<MinimalSoftwareWindow>,
    }

    impl Platform for TestPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
            Ok(self.window.clone())
        }
    }

    #[derive(Debug)]
    struct FakeTarget {
        memory: Vec<u8>,
        submitted: Vec<(DisplayUpdateAbiKind, RefreshRequest, UpdateMarker)>,
    }

    impl DisplayTarget for FakeTarget {
        fn framebuffer_memory(&mut self) -> &mut [u8] {
            self.memory.as_mut_slice()
        }

        fn submit_refresh(
            &mut self,
            update_abi: DisplayUpdateAbiKind,
            request: RefreshRequest,
            marker: UpdateMarker,
        ) -> Result<(), RefreshError> {
            self.submitted.push((update_abi, request, marker));
            Ok(())
        }
    }

    fn koa3_runtime() -> ResolvedRuntimeDevice {
        let profile = DeviceProfile::from_toml(KOA3_PROFILE).unwrap();
        let report = ProbeReport::from_json(KOA3_REPORT).unwrap();
        ResolvedRuntimeDevice::resolve(&profile, &report).unwrap()
    }

    #[test]
    fn contacts_map_to_logical_left_pointer_events() {
        let point = DisplayPoint { x: 300, y: 450 };
        let pressed = pointer_event(
            TouchContactEvent {
                phase: LogicalTouchPhase::Pressed,
                point,
            },
            1.5,
        )
        .unwrap();
        assert_eq!(
            pressed,
            WindowEvent::PointerPressed {
                position: slint::LogicalPosition::new(200.0, 300.0),
                button: PointerEventButton::Left,
            }
        );
        assert!(matches!(
            pointer_event(
                TouchContactEvent {
                    phase: LogicalTouchPhase::Moved,
                    point,
                },
                f32::NAN,
            ),
            Err(SlintPointerError::InvalidScaleFactor)
        ));
    }

    #[test]
    fn real_slint_callback_replaces_full_target_then_renders_partial_region() {
        let window = new_slint_window();
        slint::platform::set_platform(Box::new(TestPlatform {
            window: window.clone(),
        }))
        .unwrap();

        let runtime = koa3_runtime();
        let mut display = L0DisplayCore::try_from_runtime(&runtime).unwrap();
        let mut frame = SlintFrameBuffer::try_for_display(&display).unwrap();
        let mut target = FakeTarget {
            memory: vec![0x55; usize::try_from(display.layout().memory_length()).unwrap()],
            submitted: Vec::new(),
        };
        window.set_size(slint::PhysicalSize::new(1264, 1680));
        let card = BridgeCard::new().unwrap();
        card.show().unwrap();
        window.request_redraw();

        let first = frame
            .present_if_needed(
                &window,
                &mut display,
                &mut target,
                RefreshMode::Full,
                RefreshCompletionPolicy::DoNotWait,
                Gray8Conversion::Grayscale,
            )
            .unwrap();
        assert!(matches!(
            first,
            SlintPresentOutcome::Presented { region, marker }
                if region == RefreshRegion::full(display.layout().visible()) && marker.get() == 1
        ));

        let mut pointer = SlintPointerBridge::default();
        for phase in [LogicalTouchPhase::Pressed, LogicalTouchPhase::Released] {
            pointer
                .dispatch(
                    card.window(),
                    TouchContactEvent {
                        phase,
                        point: DisplayPoint { x: 20, y: 20 },
                    },
                )
                .unwrap();
        }
        assert!(card.get_activated());

        let retained_background_offset = 1_600 * 1_280 + 1_200;
        target.memory[retained_background_offset] = 0x55;
        let second = frame
            .present_if_needed(
                &window,
                &mut display,
                &mut target,
                RefreshMode::Full,
                RefreshCompletionPolicy::DoNotWait,
                Gray8Conversion::Grayscale,
            )
            .unwrap();
        assert!(matches!(
            second,
            SlintPresentOutcome::Presented { region, marker }
                if region == RefreshRegion::full(display.layout().visible()) && marker.get() == 2
        ));
        assert_eq!(target.memory[retained_background_offset], u8::MAX);
        assert_eq!(target.memory[20 * 1280 + 20], 0);

        card.set_activated(false);
        let third = frame
            .present_if_needed(
                &window,
                &mut display,
                &mut target,
                RefreshMode::Partial,
                RefreshCompletionPolicy::DoNotWait,
                Gray8Conversion::Grayscale,
            )
            .unwrap();
        let third_region = match third {
            SlintPresentOutcome::Presented { region, marker } => {
                assert_eq!(marker.get(), 3);
                region
            }
            other => panic!("callback did not produce a visible frame: {other:?}"),
        };
        assert!(third_region.pixel_count() < u64::from(1264_u32) * u64::from(1680_u32));
        assert_eq!(target.submitted.len(), 3);
        assert_eq!(target.submitted[0].0, DisplayUpdateAbiKind::Zelda88);
        assert_eq!(target.submitted[1].0, DisplayUpdateAbiKind::Zelda88);
        assert_eq!(target.submitted[1].1.mode(), RefreshMode::Full);
        assert_eq!(target.submitted[2].0, DisplayUpdateAbiKind::Zelda88);
        assert_eq!(target.submitted[2].1.mode(), RefreshMode::Partial);
        assert_eq!(
            frame
                .present_if_needed(
                    &window,
                    &mut display,
                    &mut target,
                    RefreshMode::Partial,
                    RefreshCompletionPolicy::DoNotWait,
                    Gray8Conversion::Grayscale,
                )
                .unwrap(),
            SlintPresentOutcome::Idle
        );

        pointer.stop(card.window()).unwrap();
        assert!(pointer.is_stopped());
        assert!(matches!(
            pointer.dispatch(
                card.window(),
                TouchContactEvent {
                    phase: LogicalTouchPhase::Moved,
                    point: DisplayPoint { x: 21, y: 21 },
                },
            ),
            Err(SlintPointerError::Stopped)
        ));
        card.hide().unwrap();
    }
}
