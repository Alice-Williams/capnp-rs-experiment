//! Owned, executor-neutral RPC transport envelopes.

use std::any::Any;
use std::collections::VecDeque;
use std::fmt;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll, Waker};

use capnp_message::OwnedMessage;

/// Exact per-envelope and queued-resource limits for a transport endpoint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EnvelopeLimits {
    pub max_message_bytes: usize,
    pub max_resources_per_envelope: usize,
    pub max_resource_bytes_per_envelope: usize,
    pub max_queued_envelopes: usize,
    pub max_queued_bytes: usize,
    pub max_queued_resources: usize,
    pub max_queued_resource_bytes: usize,
}

impl Default for EnvelopeLimits {
    fn default() -> Self {
        Self {
            max_message_bytes: 64 * 1024 * 1024,
            max_resources_per_envelope: 64,
            max_resource_bytes_per_envelope: 1024 * 1024,
            max_queued_envelopes: 64,
            max_queued_bytes: 64 * 1024 * 1024,
            max_queued_resources: 256,
            max_queued_resource_bytes: 4 * 1024 * 1024,
        }
    }
}

/// A move-only ancillary resource plus its caller-declared memory reservation.
///
/// The RPC layer does not interpret file descriptors, handles, or application
/// resources. A concrete transport owns that conversion. The byte charge is a
/// portable backpressure accounting unit, not a serialized wire length.
pub struct OwnedResource {
    value: Box<dyn Any + Send + 'static>,
    byte_charge: usize,
}

impl OwnedResource {
    pub fn new<T: Any + Send + 'static>(value: T, byte_charge: usize) -> Self {
        Self {
            value: Box::new(value),
            byte_charge,
        }
    }

    pub const fn byte_charge(&self) -> usize {
        self.byte_charge
    }

    pub fn is<T: Any>(&self) -> bool {
        self.value.is::<T>()
    }

    pub fn downcast<T: Any + Send>(self) -> Result<T, Self> {
        let Self { value, byte_charge } = self;
        match value.downcast::<T>() {
            Ok(value) => Ok(*value),
            Err(value) => Err(Self { value, byte_charge }),
        }
    }
}

impl fmt::Debug for OwnedResource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OwnedResource")
            .field("byte_charge", &self.byte_charge)
            .finish_non_exhaustive()
    }
}

/// One complete message and the resources delivered atomically with it.
#[derive(Debug)]
pub struct TransportEnvelope {
    message: Arc<OwnedMessage>,
    resources: Vec<OwnedResource>,
    message_bytes: usize,
    resource_bytes: usize,
}

impl TransportEnvelope {
    pub fn new(
        message: Arc<OwnedMessage>,
        resources: Vec<OwnedResource>,
        limits: EnvelopeLimits,
    ) -> Result<Self, TransportError> {
        let message_bytes = message_size(&message)?;
        let resource_bytes = resources.iter().try_fold(0usize, |total, resource| {
            total
                .checked_add(resource.byte_charge())
                .ok_or(TransportError::SizeOverflow)
        })?;
        check_limit("message bytes", message_bytes, limits.max_message_bytes)?;
        check_limit(
            "resources per envelope",
            resources.len(),
            limits.max_resources_per_envelope,
        )?;
        check_limit(
            "resource bytes per envelope",
            resource_bytes,
            limits.max_resource_bytes_per_envelope,
        )?;
        Ok(Self {
            message,
            resources,
            message_bytes,
            resource_bytes,
        })
    }

    pub fn message(&self) -> &Arc<OwnedMessage> {
        &self.message
    }

    pub fn resources(&self) -> &[OwnedResource] {
        &self.resources
    }

    pub fn into_parts(self) -> (Arc<OwnedMessage>, Vec<OwnedResource>) {
        (self.message, self.resources)
    }

    pub const fn message_bytes(&self) -> usize {
        self.message_bytes
    }

    pub const fn resource_bytes(&self) -> usize {
        self.resource_bytes
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TransportError {
    Limit {
        resource: &'static str,
        requested: usize,
        limit: usize,
    },
    SizeOverflow,
    Closed,
    MissingEnvelope,
    Poisoned,
}

impl fmt::Display for TransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Limit {
                resource,
                requested,
                limit,
            } => write!(
                formatter,
                "transport {resource} requires {requested}; limit is {limit}"
            ),
            Self::SizeOverflow => formatter.write_str("transport size accounting overflow"),
            Self::Closed => formatter.write_str("transport peer is closed"),
            Self::MissingEnvelope => formatter.write_str("poll_send requires an envelope"),
            Self::Poisoned => formatter.write_str("in-memory transport state was poisoned"),
        }
    }
}

impl std::error::Error for TransportError {}

/// Minimal executor-neutral duplex contract.
///
/// A pending send retains ownership in `envelope`; a successful send consumes
/// it. Implementations must preserve message order and atomically associate
/// ancillary resources with their message. At most one task may poll each
/// direction of an endpoint at a time.
pub trait DuplexTransport: Send + Unpin + 'static {
    type Error: std::error::Error + Send + Sync + 'static;

    fn poll_receive(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Result<Option<TransportEnvelope>, Self::Error>>;

    fn poll_send(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        envelope: &mut Option<TransportEnvelope>,
    ) -> Poll<Result<(), Self::Error>>;

    fn poll_close(self: Pin<&mut Self>, context: &mut Context<'_>)
    -> Poll<Result<(), Self::Error>>;
}

/// One endpoint of a deterministic, bounded in-memory duplex transport.
#[derive(Debug)]
pub struct MemoryTransport {
    incoming: Arc<Mutex<Queue>>,
    outgoing: Arc<Mutex<Queue>>,
    detached: bool,
}

/// Creates two peers with independently bounded queues in each direction.
pub fn memory_transport_pair(limits: EnvelopeLimits) -> (MemoryTransport, MemoryTransport) {
    let left_to_right = Arc::new(Mutex::new(Queue::new(limits)));
    let right_to_left = Arc::new(Mutex::new(Queue::new(limits)));
    (
        MemoryTransport {
            incoming: Arc::clone(&right_to_left),
            outgoing: Arc::clone(&left_to_right),
            detached: false,
        },
        MemoryTransport {
            incoming: left_to_right,
            outgoing: right_to_left,
            detached: false,
        },
    )
}

impl DuplexTransport for MemoryTransport {
    type Error = TransportError;

    fn poll_receive(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Result<Option<TransportEnvelope>, Self::Error>> {
        let mut queue = match lock(&self.incoming) {
            Ok(queue) => queue,
            Err(error) => return Poll::Ready(Err(error)),
        };
        if let Some(envelope) = queue.items.pop_front() {
            queue.queued_bytes = queue.queued_bytes.saturating_sub(envelope.message_bytes());
            queue.queued_resources = queue
                .queued_resources
                .saturating_sub(envelope.resources().len());
            queue.queued_resource_bytes = queue
                .queued_resource_bytes
                .saturating_sub(envelope.resource_bytes());
            let sender = queue.sender.take();
            drop(queue);
            wake(sender);
            return Poll::Ready(Ok(Some(envelope)));
        }
        if queue.closed {
            return Poll::Ready(Ok(None));
        }
        queue.receiver = Some(context.waker().clone());
        Poll::Pending
    }

    fn poll_send(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        envelope: &mut Option<TransportEnvelope>,
    ) -> Poll<Result<(), Self::Error>> {
        let Some(candidate) = envelope.as_ref() else {
            return Poll::Ready(Err(TransportError::MissingEnvelope));
        };
        let mut queue = match lock(&self.outgoing) {
            Ok(queue) => queue,
            Err(error) => return Poll::Ready(Err(error)),
        };
        if queue.closed || !queue.receiver_alive {
            return Poll::Ready(Err(TransportError::Closed));
        }
        if let Err(error) = queue.check_item(candidate) {
            return Poll::Ready(Err(error));
        }
        if !queue.has_capacity(candidate) {
            queue.sender = Some(context.waker().clone());
            return Poll::Pending;
        }
        let Some(candidate) = envelope.take() else {
            return Poll::Ready(Err(TransportError::MissingEnvelope));
        };
        queue.queued_bytes += candidate.message_bytes();
        queue.queued_resources += candidate.resources().len();
        queue.queued_resource_bytes += candidate.resource_bytes();
        queue.items.push_back(candidate);
        let receiver = queue.receiver.take();
        drop(queue);
        wake(receiver);
        Poll::Ready(Ok(()))
    }

    fn poll_close(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        let mut queue = match lock(&self.outgoing) {
            Ok(queue) => queue,
            Err(error) => return Poll::Ready(Err(error)),
        };
        queue.closed = true;
        let receiver = queue.receiver.take();
        drop(queue);
        wake(receiver);
        Poll::Ready(Ok(()))
    }
}

impl Drop for MemoryTransport {
    fn drop(&mut self) {
        if self.detached {
            return;
        }
        let outgoing_waker = if let Ok(mut outgoing) = self.outgoing.lock() {
            outgoing.closed = true;
            outgoing.receiver.take()
        } else {
            None
        };
        wake(outgoing_waker);
        let incoming_waker = if let Ok(mut incoming) = self.incoming.lock() {
            incoming.receiver_alive = false;
            incoming.sender.take()
        } else {
            None
        };
        wake(incoming_waker);
        self.detached = true;
    }
}

#[derive(Debug)]
struct Queue {
    limits: EnvelopeLimits,
    items: VecDeque<TransportEnvelope>,
    queued_bytes: usize,
    queued_resources: usize,
    queued_resource_bytes: usize,
    closed: bool,
    receiver_alive: bool,
    sender: Option<Waker>,
    receiver: Option<Waker>,
}

impl Queue {
    fn new(limits: EnvelopeLimits) -> Self {
        Self {
            limits,
            items: VecDeque::new(),
            queued_bytes: 0,
            queued_resources: 0,
            queued_resource_bytes: 0,
            closed: false,
            receiver_alive: true,
            sender: None,
            receiver: None,
        }
    }

    fn check_item(&self, envelope: &TransportEnvelope) -> Result<(), TransportError> {
        check_limit(
            "queued message bytes",
            envelope.message_bytes(),
            self.limits.max_queued_bytes,
        )?;
        check_limit(
            "queued resources",
            envelope.resources().len(),
            self.limits.max_queued_resources,
        )?;
        check_limit(
            "queued resource bytes",
            envelope.resource_bytes(),
            self.limits.max_queued_resource_bytes,
        )?;
        if self.limits.max_queued_envelopes == 0 {
            return Err(TransportError::Limit {
                resource: "queued envelopes",
                requested: 1,
                limit: 0,
            });
        }
        Ok(())
    }

    fn has_capacity(&self, envelope: &TransportEnvelope) -> bool {
        self.items.len() < self.limits.max_queued_envelopes
            && self
                .queued_bytes
                .checked_add(envelope.message_bytes())
                .is_some_and(|value| value <= self.limits.max_queued_bytes)
            && self
                .queued_resources
                .checked_add(envelope.resources().len())
                .is_some_and(|value| value <= self.limits.max_queued_resources)
            && self
                .queued_resource_bytes
                .checked_add(envelope.resource_bytes())
                .is_some_and(|value| value <= self.limits.max_queued_resource_bytes)
    }
}

fn check_limit(
    resource: &'static str,
    requested: usize,
    limit: usize,
) -> Result<(), TransportError> {
    if requested > limit {
        return Err(TransportError::Limit {
            resource,
            requested,
            limit,
        });
    }
    Ok(())
}

fn message_size(message: &OwnedMessage) -> Result<usize, TransportError> {
    let mut bytes = 0usize;
    for index in 0..message.segment_count() {
        let segment = message
            .segment(u32::try_from(index).map_err(|_| TransportError::SizeOverflow)?)
            .ok_or(TransportError::SizeOverflow)?;
        bytes = bytes
            .checked_add(segment.len())
            .ok_or(TransportError::SizeOverflow)?;
    }
    Ok(bytes)
}

fn lock(queue: &Mutex<Queue>) -> Result<MutexGuard<'_, Queue>, TransportError> {
    queue.lock().map_err(|_| TransportError::Poisoned)
}

fn wake(slot: Option<Waker>) {
    if let Some(waker) = slot {
        waker.wake();
    }
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::Waker;

    use capnp_message::{ExclusiveArena, ReaderLimits};

    fn message(value: u64) -> Arc<OwnedMessage> {
        let mut arena = ExclusiveArena::new(2, 8).expect("arena");
        arena
            .init_root_struct(1, 0)
            .expect("root")
            .set_u64(0, value, 0)
            .expect("value");
        OwnedMessage::new(arena.into_segments(), ReaderLimits::default()).expect("owned")
    }

    fn envelope(value: u64, limits: EnvelopeLimits) -> TransportEnvelope {
        TransportEnvelope::new(message(value), Vec::new(), limits).expect("envelope")
    }

    fn poll_send_now(
        transport: &mut MemoryTransport,
        envelope: &mut Option<TransportEnvelope>,
    ) -> Poll<Result<(), TransportError>> {
        let mut context = Context::from_waker(Waker::noop());
        Pin::new(transport).poll_send(&mut context, envelope)
    }

    fn poll_receive_now(
        transport: &mut MemoryTransport,
    ) -> Poll<Result<Option<TransportEnvelope>, TransportError>> {
        let mut context = Context::from_waker(Waker::noop());
        Pin::new(transport).poll_receive(&mut context)
    }

    #[test]
    fn endpoints_are_send_and_preserve_order() {
        fn assert_send<T: Send>() {}
        assert_send::<MemoryTransport>();
        assert_send::<TransportEnvelope>();

        let limits = EnvelopeLimits::default();
        let (mut left, mut right) = memory_transport_pair(limits);
        for value in [11, 22] {
            let mut item = Some(envelope(value, limits));
            assert!(matches!(
                poll_send_now(&mut left, &mut item),
                Poll::Ready(Ok(()))
            ));
            assert!(item.is_none());
        }
        for expected in [11, 22] {
            let Poll::Ready(Ok(Some(item))) = poll_receive_now(&mut right) else {
                panic!("queued item")
            };
            let root = item.message().root_struct().expect("root").into_root();
            assert_eq!(
                root.with_reader(|reader| {
                    reader
                        .data_section()
                        .expect("data")
                        .read_u64(0, 0)
                        .expect("value")
                })
                .expect("reader"),
                expected
            );
        }
    }

    #[test]
    fn queue_capacity_applies_backpressure_until_receive() {
        let limits = EnvelopeLimits {
            max_queued_envelopes: 1,
            ..EnvelopeLimits::default()
        };
        let (mut left, mut right) = memory_transport_pair(limits);
        let mut first = Some(envelope(1, limits));
        let mut second = Some(envelope(2, limits));
        assert!(matches!(
            poll_send_now(&mut left, &mut first),
            Poll::Ready(Ok(()))
        ));
        assert!(matches!(
            poll_send_now(&mut left, &mut second),
            Poll::Pending
        ));
        assert!(second.is_some());
        assert!(matches!(
            poll_receive_now(&mut right),
            Poll::Ready(Ok(Some(_)))
        ));
        assert!(matches!(
            poll_send_now(&mut left, &mut second),
            Poll::Ready(Ok(()))
        ));
    }

    #[test]
    fn ancillary_resources_move_atomically_and_drop_exactly_once() {
        struct CountDrop(Arc<AtomicUsize>);
        impl Drop for CountDrop {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }

        let limits = EnvelopeLimits::default();
        let drops = Arc::new(AtomicUsize::new(0));
        let resource = OwnedResource::new(CountDrop(Arc::clone(&drops)), 17);
        let item = TransportEnvelope::new(message(3), vec![resource], limits).expect("envelope");
        assert_eq!(item.resource_bytes(), 17);
        let (mut left, mut right) = memory_transport_pair(limits);
        let mut item = Some(item);
        assert!(matches!(
            poll_send_now(&mut left, &mut item),
            Poll::Ready(Ok(()))
        ));
        let Poll::Ready(Ok(Some(item))) = poll_receive_now(&mut right) else {
            panic!("resource envelope")
        };
        let (_, mut resources) = item.into_parts();
        let resource = resources.pop().expect("one resource");
        assert!(resource.is::<CountDrop>());
        drop(resource);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn per_envelope_and_queue_resource_quotas_are_exact() {
        let strict = EnvelopeLimits {
            max_resources_per_envelope: 1,
            max_resource_bytes_per_envelope: 4,
            ..EnvelopeLimits::default()
        };
        assert!(matches!(
            TransportEnvelope::new(message(0), vec![OwnedResource::new((), 5)], strict),
            Err(TransportError::Limit {
                resource: "resource bytes per envelope",
                requested: 5,
                limit: 4
            })
        ));

        let envelope_limits = EnvelopeLimits::default();
        let queue_limits = EnvelopeLimits {
            max_queued_resources: 0,
            ..EnvelopeLimits::default()
        };
        let (mut left, _right) = memory_transport_pair(queue_limits);
        let mut item = Some(
            TransportEnvelope::new(message(0), vec![OwnedResource::new((), 0)], envelope_limits)
                .expect("locally valid"),
        );
        assert!(matches!(
            poll_send_now(&mut left, &mut item),
            Poll::Ready(Err(TransportError::Limit {
                resource: "queued resources",
                requested: 1,
                limit: 0
            }))
        ));
        assert!(item.is_some());
    }

    #[test]
    fn closing_one_direction_delivers_eof_after_queued_messages() {
        let limits = EnvelopeLimits::default();
        let (mut left, mut right) = memory_transport_pair(limits);
        let mut item = Some(envelope(9, limits));
        assert!(matches!(
            poll_send_now(&mut left, &mut item),
            Poll::Ready(Ok(()))
        ));
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(
            Pin::new(&mut left).poll_close(&mut context),
            Poll::Ready(Ok(()))
        ));
        assert!(matches!(
            poll_receive_now(&mut right),
            Poll::Ready(Ok(Some(_)))
        ));
        assert!(matches!(
            poll_receive_now(&mut right),
            Poll::Ready(Ok(None))
        ));
    }
}
