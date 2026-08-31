//! Pinned `ByteStream` lifecycle and path-shortening adapter.

use std::fmt;

/// Synchronous destination used by the executor-neutral ByteStream state
/// machine. Async transports drive the same methods from their owning actor.
pub trait ByteSink {
    type Error: std::error::Error + Send + Sync + 'static;

    fn write(&mut self, bytes: &[u8]) -> Result<(), Self::Error>;
    fn end(&mut self) -> Result<(), Self::Error>;
    fn start_tls(&mut self, expected_server_hostname: &str) -> Result<(), Self::Error>;

    /// Called when the adapter is dropped or explicitly canceled before end.
    fn cancel(&mut self) {}
}

/// Callback from a bounded substream. Both methods are one-shot.
pub trait SubstreamCallback<S: ByteSink> {
    fn ended(&mut self, byte_count: u64) -> Result<(), S::Error>;
    fn reached_limit(&mut self) -> Result<ByteStream<S>, S::Error>;
    fn canceled(&mut self) {}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ByteStreamState {
    Open,
    Ended,
    Canceled,
    Failed,
    Transferred,
}

#[derive(Debug)]
pub enum ByteStreamError<E> {
    Backend(E),
    NotOpen(ByteStreamState),
    CountOverflow,
    SubstreamNotEnded,
}

impl<E: fmt::Display> fmt::Display for ByteStreamError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Backend(error) => error.fmt(formatter),
            Self::NotOpen(state) => write!(formatter, "ByteStream is not open ({state:?})"),
            Self::CountOverflow => formatter.write_str("ByteStream byte count overflow"),
            Self::SubstreamNotEnded => {
                formatter.write_str("substream destination cannot be reclaimed before clean end")
            }
        }
    }
}

impl<E> std::error::Error for ByteStreamError<E>
where
    E: std::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Backend(error) => Some(error),
            _ => None,
        }
    }
}

/// Explicit-end stream. Dropping an open value calls [`ByteSink::cancel`].
///
/// Empty writes are ignored, TLS upgrade does not end the stream, and a clean
/// end is distinct from cancellation. Backend failures make the stream
/// terminal so callers cannot accidentally continue on a partly-failed path.
///
/// ```
/// use capnp_compat::{ByteSink, ByteStream, ByteStreamState};
/// use std::fmt;
///
/// #[derive(Debug)]
/// struct Sink(Vec<u8>);
/// #[derive(Debug)]
/// struct Error;
/// impl fmt::Display for Error {
///     fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
///         f.write_str("sink error")
///     }
/// }
/// impl std::error::Error for Error {}
/// impl ByteSink for Sink {
///     type Error = Error;
///     fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
///         self.0.extend_from_slice(bytes);
///         Ok(())
///     }
///     fn end(&mut self) -> Result<(), Error> { Ok(()) }
///     fn start_tls(&mut self, _: &str) -> Result<(), Error> { Ok(()) }
/// }
///
/// let mut stream = ByteStream::new(Sink(Vec::new()));
/// stream.write(b"hello")?;
/// stream.start_tls("example.test")?;
/// stream.write(b" world")?;
/// stream.end()?;
/// assert_eq!(stream.state(), ByteStreamState::Ended);
/// # Ok::<(), capnp_compat::ByteStreamError<Error>>(())
/// ```
#[derive(Debug)]
pub struct ByteStream<S: ByteSink> {
    sink: Option<S>,
    state: ByteStreamState,
}

impl<S: ByteSink> ByteStream<S> {
    pub fn new(sink: S) -> Self {
        Self {
            sink: Some(sink),
            state: ByteStreamState::Open,
        }
    }

    pub fn state(&self) -> ByteStreamState {
        self.state
    }

    pub fn write(&mut self, bytes: &[u8]) -> Result<(), ByteStreamError<S::Error>> {
        self.require_open()?;
        if bytes.is_empty() {
            return Ok(());
        }
        if let Err(error) = self.sink_mut().write(bytes) {
            self.state = ByteStreamState::Failed;
            return Err(ByteStreamError::Backend(error));
        }
        Ok(())
    }

    pub fn start_tls(
        &mut self,
        expected_server_hostname: &str,
    ) -> Result<(), ByteStreamError<S::Error>> {
        self.require_open()?;
        if let Err(error) = self.sink_mut().start_tls(expected_server_hostname) {
            self.state = ByteStreamState::Failed;
            return Err(ByteStreamError::Backend(error));
        }
        Ok(())
    }

    pub fn end(&mut self) -> Result<(), ByteStreamError<S::Error>> {
        self.require_open()?;
        if let Err(error) = self.sink_mut().end() {
            self.state = ByteStreamState::Failed;
            return Err(ByteStreamError::Backend(error));
        }
        self.state = ByteStreamState::Ended;
        Ok(())
    }

    pub fn cancel(&mut self) -> Result<(), ByteStreamError<S::Error>> {
        self.require_open()?;
        self.sink_mut().cancel();
        self.state = ByteStreamState::Canceled;
        Ok(())
    }

    /// Transfers this stream into a bounded path-shortening operation.
    ///
    /// The original handle cannot be used after transfer; the destination is
    /// returned only by [`ByteSubstream::into_destination`] after clean end.
    ///
    /// ```compile_fail,E0382
    /// use capnp_compat::{ByteSink, ByteStream, SubstreamCallback};
    ///
    /// struct Sink;
    /// impl ByteSink for Sink {
    ///     type Error = std::io::Error;
    ///     fn write(&mut self, _: &[u8]) -> std::io::Result<()> { Ok(()) }
    ///     fn end(&mut self) -> std::io::Result<()> { Ok(()) }
    ///     fn start_tls(&mut self, _: &str) -> std::io::Result<()> { Ok(()) }
    /// }
    /// struct Callback;
    /// impl SubstreamCallback<Sink> for Callback {
    ///     fn ended(&mut self, _: u64) -> std::io::Result<()> { Ok(()) }
    ///     fn reached_limit(&mut self) -> std::io::Result<ByteStream<Sink>> {
    ///         Ok(ByteStream::new(Sink))
    ///     }
    /// }
    ///
    /// let mut stream = ByteStream::new(Sink);
    /// let _substream = stream.into_substream(Callback, 4).unwrap();
    /// stream.write(b"moved").unwrap();
    /// ```
    pub fn into_substream<C>(
        mut self,
        callback: C,
        limit: u64,
    ) -> Result<ByteSubstream<S, C>, ByteStreamError<S::Error>>
    where
        C: SubstreamCallback<S>,
    {
        self.require_open()?;
        let sink = self
            .sink
            .take()
            .expect("open ByteStream always owns its sink");
        self.state = ByteStreamState::Transferred;
        let mut substream = ByteSubstream {
            destination: Some(ByteStream::new(sink)),
            continuation: None,
            callback,
            limit,
            byte_count: 0,
            state: ByteStreamState::Open,
            callback_completed: false,
        };
        if limit == 0 {
            substream.activate_continuation()?;
        }
        Ok(substream)
    }

    fn require_open(&self) -> Result<(), ByteStreamError<S::Error>> {
        if self.state == ByteStreamState::Open {
            Ok(())
        } else {
            Err(ByteStreamError::NotOpen(self.state))
        }
    }

    fn sink_mut(&mut self) -> &mut S {
        self.sink
            .as_mut()
            .expect("non-transferred stream owns sink")
    }
}

impl<S: ByteSink> Drop for ByteStream<S> {
    fn drop(&mut self) {
        if self.state == ByteStreamState::Open {
            if let Some(sink) = &mut self.sink {
                sink.cancel();
            }
            self.state = ByteStreamState::Canceled;
        }
    }
}

/// A path-shortening stream which temporarily owns the original destination.
#[derive(Debug)]
pub struct ByteSubstream<S: ByteSink, C: SubstreamCallback<S>> {
    destination: Option<ByteStream<S>>,
    continuation: Option<ByteStream<S>>,
    callback: C,
    limit: u64,
    byte_count: u64,
    state: ByteStreamState,
    callback_completed: bool,
}

impl<S: ByteSink, C: SubstreamCallback<S>> ByteSubstream<S, C> {
    pub fn state(&self) -> ByteStreamState {
        self.state
    }

    pub fn byte_count(&self) -> u64 {
        self.byte_count
    }

    pub fn write(&mut self, bytes: &[u8]) -> Result<(), ByteStreamError<S::Error>> {
        self.require_open()?;
        if let Some(continuation) = &mut self.continuation {
            return continuation.write(bytes);
        }
        let length = u64::try_from(bytes.len()).map_err(|_| ByteStreamError::CountOverflow)?;
        let remaining = self
            .limit
            .checked_sub(self.byte_count)
            .ok_or(ByteStreamError::CountOverflow)?;
        let prefix_len_u64 = remaining.min(length);
        let prefix_len =
            usize::try_from(prefix_len_u64).map_err(|_| ByteStreamError::CountOverflow)?;
        if prefix_len != 0 {
            self.destination_mut().write(&bytes[..prefix_len])?;
            self.byte_count = self
                .byte_count
                .checked_add(prefix_len_u64)
                .ok_or(ByteStreamError::CountOverflow)?;
        }
        if self.byte_count == self.limit {
            self.activate_continuation()?;
            if prefix_len < bytes.len() {
                self.continuation_mut().write(&bytes[prefix_len..])?;
            }
        }
        Ok(())
    }

    pub fn end(&mut self) -> Result<(), ByteStreamError<S::Error>> {
        self.require_open()?;
        let result = if let Some(continuation) = &mut self.continuation {
            continuation.end()
        } else {
            self.callback
                .ended(self.byte_count)
                .map_err(ByteStreamError::Backend)
        };
        match result {
            Ok(()) => {
                self.callback_completed = true;
                self.state = ByteStreamState::Ended;
                Ok(())
            }
            Err(error) => {
                self.state = ByteStreamState::Failed;
                Err(error)
            }
        }
    }

    pub fn into_destination(mut self) -> Result<ByteStream<S>, ByteStreamError<S::Error>> {
        if self.state != ByteStreamState::Ended {
            return Err(ByteStreamError::SubstreamNotEnded);
        }
        self.state = ByteStreamState::Transferred;
        Ok(self.destination.take().expect("substream owns destination"))
    }

    fn activate_continuation(&mut self) -> Result<(), ByteStreamError<S::Error>> {
        if self.continuation.is_some() {
            return Ok(());
        }
        match self.callback.reached_limit() {
            Ok(next) => {
                self.continuation = Some(next);
                self.callback_completed = true;
                Ok(())
            }
            Err(error) => {
                self.state = ByteStreamState::Failed;
                Err(ByteStreamError::Backend(error))
            }
        }
    }

    fn require_open(&self) -> Result<(), ByteStreamError<S::Error>> {
        if self.state == ByteStreamState::Open {
            Ok(())
        } else {
            Err(ByteStreamError::NotOpen(self.state))
        }
    }

    fn destination_mut(&mut self) -> &mut ByteStream<S> {
        self.destination
            .as_mut()
            .expect("substream owns destination")
    }

    fn continuation_mut(&mut self) -> &mut ByteStream<S> {
        self.continuation.as_mut().expect("continuation activated")
    }
}

impl<S: ByteSink, C: SubstreamCallback<S>> Drop for ByteSubstream<S, C> {
    fn drop(&mut self) {
        if self.state == ByteStreamState::Open || self.state == ByteStreamState::Failed {
            if !self.callback_completed {
                self.callback.canceled();
            }
            if let Some(destination) = &mut self.destination {
                let _ = destination.cancel();
            }
            if let Some(continuation) = &mut self.continuation {
                let _ = continuation.cancel();
            }
            self.state = ByteStreamState::Canceled;
        }
    }
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct TestError;

    impl fmt::Display for TestError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("test sink failure")
        }
    }

    impl std::error::Error for TestError {}

    #[derive(Debug, Default)]
    struct Recording {
        bytes: Vec<u8>,
        ends: usize,
        cancels: usize,
        tls: Vec<String>,
        fail_write: bool,
    }

    #[derive(Clone, Debug)]
    struct RecordingSink(Arc<Mutex<Recording>>);

    impl RecordingSink {
        fn new() -> (Self, Arc<Mutex<Recording>>) {
            let record = Arc::new(Mutex::new(Recording::default()));
            (Self(Arc::clone(&record)), record)
        }
    }

    impl ByteSink for RecordingSink {
        type Error = TestError;

        fn write(&mut self, bytes: &[u8]) -> Result<(), Self::Error> {
            let mut record = self.0.lock().expect("recording sink lock");
            if record.fail_write {
                return Err(TestError);
            }
            record.bytes.extend_from_slice(bytes);
            Ok(())
        }

        fn end(&mut self) -> Result<(), Self::Error> {
            self.0.lock().expect("recording sink lock").ends += 1;
            Ok(())
        }

        fn start_tls(&mut self, hostname: &str) -> Result<(), Self::Error> {
            self.0
                .lock()
                .expect("recording sink lock")
                .tls
                .push(hostname.to_owned());
            Ok(())
        }

        fn cancel(&mut self) {
            self.0.lock().expect("recording sink lock").cancels += 1;
        }
    }

    #[derive(Debug, Default)]
    struct CallbackRecord {
        ended: Vec<u64>,
        reached_limit: usize,
        canceled: usize,
    }

    #[derive(Debug)]
    struct Callback {
        record: Arc<Mutex<CallbackRecord>>,
        next: Option<ByteStream<RecordingSink>>,
    }

    impl Callback {
        fn new(next: ByteStream<RecordingSink>) -> (Self, Arc<Mutex<CallbackRecord>>) {
            let record = Arc::new(Mutex::new(CallbackRecord::default()));
            (
                Self {
                    record: Arc::clone(&record),
                    next: Some(next),
                },
                record,
            )
        }
    }

    impl SubstreamCallback<RecordingSink> for Callback {
        fn ended(&mut self, byte_count: u64) -> Result<(), TestError> {
            self.record
                .lock()
                .expect("callback lock")
                .ended
                .push(byte_count);
            Ok(())
        }

        fn reached_limit(&mut self) -> Result<ByteStream<RecordingSink>, TestError> {
            self.record.lock().expect("callback lock").reached_limit += 1;
            self.next.take().ok_or(TestError)
        }

        fn canceled(&mut self) {
            self.record.lock().expect("callback lock").canceled += 1;
        }
    }

    #[test]
    fn explicit_end_tls_and_drop_have_distinct_lifecycles() {
        let (sink, record) = RecordingSink::new();
        let mut stream = ByteStream::new(sink);
        stream.write(b"abc").expect("write");
        stream.start_tls("example.test").expect("TLS");
        stream.write(b"d").expect("post-TLS write");
        stream.end().expect("clean end");
        assert!(matches!(
            stream.write(b"late"),
            Err(ByteStreamError::NotOpen(ByteStreamState::Ended))
        ));
        drop(stream);
        let record = record.lock().expect("recording sink lock");
        assert_eq!(record.bytes, b"abcd");
        assert_eq!(record.tls, ["example.test"]);
        assert_eq!((record.ends, record.cancels), (1, 0));
        drop(record);

        let (sink, canceled) = RecordingSink::new();
        drop(ByteStream::new(sink));
        assert_eq!(canceled.lock().expect("recording sink lock").cancels, 1);
    }

    #[test]
    fn early_end_reports_count_without_ending_the_reclaimed_parent() {
        let (sink, destination) = RecordingSink::new();
        let (next, _) = RecordingSink::new();
        let (callback, callback_record) = Callback::new(ByteStream::new(next));
        let mut substream = ByteStream::new(sink)
            .into_substream(callback, 10)
            .expect("substream");
        substream.write(b"abc").expect("substream write");
        substream.end().expect("substream end");
        let mut parent = substream.into_destination().expect("reclaim parent");
        parent.write(b"d").expect("parent resumes");
        parent.end().expect("parent end");
        let destination = destination.lock().expect("destination lock");
        assert_eq!(destination.bytes, b"abcd");
        assert_eq!(destination.ends, 1);
        assert_eq!(callback_record.lock().expect("callback lock").ended, [3]);
    }

    #[test]
    fn limit_forwards_overrun_and_future_writes_to_the_callback_stream() {
        let (sink, destination) = RecordingSink::new();
        let (next_sink, next) = RecordingSink::new();
        let (callback, callback_record) = Callback::new(ByteStream::new(next_sink));
        let mut substream = ByteStream::new(sink)
            .into_substream(callback, 3)
            .expect("substream");
        substream.write(b"abcdef").expect("limit write");
        substream.write(b"gh").expect("forwarded write");
        substream.end().expect("forwarded end");
        let mut parent = substream.into_destination().expect("reclaim parent");
        parent.write(b"!").expect("parent remains open");
        parent.end().expect("parent end");
        assert_eq!(destination.lock().expect("destination lock").bytes, b"abc!");
        let next = next.lock().expect("next lock");
        assert_eq!(next.bytes, b"defgh");
        assert_eq!(next.ends, 1);
        drop(next);
        let callback = callback_record.lock().expect("callback lock");
        assert_eq!(callback.reached_limit, 1);
        assert!(callback.ended.is_empty());
    }

    #[test]
    fn zero_limit_shortens_immediately_and_abandonment_cancels_both_paths() {
        let (sink, destination) = RecordingSink::new();
        let (next_sink, next) = RecordingSink::new();
        let (callback, callback_record) = Callback::new(ByteStream::new(next_sink));
        let mut substream = ByteStream::new(sink)
            .into_substream(callback, 0)
            .expect("zero-limit substream");
        substream.write(b"next").expect("immediate forward");
        drop(substream);
        assert!(
            destination
                .lock()
                .expect("destination lock")
                .bytes
                .is_empty()
        );
        assert_eq!(destination.lock().expect("destination lock").cancels, 1);
        assert_eq!(next.lock().expect("next lock").bytes, b"next");
        assert_eq!(next.lock().expect("next lock").cancels, 1);
        assert_eq!(
            callback_record.lock().expect("callback lock").reached_limit,
            1
        );
    }

    #[test]
    fn sink_failure_is_sticky() {
        let (sink, record) = RecordingSink::new();
        record.lock().expect("recording sink lock").fail_write = true;
        let mut stream = ByteStream::new(sink);
        assert!(matches!(
            stream.write(b"x"),
            Err(ByteStreamError::Backend(TestError))
        ));
        assert!(matches!(
            stream.write(b"y"),
            Err(ByteStreamError::NotOpen(ByteStreamState::Failed))
        ));
    }

    #[test]
    fn ended_stream_cannot_be_transferred_into_a_substream() {
        let (sink, _) = RecordingSink::new();
        let (next, _) = RecordingSink::new();
        let (callback, _) = Callback::new(ByteStream::new(next));
        let mut stream = ByteStream::new(sink);
        stream.end().expect("clean end");
        assert!(matches!(
            stream.into_substream(callback, 1),
            Err(ByteStreamError::NotOpen(ByteStreamState::Ended))
        ));
    }
}
