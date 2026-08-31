//! Unix-stream framing with bounded SCM_RIGHTS attachment transfer.

#![cfg(unix)]

use std::fmt;
use std::io::{self, IoSlice, IoSliceMut};
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use async_io::Async;
use capnp_io::{FrameError, FrameLimits, FrameRead, encode_frame, parse_frame};
use capnp_message::{OwnedMessage, ReaderLimits};
use capnp_rpc_core::{
    AttachedResource, DuplexTransport, EnvelopeLimits, OwnedResource, TransportEnvelope,
    TransportError,
};
use rustix::net::{
    RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, ReturnFlags, SendAncillaryBuffer,
    SendAncillaryMessage, SendFlags, recvmsg, send, sendmsg,
};

const IO_CHUNK_BYTES: usize = 64 * 1024;

#[derive(Debug)]
pub enum UnixTransportError {
    Io(io::Error),
    Frame(FrameError),
    Envelope(TransportError),
    UnsupportedResource { index: usize },
    MissingEnvelope,
    ChangedPendingEnvelope,
    TruncatedFrame { received: usize, expected: usize },
    WriteZero,
}

impl fmt::Display for UnixTransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(formatter),
            Self::Frame(error) => error.fmt(formatter),
            Self::Envelope(error) => error.fmt(formatter),
            Self::UnsupportedResource { index } => {
                write!(formatter, "transport resource {index} is not an OwnedFd")
            }
            Self::MissingEnvelope => formatter.write_str("poll_send requires an envelope"),
            Self::ChangedPendingEnvelope => {
                formatter.write_str("pending poll_send envelope changed before completion")
            }
            Self::TruncatedFrame { received, expected } => write!(
                formatter,
                "Unix RPC stream ended after {received} of {expected} frame bytes"
            ),
            Self::WriteZero => formatter.write_str("Unix RPC stream wrote zero bytes"),
        }
    }
}

impl std::error::Error for UnixTransportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Frame(error) => Some(error),
            Self::Envelope(error) => Some(error),
            Self::UnsupportedResource { .. }
            | Self::MissingEnvelope
            | Self::ChangedPendingEnvelope
            | Self::TruncatedFrame { .. }
            | Self::WriteZero => None,
        }
    }
}

impl From<io::Error> for UnixTransportError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<FrameError> for UnixTransportError {
    fn from(error: FrameError) -> Self {
        Self::Frame(error)
    }
}

impl From<TransportError> for UnixTransportError {
    fn from(error: TransportError) -> Self {
        Self::Envelope(error)
    }
}

#[derive(Debug)]
struct SendState {
    message_identity: usize,
    bytes: Vec<u8>,
    offset: usize,
}

#[derive(Debug, Default)]
struct ReceiveState {
    bytes: Vec<u8>,
    resources: Vec<OwnedResource>,
}

/// An executor-neutral `DuplexTransport` over a connected Unix stream.
///
/// The adapter reads exactly one standard frame at a time, so ancillary data
/// cannot drift to an adjacent RPC message even when the kernel coalesces
/// stream writes. Received descriptors are `OwnedFd`s immediately and excess
/// descriptors are discarded by the kernel or dropped before returning.
pub struct UnixScmRightsTransport {
    io: Async<UnixStream>,
    envelope_limits: EnvelopeLimits,
    frame_limits: FrameLimits,
    max_fds_per_message: usize,
    sending: Option<SendState>,
    receiving: ReceiveState,
    closed: bool,
}

impl fmt::Debug for UnixScmRightsTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UnixScmRightsTransport")
            .field("max_fds_per_message", &self.max_fds_per_message)
            .field("sending", &self.sending)
            .field("receiving_bytes", &self.receiving.bytes.len())
            .field("receiving_resources", &self.receiving.resources.len())
            .field("closed", &self.closed)
            .finish_non_exhaustive()
    }
}

impl UnixScmRightsTransport {
    pub fn new(
        stream: UnixStream,
        envelope_limits: EnvelopeLimits,
        frame_limits: FrameLimits,
        max_fds_per_message: usize,
    ) -> Result<Self, UnixTransportError> {
        let max_fds_per_message = max_fds_per_message
            .min(envelope_limits.max_resources_per_envelope)
            .min(u8::MAX as usize);
        Ok(Self {
            io: Async::new(stream)?,
            envelope_limits,
            frame_limits,
            max_fds_per_message,
            sending: None,
            receiving: ReceiveState::default(),
            closed: false,
        })
    }

    pub const fn max_fds_per_message(&self) -> usize {
        self.max_fds_per_message
    }

    fn poll_receive_chunk(
        &mut self,
        context: &mut Context<'_>,
    ) -> Poll<Result<(), UnixTransportError>> {
        let expected = expected_frame_len(&self.receiving.bytes, self.frame_limits)?;
        let remaining = expected.saturating_sub(self.receiving.bytes.len());
        if remaining == 0 {
            return Poll::Ready(Ok(()));
        }
        let mut bytes = vec![0_u8; remaining.min(IO_CHUNK_BYTES)];
        loop {
            let available_fd_slots = self
                .max_fds_per_message
                .saturating_sub(self.receiving.resources.len());
            let mut ancillary_space =
                vec![MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(available_fd_slots))];
            let mut ancillary = RecvAncillaryBuffer::new(&mut ancillary_space);
            let mut slices = [IoSliceMut::new(&mut bytes)];
            match recvmsg(
                &self.io,
                &mut slices,
                &mut ancillary,
                RecvFlags::CMSG_CLOEXEC,
            ) {
                Ok(received) => {
                    for message in ancillary.drain() {
                        if let RecvAncillaryMessage::ScmRights(fds) = message {
                            for fd in fds {
                                if self.receiving.resources.len() < self.max_fds_per_message {
                                    self.receiving.resources.push(OwnedResource::new(fd, 0));
                                }
                            }
                        }
                    }
                    if received.flags.contains(ReturnFlags::CTRUNC) {
                        // Any descriptor that did not fit in the control buffer
                        // is closed by the kernel. Retained OwnedFds stay valid.
                    }
                    if received.bytes == 0 {
                        if self.receiving.bytes.is_empty() {
                            self.closed = true;
                            return Poll::Ready(Ok(()));
                        }
                        return Poll::Ready(Err(UnixTransportError::TruncatedFrame {
                            received: self.receiving.bytes.len(),
                            expected,
                        }));
                    }
                    self.receiving
                        .bytes
                        .extend_from_slice(&bytes[..received.bytes]);
                    return Poll::Ready(Ok(()));
                }
                Err(error) if error == rustix::io::Errno::INTR => continue,
                Err(error) if error == rustix::io::Errno::AGAIN => {
                    return match self.io.poll_readable(context) {
                        Poll::Ready(Ok(())) => continue,
                        Poll::Ready(Err(error)) => Poll::Ready(Err(error.into())),
                        Poll::Pending => Poll::Pending,
                    };
                }
                Err(error) => return Poll::Ready(Err(io::Error::from(error).into())),
            }
        }
    }

    fn duplicate_fds(envelope: &TransportEnvelope) -> Result<Vec<OwnedFd>, UnixTransportError> {
        envelope
            .resources()
            .iter()
            .enumerate()
            .map(|(index, resource)| {
                if let Some(fd) = resource.downcast_ref::<OwnedFd>() {
                    return fd.try_clone().map_err(UnixTransportError::Io);
                }
                if let Some(attached) = resource.downcast_ref::<AttachedResource>() {
                    return attached
                        .with::<OwnedFd, _>(OwnedFd::try_clone)
                        .ok_or(UnixTransportError::UnsupportedResource { index })?
                        .map_err(UnixTransportError::Io);
                }
                Err(UnixTransportError::UnsupportedResource { index })
            })
            .collect()
    }

    fn poll_write_chunk(
        &mut self,
        context: &mut Context<'_>,
        envelope: &TransportEnvelope,
    ) -> Poll<Result<(), UnixTransportError>> {
        let Some(state) = self.sending.as_mut() else {
            return Poll::Ready(Err(UnixTransportError::MissingEnvelope));
        };
        loop {
            let result = if state.offset == 0 && !envelope.resources().is_empty() {
                let owned_fds = Self::duplicate_fds(envelope)?;
                let borrowed_fds = owned_fds.iter().map(AsFd::as_fd).collect::<Vec<_>>();
                let mut ancillary_space =
                    vec![MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(borrowed_fds.len()))];
                let mut ancillary = SendAncillaryBuffer::new(&mut ancillary_space);
                if !ancillary.push(SendAncillaryMessage::ScmRights(&borrowed_fds)) {
                    return Poll::Ready(Err(io::Error::other(
                        "SCM_RIGHTS control buffer was undersized",
                    )
                    .into()));
                }
                sendmsg(
                    &self.io,
                    &[IoSlice::new(&state.bytes[state.offset..])],
                    &mut ancillary,
                    SendFlags::NOSIGNAL,
                )
            } else {
                send(&self.io, &state.bytes[state.offset..], SendFlags::NOSIGNAL)
            };
            match result {
                Ok(0) => return Poll::Ready(Err(UnixTransportError::WriteZero)),
                Ok(written) => {
                    state.offset = state.offset.saturating_add(written);
                    return Poll::Ready(Ok(()));
                }
                Err(error) if error == rustix::io::Errno::INTR => continue,
                Err(error) if error == rustix::io::Errno::AGAIN => {
                    return match self.io.poll_writable(context) {
                        Poll::Ready(Ok(())) => continue,
                        Poll::Ready(Err(error)) => Poll::Ready(Err(error.into())),
                        Poll::Pending => Poll::Pending,
                    };
                }
                Err(error) => return Poll::Ready(Err(io::Error::from(error).into())),
            }
        }
    }
}

impl DuplexTransport for UnixScmRightsTransport {
    type Error = UnixTransportError;

    fn poll_receive(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Result<Option<TransportEnvelope>, Self::Error>> {
        loop {
            let expected = expected_frame_len(&self.receiving.bytes, self.frame_limits)?;
            if !self.receiving.bytes.is_empty() && self.receiving.bytes.len() == expected {
                let bytes = core::mem::take(&mut self.receiving.bytes);
                let resources = core::mem::take(&mut self.receiving.resources);
                let message = decode_frame(bytes, self.frame_limits)?;
                let envelope = TransportEnvelope::new(message, resources, self.envelope_limits)?;
                return Poll::Ready(Ok(Some(envelope)));
            }
            if self.closed {
                return Poll::Ready(Ok(None));
            }
            match self.poll_receive_chunk(context) {
                Poll::Ready(Ok(())) => {}
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Pending => return Poll::Pending,
            }
        }
    }

    fn poll_send(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        envelope: &mut Option<TransportEnvelope>,
    ) -> Poll<Result<(), Self::Error>> {
        let Some(candidate) = envelope.as_ref() else {
            return Poll::Ready(Err(UnixTransportError::MissingEnvelope));
        };
        let identity = Arc::as_ptr(candidate.message()) as usize;
        if let Some(state) = &self.sending {
            if state.message_identity != identity {
                return Poll::Ready(Err(UnixTransportError::ChangedPendingEnvelope));
            }
        } else {
            // Validate every resource before sending the first frame byte.
            drop(Self::duplicate_fds(candidate)?);
            self.sending = Some(SendState {
                message_identity: identity,
                bytes: encode_message(candidate.message(), self.frame_limits)?,
                offset: 0,
            });
        }
        match self.poll_write_chunk(context, candidate) {
            Poll::Ready(Ok(())) => {
                let complete = self
                    .sending
                    .as_ref()
                    .is_some_and(|state| state.offset == state.bytes.len());
                if complete {
                    self.sending = None;
                    let _ = envelope.take();
                    Poll::Ready(Ok(()))
                } else {
                    context.waker().wake_by_ref();
                    Poll::Pending
                }
            }
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Pending => Poll::Pending,
        }
    }

    fn poll_close(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        match self.io.get_ref().shutdown(std::net::Shutdown::Write) {
            Ok(()) => Poll::Ready(Ok(())),
            Err(error) if error.kind() == io::ErrorKind::NotConnected => Poll::Ready(Ok(())),
            Err(error) => Poll::Ready(Err(error.into())),
        }
    }
}

fn encode_message(
    message: &OwnedMessage,
    limits: FrameLimits,
) -> Result<Vec<u8>, UnixTransportError> {
    let segments = (0..message.segment_count())
        .map(|index| {
            let index = u32::try_from(index).map_err(|_| FrameError::BodyLengthOverflow)?;
            message.segment(index).ok_or(FrameError::BodyLengthOverflow)
        })
        .collect::<Result<Vec<_>, _>>()?;
    encode_frame(&segments, limits).map_err(Into::into)
}

fn decode_frame(
    bytes: Vec<u8>,
    limits: FrameLimits,
) -> Result<Arc<OwnedMessage>, UnixTransportError> {
    let FrameRead::Message { frame, remaining } = parse_frame(&bytes, limits)? else {
        return Err(FrameError::TruncatedHeader { available: 0 }.into());
    };
    if !remaining.is_empty() {
        return Err(FrameError::TrailingData {
            bytes: remaining.len(),
        }
        .into());
    }
    let segments = frame
        .segments()
        .iter()
        .map(|segment| segment.bytes().to_vec().into_boxed_slice())
        .collect::<Vec<_>>();
    OwnedMessage::new(
        segments,
        ReaderLimits {
            traversal_words: limits.max_total_words,
            nesting_levels: 64,
        },
    )
    .map_err(|error| io::Error::other(error.to_string()).into())
}

fn expected_frame_len(bytes: &[u8], limits: FrameLimits) -> Result<usize, UnixTransportError> {
    if bytes.len() < 4 {
        return Ok(4);
    }
    let encoded_count =
        u32::from_le_bytes(
            bytes[..4]
                .try_into()
                .map_err(|_| FrameError::TruncatedHeader {
                    available: bytes.len(),
                })?,
        );
    let count = encoded_count
        .checked_add(1)
        .ok_or(FrameError::SegmentCountOverflow)?;
    if count > limits.max_segments.min(capnp_io::MAX_SEGMENTS) {
        return Err(FrameError::TooManySegments {
            count,
            limit: limits.max_segments.min(capnp_io::MAX_SEGMENTS),
        }
        .into());
    }
    let count = usize::try_from(count).map_err(|_| FrameError::BodyLengthOverflow)?;
    let table_len = (count / 2 + 1)
        .checked_mul(8)
        .ok_or(FrameError::BodyLengthOverflow)?;
    if bytes.len() < table_len {
        return Ok(table_len);
    }
    let mut words = 0_u64;
    for index in 0..count {
        let offset = (index + 1)
            .checked_mul(4)
            .ok_or(FrameError::BodyLengthOverflow)?;
        let encoded = bytes
            .get(offset..offset + 4)
            .ok_or(FrameError::TruncatedSegmentTable {
                expected: table_len,
                available: bytes.len(),
            })?;
        words = words
            .checked_add(u64::from(u32::from_le_bytes(
                encoded
                    .try_into()
                    .map_err(|_| FrameError::BodyLengthOverflow)?,
            )))
            .ok_or(FrameError::TotalWordsOverflow)?;
        if words > limits.max_total_words {
            return Err(FrameError::MessageTooLarge {
                words,
                limit: limits.max_total_words,
            }
            .into());
        }
    }
    let body_len = usize::try_from(words.checked_mul(8).ok_or(FrameError::BodyLengthOverflow)?)
        .map_err(|_| FrameError::BodyLengthOverflow)?;
    table_len
        .checked_add(body_len)
        .ok_or(FrameError::BodyLengthOverflow.into())
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::task::Waker;
    use std::time::Duration;

    fn limits() -> EnvelopeLimits {
        EnvelopeLimits {
            max_message_bytes: 16 * 1024 * 1024,
            max_resources_per_envelope: 8,
            max_resource_bytes_per_envelope: 1024,
            max_queued_envelopes: 8,
            max_queued_bytes: 16 * 1024 * 1024,
            max_queued_resources: 8,
            max_queued_resource_bytes: 1024,
        }
    }

    fn message(words: usize) -> Arc<OwnedMessage> {
        OwnedMessage::new(
            vec![vec![0_u8; words * 8].into_boxed_slice()],
            ReaderLimits::default(),
        )
        .expect("owned message")
    }

    fn transport_pair(
        receiver_fd_limit: usize,
    ) -> (UnixScmRightsTransport, UnixScmRightsTransport) {
        let (left, right) = UnixStream::pair().expect("socket pair");
        let frame_limits = FrameLimits {
            max_segments: 8,
            max_total_words: 2 * 1024 * 1024,
        };
        (
            UnixScmRightsTransport::new(left, limits(), frame_limits, 8).expect("left transport"),
            UnixScmRightsTransport::new(right, limits(), frame_limits, receiver_fd_limit)
                .expect("right transport"),
        )
    }

    fn transfer(
        sender: &mut UnixScmRightsTransport,
        receiver: &mut UnixScmRightsTransport,
        envelope: TransportEnvelope,
    ) -> TransportEnvelope {
        let mut envelope = Some(envelope);
        let mut received = None;
        let mut context = Context::from_waker(Waker::noop());
        for _ in 0..200_000 {
            if envelope.is_some() {
                match Pin::new(&mut *sender).poll_send(&mut context, &mut envelope) {
                    Poll::Ready(Ok(())) | Poll::Pending => {}
                    Poll::Ready(Err(error)) => panic!("send failed: {error}"),
                }
            }
            if received.is_none() {
                match Pin::new(&mut *receiver).poll_receive(&mut context) {
                    Poll::Ready(Ok(Some(envelope))) => received = Some(envelope),
                    Poll::Ready(Ok(None)) => panic!("unexpected EOF"),
                    Poll::Ready(Err(error)) => panic!("receive failed: {error}"),
                    Poll::Pending => {}
                }
            }
            if envelope.is_none() {
                if let Some(received) = received {
                    return received;
                }
            }
            std::thread::yield_now();
        }
        panic!("Unix transport did not complete within the poll bound")
    }

    #[test]
    fn scm_rights_round_trip_keeps_one_owner_through_a_fragmented_frame() {
        let (mut sender, mut receiver) = transport_pair(2);
        let (mut peer, attached) = UnixStream::pair().expect("attached socket pair");
        let attached = AttachedResource::new(OwnedFd::from(attached), 0);
        let envelope = TransportEnvelope::new(
            message(128 * 1024),
            vec![attached.clone().into_transport_resource()],
            limits(),
        )
        .expect("envelope");
        drop(attached);

        let mut pending = Some(envelope);
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(
            Pin::new(&mut sender).poll_send(&mut context, &mut pending),
            Poll::Pending
        ));
        assert!(pending.is_some(), "partial write retains the FD owner");
        let envelope = transfer(
            &mut sender,
            &mut receiver,
            pending.take().expect("pending envelope"),
        );
        let (_, mut resources) = envelope.into_parts();
        let fd = resources
            .pop()
            .expect("one descriptor")
            .downcast::<OwnedFd>()
            .expect("received OwnedFd");
        let mut received = UnixStream::from(fd);
        peer.write_all(b"owned").expect("write through peer");
        let mut bytes = [0_u8; 5];
        received.read_exact(&mut bytes).expect("read received FD");
        assert_eq!(&bytes, b"owned");
    }

    #[test]
    fn receive_limit_discards_and_closes_excess_fds() {
        let (mut sender, mut receiver) = transport_pair(1);
        let (mut first_peer, first_fd) = UnixStream::pair().expect("first pair");
        let (mut excess_peer, excess_fd) = UnixStream::pair().expect("excess pair");
        excess_peer
            .set_read_timeout(Some(Duration::from_secs(1)))
            .expect("read timeout");
        let envelope = TransportEnvelope::new(
            message(1),
            vec![
                OwnedResource::new(OwnedFd::from(first_fd), 0),
                OwnedResource::new(OwnedFd::from(excess_fd), 0),
            ],
            limits(),
        )
        .expect("envelope");
        let envelope = transfer(&mut sender, &mut receiver, envelope);
        let (_, mut resources) = envelope.into_parts();
        assert_eq!(resources.len(), 1);
        let received = resources
            .pop()
            .expect("retained FD")
            .downcast::<OwnedFd>()
            .expect("OwnedFd");
        let mut received = UnixStream::from(received);
        first_peer.write_all(b"x").expect("first peer write");
        let mut byte = [0_u8; 1];
        received.read_exact(&mut byte).expect("first FD retained");
        assert_eq!(byte, [b'x']);

        let mut excess = [0_u8; 1];
        assert_eq!(
            excess_peer
                .read(&mut excess)
                .expect("excess peer observes close"),
            0,
            "truncated descriptor is closed after the send owner drops"
        );
    }

    #[test]
    fn zero_fd_limit_provides_rpc_fallback_without_leaking_authority() {
        let (mut sender, mut receiver) = transport_pair(0);
        let (mut peer, attached) = UnixStream::pair().expect("attached pair");
        peer.set_read_timeout(Some(Duration::from_secs(1)))
            .expect("read timeout");
        let envelope = TransportEnvelope::new(
            message(1),
            vec![OwnedResource::new(OwnedFd::from(attached), 0)],
            limits(),
        )
        .expect("envelope");
        let received = transfer(&mut sender, &mut receiver, envelope);
        assert!(received.resources().is_empty());
        let mut byte = [0_u8; 1];
        assert_eq!(peer.read(&mut byte).expect("peer observes close"), 0);
    }
}
