//! Executor-neutral bridge between the connection actor and a duplex transport.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use capnp_message::OwnedMessage;
use capnp_rpc_core::{
    ActorEffect, ActorLimits, CancellationSignal, CompletionToken, ConnectionActor,
    ConnectionError, ConnectionHandle, DuplexTransport, EnvelopeLimits, HandlerResult,
    IncomingRequest, LocalCompletionToken, OutgoingCapability, ProtocolLimits, TransportEnvelope,
    TransportError,
};
use std::sync::Arc;

#[derive(Debug)]
pub struct DriverDispatch {
    pub request: IncomingRequest,
    pub completion: DriverCompletion,
}

#[derive(Debug)]
pub enum DriverCompletion {
    Remote(CompletionToken),
    Local(LocalCompletionToken),
}

impl DriverCompletion {
    /// Returns the cooperative cancellation signal for a peer-originated
    /// dispatch. Calls shortened to a local capability have no wire caller and
    /// therefore no remote cancellation signal.
    pub fn cancellation(&self) -> Option<CancellationSignal> {
        match self {
            Self::Remote(completion) => Some(completion.cancellation()),
            Self::Local(_) => None,
        }
    }

    /// Opts a peer-originated dispatch out of cancellation before cancellation
    /// wins the race. Local dispatches already run independently of a peer
    /// `Finish` and return `true`.
    pub fn disallow_cancellation(&self) -> bool {
        match self {
            Self::Remote(completion) => completion.disallow_cancellation(),
            Self::Local(_) => true,
        }
    }

    pub fn complete(self, result: HandlerResult) -> Result<(), ConnectionError> {
        match self {
            Self::Remote(completion) => completion.complete(result),
            Self::Local(completion) => completion.complete(result),
        }
    }

    pub fn complete_with_capabilities(
        self,
        content: Arc<OwnedMessage>,
        capabilities: Vec<OutgoingCapability>,
    ) -> Result<(), ConnectionError> {
        match self {
            Self::Remote(completion) => {
                completion.complete_with_capabilities(content, capabilities)
            }
            Self::Local(completion) => completion.complete_with_capabilities(content, capabilities),
        }
    }
}

#[derive(Debug)]
pub enum DriverError<E> {
    Actor(ConnectionError),
    Envelope(TransportError),
    Transport(E),
}

impl<E: fmt::Display> fmt::Display for DriverError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Actor(error) => error.fmt(formatter),
            Self::Envelope(error) => error.fmt(formatter),
            Self::Transport(error) => error.fmt(formatter),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for DriverError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Actor(error) => Some(error),
            Self::Envelope(error) => Some(error),
            Self::Transport(error) => Some(error),
        }
    }
}

/// Drives one connection without choosing an executor or spawning handlers.
///
/// Each ready item is an application dispatch. The caller may run independent
/// dispatches on any executor and return through their `DriverCompletion`s.
/// Outbound frames are drained before further inbound reads, preserving actor
/// order, attached-resource ownership, and transport backpressure.
/// `Ready(Ok(None))` means the transport is closed and all actor waiters have
/// been completed.
pub struct ConnectionDriver<T: DuplexTransport> {
    transport: T,
    actor: ConnectionActor,
    handle: ConnectionHandle,
    envelope_limits: EnvelopeLimits,
    outbound: Option<TransportEnvelope>,
    closing: bool,
}

/// A shutdown operation that keeps driving the actor until the transport's
/// close operation itself completes.
pub struct DriverShutdown<'a, T: DuplexTransport> {
    driver: &'a mut ConnectionDriver<T>,
    started: bool,
}

impl<T: DuplexTransport> fmt::Debug for DriverShutdown<'_, T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DriverShutdown")
            .field("started", &self.started)
            .finish_non_exhaustive()
    }
}

impl<T: DuplexTransport> Future for DriverShutdown<'_, T> {
    type Output = Result<(), DriverError<T::Error>>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        if !self.started {
            self.started = true;
            if let Err(error) = self.driver.handle.shutdown() {
                if !matches!(error, ConnectionError::Disconnected) {
                    return Poll::Ready(Err(DriverError::Actor(error)));
                }
            }
        }
        loop {
            match self.driver.poll_next_dispatch(context) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Ok(Some(_dispatch))) => {
                    // Shutdown owns the driver exclusively. Any dispatch that
                    // raced ahead of the shutdown command is abandoned; the
                    // actor's terminal transition cancels its signal.
                }
                Poll::Ready(Ok(None)) => return Poll::Ready(Ok(())),
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
            }
        }
    }
}

impl<T: DuplexTransport> fmt::Debug for ConnectionDriver<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConnectionDriver")
            .field("actor", &self.actor)
            .field("outbound", &self.outbound.is_some())
            .field("closing", &self.closing)
            .finish_non_exhaustive()
    }
}

impl<T: DuplexTransport> ConnectionDriver<T> {
    pub fn new(
        transport: T,
        actor_limits: ActorLimits,
        protocol_limits: ProtocolLimits,
        envelope_limits: EnvelopeLimits,
    ) -> (ConnectionHandle, Self) {
        let (handle, actor) = ConnectionActor::new(actor_limits, protocol_limits);
        (
            handle.clone(),
            Self {
                transport,
                actor,
                handle,
                envelope_limits,
                outbound: None,
                closing: false,
            },
        )
    }

    pub fn stats(&self) -> capnp_rpc_core::ConnectionStats {
        self.actor.stats()
    }

    pub fn shutdown(&mut self) -> DriverShutdown<'_, T> {
        DriverShutdown {
            driver: self,
            started: false,
        }
    }

    /// Releases settled import references and queues one batched Level-1
    /// `Release` before subsequent transport reads.
    pub fn release_import(
        &mut self,
        import_id: u32,
        reference_count: u32,
    ) -> Result<(), capnp_rpc_core::ConnectionError> {
        self.actor.release_import(import_id, reference_count)
    }

    pub fn poll_next_dispatch(
        &mut self,
        context: &mut Context<'_>,
    ) -> Poll<Result<Option<DriverDispatch>, DriverError<T::Error>>> {
        loop {
            if self.outbound.is_some() {
                match Pin::new(&mut self.transport).poll_send(context, &mut self.outbound) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(Ok(())) => {}
                    Poll::Ready(Err(error)) => {
                        self.request_shutdown();
                        return Poll::Ready(Err(DriverError::Transport(error)));
                    }
                }
            }

            if self.closing {
                return match Pin::new(&mut self.transport).poll_close(context) {
                    Poll::Pending => Poll::Pending,
                    Poll::Ready(Ok(())) => Poll::Ready(Ok(None)),
                    Poll::Ready(Err(error)) => Poll::Ready(Err(DriverError::Transport(error))),
                };
            }

            match self.actor.poll_next_effect(context) {
                Poll::Ready(Some(ActorEffect::Send(message))) => {
                    self.outbound =
                        match TransportEnvelope::new(message, Vec::new(), self.envelope_limits) {
                            Ok(envelope) => Some(envelope),
                            Err(error) => {
                                self.request_shutdown();
                                return Poll::Ready(Err(DriverError::Envelope(error)));
                            }
                        };
                    continue;
                }
                Poll::Ready(Some(ActorEffect::SendWithResources { message, resources })) => {
                    self.outbound =
                        match TransportEnvelope::new(message, resources, self.envelope_limits) {
                            Ok(envelope) => Some(envelope),
                            Err(error) => {
                                self.request_shutdown();
                                return Poll::Ready(Err(DriverError::Envelope(error)));
                            }
                        };
                    continue;
                }
                Poll::Ready(Some(ActorEffect::Dispatch {
                    request,
                    completion,
                })) => {
                    return Poll::Ready(Ok(Some(DriverDispatch {
                        request,
                        completion: DriverCompletion::Remote(completion),
                    })));
                }
                Poll::Ready(Some(ActorEffect::DispatchLocal {
                    request,
                    completion,
                })) => {
                    return Poll::Ready(Ok(Some(DriverDispatch {
                        request,
                        completion: DriverCompletion::Local(completion),
                    })));
                }
                Poll::Ready(Some(ActorEffect::CloseTransport)) | Poll::Ready(None) => {
                    self.closing = true;
                    continue;
                }
                Poll::Pending => {}
            }

            match Pin::new(&mut self.transport).poll_receive(context) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Ok(Some(envelope))) => {
                    let (message, resources) = envelope.into_parts();
                    self.handle
                        .receive_with_resources(message, resources)
                        .map_err(DriverError::Actor)?;
                }
                Poll::Ready(Ok(None)) => {
                    self.request_shutdown();
                }
                Poll::Ready(Err(error)) => {
                    self.request_shutdown();
                    return Poll::Ready(Err(DriverError::Transport(error)));
                }
            }
        }
    }

    fn request_shutdown(&mut self) {
        if !self.closing {
            let _ = self.handle.shutdown();
        }
    }
}

fn _assert_driver_is_send<T: DuplexTransport>() {
    fn assert_send<T: Send>() {}
    assert_send::<ConnectionDriver<T>>();
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::Waker;

    use capnp_message::{ExclusiveArena, OwnedMessage, ReaderLimits};
    use capnp_rpc_core::{HandlerResult, ReturnPayload, memory_transport_pair};

    fn message(value: u64) -> Arc<OwnedMessage> {
        let mut arena = ExclusiveArena::new(2, 16).expect("arena");
        arena
            .init_root_struct(1, 0)
            .expect("root")
            .set_u64(0, value, 0)
            .expect("value");
        OwnedMessage::new(arena.into_segments(), ReaderLimits::default()).expect("owned")
    }

    fn drive<T: DuplexTransport>(
        driver: &mut ConnectionDriver<T>,
    ) -> Poll<Result<Option<DriverDispatch>, DriverError<T::Error>>> {
        let mut context = Context::from_waker(Waker::noop());
        driver.poll_next_dispatch(&mut context)
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct CloseError;

    impl fmt::Display for CloseError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("close failed")
        }
    }

    impl std::error::Error for CloseError {}

    #[derive(Debug)]
    struct StagedCloseTransport {
        close_polls: Arc<AtomicUsize>,
        fail_close: bool,
    }

    impl DuplexTransport for StagedCloseTransport {
        type Error = CloseError;

        fn poll_receive(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
        ) -> Poll<Result<Option<TransportEnvelope>, Self::Error>> {
            Poll::Pending
        }

        fn poll_send(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
            envelope: &mut Option<TransportEnvelope>,
        ) -> Poll<Result<(), Self::Error>> {
            let _ = envelope.take();
            Poll::Ready(Ok(()))
        }

        fn poll_close(
            self: Pin<&mut Self>,
            context: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            let poll = self.close_polls.fetch_add(1, Ordering::SeqCst);
            if self.fail_close {
                Poll::Ready(Err(CloseError))
            } else if poll == 0 {
                context.waker().wake_by_ref();
                Poll::Pending
            } else {
                Poll::Ready(Ok(()))
            }
        }
    }

    #[test]
    fn two_drivers_exchange_level_zero_request_return_and_finish() {
        let envelope_limits = EnvelopeLimits::default();
        let (left_transport, right_transport) = memory_transport_pair(envelope_limits);
        let (left_handle, mut left) = ConnectionDriver::new(
            left_transport,
            ActorLimits::default(),
            ProtocolLimits::default(),
            envelope_limits,
        );
        let (_right_handle, mut right) = ConnectionDriver::new(
            right_transport,
            ActorLimits::default(),
            ProtocolLimits::default(),
            envelope_limits,
        );

        let mut response = left_handle.bootstrap().expect("bootstrap");
        assert!(matches!(drive(&mut left), Poll::Pending));
        let Poll::Ready(Ok(Some(dispatch))) = drive(&mut right) else {
            panic!("right dispatch")
        };
        assert!(matches!(dispatch.request, IncomingRequest::Bootstrap));
        dispatch
            .completion
            .complete(HandlerResult::Results(message(91)))
            .expect("completion");
        assert!(matches!(drive(&mut right), Poll::Pending));
        assert!(matches!(drive(&mut left), Poll::Pending));

        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(
            Pin::new(&mut response).poll(&mut context),
            Poll::Ready(Ok(ReturnPayload::Results(_)))
        ));
        assert!(matches!(drive(&mut right), Poll::Pending));
        assert_eq!(left.stats().active_questions, 0);
        assert_eq!(right.stats().active_answers, 0);
    }

    #[test]
    fn shutdown_future_waits_for_transport_close_completion() {
        let close_polls = Arc::new(AtomicUsize::new(0));
        let transport = StagedCloseTransport {
            close_polls: Arc::clone(&close_polls),
            fail_close: false,
        };
        let (_handle, mut driver) = ConnectionDriver::new(
            transport,
            ActorLimits::default(),
            ProtocolLimits::default(),
            EnvelopeLimits::default(),
        );
        let mut shutdown = driver.shutdown();
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(
            Pin::new(&mut shutdown).poll(&mut context),
            Poll::Pending
        ));
        assert_eq!(close_polls.load(Ordering::SeqCst), 1);
        assert!(matches!(
            Pin::new(&mut shutdown).poll(&mut context),
            Poll::Ready(Ok(()))
        ));
        assert_eq!(close_polls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn shutdown_future_surfaces_transport_close_error() {
        let close_polls = Arc::new(AtomicUsize::new(0));
        let transport = StagedCloseTransport {
            close_polls: Arc::clone(&close_polls),
            fail_close: true,
        };
        let (_handle, mut driver) = ConnectionDriver::new(
            transport,
            ActorLimits::default(),
            ProtocolLimits::default(),
            EnvelopeLimits::default(),
        );
        let mut shutdown = driver.shutdown();
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(
            Pin::new(&mut shutdown).poll(&mut context),
            Poll::Ready(Err(DriverError::Transport(CloseError)))
        ));
        assert_eq!(close_polls.load(Ordering::SeqCst), 1);
    }
}
