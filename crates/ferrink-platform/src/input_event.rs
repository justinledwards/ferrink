//! Bounded, allocation-aware decoding of raw Linux `input_event` byte streams.

use std::num::NonZeroU32;

use crate::{
    Endianness, InputEventAbi, InputTraceEvent, MAX_INPUT_TRACE_DURATION_MILLIS,
    MAX_INPUT_TRACE_EVENTS,
};

const MAX_INPUT_EVENT_BYTES: usize = 24;

/// One complete raw Linux input event with a validated absolute timestamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodedInputEvent {
    /// Kernel timestamp converted to microseconds without changing its clock.
    pub timestamp_micros: u64,
    /// Linux input event type.
    pub event_type: u16,
    /// Linux input event code.
    pub code: u16,
    /// Signed Linux input event value.
    pub value: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputEventRecordLayout {
    Bits32,
    Bits64,
}

impl InputEventRecordLayout {
    fn try_from_abi(abi: InputEventAbi) -> Result<Self, InputEventDecodeError> {
        match (
            abi.pointer_width_bits,
            abi.libc_timeval_bytes,
            abi.libc_input_event_bytes,
        ) {
            (32, 8, 16) => Ok(Self::Bits32),
            (64, 16, 24) => Ok(Self::Bits64),
            _ => Err(InputEventDecodeError::UnsupportedAbi { abi }),
        }
    }

    const fn record_bytes(self) -> usize {
        match self {
            Self::Bits32 => 16,
            Self::Bits64 => 24,
        }
    }

    fn decode(
        self,
        byte_order: Endianness,
        record: &[u8],
        record_index: u32,
    ) -> Result<DecodedInputEvent, InputEventDecodeError> {
        if record.len() != self.record_bytes() {
            return Err(InputEventDecodeError::InternalRecordSize {
                observed: record.len(),
                expected: self.record_bytes(),
            });
        }
        let (seconds, microseconds, fields_offset) = match self {
            Self::Bits32 => (
                i64::from(read_i32(byte_order, record, 0)?),
                i64::from(read_i32(byte_order, record, 4)?),
                8,
            ),
            Self::Bits64 => (
                read_i64(byte_order, record, 0)?,
                read_i64(byte_order, record, 8)?,
                16,
            ),
        };
        if seconds < 0 || !(0..1_000_000).contains(&microseconds) {
            return Err(InputEventDecodeError::InvalidTimestamp {
                record_index,
                seconds,
                microseconds,
            });
        }
        let timestamp_micros = u64::try_from(seconds)
            .ok()
            .and_then(|seconds| seconds.checked_mul(1_000_000))
            .and_then(|timestamp| {
                u64::try_from(microseconds)
                    .ok()
                    .and_then(|microseconds| timestamp.checked_add(microseconds))
            })
            .ok_or(InputEventDecodeError::TimestampOverflow { record_index })?;
        Ok(DecodedInputEvent {
            timestamp_micros,
            event_type: read_u16(byte_order, record, fields_offset)?,
            code: read_u16(byte_order, record, fields_offset + 2)?,
            value: read_i32(byte_order, record, fields_offset + 4)?,
        })
    }
}

/// Incrementally decodes complete Linux input records from arbitrary read chunks.
///
/// The decoder owns at most one 24-byte partial record. Every [`Self::push`]
/// call is atomic: invalid bytes or an exceeded event bound leave its prior
/// state unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputEventDecoder {
    layout: InputEventRecordLayout,
    byte_order: Endianness,
    maximum_records: NonZeroU32,
    decoded_records: u32,
    partial: [u8; MAX_INPUT_EVENT_BYTES],
    partial_len: usize,
}

impl InputEventDecoder {
    /// Validates an ABI and creates a bounded decoder.
    ///
    /// # Errors
    ///
    /// Returns [`InputEventDecodeError::UnsupportedAbi`] for a layout other
    /// than the reviewed Linux 32-bit/16-byte or 64-bit/24-byte forms, and
    /// [`InputEventDecodeError::RecordLimitAboveMaximum`] when the declared
    /// bound exceeds the characterization schema limit.
    pub fn try_new(
        abi: InputEventAbi,
        byte_order: Endianness,
        maximum_records: NonZeroU32,
    ) -> Result<Self, InputEventDecodeError> {
        let layout = InputEventRecordLayout::try_from_abi(abi)?;
        if maximum_records.get() > MAX_INPUT_TRACE_EVENTS {
            return Err(InputEventDecodeError::RecordLimitAboveMaximum {
                declared: maximum_records.get(),
                maximum: MAX_INPUT_TRACE_EVENTS,
            });
        }
        Ok(Self {
            layout,
            byte_order,
            maximum_records,
            decoded_records: 0,
            partial: [0; MAX_INPUT_EVENT_BYTES],
            partial_len: 0,
        })
    }

    /// Returns the exact raw record size selected by the validated ABI.
    #[must_use]
    pub const fn record_bytes(&self) -> usize {
        self.layout.record_bytes()
    }

    /// Returns the number of complete records emitted in the current budget.
    #[must_use]
    pub const fn decoded_records(&self) -> u32 {
        self.decoded_records
    }

    /// Starts a fresh bounded record budget without changing stream state.
    ///
    /// Long-running consumers may call this before the configured bound is
    /// exhausted. Any incomplete record bytes remain buffered so renewal can
    /// never splice, discard, or reinterpret the input stream. Bounded trace
    /// capture must not call this method.
    pub fn renew_record_budget(&mut self) -> u32 {
        let completed = self.decoded_records;
        self.decoded_records = 0;
        completed
    }

    /// Returns how many bytes are retained for an incomplete record.
    #[must_use]
    pub const fn partial_bytes(&self) -> usize {
        self.partial_len
    }

    /// Decodes every complete record in a read chunk and retains its suffix.
    ///
    /// # Errors
    ///
    /// Returns [`InputEventDecodeError`] if the chunk would exceed the declared
    /// record bound, contains an invalid timestamp, or overflows checked length
    /// arithmetic. The decoder is unchanged on error.
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<DecodedInputEvent>, InputEventDecodeError> {
        let mut staged = self.clone();
        let events = staged.push_inner(bytes)?;
        *self = staged;
        Ok(events)
    }

    /// Reports whether the stream ends on a complete record boundary.
    ///
    /// # Errors
    ///
    /// Returns [`InputEventDecodeError::TruncatedRecord`] when a final partial
    /// record remains buffered.
    pub fn finish(&self) -> Result<(), InputEventDecodeError> {
        if self.partial_len == 0 {
            Ok(())
        } else {
            Err(InputEventDecodeError::TruncatedRecord {
                observed: self.partial_len,
                expected: self.record_bytes(),
            })
        }
    }

    /// Drops bytes from an interrupted record while preserving the event count.
    ///
    /// This is intended for a reviewed device-loss/reopen boundary so bytes
    /// from two device instances can never be combined into one event.
    pub fn discard_partial_record(&mut self) -> usize {
        let discarded = self.partial_len;
        self.partial[..self.partial_len].fill(0);
        self.partial_len = 0;
        discarded
    }

    fn push_inner(
        &mut self,
        bytes: &[u8],
    ) -> Result<Vec<DecodedInputEvent>, InputEventDecodeError> {
        let record_bytes = self.record_bytes();
        let combined_bytes = self
            .partial_len
            .checked_add(bytes.len())
            .ok_or(InputEventDecodeError::InputLengthOverflow)?;
        let incoming_records = combined_bytes / record_bytes;
        let incoming_records_u32 = u32::try_from(incoming_records).map_err(|_| {
            InputEventDecodeError::RecordLimitExceeded {
                decoded: self.decoded_records,
                incoming: u32::MAX,
                maximum: self.maximum_records.get(),
            }
        })?;
        let total_records = self
            .decoded_records
            .checked_add(incoming_records_u32)
            .ok_or(InputEventDecodeError::RecordLimitExceeded {
                decoded: self.decoded_records,
                incoming: incoming_records_u32,
                maximum: self.maximum_records.get(),
            })?;
        if total_records > self.maximum_records.get() {
            return Err(InputEventDecodeError::RecordLimitExceeded {
                decoded: self.decoded_records,
                incoming: incoming_records_u32,
                maximum: self.maximum_records.get(),
            });
        }

        let mut events = Vec::with_capacity(incoming_records);
        let mut remaining = bytes;
        if self.partial_len != 0 {
            let needed = record_bytes - self.partial_len;
            let consumed = needed.min(remaining.len());
            self.partial[self.partial_len..self.partial_len + consumed]
                .copy_from_slice(&remaining[..consumed]);
            self.partial_len += consumed;
            remaining = &remaining[consumed..];
            if self.partial_len == record_bytes {
                events.push(self.layout.decode(
                    self.byte_order,
                    &self.partial[..record_bytes],
                    self.decoded_records,
                )?);
                self.partial[..record_bytes].fill(0);
                self.partial_len = 0;
            }
            if self.partial_len != 0 {
                self.decoded_records = total_records;
                return Ok(events);
            }
        }

        while remaining.len() >= record_bytes {
            let index = self
                .decoded_records
                .checked_add(u32::try_from(events.len()).map_err(|_| {
                    InputEventDecodeError::RecordLimitExceeded {
                        decoded: self.decoded_records,
                        incoming: u32::MAX,
                        maximum: self.maximum_records.get(),
                    }
                })?)
                .ok_or(InputEventDecodeError::RecordLimitExceeded {
                    decoded: self.decoded_records,
                    incoming: incoming_records_u32,
                    maximum: self.maximum_records.get(),
                })?;
            events.push(
                self.layout
                    .decode(self.byte_order, &remaining[..record_bytes], index)?,
            );
            remaining = &remaining[record_bytes..];
        }
        self.partial[..remaining.len()].copy_from_slice(remaining);
        self.partial_len = remaining.len();
        self.decoded_records = total_records;
        Ok(events)
    }
}

/// Converts absolute decoded timestamps into one bounded trace timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputEventTimeline {
    maximum_duration_micros: u64,
    first_timestamp_micros: Option<u64>,
    previous_timestamp_micros: Option<u64>,
}

impl InputEventTimeline {
    /// Creates a timeline with the same hard duration bound as input traces.
    ///
    /// # Errors
    ///
    /// Returns [`InputEventTimelineError::DurationAboveMaximum`] when the
    /// declared capture duration exceeds the characterization schema maximum.
    pub fn try_new(maximum_duration_millis: NonZeroU32) -> Result<Self, InputEventTimelineError> {
        if maximum_duration_millis.get() > MAX_INPUT_TRACE_DURATION_MILLIS {
            return Err(InputEventTimelineError::DurationAboveMaximum {
                declared_millis: maximum_duration_millis.get(),
                maximum_millis: MAX_INPUT_TRACE_DURATION_MILLIS,
            });
        }
        Ok(Self {
            maximum_duration_micros: u64::from(maximum_duration_millis.get()) * 1_000,
            first_timestamp_micros: None,
            previous_timestamp_micros: None,
        })
    }

    /// Converts one decoded event to a monotonic offset from the first event.
    ///
    /// # Errors
    ///
    /// Returns [`InputEventTimelineError`] for decreasing timestamps or an
    /// event beyond the declared capture duration. State is unchanged on error.
    pub fn normalize(
        &mut self,
        event: DecodedInputEvent,
    ) -> Result<InputTraceEvent, InputEventTimelineError> {
        if self
            .previous_timestamp_micros
            .is_some_and(|previous| event.timestamp_micros < previous)
        {
            return Err(InputEventTimelineError::DecreasingTimestamp {
                previous_micros: self.previous_timestamp_micros.unwrap_or(0),
                observed_micros: event.timestamp_micros,
            });
        }
        let first = self
            .first_timestamp_micros
            .unwrap_or(event.timestamp_micros);
        let offset_micros = event.timestamp_micros.checked_sub(first).ok_or(
            InputEventTimelineError::DecreasingTimestamp {
                previous_micros: self.previous_timestamp_micros.unwrap_or(first),
                observed_micros: event.timestamp_micros,
            },
        )?;
        if offset_micros > self.maximum_duration_micros {
            return Err(InputEventTimelineError::DurationExceeded {
                observed_micros: offset_micros,
                maximum_micros: self.maximum_duration_micros,
            });
        }
        self.first_timestamp_micros = Some(first);
        self.previous_timestamp_micros = Some(event.timestamp_micros);
        Ok(InputTraceEvent {
            offset_micros,
            event_type: event.event_type,
            code: event.code,
            value: event.value,
        })
    }

    /// Clears the clock origin for a new bounded capture.
    pub fn reset(&mut self) {
        self.first_timestamp_micros = None;
        self.previous_timestamp_micros = None;
    }
}

/// A rejected raw input-event stream or record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum InputEventDecodeError {
    /// The declared C ABI was not one of the two reviewed Linux layouts.
    UnsupportedAbi { abi: InputEventAbi },
    /// The record limit exceeded the hard trace bound.
    RecordLimitAboveMaximum { declared: u32, maximum: u32 },
    /// A read chunk would exceed the declared record limit.
    RecordLimitExceeded {
        decoded: u32,
        incoming: u32,
        maximum: u32,
    },
    /// Combining a chunk and partial record overflowed `usize`.
    InputLengthOverflow,
    /// A complete record contained a negative or invalid `timeval`.
    InvalidTimestamp {
        record_index: u32,
        seconds: i64,
        microseconds: i64,
    },
    /// Converting a valid `timeval` to microseconds overflowed.
    TimestampOverflow { record_index: u32 },
    /// The stream ended with only part of a record.
    TruncatedRecord { observed: usize, expected: usize },
    /// An internal caller supplied a slice of the wrong record size.
    InternalRecordSize { observed: usize, expected: usize },
}

impl std::fmt::Display for InputEventDecodeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedAbi { abi } => write!(
                formatter,
                "unsupported input_event ABI: pointer {}, timeval {}, record {} bytes",
                abi.pointer_width_bits, abi.libc_timeval_bytes, abi.libc_input_event_bytes
            ),
            Self::RecordLimitAboveMaximum { declared, maximum } => {
                write!(formatter, "input record limit {declared} exceeds {maximum}")
            }
            Self::RecordLimitExceeded {
                decoded,
                incoming,
                maximum,
            } => write!(
                formatter,
                "{decoded} decoded plus {incoming} incoming records exceeds {maximum}"
            ),
            Self::InputLengthOverflow => formatter.write_str("input chunk length overflow"),
            Self::InvalidTimestamp {
                record_index,
                seconds,
                microseconds,
            } => write!(
                formatter,
                "input record {record_index} has invalid timestamp {seconds}.{microseconds}"
            ),
            Self::TimestampOverflow { record_index } => {
                write!(formatter, "input record {record_index} timestamp overflow")
            }
            Self::TruncatedRecord { observed, expected } => write!(
                formatter,
                "input stream ended with {observed} of {expected} record bytes"
            ),
            Self::InternalRecordSize { observed, expected } => write!(
                formatter,
                "input decoder received {observed} bytes for a {expected}-byte record"
            ),
        }
    }
}

impl std::error::Error for InputEventDecodeError {}

/// A decoded event that cannot belong to the bounded trace timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum InputEventTimelineError {
    /// The declared duration exceeded the hard trace bound.
    DurationAboveMaximum {
        declared_millis: u32,
        maximum_millis: u32,
    },
    /// An event timestamp moved backwards.
    DecreasingTimestamp {
        previous_micros: u64,
        observed_micros: u64,
    },
    /// An event occurred after the declared capture duration.
    DurationExceeded {
        observed_micros: u64,
        maximum_micros: u64,
    },
}

impl std::fmt::Display for InputEventTimelineError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DurationAboveMaximum {
                declared_millis,
                maximum_millis,
            } => write!(
                formatter,
                "input duration {declared_millis} ms exceeds {maximum_millis} ms"
            ),
            Self::DecreasingTimestamp {
                previous_micros,
                observed_micros,
            } => write!(
                formatter,
                "input timestamp {observed_micros} precedes {previous_micros}"
            ),
            Self::DurationExceeded {
                observed_micros,
                maximum_micros,
            } => write!(
                formatter,
                "input offset {observed_micros} us exceeds {maximum_micros} us"
            ),
        }
    }
}

impl std::error::Error for InputEventTimelineError {}

fn read_u16(
    byte_order: Endianness,
    record: &[u8],
    offset: usize,
) -> Result<u16, InputEventDecodeError> {
    let bytes = read_array::<2>(record, offset)?;
    Ok(match byte_order {
        Endianness::Little => u16::from_le_bytes(bytes),
        Endianness::Big => u16::from_be_bytes(bytes),
    })
}

fn read_i32(
    byte_order: Endianness,
    record: &[u8],
    offset: usize,
) -> Result<i32, InputEventDecodeError> {
    let bytes = read_array::<4>(record, offset)?;
    Ok(match byte_order {
        Endianness::Little => i32::from_le_bytes(bytes),
        Endianness::Big => i32::from_be_bytes(bytes),
    })
}

fn read_i64(
    byte_order: Endianness,
    record: &[u8],
    offset: usize,
) -> Result<i64, InputEventDecodeError> {
    let bytes = read_array::<8>(record, offset)?;
    Ok(match byte_order {
        Endianness::Little => i64::from_le_bytes(bytes),
        Endianness::Big => i64::from_be_bytes(bytes),
    })
}

fn read_array<const N: usize>(
    record: &[u8],
    offset: usize,
) -> Result<[u8; N], InputEventDecodeError> {
    let end = offset
        .checked_add(N)
        .ok_or(InputEventDecodeError::InternalRecordSize {
            observed: record.len(),
            expected: usize::MAX,
        })?;
    record
        .get(offset..end)
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or(InputEventDecodeError::InternalRecordSize {
            observed: record.len(),
            expected: end,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn abi32() -> InputEventAbi {
        InputEventAbi {
            pointer_width_bits: 32,
            libc_timeval_bytes: 8,
            libc_input_event_bytes: 16,
        }
    }

    fn abi64() -> InputEventAbi {
        InputEventAbi {
            pointer_width_bits: 64,
            libc_timeval_bytes: 16,
            libc_input_event_bytes: 24,
        }
    }

    fn non_zero(value: u32) -> NonZeroU32 {
        NonZeroU32::new(value).unwrap()
    }

    fn record32(seconds: i32, micros: i32, event_type: u16, code: u16, value: i32) -> [u8; 16] {
        let mut record = [0; 16];
        record[0..4].copy_from_slice(&seconds.to_le_bytes());
        record[4..8].copy_from_slice(&micros.to_le_bytes());
        record[8..10].copy_from_slice(&event_type.to_le_bytes());
        record[10..12].copy_from_slice(&code.to_le_bytes());
        record[12..16].copy_from_slice(&value.to_le_bytes());
        record
    }

    fn record64(seconds: i64, micros: i64, event_type: u16, code: u16, value: i32) -> [u8; 24] {
        let mut record = [0; 24];
        record[0..8].copy_from_slice(&seconds.to_be_bytes());
        record[8..16].copy_from_slice(&micros.to_be_bytes());
        record[16..18].copy_from_slice(&event_type.to_be_bytes());
        record[18..20].copy_from_slice(&code.to_be_bytes());
        record[20..24].copy_from_slice(&value.to_be_bytes());
        record
    }

    #[test]
    fn decoder_rejects_unreviewed_abi_shapes_and_limits() {
        let mut invalid = abi32();
        invalid.libc_input_event_bytes = 24;
        assert!(matches!(
            InputEventDecoder::try_new(invalid, Endianness::Little, non_zero(1)),
            Err(InputEventDecodeError::UnsupportedAbi { .. })
        ));
        assert!(matches!(
            InputEventDecoder::try_new(
                abi32(),
                Endianness::Little,
                non_zero(MAX_INPUT_TRACE_EVENTS + 1)
            ),
            Err(InputEventDecodeError::RecordLimitAboveMaximum { .. })
        ));
    }

    #[test]
    fn thirty_two_bit_records_survive_arbitrary_split_reads() {
        let first = record32(10, 20, 3, 53, 400);
        let second = record32(10, 30, 0, 0, 0);
        let mut combined = Vec::from(first);
        combined.extend_from_slice(&second);
        let mut decoder =
            InputEventDecoder::try_new(abi32(), Endianness::Little, non_zero(2)).unwrap();

        assert!(decoder.push(&combined[..5]).unwrap().is_empty());
        assert_eq!(decoder.partial_bytes(), 5);
        let decoded = decoder.push(&combined[5..19]).unwrap();
        assert_eq!(
            decoded,
            vec![DecodedInputEvent {
                timestamp_micros: 10_000_020,
                event_type: 3,
                code: 53,
                value: 400,
            }]
        );
        assert_eq!(decoder.partial_bytes(), 3);
        assert_eq!(decoder.push(&combined[19..]).unwrap().len(), 1);
        assert_eq!(decoder.decoded_records(), 2);
        decoder.finish().unwrap();
    }

    #[test]
    fn sixty_four_bit_big_endian_records_decode_without_native_layout_assumptions() {
        let record = record64(2, 500_000, 1, 330, -1);
        let mut decoder =
            InputEventDecoder::try_new(abi64(), Endianness::Big, non_zero(1)).unwrap();
        assert_eq!(decoder.record_bytes(), 24);
        assert_eq!(
            decoder.push(&record).unwrap(),
            vec![DecodedInputEvent {
                timestamp_micros: 2_500_000,
                event_type: 1,
                code: 330,
                value: -1,
            }]
        );
    }

    #[test]
    fn invalid_timestamp_and_record_limit_fail_atomically() {
        let invalid = record32(1, 1_000_000, 0, 0, 0);
        let valid = record32(1, 999_999, 0, 0, 0);
        let mut decoder =
            InputEventDecoder::try_new(abi32(), Endianness::Little, non_zero(1)).unwrap();
        let before = decoder.clone();
        assert!(matches!(
            decoder.push(&invalid),
            Err(InputEventDecodeError::InvalidTimestamp { .. })
        ));
        assert_eq!(decoder, before);
        assert_eq!(decoder.push(&valid).unwrap().len(), 1);

        let before = decoder.clone();
        assert!(matches!(
            decoder.push(&valid),
            Err(InputEventDecodeError::RecordLimitExceeded { .. })
        ));
        assert_eq!(decoder, before);
    }

    #[test]
    fn truncated_records_are_reported_and_can_be_discarded_at_reopen() {
        let record = record32(1, 2, 3, 54, 500);
        let mut decoder =
            InputEventDecoder::try_new(abi32(), Endianness::Little, non_zero(2)).unwrap();
        decoder.push(&record[..7]).unwrap();
        assert_eq!(
            decoder.finish(),
            Err(InputEventDecodeError::TruncatedRecord {
                observed: 7,
                expected: 16,
            })
        );
        assert_eq!(decoder.discard_partial_record(), 7);
        assert_eq!(decoder.discard_partial_record(), 0);
        decoder.finish().unwrap();
        assert_eq!(decoder.push(&record).unwrap().len(), 1);
    }

    #[test]
    fn record_budget_renewal_preserves_partial_stream_bytes() {
        let first = record32(1, 2, 3, 54, 500);
        let second = record32(1, 3, 0, 0, 0);
        let mut decoder =
            InputEventDecoder::try_new(abi32(), Endianness::Little, non_zero(1)).unwrap();

        let mut chunk = Vec::from(first);
        chunk.extend_from_slice(&second[..7]);
        assert_eq!(decoder.push(&chunk).unwrap().len(), 1);
        assert_eq!(decoder.decoded_records(), 1);
        assert_eq!(decoder.partial_bytes(), 7);

        assert_eq!(decoder.renew_record_budget(), 1);
        assert_eq!(decoder.decoded_records(), 0);
        assert_eq!(decoder.partial_bytes(), 7);
        assert_eq!(decoder.push(&second[7..]).unwrap().len(), 1);
        assert_eq!(decoder.decoded_records(), 1);
        decoder.finish().unwrap();
    }

    #[test]
    fn timeline_normalizes_offsets_and_rejects_decrease_or_excess_duration() {
        assert_eq!(
            InputEventTimeline::try_new(non_zero(MAX_INPUT_TRACE_DURATION_MILLIS + 1)),
            Err(InputEventTimelineError::DurationAboveMaximum {
                declared_millis: MAX_INPUT_TRACE_DURATION_MILLIS + 1,
                maximum_millis: MAX_INPUT_TRACE_DURATION_MILLIS,
            })
        );
        let mut timeline = InputEventTimeline::try_new(non_zero(10)).unwrap();
        let event = DecodedInputEvent {
            timestamp_micros: 1_000_000,
            event_type: 3,
            code: 53,
            value: 10,
        };
        assert_eq!(timeline.normalize(event).unwrap().offset_micros, 0);
        assert_eq!(
            timeline
                .normalize(DecodedInputEvent {
                    timestamp_micros: 1_010_000,
                    ..event
                })
                .unwrap()
                .offset_micros,
            10_000
        );

        let before = timeline;
        assert!(matches!(
            timeline.normalize(DecodedInputEvent {
                timestamp_micros: 1_009_999,
                ..event
            }),
            Err(InputEventTimelineError::DecreasingTimestamp { .. })
        ));
        assert_eq!(timeline, before);
        assert!(matches!(
            timeline.normalize(DecodedInputEvent {
                timestamp_micros: 1_010_001,
                ..event
            }),
            Err(InputEventTimelineError::DurationExceeded { .. })
        ));
        assert_eq!(timeline, before);

        timeline.reset();
        assert_eq!(timeline.normalize(event).unwrap().offset_micros, 0);
    }
}
