//! Executor-neutral bridge between the connection actor and a duplex transport.

use std::fmt;
use std::pin::Pin;
use std::task::{Context, Poll};

use capnp_rpc_core::{
    ActorEffect, ActorLimits, CompletionToken, ConnectionActor, ConnectionError, ConnectionHandle,
    DuplexTransport, EnvelopeLimits, IncomingRequest, ProtocolLimits, TransportEnvelope,
    TransportError,
};

#[derive(Debug)]
pub struct DriverDispatch {
    pub request: IncomingRequest,
    pub completion: CompletionToken,
}

#[derive(Debug)]
pub enum DriverError<E> {
    Actor(ConnectionError),
    Envelope(TransportError),
    Transport(E),
    UnexpectedResources(usize),
}

impl<E: fmt::Display> fmt::Display for DriverError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Actor(error) => error.fmt(formatter),
            Self::Envelope(error) => error.fmt(formatter),
            Self::Transport(error) => error.fmt(formatter),
            Self::UnexpectedResources(count) => {
                write!(
                    formatter,
                    "Level-0 RPC received {count} ancillary resources"
                )
            }
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for DriverError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Actor(error) => Some(error),
            Self::Envelope(error) => Some(error),
            Self::Transport(error) => Some(error),
            Self::UnexpectedResources(_) => None,
        }
    }
}

/// Drives one connection without choosing an executor or spawning handlers.
///
/// Each ready item is an application dispatch. The caller may run independent
/// dispatches on any executor and return through their `CompletionToken`s.
/// Outbound frames are drained before further inbound reads, preserving actor
/// order and transport backpressure. `Ready(Ok(None))` means the transport is
/// closed and all actor waiters have been completed.
pub struct ConnectionDriver<T: DuplexTransport> {
    transport: T,
    actor: ConnectionActor,
    handle: ConnectionHandle,
    envelope_limits: EnvelopeLimits,
    outbound: Option<TransportEnvelope>,
    closing: bool,
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
                Poll::Ready(Some(ActorEffect::Dispatch {
                    request,
                    completion,
                })) => {
                    return Poll::Ready(Ok(Some(DriverDispatch {
                        request,
                        completion,
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
                    if !resources.is_empty() {
                        let count = resources.len();
                        drop(resources);
                        self.request_shutdown();
                        return Poll::Ready(Err(DriverError::UnexpectedResources(count)));
                    }
                    self.handle.receive(message).map_err(DriverError::Actor)?;
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
}
