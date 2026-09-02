use core::fmt;

#[cfg(feature = "alloc")]
use alloc::{boxed::Box, vec::Vec};

use capnp_wire::{WORD_BYTES, read_u32_le};

/// The reference implementation's hard segment-count limit.
pub const MAX_SEGMENTS: u32 = 512;

/// Independent allocation/message-size limits for standard framing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameLimits {
    pub max_segments: u32,
    pub max_total_words: u64,
}

impl Default for FrameLimits {
    fn default() -> Self {
        Self {
            max_segments: MAX_SEGMENTS,
            max_total_words: 8 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FrameError {
    TruncatedHeader { available: usize },
    SegmentCountOverflow,
    TooManySegments { count: u32, limit: u32 },
    TruncatedSegmentTable { expected: usize, available: usize },
    TotalWordsOverflow,
    MessageTooLarge { words: u64, limit: u64 },
    BodyLengthOverflow,
    TruncatedBody { expected: usize, available: usize },
    NoSegments,
    SegmentNotWordAligned { index: usize, bytes: usize },
    SegmentTooLarge { index: usize, words: u64 },
    SegmentBufferTooSmall { required: usize, available: usize },
    TrailingData { bytes: usize },
}

impl fmt::Display for FrameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::TruncatedHeader { available } => {
                write!(formatter, "truncated frame header: {available} of 4 bytes")
            }
            Self::SegmentCountOverflow => formatter.write_str("encoded segment count overflows"),
            Self::TooManySegments { count, limit } => {
                write!(formatter, "frame has {count} segments; limit is {limit}")
            }
            Self::TruncatedSegmentTable {
                expected,
                available,
            } => write!(
                formatter,
                "truncated segment table: {available} of {expected} bytes"
            ),
            Self::TotalWordsOverflow => formatter.write_str("total segment words overflow"),
            Self::MessageTooLarge { words, limit } => {
                write!(formatter, "frame has {words} words; limit is {limit}")
            }
            Self::BodyLengthOverflow => formatter.write_str("frame body byte length overflows"),
            Self::TruncatedBody {
                expected,
                available,
            } => {
                write!(
                    formatter,
                    "truncated frame body: {available} of {expected} bytes"
                )
            }
            Self::NoSegments => formatter.write_str("a framed message needs at least one segment"),
            Self::SegmentNotWordAligned { index, bytes } => write!(
                formatter,
                "segment {index} has {bytes} bytes, which is not word-aligned"
            ),
            Self::SegmentTooLarge { index, words } => {
                write!(
                    formatter,
                    "segment {index} has {words} words; maximum is u32::MAX"
                )
            }
            Self::SegmentBufferTooSmall {
                required,
                available,
            } => write!(
                formatter,
                "frame needs {required} segment slots but only {available} were provided"
            ),
            Self::TrailingData { bytes } => {
                write!(formatter, "{bytes} trailing bytes follow the frame")
            }
        }
    }
}

impl core::error::Error for FrameError {}

/// Immutable location and contents of one segment within a parsed frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Segment<'a> {
    id: u32,
    word_count: u32,
    bytes: &'a [u8],
}

impl<'a> Segment<'a> {
    pub const EMPTY: Self = Self {
        id: 0,
        word_count: 0,
        bytes: &[],
    };

    pub const fn id(self) -> u32 {
        self.id
    }

    pub const fn word_count(self) -> u32 {
        self.word_count
    }

    pub const fn bytes(self) -> &'a [u8] {
        self.bytes
    }
}

/// A standard frame whose segment descriptors occupy caller-provided storage.
#[derive(Debug, Eq, PartialEq)]
pub struct BorrowedFrame<'input, 'storage> {
    segments: &'storage [Segment<'input>],
    table_len: usize,
    encoded_len: usize,
}

impl<'input, 'storage> BorrowedFrame<'input, 'storage> {
    pub const fn segments(&self) -> &'storage [Segment<'input>] {
        self.segments
    }

    pub fn segment(&self, id: u32) -> Option<Segment<'input>> {
        usize::try_from(id)
            .ok()
            .and_then(|index| self.segments.get(index).copied())
    }

    pub const fn table_len(&self) -> usize {
        self.table_len
    }

    pub const fn encoded_len(&self) -> usize {
        self.encoded_len
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum BorrowedFrameRead<'input, 'storage> {
    EndOfInput,
    Message {
        frame: BorrowedFrame<'input, 'storage>,
        remaining: &'input [u8],
    },
}

/// Parses one standard frame without allocation, writing only segment
/// descriptors into `storage`; every segment continues to borrow `input`.
///
/// ```
/// use capnp_io::{BorrowedFrameRead, FrameLimits, Segment, parse_frame_into};
/// let bytes = [0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
/// let mut slots = [Segment::EMPTY; 1];
/// let BorrowedFrameRead::Message { frame, remaining } =
///     parse_frame_into(&bytes, FrameLimits::default(), &mut slots)? else {
///         unreachable!();
///     };
/// assert_eq!(frame.segments()[0].bytes().as_ptr(), bytes[8..].as_ptr());
/// assert!(remaining.is_empty());
/// # Ok::<(), capnp_io::FrameError>(())
/// ```
pub fn parse_frame_into<'input, 'storage>(
    input: &'input [u8],
    limits: FrameLimits,
    storage: &'storage mut [Segment<'input>],
) -> Result<BorrowedFrameRead<'input, 'storage>, FrameError> {
    if input.is_empty() {
        return Ok(BorrowedFrameRead::EndOfInput);
    }
    if input.len() < 4 {
        return Err(FrameError::TruncatedHeader {
            available: input.len(),
        });
    }
    let encoded_count = read_u32_le(input, 0).expect("four-byte header was checked");
    let segment_count = encoded_count
        .checked_add(1)
        .ok_or(FrameError::SegmentCountOverflow)?;
    check_segment_count(segment_count, limits)?;
    let count = usize::try_from(segment_count).map_err(|_| FrameError::BodyLengthOverflow)?;
    if count > storage.len() {
        return Err(FrameError::SegmentBufferTooSmall {
            required: count,
            available: storage.len(),
        });
    }
    let table_len = (count / 2 + 1)
        .checked_mul(8)
        .ok_or(FrameError::BodyLengthOverflow)?;
    if input.len() < table_len {
        return Err(FrameError::TruncatedSegmentTable {
            expected: table_len,
            available: input.len(),
        });
    }

    let mut total_words = 0_u64;
    for index in 0..count {
        let table_offset = 4_usize
            .checked_mul(index + 1)
            .ok_or(FrameError::BodyLengthOverflow)?;
        let words = read_u32_le(input, table_offset).expect("complete table was checked");
        total_words = total_words
            .checked_add(u64::from(words))
            .ok_or(FrameError::TotalWordsOverflow)?;
        if total_words > limits.max_total_words {
            return Err(FrameError::MessageTooLarge {
                words: total_words,
                limit: limits.max_total_words,
            });
        }
    }
    let body_len = usize::try_from(
        total_words
            .checked_mul(WORD_BYTES as u64)
            .ok_or(FrameError::BodyLengthOverflow)?,
    )
    .map_err(|_| FrameError::BodyLengthOverflow)?;
    let encoded_len = table_len
        .checked_add(body_len)
        .ok_or(FrameError::BodyLengthOverflow)?;
    if input.len() < encoded_len {
        return Err(FrameError::TruncatedBody {
            expected: body_len,
            available: input.len() - table_len,
        });
    }

    let mut body_offset = table_len;
    for (index, slot) in storage[..count].iter_mut().enumerate() {
        let table_offset = 4 * (index + 1);
        let word_count = read_u32_le(input, table_offset).expect("complete table was checked");
        let byte_len = usize::try_from(u64::from(word_count) * WORD_BYTES as u64)
            .map_err(|_| FrameError::BodyLengthOverflow)?;
        let end = body_offset
            .checked_add(byte_len)
            .ok_or(FrameError::BodyLengthOverflow)?;
        *slot = Segment {
            id: u32::try_from(index).map_err(|_| FrameError::BodyLengthOverflow)?,
            word_count,
            bytes: &input[body_offset..end],
        };
        body_offset = end;
    }
    Ok(BorrowedFrameRead::Message {
        frame: BorrowedFrame {
            segments: &storage[..count],
            table_len,
            encoded_len,
        },
        remaining: &input[encoded_len..],
    })
}

/// A complete standard frame borrowing immutable segment bodies from its input.
#[cfg(feature = "alloc")]
#[derive(Debug, Eq, PartialEq)]
pub struct Frame<'a> {
    segments: Box<[Segment<'a>]>,
    table_len: usize,
    encoded_len: usize,
}

#[cfg(feature = "alloc")]
impl<'a> Frame<'a> {
    pub fn segments(&self) -> &[Segment<'a>] {
        &self.segments
    }

    pub fn segment(&self, id: u32) -> Option<Segment<'a>> {
        usize::try_from(id)
            .ok()
            .and_then(|index| self.segments.get(index).copied())
    }

    pub const fn table_len(&self) -> usize {
        self.table_len
    }

    pub const fn encoded_len(&self) -> usize {
        self.encoded_len
    }
}

/// Distinguishes a clean stream boundary from a malformed partial frame.
#[cfg(feature = "alloc")]
#[derive(Debug, Eq, PartialEq)]
pub enum FrameRead<'a> {
    EndOfInput,
    Message {
        frame: Frame<'a>,
        remaining: &'a [u8],
    },
}

/// Segment descriptors whose alignment, word counts, and aggregate limits have
/// been validated once for repeated standard-frame encoding.
#[cfg(feature = "alloc")]
#[derive(Debug, Eq, PartialEq)]
pub struct PreparedSegments<'a> {
    segments: Box<[PreparedSegment<'a>]>,
    table_len: usize,
    encoded_len: usize,
}

#[cfg(feature = "alloc")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PreparedSegment<'a> {
    bytes: &'a [u8],
    word_count: u32,
}

#[cfg(feature = "alloc")]
impl<'a> PreparedSegments<'a> {
    pub fn new(segments: &[&'a [u8]], limits: FrameLimits) -> Result<Self, FrameError> {
        let (segment_count, table_len, body_len) = validate_output_segments(segments, limits)?;
        let prepared = segments
            .iter()
            .map(|bytes| PreparedSegment {
                bytes,
                word_count: (bytes.len() / WORD_BYTES) as u32,
            })
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let encoded_len = table_len
            .checked_add(body_len)
            .ok_or(FrameError::BodyLengthOverflow)?;
        debug_assert_eq!(prepared.len(), segment_count as usize);
        Ok(Self {
            segments: prepared,
            table_len,
            encoded_len,
        })
    }

    pub fn len(&self) -> usize {
        self.segments.len()
    }

    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    pub const fn table_len(&self) -> usize {
        self.table_len
    }

    pub const fn encoded_len(&self) -> usize {
        self.encoded_len
    }
}

#[cfg(feature = "alloc")]
pub fn parse_frame(input: &[u8], limits: FrameLimits) -> Result<FrameRead<'_>, FrameError> {
    if input.is_empty() {
        return Ok(FrameRead::EndOfInput);
    }
    if input.len() < 4 {
        return Err(FrameError::TruncatedHeader {
            available: input.len(),
        });
    }

    let encoded_count = read_u32_le(input, 0).expect("four-byte header was checked");
    let segment_count = encoded_count
        .checked_add(1)
        .ok_or(FrameError::SegmentCountOverflow)?;
    check_segment_count(segment_count, limits)?;

    let count = usize::try_from(segment_count).map_err(|_| FrameError::BodyLengthOverflow)?;
    let table_words = count / 2 + 1;
    let table_len = table_words
        .checked_mul(8)
        .ok_or(FrameError::BodyLengthOverflow)?;
    if input.len() < table_len {
        return Err(FrameError::TruncatedSegmentTable {
            expected: table_len,
            available: input.len(),
        });
    }

    let mut sizes = Vec::with_capacity(count);
    let mut total_words = 0u64;
    for index in 0..count {
        let table_offset = 4usize
            .checked_mul(index + 1)
            .ok_or(FrameError::BodyLengthOverflow)?;
        let words = read_u32_le(input, table_offset).expect("complete table was checked");
        total_words = total_words
            .checked_add(u64::from(words))
            .ok_or(FrameError::TotalWordsOverflow)?;
        if total_words > limits.max_total_words {
            return Err(FrameError::MessageTooLarge {
                words: total_words,
                limit: limits.max_total_words,
            });
        }
        sizes.push(words);
    }

    let body_len_u64 = total_words
        .checked_mul(8)
        .ok_or(FrameError::BodyLengthOverflow)?;
    let body_len = usize::try_from(body_len_u64).map_err(|_| FrameError::BodyLengthOverflow)?;
    let encoded_len = table_len
        .checked_add(body_len)
        .ok_or(FrameError::BodyLengthOverflow)?;
    if input.len() < encoded_len {
        return Err(FrameError::TruncatedBody {
            expected: body_len,
            available: input.len() - table_len,
        });
    }

    let mut segments = Vec::with_capacity(count);
    let mut body_offset = table_len;
    for (index, word_count) in sizes.into_iter().enumerate() {
        let byte_len = usize::try_from(u64::from(word_count) * 8)
            .map_err(|_| FrameError::BodyLengthOverflow)?;
        let end = body_offset
            .checked_add(byte_len)
            .ok_or(FrameError::BodyLengthOverflow)?;
        segments.push(Segment {
            id: u32::try_from(index).map_err(|_| FrameError::BodyLengthOverflow)?,
            word_count,
            bytes: &input[body_offset..end],
        });
        body_offset = end;
    }

    Ok(FrameRead::Message {
        frame: Frame {
            segments: segments.into_boxed_slice(),
            table_len,
            encoded_len,
        },
        remaining: &input[encoded_len..],
    })
}

#[cfg(feature = "alloc")]
pub fn encode_frame(segments: &[&[u8]], limits: FrameLimits) -> Result<Vec<u8>, FrameError> {
    let (segment_count, table_len, body_len) = validate_output_segments(segments, limits)?;
    let encoded_len = table_len
        .checked_add(body_len)
        .ok_or(FrameError::BodyLengthOverflow)?;
    let mut output = Vec::with_capacity(encoded_len);
    output.extend_from_slice(&(segment_count - 1).to_le_bytes());
    append_segment_sizes(&mut output, segments);
    if segment_count % 2 == 0 {
        output.extend_from_slice(&[0; 4]);
    }
    for segment in segments {
        output.extend_from_slice(segment);
    }
    debug_assert_eq!(output.len(), encoded_len);
    Ok(output)
}

#[cfg(feature = "alloc")]
pub fn encode_prepared_frame(segments: &PreparedSegments<'_>) -> Vec<u8> {
    let segment_count = segments.segments.len() as u32;
    let mut output = Vec::with_capacity(segments.encoded_len);
    output.extend_from_slice(&(segment_count - 1).to_le_bytes());
    append_prepared_sizes(&mut output, &segments.segments);
    if segment_count % 2 == 0 {
        output.extend_from_slice(&[0; 4]);
    }
    for segment in &segments.segments {
        output.extend_from_slice(segment.bytes);
    }
    debug_assert_eq!(output.len(), segments.encoded_len);
    output
}

#[cfg(feature = "alloc")]
fn validate_output_segments(
    segments: &[&[u8]],
    limits: FrameLimits,
) -> Result<(u32, usize, usize), FrameError> {
    if segments.is_empty() {
        return Err(FrameError::NoSegments);
    }
    let segment_count = u32::try_from(segments.len()).map_err(|_| FrameError::TooManySegments {
        count: u32::MAX,
        limit: effective_segment_limit(limits),
    })?;
    check_segment_count(segment_count, limits)?;

    let table_len = (segments.len() / 2 + 1)
        .checked_mul(8)
        .ok_or(FrameError::BodyLengthOverflow)?;
    let mut total_words = 0u64;
    for (index, segment) in segments.iter().enumerate() {
        if segment.len() % 8 != 0 {
            return Err(FrameError::SegmentNotWordAligned {
                index,
                bytes: segment.len(),
            });
        }
        let words = u64::try_from(segment.len() / 8).map_err(|_| FrameError::BodyLengthOverflow)?;
        if words > u64::from(u32::MAX) {
            return Err(FrameError::SegmentTooLarge { index, words });
        }
        total_words = total_words
            .checked_add(words)
            .ok_or(FrameError::TotalWordsOverflow)?;
        if total_words > limits.max_total_words {
            return Err(FrameError::MessageTooLarge {
                words: total_words,
                limit: limits.max_total_words,
            });
        }
    }

    let body_len = usize::try_from(
        total_words
            .checked_mul(8)
            .ok_or(FrameError::BodyLengthOverflow)?,
    )
    .map_err(|_| FrameError::BodyLengthOverflow)?;
    Ok((segment_count, table_len, body_len))
}

#[cfg(feature = "alloc")]
fn append_segment_sizes(output: &mut Vec<u8>, segments: &[&[u8]]) {
    const BATCH_SEGMENTS: usize = 32;
    if segments.len() < 8 {
        for segment in segments {
            output.extend_from_slice(&((segment.len() / 8) as u32).to_le_bytes());
        }
        return;
    }

    for batch in segments.chunks(BATCH_SEGMENTS) {
        let mut encoded = [0_u8; BATCH_SEGMENTS * 4];
        for (slot, segment) in encoded.chunks_exact_mut(4).zip(batch) {
            slot.copy_from_slice(&((segment.len() / 8) as u32).to_le_bytes());
        }
        output.extend_from_slice(&encoded[..batch.len() * 4]);
    }
}

#[cfg(feature = "alloc")]
fn append_prepared_sizes(output: &mut Vec<u8>, segments: &[PreparedSegment<'_>]) {
    const BATCH_SEGMENTS: usize = 32;
    if segments.len() < 8 {
        for segment in segments {
            output.extend_from_slice(&segment.word_count.to_le_bytes());
        }
        return;
    }
    for batch in segments.chunks(BATCH_SEGMENTS) {
        let mut encoded = [0_u8; BATCH_SEGMENTS * 4];
        for (slot, segment) in encoded.chunks_exact_mut(4).zip(batch) {
            slot.copy_from_slice(&segment.word_count.to_le_bytes());
        }
        output.extend_from_slice(&encoded[..batch.len() * 4]);
    }
}

fn effective_segment_limit(limits: FrameLimits) -> u32 {
    limits.max_segments.min(MAX_SEGMENTS)
}

fn check_segment_count(segment_count: u32, limits: FrameLimits) -> Result<(), FrameError> {
    let limit = effective_segment_limit(limits);
    if segment_count > limit {
        Err(FrameError::TooManySegments {
            count: segment_count,
            limit,
        })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE: &[u8] = include_bytes!(concat!(
        "../../../conformance/fixtures/cpp/",
        "e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/wire-unpacked.bin"
    ));
    const TWO: &[u8] = include_bytes!(concat!(
        "../../../conformance/fixtures/cpp/",
        "e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/wire-two-segment.bin"
    ));
    const MANY: &[u8] = include_bytes!(concat!(
        "../../../conformance/fixtures/cpp/",
        "e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/wire-multisegment.bin"
    ));

    fn message_parts(read: FrameRead<'_>) -> Option<(Frame<'_>, &[u8])> {
        match read {
            FrameRead::Message { frame, remaining } => Some((frame, remaining)),
            FrameRead::EndOfInput => None,
        }
    }

    #[test]
    fn caller_segment_storage_parses_without_copying_or_allocation() {
        let mut storage = [Segment::EMPTY; MAX_SEGMENTS as usize];
        let (frame, remaining) = match parse_frame_into(MANY, FrameLimits::default(), &mut storage)
            .expect("many-segment frame parses")
        {
            BorrowedFrameRead::Message { frame, remaining } => Some((frame, remaining)),
            BorrowedFrameRead::EndOfInput => None,
        }
        .expect("fixture contains a message");
        assert!(remaining.is_empty());
        assert_eq!(frame.segments().len(), 33);
        let first = frame.segment(0).expect("segment zero exists");
        assert_eq!(first.bytes().as_ptr(), MANY[frame.table_len()..].as_ptr());

        let mut too_small = [Segment::EMPTY; 1];
        assert_eq!(
            parse_frame_into(TWO, FrameLimits::default(), &mut too_small),
            Err(FrameError::SegmentBufferTooSmall {
                required: 2,
                available: 1,
            })
        );
    }

    fn parse_message(bytes: &[u8]) -> (Frame<'_>, &[u8]) {
        message_parts(parse_frame(bytes, FrameLimits::default()).expect("oracle frame parses"))
            .expect("oracle frame is not empty")
    }

    #[test]
    fn pinned_cpp_one_two_and_many_segment_frames_cross_read() {
        for (bytes, expected_count) in [(ONE, 1usize), (TWO, 2), (MANY, 33)] {
            let (frame, remaining) = parse_message(bytes);
            assert_eq!(frame.segments().len(), expected_count);
            assert!(remaining.is_empty());
            let segment_bytes: Vec<&[u8]> = frame
                .segments()
                .iter()
                .map(|segment| segment.bytes())
                .collect();
            assert_eq!(
                encode_frame(&segment_bytes, FrameLimits::default())
                    .expect("oracle segments re-encode"),
                bytes
            );
        }
    }

    #[test]
    fn clean_eof_is_distinct_from_every_truncation_phase() {
        assert_eq!(
            parse_frame(&[], FrameLimits::default()),
            Ok(FrameRead::EndOfInput)
        );
        assert!(matches!(
            parse_frame(&[0], FrameLimits::default()),
            Err(FrameError::TruncatedHeader { .. })
        ));
        assert!(matches!(
            parse_frame(&TWO[..12], FrameLimits::default()),
            Err(FrameError::TruncatedSegmentTable { .. })
        ));
        assert!(matches!(
            parse_frame(&ONE[..ONE.len() - 1], FrameLimits::default()),
            Err(FrameError::TruncatedBody { .. })
        ));
    }

    #[test]
    fn limits_and_count_overflow_fail_before_body_access() {
        assert!(matches!(
            parse_frame(&u32::MAX.to_le_bytes(), FrameLimits::default()),
            Err(FrameError::SegmentCountOverflow)
        ));
        let too_many = 512u32.to_le_bytes();
        assert_eq!(
            parse_frame(&too_many, FrameLimits::default()),
            Err(FrameError::TooManySegments {
                count: 513,
                limit: MAX_SEGMENTS,
            })
        );
        assert!(matches!(
            parse_frame(
                ONE,
                FrameLimits {
                    max_segments: MAX_SEGMENTS,
                    max_total_words: 1,
                }
            ),
            Err(FrameError::MessageTooLarge { .. })
        ));
    }

    #[test]
    fn concatenated_frames_return_the_exact_remainder() {
        let mut joined = ONE.to_vec();
        joined.extend_from_slice(TWO);
        let (_, remaining) = message_parts(
            parse_frame(&joined, FrameLimits::default()).expect("first frame parses"),
        )
        .expect("joined input is not empty");
        assert_eq!(remaining, TWO);
    }

    #[test]
    fn writer_and_parser_round_trip_many_table_shapes() {
        let first = [0x11u8; 8];
        let second = [0x22; 16];
        let third = [0x33; 24];
        let fourth = [0x44; 32];
        let storage: [&[u8]; 4] = [&first, &second, &third, &fourth];
        for count in 1..=4 {
            let segments = &storage[..count];
            let encoded = encode_frame(segments, FrameLimits::default()).expect("frame encodes");
            let prepared =
                PreparedSegments::new(segments, FrameLimits::default()).expect("segments prepare");
            assert_eq!(prepared.len(), count);
            assert!(!prepared.is_empty());
            assert_eq!(prepared.encoded_len(), encoded.len());
            assert_eq!(prepared.table_len(), (count / 2 + 1) * 8);
            assert_eq!(encode_prepared_frame(&prepared), encoded);
            let (frame, remaining) =
                message_parts(parse_frame(&encoded, FrameLimits::default()).expect("frame parses"))
                    .expect("encoded frame is not empty");
            assert!(remaining.is_empty());
            assert_eq!(frame.segments().len(), count);
            for (actual, expected) in frame.segments().iter().zip(segments.iter().copied()) {
                assert_eq!(actual.bytes(), expected);
            }
        }
    }

    #[test]
    fn writer_rejects_uninitialized_unaligned_and_limited_inputs() {
        assert_eq!(
            encode_frame(&[], FrameLimits::default()),
            Err(FrameError::NoSegments)
        );
        assert!(matches!(
            encode_frame(&[&[0; 7]], FrameLimits::default()),
            Err(FrameError::SegmentNotWordAligned { .. })
        ));
        assert!(matches!(
            encode_frame(
                &[&[0; 8], &[0; 8]],
                FrameLimits {
                    max_segments: 1,
                    max_total_words: 2,
                }
            ),
            Err(FrameError::TooManySegments { .. })
        ));
    }
}
