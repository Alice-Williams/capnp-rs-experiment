#![doc = "Executor-neutral asynchronous Cap'n Proto framing and bounded output."]
//!
//! These traits use `Poll` directly and do not depend on an executor or reactor.
//! Reader and writer state lives in the adapter rather than a temporary future,
//! so cancellation between polls preserves partial bytes and ordered progress.
//! Only one mutable poll may exist at a time. Queued writer bytes are bounded,
//! frames are emitted serially, and no lock or borrow crosses `Pending`.
//!
//! Bounded batch work also preserves input order without a persistent pool:
//!
//! ```
//! use capnp_async::{BatchJob, BatchLimits, BatchOutput, run_ordered_batch};
//! use std::convert::Infallible;
//!
//! let jobs = vec![BatchJob::new(1_u8, 1), BatchJob::new(2, 1)];
//! let mut ordered = Vec::new();
//! run_ordered_batch(
//!     jobs,
//!     BatchLimits::default(),
//!     |value| Ok::<_, Infallible>(BatchOutput::new(value * 2, 1)),
//!     |sequence, value| {
//!         ordered.push((sequence, value));
//!         Ok::<_, Infallible>(())
//!     },
//! )?;
//! assert_eq!(ordered, [(0, 2), (1, 4)]);
//! # Ok::<(), capnp_async::BatchError<Infallible, Infallible>>(())
//! ```

use core::fmt;
use core::pin::Pin;
use core::task::{Context, Poll};
use std::collections::VecDeque;
use std::io;

use capnp_io::{FrameError, FrameLimits, MAX_SEGMENTS, encode_frame};
use capnp_wire::read_u32_le;

mod batch;

pub use batch::{
    BatchError, BatchJob, BatchLimits, BatchOutput, BatchStats, pack_messages_ordered,
    run_ordered_batch, unpack_messages_ordered,
};

pub trait AsyncRead {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        output: &mut [u8],
    ) -> Poll<io::Result<usize>>;
}

pub trait AsyncWrite {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        input: &[u8],
    ) -> Poll<io::Result<usize>>;

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>>;
}

#[derive(Debug)]
pub enum AsyncFrameError {
    Io(io::Error),
    Frame(FrameError),
    Backpressure { requested: usize, limit: usize },
    PreviousFailure,
}

impl fmt::Display for AsyncFrameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(formatter),
            Self::Frame(error) => error.fmt(formatter),
            Self::Backpressure { requested, limit } => {
                write!(
                    formatter,
                    "queue requires {requested} bytes; limit is {limit}"
                )
            }
            Self::PreviousFailure => formatter.write_str("asynchronous adapter previously failed"),
        }
    }
}

impl std::error::Error for AsyncFrameError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Frame(error) => Some(error),
            Self::Backpressure { .. } | Self::PreviousFailure => None,
        }
    }
}

impl From<io::Error> for AsyncFrameError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<FrameError> for AsyncFrameError {
    fn from(value: FrameError) -> Self {
        Self::Frame(value)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReadPhase {
    Header,
    Table,
    Body,
}

#[derive(Debug)]
pub struct AsyncFrameReader<R> {
    inner: R,
    limits: FrameLimits,
    bytes: Vec<u8>,
    filled: usize,
    phase: ReadPhase,
    failed: bool,
}

impl<R> AsyncFrameReader<R> {
    pub fn new(inner: R, limits: FrameLimits) -> Self {
        Self {
            inner,
            limits,
            bytes: vec![0; 4],
            filled: 0,
            phase: ReadPhase::Header,
            failed: false,
        }
    }

    pub fn into_inner(self) -> R {
        self.inner
    }
}

impl<R: AsyncRead + Unpin> AsyncFrameReader<R> {
    pub fn poll_next_frame(
        &mut self,
        context: &mut Context<'_>,
    ) -> Poll<Result<Option<Vec<u8>>, AsyncFrameError>> {
        if self.failed {
            return Poll::Ready(Err(AsyncFrameError::PreviousFailure));
        }
        let result = self.poll_next_frame_inner(context);
        if matches!(result, Poll::Ready(Err(_))) {
            self.failed = true;
        }
        result
    }

    fn poll_next_frame_inner(
        &mut self,
        context: &mut Context<'_>,
    ) -> Poll<Result<Option<Vec<u8>>, AsyncFrameError>> {
        loop {
            while self.filled < self.bytes.len() {
                let read = match Pin::new(&mut self.inner)
                    .poll_read(context, &mut self.bytes[self.filled..])
                {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(Ok(read)) => read,
                    Poll::Ready(Err(error)) if error.kind() == io::ErrorKind::Interrupted => {
                        continue;
                    }
                    Poll::Ready(Err(error)) => return Poll::Ready(Err(error.into())),
                };
                if read == 0 {
                    if self.phase == ReadPhase::Header && self.filled == 0 {
                        return Poll::Ready(Ok(None));
                    }
                    return Poll::Ready(Err(self.truncated_error().into()));
                }
                self.filled += read;
            }

            match self.phase {
                ReadPhase::Header => self.prepare_table()?,
                ReadPhase::Table => self.prepare_body()?,
                ReadPhase::Body => {
                    let completed = core::mem::replace(&mut self.bytes, vec![0; 4]);
                    self.filled = 0;
                    self.phase = ReadPhase::Header;
                    return Poll::Ready(Ok(Some(completed)));
                }
            }
        }
    }

    fn prepare_table(&mut self) -> Result<(), AsyncFrameError> {
        let count = read_u32_le(&self.bytes, 0)
            .expect("header is complete")
            .checked_add(1)
            .ok_or(FrameError::SegmentCountOverflow)?;
        let limit = self.limits.max_segments.min(MAX_SEGMENTS);
        if count > limit {
            return Err(FrameError::TooManySegments { count, limit }.into());
        }
        let count = usize::try_from(count).map_err(|_| FrameError::BodyLengthOverflow)?;
        let table_len = (count / 2 + 1)
            .checked_mul(8)
            .ok_or(FrameError::BodyLengthOverflow)?;
        self.bytes
            .try_reserve_exact(table_len - self.bytes.len())
            .map_err(|_| io::Error::other("frame table allocation failed"))?;
        self.bytes.resize(table_len, 0);
        self.phase = ReadPhase::Table;
        Ok(())
    }

    fn prepare_body(&mut self) -> Result<(), AsyncFrameError> {
        let count = usize::try_from(
            read_u32_le(&self.bytes, 0)
                .expect("table is complete")
                .checked_add(1)
                .ok_or(FrameError::SegmentCountOverflow)?,
        )
        .map_err(|_| FrameError::BodyLengthOverflow)?;
        let mut total_words = 0_u64;
        for index in 0..count {
            let words = read_u32_le(&self.bytes, 4 * (index + 1)).expect("table is complete");
            total_words = total_words
                .checked_add(u64::from(words))
                .ok_or(FrameError::TotalWordsOverflow)?;
            if total_words > self.limits.max_total_words {
                return Err(FrameError::MessageTooLarge {
                    words: total_words,
                    limit: self.limits.max_total_words,
                }
                .into());
            }
        }
        let body_len = usize::try_from(
            total_words
                .checked_mul(8)
                .ok_or(FrameError::BodyLengthOverflow)?,
        )
        .map_err(|_| FrameError::BodyLengthOverflow)?;
        let encoded_len = self
            .bytes
            .len()
            .checked_add(body_len)
            .ok_or(FrameError::BodyLengthOverflow)?;
        self.bytes
            .try_reserve_exact(body_len)
            .map_err(|_| io::Error::other("frame body allocation failed"))?;
        self.bytes.resize(encoded_len, 0);
        self.phase = ReadPhase::Body;
        Ok(())
    }

    fn truncated_error(&self) -> FrameError {
        match self.phase {
            ReadPhase::Header => FrameError::TruncatedHeader {
                available: self.filled,
            },
            ReadPhase::Table => FrameError::TruncatedSegmentTable {
                expected: self.bytes.len(),
                available: self.filled,
            },
            ReadPhase::Body => {
                let count = read_u32_le(&self.bytes, 0)
                    .ok()
                    .and_then(|encoded| usize::try_from(u64::from(encoded) + 1).ok())
                    .unwrap_or(0);
                let table_len = count / 2 * 8 + 8;
                FrameError::TruncatedBody {
                    expected: self.bytes.len().saturating_sub(table_len),
                    available: self.filled.saturating_sub(table_len),
                }
            }
        }
    }
}

/// Ordered frame queue with an exact unsent-byte backpressure limit.
///
/// ```
/// use capnp_async::AsyncFrameWriter;
/// use capnp_io::FrameLimits;
/// let segment = [0_u8; 8];
/// let mut writer = AsyncFrameWriter::new((), FrameLimits::default(), 16);
/// writer.enqueue_frame(&[&segment])?;
/// assert_eq!(writer.queued_bytes(), 16);
/// # Ok::<(), capnp_async::AsyncFrameError>(())
/// ```
#[derive(Debug)]
pub struct AsyncFrameWriter<W> {
    inner: W,
    frame_limits: FrameLimits,
    max_queued_bytes: usize,
    queued_bytes: usize,
    queue: VecDeque<Vec<u8>>,
    front_offset: usize,
    failed: bool,
}

impl<W> AsyncFrameWriter<W> {
    pub fn new(inner: W, frame_limits: FrameLimits, max_queued_bytes: usize) -> Self {
        Self {
            inner,
            frame_limits,
            max_queued_bytes,
            queued_bytes: 0,
            queue: VecDeque::new(),
            front_offset: 0,
            failed: false,
        }
    }

    pub const fn queued_bytes(&self) -> usize {
        self.queued_bytes
    }

    pub fn enqueue_frame(&mut self, segments: &[&[u8]]) -> Result<(), AsyncFrameError> {
        if self.failed {
            return Err(AsyncFrameError::PreviousFailure);
        }
        let frame = encode_frame(segments, self.frame_limits)?;
        let requested =
            self.queued_bytes
                .checked_add(frame.len())
                .ok_or(AsyncFrameError::Backpressure {
                    requested: usize::MAX,
                    limit: self.max_queued_bytes,
                })?;
        if requested > self.max_queued_bytes {
            return Err(AsyncFrameError::Backpressure {
                requested,
                limit: self.max_queued_bytes,
            });
        }
        self.queued_bytes = requested;
        self.queue.push_back(frame);
        Ok(())
    }

    pub fn into_inner(self) -> W {
        self.inner
    }
}

impl<W: AsyncWrite + Unpin> AsyncFrameWriter<W> {
    pub fn poll_flush(&mut self, context: &mut Context<'_>) -> Poll<Result<(), AsyncFrameError>> {
        if self.failed {
            return Poll::Ready(Err(AsyncFrameError::PreviousFailure));
        }
        let result = self.poll_flush_inner(context);
        if matches!(result, Poll::Ready(Err(_))) {
            self.failed = true;
        }
        result
    }

    fn poll_flush_inner(&mut self, context: &mut Context<'_>) -> Poll<Result<(), AsyncFrameError>> {
        while let Some(front) = self.queue.front() {
            let remaining = &front[self.front_offset..];
            let written = match Pin::new(&mut self.inner).poll_write(context, remaining) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Ok(0)) => {
                    return Poll::Ready(Err(io::Error::from(io::ErrorKind::WriteZero).into()));
                }
                Poll::Ready(Ok(written)) => written,
                Poll::Ready(Err(error)) if error.kind() == io::ErrorKind::Interrupted => continue,
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error.into())),
            };
            self.front_offset += written;
            self.queued_bytes -= written;
            if self.front_offset == front.len() {
                self.queue.pop_front();
                self.front_offset = 0;
            }
        }
        match Pin::new(&mut self.inner).poll_flush(context) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(())) => Poll::Ready(Ok(())),
            Poll::Ready(Err(error)) if error.kind() == io::ErrorKind::Interrupted => {
                context.waker().wake_by_ref();
                Poll::Pending
            }
            Poll::Ready(Err(error)) => Poll::Ready(Err(error.into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::task::Waker;

    const FRAME: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../conformance/fixtures/cpp/",
        "e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/wire-unpacked.bin"
    ));

    struct ChunkReader {
        bytes: Cursor<Vec<u8>>,
        max: usize,
        pending: bool,
    }

    impl AsyncRead for ChunkReader {
        fn poll_read(
            mut self: Pin<&mut Self>,
            context: &mut Context<'_>,
            output: &mut [u8],
        ) -> Poll<io::Result<usize>> {
            if self.pending {
                self.pending = false;
                context.waker().wake_by_ref();
                return Poll::Pending;
            }
            self.pending = true;
            let max = self.max.min(output.len());
            Poll::Ready(std::io::Read::read(&mut self.bytes, &mut output[..max]))
        }
    }

    struct ChunkWriter {
        bytes: Vec<u8>,
        max: usize,
        pending: bool,
    }

    impl AsyncWrite for ChunkWriter {
        fn poll_write(
            mut self: Pin<&mut Self>,
            context: &mut Context<'_>,
            input: &[u8],
        ) -> Poll<io::Result<usize>> {
            if self.pending {
                self.pending = false;
                context.waker().wake_by_ref();
                return Poll::Pending;
            }
            self.pending = true;
            let written = self.max.min(input.len());
            self.bytes.extend_from_slice(&input[..written]);
            Poll::Ready(Ok(written))
        }

        fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    fn context() -> Context<'static> {
        Context::from_waker(Waker::noop())
    }

    #[test]
    fn partial_reads_survive_pending_cancellation_boundaries() {
        for max in 1..=FRAME.len() + 1 {
            let inner = ChunkReader {
                bytes: Cursor::new(FRAME.to_vec()),
                max,
                pending: true,
            };
            let mut reader = AsyncFrameReader::new(inner, FrameLimits::default());
            let mut cx = context();
            assert!(reader.poll_next_frame(&mut cx).is_pending());
            let frame = loop {
                if let Poll::Ready(result) = reader.poll_next_frame(&mut cx) {
                    break result.expect("frame read").expect("message exists");
                }
            };
            assert_eq!(frame, FRAME);
        }
    }

    #[test]
    fn concatenated_frames_are_returned_in_order_without_overread() {
        let mut joined = FRAME.to_vec();
        joined.extend_from_slice(FRAME);
        let inner = ChunkReader {
            bytes: Cursor::new(joined),
            max: 11,
            pending: true,
        };
        let mut reader = AsyncFrameReader::new(inner, FrameLimits::default());
        let mut cx = context();
        for _ in 0..2 {
            let frame = loop {
                if let Poll::Ready(result) = reader.poll_next_frame(&mut cx) {
                    break result.expect("frame read").expect("message exists");
                }
            };
            assert_eq!(frame, FRAME);
        }
    }

    #[test]
    fn bounded_queue_applies_backpressure_and_preserves_frame_order() {
        let first = encode_frame(&[&[0_u8; 8]], FrameLimits::default()).expect("frame encodes");
        let second = encode_frame(&[&[1_u8; 8]], FrameLimits::default()).expect("frame encodes");
        for max in 1..=first.len() + second.len() + 1 {
            let inner = ChunkWriter {
                bytes: Vec::new(),
                max,
                pending: true,
            };
            let mut writer =
                AsyncFrameWriter::new(inner, FrameLimits::default(), first.len() + second.len());
            writer.enqueue_frame(&[&[0_u8; 8]]).expect("first queues");
            writer.enqueue_frame(&[&[1_u8; 8]]).expect("second queues");
            assert!(matches!(
                writer.enqueue_frame(&[&[2_u8; 8]]),
                Err(AsyncFrameError::Backpressure { .. })
            ));
            let mut cx = context();
            assert!(writer.poll_flush(&mut cx).is_pending());
            loop {
                match writer.poll_flush(&mut cx) {
                    Poll::Pending => {}
                    Poll::Ready(result) => {
                        result.expect("queue flushes");
                        break;
                    }
                }
            }
            assert_eq!(writer.queued_bytes(), 0);
            let mut expected = first.clone();
            expected.extend_from_slice(&second);
            assert_eq!(writer.into_inner().bytes, expected);
        }
    }
}
