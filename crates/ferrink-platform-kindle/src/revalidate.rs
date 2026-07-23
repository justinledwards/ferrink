//! Typed read-only descriptor revalidation before any active device operation.

#![deny(unsafe_code)]

use std::num::NonZeroI32;

use ferrink_platform::{
    FramebufferCapability, InputAxisCapability, InputDeviceCapability, InputDeviceId,
    ResolvedRuntimeDevice,
};

/// One read-only operation in the descriptor revalidation sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ReadOnlyOperation {
    /// Open the exact resolved framebuffer path without write access.
    OpenFramebuffer,
    /// Query fixed and variable metadata from the opened framebuffer.
    QueryFramebuffer,
    /// Open the exact resolved input path read-only and nonblocking.
    OpenInput,
    /// Query identity and advertised axes from the opened input descriptor.
    QueryInput,
}

impl std::fmt::Display for ReadOnlyOperation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OpenFramebuffer => formatter.write_str("open framebuffer read-only"),
            Self::QueryFramebuffer => formatter.write_str("query framebuffer metadata"),
            Self::OpenInput => formatter.write_str("open input read-only and nonblocking"),
            Self::QueryInput => formatter.write_str("query input metadata"),
        }
    }
}

/// Sanitized operating-system failure during read-only revalidation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadOnlyIoError {
    operation: ReadOnlyOperation,
    errno: Option<NonZeroI32>,
}

impl ReadOnlyIoError {
    /// Creates an error without retaining a device path or other diagnostic text.
    #[must_use]
    pub const fn new(operation: ReadOnlyOperation, errno: Option<NonZeroI32>) -> Self {
        Self { operation, errno }
    }

    /// Returns the failed read-only operation.
    #[must_use]
    pub const fn operation(self) -> ReadOnlyOperation {
        self.operation
    }

    /// Returns the positive OS error number when one was available.
    #[must_use]
    pub const fn errno(self) -> Option<NonZeroI32> {
        self.errno
    }
}

impl std::fmt::Display for ReadOnlyIoError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{} failed", self.operation)?;
        if let Some(errno) = self.errno {
            write!(formatter, " with errno {}", errno.get())?;
        }
        Ok(())
    }
}

impl std::error::Error for ReadOnlyIoError {}

/// Read-only framebuffer descriptor capable of repeating passive metadata queries.
pub trait ReadOnlyFramebuffer {
    /// Repeats the passive fixed/variable framebuffer capability query.
    ///
    /// # Errors
    ///
    /// Returns a sanitized I/O error without reading or changing framebuffer pixels.
    fn query_capability(&mut self) -> Result<FramebufferCapability, ReadOnlyIoError>;
}

/// Exact input identity and axes that can be re-queried from an open descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadOnlyInputSnapshot {
    device: String,
    name: Option<String>,
    id: InputDeviceId,
    axes: Vec<InputAxisCapability>,
}

impl ReadOnlyInputSnapshot {
    /// Constructs a snapshot from descriptor-queryable fields.
    #[must_use]
    pub fn new(
        device: String,
        name: Option<String>,
        id: InputDeviceId,
        axes: Vec<InputAxisCapability>,
    ) -> Self {
        Self {
            device,
            name,
            id,
            axes,
        }
    }

    /// Selects every descriptor-queryable field from a passive capability.
    #[must_use]
    pub fn from_capability(capability: &InputDeviceCapability) -> Self {
        Self::new(
            capability.device.clone(),
            capability.name.clone(),
            capability.id,
            capability.axes.clone(),
        )
    }

    /// Returns the exact numbered input path.
    #[must_use]
    pub fn device(&self) -> &str {
        &self.device
    }

    /// Returns the kernel input name.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Returns the descriptor-reported input identifier.
    #[must_use]
    pub const fn id(&self) -> InputDeviceId {
        self.id
    }

    /// Returns every absolute axis advertised by the passive snapshot.
    #[must_use]
    pub fn axes(&self) -> &[InputAxisCapability] {
        &self.axes
    }
}

/// Read-only nonblocking input descriptor capable of repeating passive queries.
pub trait ReadOnlyInput {
    /// Repeats the passive input identity, capability, and axis queries.
    ///
    /// # Errors
    ///
    /// Returns a sanitized I/O error without reading input events or requesting
    /// exclusive ownership.
    fn query_snapshot(
        &mut self,
        expected: &ReadOnlyInputSnapshot,
    ) -> Result<ReadOnlyInputSnapshot, ReadOnlyIoError>;
}

/// Factory that opens only the exact paths already selected by runtime resolution.
pub trait ReadOnlyDeviceIo {
    /// Owned framebuffer descriptor type.
    type Framebuffer: ReadOnlyFramebuffer;
    /// Owned input descriptor type.
    type Input: ReadOnlyInput;

    /// Opens the exact framebuffer path read-only.
    ///
    /// # Errors
    ///
    /// Returns a sanitized open failure. Implementations must not substitute a
    /// different numbered device.
    fn open_framebuffer_read_only(
        &mut self,
        path: &str,
    ) -> Result<Self::Framebuffer, ReadOnlyIoError>;

    /// Opens the exact input path read-only and nonblocking.
    ///
    /// # Errors
    ///
    /// Returns a sanitized open failure. Implementations must not substitute a
    /// different numbered device or read an event during construction.
    fn open_input_read_only_nonblocking(
        &mut self,
        path: &str,
    ) -> Result<Self::Input, ReadOnlyIoError>;
}

/// Two exact descriptors that passed fresh metadata comparison.
///
/// Fields are intentionally private. Input is declared first so normal struct
/// destruction closes it before the framebuffer, reversing acquisition order.
#[derive(Debug)]
pub struct RevalidatedReadOnlySession<I, F> {
    input: I,
    framebuffer: F,
}

impl<I, F> RevalidatedReadOnlySession<I, F> {
    #[cfg(test)]
    pub(crate) fn from_parts(input: I, framebuffer: F) -> Self {
        Self { input, framebuffer }
    }

    pub(crate) fn input_mut(&mut self) -> &mut I {
        &mut self.input
    }

    #[allow(dead_code)]
    pub(crate) fn framebuffer_mut(&mut self) -> &mut F {
        &mut self.framebuffer
    }
}

/// Opens and revalidates the exact resolved framebuffer and input descriptors.
///
/// The framebuffer is opened and compared before the input is opened. Any
/// mismatch or I/O failure drops every descriptor already acquired. Success
/// retains both descriptors to prevent a path-reopen race before later,
/// separately authorized transitions.
///
/// # Errors
///
/// Returns [`ReadOnlyRevalidationError`] on any I/O failure or exact capability
/// mismatch. No active operation is attempted after an error.
pub fn revalidate_read_only<T: ReadOnlyDeviceIo>(
    runtime: &ResolvedRuntimeDevice,
    io: &mut T,
) -> Result<RevalidatedReadOnlySession<T::Input, T::Framebuffer>, ReadOnlyRevalidationError> {
    let mut framebuffer = io
        .open_framebuffer_read_only(runtime.framebuffer_path())
        .map_err(ReadOnlyRevalidationError::Io)?;
    let observed_framebuffer = framebuffer
        .query_capability()
        .map_err(ReadOnlyRevalidationError::Io)?;
    if &observed_framebuffer != runtime.framebuffer_capability() {
        return Err(ReadOnlyRevalidationError::FramebufferMismatch);
    }

    let mut input = io
        .open_input_read_only_nonblocking(runtime.input_path())
        .map_err(ReadOnlyRevalidationError::Io)?;
    let expected_input = ReadOnlyInputSnapshot::from_capability(runtime.input_capability());
    let observed_input = input
        .query_snapshot(&expected_input)
        .map_err(ReadOnlyRevalidationError::Io)?;
    if observed_input != expected_input {
        return Err(ReadOnlyRevalidationError::InputMismatch);
    }

    Ok(RevalidatedReadOnlySession { input, framebuffer })
}

/// Failure while revalidating fresh read-only descriptors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ReadOnlyRevalidationError {
    /// An exact read-only open or metadata query failed.
    Io(ReadOnlyIoError),
    /// Fresh framebuffer metadata differed from the selected passive snapshot.
    FramebufferMismatch,
    /// Fresh input metadata differed from the selected passive snapshot.
    InputMismatch,
}

impl std::fmt::Display for ReadOnlyRevalidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "read-only revalidation I/O failed: {error}"),
            Self::FramebufferMismatch => {
                formatter.write_str("fresh framebuffer metadata did not match the resolution")
            }
            Self::InputMismatch => {
                formatter.write_str("fresh input metadata did not match the resolution")
            }
        }
    }
}

impl std::error::Error for ReadOnlyRevalidationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::FramebufferMismatch | Self::InputMismatch => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrink_platform::{DeviceProfile, ProbeReport};
    use std::cell::RefCell;
    use std::rc::Rc;

    const KOA3_REPORT: &str =
        include_str!("../../ferrink-platform/tests/fixtures/probe-reference-portrait.json");
    const KOA3_PROFILE: &str = include_str!("../../../device-profiles/reference-portrait.toml");

    #[derive(Debug)]
    struct FakeFramebuffer {
        capability: FramebufferCapability,
        log: Rc<RefCell<Vec<&'static str>>>,
    }

    impl ReadOnlyFramebuffer for FakeFramebuffer {
        fn query_capability(&mut self) -> Result<FramebufferCapability, ReadOnlyIoError> {
            self.log.borrow_mut().push("query framebuffer");
            Ok(self.capability.clone())
        }
    }

    impl Drop for FakeFramebuffer {
        fn drop(&mut self) {
            self.log.borrow_mut().push("drop framebuffer");
        }
    }

    #[derive(Debug)]
    struct FakeInput {
        capability: InputDeviceCapability,
        log: Rc<RefCell<Vec<&'static str>>>,
    }

    impl ReadOnlyInput for FakeInput {
        fn query_snapshot(
            &mut self,
            _expected: &ReadOnlyInputSnapshot,
        ) -> Result<ReadOnlyInputSnapshot, ReadOnlyIoError> {
            self.log.borrow_mut().push("query input");
            Ok(ReadOnlyInputSnapshot::from_capability(&self.capability))
        }
    }

    impl Drop for FakeInput {
        fn drop(&mut self) {
            self.log.borrow_mut().push("drop input");
        }
    }

    struct FakeIo {
        framebuffer: FramebufferCapability,
        input: InputDeviceCapability,
        log: Rc<RefCell<Vec<&'static str>>>,
    }

    impl ReadOnlyDeviceIo for FakeIo {
        type Framebuffer = FakeFramebuffer;
        type Input = FakeInput;

        fn open_framebuffer_read_only(
            &mut self,
            path: &str,
        ) -> Result<Self::Framebuffer, ReadOnlyIoError> {
            assert_eq!(path, self.framebuffer.device);
            self.log.borrow_mut().push("open framebuffer");
            Ok(FakeFramebuffer {
                capability: self.framebuffer.clone(),
                log: self.log.clone(),
            })
        }

        fn open_input_read_only_nonblocking(
            &mut self,
            path: &str,
        ) -> Result<Self::Input, ReadOnlyIoError> {
            assert_eq!(path, self.input.device);
            self.log.borrow_mut().push("open input");
            Ok(FakeInput {
                capability: self.input.clone(),
                log: self.log.clone(),
            })
        }
    }

    fn runtime() -> ResolvedRuntimeDevice {
        let profile = DeviceProfile::from_toml(KOA3_PROFILE).unwrap();
        let report = ProbeReport::from_json(KOA3_REPORT).unwrap();
        ResolvedRuntimeDevice::resolve(&profile, &report).unwrap()
    }

    fn fake_io(runtime: &ResolvedRuntimeDevice) -> FakeIo {
        FakeIo {
            framebuffer: runtime.framebuffer_capability().clone(),
            input: runtime.input_capability().clone(),
            log: Rc::new(RefCell::new(Vec::new())),
        }
    }

    #[test]
    fn exact_metadata_is_held_and_dropped_in_reverse_acquisition_order() {
        let runtime = runtime();
        let mut io = fake_io(&runtime);
        let log = io.log.clone();

        let session = revalidate_read_only(&runtime, &mut io).unwrap();
        assert_eq!(
            *log.borrow(),
            [
                "open framebuffer",
                "query framebuffer",
                "open input",
                "query input"
            ]
        );
        drop(session);
        assert_eq!(
            *log.borrow(),
            [
                "open framebuffer",
                "query framebuffer",
                "open input",
                "query input",
                "drop input",
                "drop framebuffer"
            ]
        );
    }

    #[test]
    fn framebuffer_mismatch_stops_before_input_open() {
        let runtime = runtime();
        let mut io = fake_io(&runtime);
        io.framebuffer.line_length += 1;
        let log = io.log.clone();

        assert!(matches!(
            revalidate_read_only(&runtime, &mut io),
            Err(ReadOnlyRevalidationError::FramebufferMismatch)
        ));
        assert_eq!(
            *log.borrow(),
            ["open framebuffer", "query framebuffer", "drop framebuffer"]
        );
    }

    #[test]
    fn input_mismatch_closes_input_then_framebuffer() {
        let runtime = runtime();
        let mut io = fake_io(&runtime);
        io.input.name = Some("unexpected_touch".to_owned());
        let log = io.log.clone();

        assert!(matches!(
            revalidate_read_only(&runtime, &mut io),
            Err(ReadOnlyRevalidationError::InputMismatch)
        ));
        assert_eq!(
            *log.borrow(),
            [
                "open framebuffer",
                "query framebuffer",
                "open input",
                "query input",
                "drop input",
                "drop framebuffer"
            ]
        );
    }
}
