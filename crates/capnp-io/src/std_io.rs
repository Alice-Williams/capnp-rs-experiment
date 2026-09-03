//! Partial-I/O-safe standard-library adapters and generic mapped backing.
//!
//! The frame reader validates the segment table and configured limits before
//! allocating the body. The bounded writer rejects an over-limit operation
//! before its first write. `MappedFrame<B>` accepts any stable byte backing
//! (including an mmap type supplied by an application) and returns segment
//! slices that point directly into that backing; this crate performs no unsafe
//! OS mapping itself.

use core::fmt;

use alloc::vec::Vec;
use capnp_wire::read_u32_le;
use std::io::{self, Read, Write};

use crate::framing::validate_output_segments;
use crate::{
    BorrowedFrameRead, FrameError, FrameLimits, MAX_SEGMENTS, PreparedSegments, Segment,
    parse_frame, parse_frame_into,
};

#[derive(Debug)]
pub enum IoFrameError {
    Io(io::Error),
    Frame(FrameError),
    OutputLimit { requested: usize, limit: usize },
}

impl fmt::Display for IoFrameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(formatter),
            Self::Frame(error) => error.fmt(formatter),
            Self::OutputLimit { requested, limit } => {
                write!(
                    formatter,
                    "write requires {requested} bytes; limit is {limit}"
                )
            }
        }
    }
}

impl core::error::Error for IoFrameError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Frame(error) => Some(error),
            Self::OutputLimit { .. } => None,
        }
    }
}

impl From<io::Error> for IoFrameError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<FrameError> for IoFrameError {
    fn from(value: FrameError) -> Self {
        Self::Frame(value)
    }
}

/// Reads one bounded standard frame, distinguishing clean EOF from truncation.
///
/// ```
/// use std::io::Cursor;
/// use capnp_io::{FrameLimits, read_frame, write_frame};
/// let segment = [0_u8; 8];
/// let encoded = write_frame(Vec::new(), &[&segment], FrameLimits::default(), 16)?;
/// let mut input = Cursor::new(encoded.clone());
/// assert_eq!(read_frame(&mut input, FrameLimits::default())?, Some(encoded));
/// assert_eq!(read_frame(&mut input, FrameLimits::default())?, None);
/// # Ok::<(), capnp_io::IoFrameError>(())
/// ```
pub fn read_frame<R: Read>(
    reader: &mut R,
    limits: FrameLimits,
) -> Result<Option<Vec<u8>>, IoFrameError> {
    let mut frame = Vec::new();
    if read_frame_reusing(reader, &mut frame, limits)? {
        Ok(Some(frame))
    } else {
        Ok(None)
    }
}

/// Reads one bounded standard frame into a reusable caller-owned buffer.
///
/// A `true` result means the buffer contains one complete frame; `false` means
/// the input was already at clean EOF and the buffer was cleared. Existing
/// initialized storage is overwritten in place. On error, the buffer may still
/// contain bytes from its previous frame alongside newly read data.
#[inline]
pub fn read_frame_reusing<R: Read>(
    reader: &mut R,
    frame: &mut Vec<u8>,
    limits: FrameLimits,
) -> Result<bool, IoFrameError> {
    let mut prefix = [0_u8; 8];
    let read = read_until_full(reader, &mut prefix)?;
    if read == 0 {
        frame.clear();
        return Ok(false);
    }
    if read < 4 {
        return Err(FrameError::TruncatedHeader { available: read }.into());
    }
    let count = read_u32_le(&prefix, 0)
        .expect("complete header")
        .checked_add(1)
        .ok_or(FrameError::SegmentCountOverflow)?;
    let segment_limit = limits.max_segments.min(MAX_SEGMENTS);
    if count > segment_limit {
        return Err(FrameError::TooManySegments {
            count,
            limit: segment_limit,
        }
        .into());
    }
    let count_usize = usize::try_from(count).map_err(|_| FrameError::BodyLengthOverflow)?;
    let table_len = (count_usize / 2 + 1)
        .checked_mul(8)
        .ok_or(FrameError::BodyLengthOverflow)?;
    if read < prefix.len() {
        return Err(FrameError::TruncatedSegmentTable {
            expected: table_len,
            available: read,
        }
        .into());
    }
    if frame.capacity() >= table_len {
        if frame.len() < table_len {
            frame.resize(table_len, 0);
        }
        frame[..8].copy_from_slice(&prefix);
        let table_read = read_until_full(reader, &mut frame[8..table_len])?;
        if table_read != table_len - 8 {
            return Err(FrameError::TruncatedSegmentTable {
                expected: table_len,
                available: table_read + 8,
            }
            .into());
        }
        let body_len = validated_body_len(&frame[..table_len], count_usize, limits)?;
        let encoded_len = table_len
            .checked_add(body_len)
            .ok_or(FrameError::BodyLengthOverflow)?;
        if frame.len() < encoded_len {
            frame.resize(encoded_len, 0);
        } else {
            frame.truncate(encoded_len);
        }
        let body_read = read_until_full(reader, &mut frame[table_len..])?;
        if body_read != body_len {
            return Err(FrameError::TruncatedBody {
                expected: body_len,
                available: body_read,
            }
            .into());
        }
        return Ok(true);
    }
    if table_len == 8 {
        finish_read_frame(reader, frame, &prefix, count_usize, limits)?;
        return Ok(true);
    }
    const STACK_TABLE_SEGMENTS: usize = 64;
    const STACK_TABLE_LEN: usize = (STACK_TABLE_SEGMENTS / 2 + 1) * 8;
    let mut stack_table = [0_u8; STACK_TABLE_LEN];
    let mut heap_table = Vec::new();
    let table = if table_len <= STACK_TABLE_LEN {
        stack_table[..8].copy_from_slice(&prefix);
        &mut stack_table[..table_len]
    } else {
        heap_table
            .try_reserve_exact(table_len)
            .map_err(|_| io::Error::other("frame table allocation failed"))?;
        heap_table.extend_from_slice(&prefix);
        heap_table.resize(table_len, 0);
        heap_table.as_mut_slice()
    };
    let table_read = read_until_full(reader, &mut table[8..])?;
    if table_read != table_len - 8 {
        return Err(FrameError::TruncatedSegmentTable {
            expected: table_len,
            available: table_read + 8,
        }
        .into());
    }

    finish_read_frame(reader, frame, table, count_usize, limits)?;
    Ok(true)
}

fn finish_read_frame<R: Read>(
    reader: &mut R,
    frame: &mut Vec<u8>,
    table: &[u8],
    count: usize,
    limits: FrameLimits,
) -> Result<(), IoFrameError> {
    let body_len = validated_body_len(table, count, limits)?;
    let encoded_len = table
        .len()
        .checked_add(body_len)
        .ok_or(FrameError::BodyLengthOverflow)?;
    if frame.len() < encoded_len {
        frame
            .try_reserve_exact(encoded_len - frame.len())
            .map_err(|_| io::Error::other("frame allocation failed"))?;
        frame.resize(encoded_len, 0);
    } else {
        frame.truncate(encoded_len);
    }
    frame[..table.len()].copy_from_slice(table);
    let body_read = read_until_full(reader, &mut frame[table.len()..])?;
    if body_read != body_len {
        return Err(FrameError::TruncatedBody {
            expected: body_len,
            available: body_read,
        }
        .into());
    }
    Ok(())
}

#[inline]
fn validated_body_len(
    table: &[u8],
    count: usize,
    limits: FrameLimits,
) -> Result<usize, IoFrameError> {
    let mut total_words = 0_u64;
    let size_table = &table[4..4 + count * 4];
    for encoded_words in size_table.chunks_exact(4) {
        let words = u32::from_le_bytes(
            encoded_words
                .try_into()
                .expect("chunks are exactly four bytes"),
        );
        total_words = total_words
            .checked_add(u64::from(words))
            .ok_or(FrameError::TotalWordsOverflow)?;
        if total_words > limits.max_total_words {
            return Err(FrameError::MessageTooLarge {
                words: total_words,
                limit: limits.max_total_words,
            }
            .into());
        }
    }
    usize::try_from(
        total_words
            .checked_mul(8)
            .ok_or(FrameError::BodyLengthOverflow)?,
    )
    .map_err(|_| FrameError::BodyLengthOverflow.into())
}

#[inline]
fn read_until_full<R: Read>(reader: &mut R, mut output: &mut [u8]) -> io::Result<usize> {
    let original = output.len();
    while !output.is_empty() {
        match reader.read(output) {
            Ok(0) => break,
            Ok(read) => output = &mut output[read..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(original - output.len())
}

#[derive(Debug)]
pub struct BoundedWriter<W> {
    inner: W,
    limit: usize,
    written: usize,
}

impl<W> BoundedWriter<W> {
    pub const fn new(inner: W, limit: usize) -> Self {
        Self {
            inner,
            limit,
            written: 0,
        }
    }

    pub const fn written(&self) -> usize {
        self.written
    }

    pub fn into_inner(self) -> W {
        self.inner
    }
}

impl<W: Write> BoundedWriter<W> {
    pub fn write_all(&mut self, bytes: &[u8]) -> Result<(), IoFrameError> {
        let requested = self
            .written
            .checked_add(bytes.len())
            .ok_or(IoFrameError::OutputLimit {
                requested: usize::MAX,
                limit: self.limit,
            })?;
        if requested > self.limit {
            return Err(IoFrameError::OutputLimit {
                requested,
                limit: self.limit,
            });
        }
        let mut remaining = bytes;
        while !remaining.is_empty() {
            match self.inner.write(remaining) {
                Ok(0) => return Err(io::Error::from(io::ErrorKind::WriteZero).into()),
                Ok(written) => {
                    self.written += written;
                    remaining = &remaining[written..];
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    pub fn flush(&mut self) -> Result<(), IoFrameError> {
        self.inner.flush().map_err(IoFrameError::from)
    }
}

pub fn write_frame<W: Write>(
    writer: W,
    segments: &[&[u8]],
    frame_limits: FrameLimits,
    output_limit: usize,
) -> Result<W, IoFrameError> {
    let (segment_count, table_len, body_len) = validate_output_segments(segments, frame_limits)?;
    let encoded_len = table_len
        .checked_add(body_len)
        .ok_or(FrameError::BodyLengthOverflow)?;
    if encoded_len > output_limit {
        return Err(IoFrameError::OutputLimit {
            requested: encoded_len,
            limit: output_limit,
        });
    }
    let mut bounded = BoundedWriter::new(writer, output_limit);
    const BATCH_SEGMENTS: usize = 32;
    for (batch_index, batch) in segments.chunks(BATCH_SEGMENTS).enumerate() {
        let prefix = usize::from(batch_index == 0) * 4;
        let mut table = [0_u8; BATCH_SEGMENTS * 4 + 4];
        if batch_index == 0 {
            table[..4].copy_from_slice(&(segment_count - 1).to_le_bytes());
        }
        for (slot, segment) in table[prefix..].chunks_exact_mut(4).zip(batch.iter()) {
            slot.copy_from_slice(&((segment.len() / 8) as u32).to_le_bytes());
        }
        bounded.write_all(&table[..prefix + batch.len() * 4])?;
    }
    if segment_count % 2 == 0 {
        bounded.write_all(&[0; 4])?;
    }
    for segment in segments {
        bounded.write_all(segment)?;
    }
    bounded.flush()?;
    Ok(bounded.into_inner())
}

/// Writes already-validated segments directly without materializing a
/// contiguous intermediate frame.
pub fn write_prepared_frame<W: Write>(
    writer: W,
    segments: &PreparedSegments<'_>,
    output_limit: usize,
) -> Result<W, IoFrameError> {
    if segments.encoded_len() > output_limit {
        return Err(IoFrameError::OutputLimit {
            requested: segments.encoded_len(),
            limit: output_limit,
        });
    }
    let mut bounded = BoundedWriter::new(writer, output_limit);
    const BATCH_SEGMENTS: usize = 32;
    for (batch_index, batch) in segments.segments.chunks(BATCH_SEGMENTS).enumerate() {
        let prefix = usize::from(batch_index == 0) * 4;
        let mut table = [0_u8; BATCH_SEGMENTS * 4 + 4];
        if batch_index == 0 {
            table[..4].copy_from_slice(&((segments.len() as u32) - 1).to_le_bytes());
        }
        for (slot, segment) in table[prefix..].chunks_exact_mut(4).zip(batch.iter()) {
            slot.copy_from_slice(&segment.word_count.to_le_bytes());
        }
        bounded.write_all(&table[..prefix + batch.len() * 4])?;
    }
    if segments.len() % 2 == 0 {
        bounded.write_all(&[0; 4])?;
    }
    for segment in &segments.segments {
        bounded.write_all(segment.bytes)?;
    }
    bounded.flush()?;
    Ok(bounded.into_inner())
}

/// Stable byte backing suitable for memory-mapped files and similar storage.
///
/// The backing can be an application-owned mmap type implementing
/// `AsRef<[u8]>`; returned segment bytes are subslices of that same mapping.
#[derive(Debug)]
pub struct MappedFrame<B> {
    backing: B,
    limits: FrameLimits,
}

impl<B: AsRef<[u8]>> MappedFrame<B> {
    pub fn new(backing: B, limits: FrameLimits) -> Result<Self, FrameError> {
        let read = parse_frame(backing.as_ref(), limits)?;
        let crate::FrameRead::Message { remaining, .. } = read else {
            return Err(FrameError::TruncatedHeader { available: 0 });
        };
        if !remaining.is_empty() {
            return Err(FrameError::TrailingData {
                bytes: remaining.len(),
            });
        }
        Ok(Self { backing, limits })
    }

    pub fn bytes(&self) -> &[u8] {
        self.backing.as_ref()
    }

    pub fn parse_into<'input, 'storage>(
        &'input self,
        storage: &'storage mut [Segment<'input>],
    ) -> Result<BorrowedFrameRead<'input, 'storage>, FrameError> {
        parse_frame_into(self.backing.as_ref(), self.limits, storage)
    }

    pub fn into_inner(self) -> B {
        self.backing
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encode_frame;
    use alloc::vec;
    use std::cell::Cell;
    use std::io::Cursor;
    use std::rc::Rc;

    const FRAME: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../conformance/fixtures/cpp/",
        "e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/wire-unpacked.bin"
    ));

    struct PartialReader {
        bytes: Cursor<Vec<u8>>,
        max: usize,
        interrupt: bool,
    }

    impl Read for PartialReader {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            if self.interrupt {
                self.interrupt = false;
                return Err(io::ErrorKind::Interrupted.into());
            }
            self.interrupt = true;
            let max = self.max.min(output.len());
            self.bytes.read(&mut output[..max])
        }
    }

    struct PartialWriter {
        bytes: Vec<u8>,
        max: usize,
        interrupt: bool,
    }

    impl Write for PartialWriter {
        fn write(&mut self, input: &[u8]) -> io::Result<usize> {
            if self.interrupt {
                self.interrupt = false;
                return Err(io::ErrorKind::Interrupted.into());
            }
            self.interrupt = true;
            let written = self.max.min(input.len());
            self.bytes.extend_from_slice(&input[..written]);
            Ok(written)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct CountingWriter(Rc<Cell<usize>>);

    impl Write for CountingWriter {
        fn write(&mut self, input: &[u8]) -> io::Result<usize> {
            self.0.set(self.0.get() + input.len());
            Ok(input.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn every_partial_read_size_and_interruption_reconstructs_the_frame() {
        for max in 1..=FRAME.len() + 1 {
            let mut reader = PartialReader {
                bytes: Cursor::new(FRAME.to_vec()),
                max,
                interrupt: true,
            };
            assert_eq!(
                read_frame(&mut reader, FrameLimits::default()).expect("frame reads"),
                Some(FRAME.to_vec())
            );
            assert_eq!(
                read_frame(&mut reader, FrameLimits::default()).expect("clean EOF"),
                None
            );
        }
    }

    #[test]
    fn reusable_reader_preserves_capacity_across_concatenated_frames() {
        let mut input_bytes = Vec::with_capacity(FRAME.len() * 2);
        input_bytes.extend_from_slice(FRAME);
        input_bytes.extend_from_slice(FRAME);
        let mut input = Cursor::new(input_bytes);
        let mut frame = Vec::with_capacity(FRAME.len());
        let allocation = frame.as_ptr();

        for _ in 0..2 {
            assert!(
                read_frame_reusing(&mut input, &mut frame, FrameLimits::default())
                    .expect("frame reads")
            );
            assert_eq!(frame, FRAME);
            assert_eq!(frame.as_ptr(), allocation);
        }
        assert!(
            !read_frame_reusing(&mut input, &mut frame, FrameLimits::default()).expect("clean EOF")
        );
        assert!(frame.is_empty());
        assert_eq!(frame.as_ptr(), allocation);
    }

    #[test]
    fn bounded_partial_writer_preserves_bytes_and_rejects_before_writing() {
        for max in 1..=17 {
            let writer = PartialWriter {
                bytes: Vec::new(),
                max,
                interrupt: true,
            };
            let mut bounded = BoundedWriter::new(writer, FRAME.len());
            bounded.write_all(FRAME).expect("frame writes");
            assert_eq!(bounded.written(), FRAME.len());
            assert_eq!(bounded.into_inner().bytes, FRAME);
        }
        let writer = PartialWriter {
            bytes: Vec::new(),
            max: 1,
            interrupt: false,
        };
        let mut bounded = BoundedWriter::new(writer, FRAME.len() - 1);
        assert!(matches!(
            bounded.write_all(FRAME),
            Err(IoFrameError::OutputLimit { .. })
        ));
        assert!(bounded.into_inner().bytes.is_empty());
    }

    #[test]
    fn prepared_writer_matches_contiguous_encoding_across_table_batches() {
        let bodies = (0_u8..64).map(|value| vec![value; 8]).collect::<Vec<_>>();
        let segments = bodies.iter().map(Vec::as_slice).collect::<Vec<_>>();
        let prepared =
            PreparedSegments::new(&segments, FrameLimits::default()).expect("segments prepare");
        let expected = encode_frame(&segments, FrameLimits::default()).expect("frame encodes");

        let actual = write_prepared_frame(
            Vec::with_capacity(prepared.encoded_len()),
            &prepared,
            prepared.encoded_len(),
        )
        .expect("prepared frame writes");

        assert_eq!(actual, expected);
    }

    #[test]
    fn checked_writer_matches_contiguous_encoding_with_partial_writes() {
        let bodies = (0_u8..64).map(|value| vec![value; 8]).collect::<Vec<_>>();
        let segments = bodies.iter().map(Vec::as_slice).collect::<Vec<_>>();
        let expected = encode_frame(&segments, FrameLimits::default()).expect("frame encodes");
        let writer = PartialWriter {
            bytes: Vec::new(),
            max: 3,
            interrupt: true,
        };

        let actual = write_frame(writer, &segments, FrameLimits::default(), expected.len())
            .expect("checked frame writes");

        assert_eq!(actual.bytes, expected);
    }

    #[test]
    fn prepared_writer_rejects_output_limit_before_first_write() {
        let segment = [0_u8; 8];
        let prepared =
            PreparedSegments::new(&[&segment], FrameLimits::default()).expect("segment prepares");
        let written = Rc::new(Cell::new(0));

        assert!(matches!(
            write_prepared_frame(
                CountingWriter(Rc::clone(&written)),
                &prepared,
                prepared.encoded_len() - 1,
            ),
            Err(IoFrameError::OutputLimit { .. })
        ));
        assert_eq!(written.get(), 0);
    }

    #[test]
    fn checked_writer_rejects_output_limit_before_first_write() {
        let segment = [0_u8; 8];
        let written = Rc::new(Cell::new(0));

        assert!(matches!(
            write_frame(
                CountingWriter(Rc::clone(&written)),
                &[&segment],
                FrameLimits::default(),
                15,
            ),
            Err(IoFrameError::OutputLimit { .. })
        ));
        assert_eq!(written.get(), 0);
    }

    #[test]
    fn mapped_backing_segments_are_exact_subslices_of_the_original() {
        let mapped = MappedFrame::new(FRAME.to_vec(), FrameLimits::default())
            .expect("mapped frame validates");
        let mut storage = [Segment::EMPTY; MAX_SEGMENTS as usize];
        let BorrowedFrameRead::Message { frame, remaining } = mapped
            .parse_into(&mut storage)
            .expect("mapped frame parses")
        else {
            unreachable!("validated backing has a message");
        };
        assert!(remaining.is_empty());
        let segment = frame.segment(0).expect("segment exists");
        assert_eq!(
            segment.bytes().as_ptr(),
            mapped.bytes()[frame.table_len()..].as_ptr()
        );
    }
}
