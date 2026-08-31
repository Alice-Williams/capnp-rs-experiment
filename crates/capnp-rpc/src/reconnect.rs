//! Capability recreation after connection-scoped state is lost.
//!
//! A lease keeps an old capability alive for in-flight work, but every newly
//! created capability receives a monotonically increasing generation. A
//! disconnect observed by a stale lease therefore cannot invalidate a newer
//! connection. Overload is classified as backoff, never as a reconnect signal.

use std::fmt;
use std::sync::{Arc, Mutex};

use capnp_rpc_core::{ConnectionError, ExceptionType, RpcException};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetryDisposition {
    Reconnect,
    Backoff,
    Stop,
}

pub fn classify_connection_error(error: &ConnectionError) -> RetryDisposition {
    match error {
        ConnectionError::Disconnected
        | ConnectionError::RemoteAbort(RpcException {
            kind: ExceptionType::Disconnected,
            ..
        }) => RetryDisposition::Reconnect,
        ConnectionError::Overloaded { .. }
        | ConnectionError::RemoteAbort(RpcException {
            kind: ExceptionType::Overloaded,
            ..
        }) => RetryDisposition::Backoff,
        ConnectionError::QuestionLimit { .. }
        | ConnectionError::AnswerLimit { .. }
        | ConnectionError::IncomingCallByteLimit { .. }
        | ConnectionError::DuplicateAnswer(_)
        | ConnectionError::UnknownQuestion(_)
        | ConnectionError::StaleTarget(_)
        | ConnectionError::StaleAnswer(_)
        | ConnectionError::GenerationExhausted
        | ConnectionError::Unimplemented
        | ConnectionError::Canceled
        | ConnectionError::RemoteAbort(_)
        | ConnectionError::Protocol(_)
        | ConnectionError::Wire(_)
        | ConnectionError::Capability(_)
        | ConnectionError::Poisoned
        | ConnectionError::PolledAfterCompletion => RetryDisposition::Stop,
    }
}

pub fn classify_exception(exception: &RpcException) -> RetryDisposition {
    match exception.kind {
        ExceptionType::Disconnected => RetryDisposition::Reconnect,
        ExceptionType::Overloaded => RetryDisposition::Backoff,
        ExceptionType::Failed | ExceptionType::Unimplemented | ExceptionType::Unrecognized(_) => {
            RetryDisposition::Stop
        }
    }
}

pub struct ReconnectLease<T> {
    generation: u64,
    capability: Arc<T>,
}

impl<T> Clone for ReconnectLease<T> {
    fn clone(&self) -> Self {
        Self {
            generation: self.generation,
            capability: Arc::clone(&self.capability),
        }
    }
}

impl<T> fmt::Debug for ReconnectLease<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReconnectLease")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

impl<T> ReconnectLease<T> {
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    pub fn capability(&self) -> &T {
        &self.capability
    }

    pub fn shared_capability(&self) -> Arc<T> {
        Arc::clone(&self.capability)
    }
}

pub struct CapabilityReconnector<T, F> {
    state: Mutex<ReconnectState<T, F>>,
}

struct ReconnectState<T, F> {
    connect: F,
    current: Option<ReconnectLease<T>>,
    next_generation: u64,
}

impl<T, F> fmt::Debug for CapabilityReconnector<T, F> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let generation = self
            .state
            .lock()
            .ok()
            .and_then(|state| state.current.as_ref().map(ReconnectLease::generation));
        formatter
            .debug_struct("CapabilityReconnector")
            .field("current_generation", &generation)
            .finish_non_exhaustive()
    }
}

impl<T, F> CapabilityReconnector<T, F>
where
    F: FnMut() -> Result<T, ConnectionError>,
{
    pub fn new(connect: F) -> Self {
        Self {
            state: Mutex::new(ReconnectState {
                connect,
                current: None,
                next_generation: 0,
            }),
        }
    }

    pub fn current(&self) -> Result<ReconnectLease<T>, ConnectionError> {
        let mut state = self.state.lock().map_err(|_| ConnectionError::Poisoned)?;
        if let Some(current) = &state.current {
            return Ok(current.clone());
        }
        let generation = state.next_generation;
        let next_generation = generation
            .checked_add(1)
            .ok_or(ConnectionError::GenerationExhausted)?;
        let capability = Arc::new((state.connect)()?);
        let lease = ReconnectLease {
            generation,
            capability,
        };
        state.next_generation = next_generation;
        state.current = Some(lease.clone());
        Ok(lease)
    }

    pub fn observe_error(
        &self,
        generation: u64,
        error: &ConnectionError,
    ) -> Result<RetryDisposition, ConnectionError> {
        let disposition = classify_connection_error(error);
        if disposition == RetryDisposition::Reconnect {
            let mut state = self.state.lock().map_err(|_| ConnectionError::Poisoned)?;
            if state
                .current
                .as_ref()
                .is_some_and(|current| current.generation == generation)
            {
                state.current = None;
            }
        }
        Ok(disposition)
    }

    /// Invalidates the current capability without affecting in-flight leases.
    pub fn reset(&self) -> Result<(), ConnectionError> {
        let mut state = self.state.lock().map_err(|_| ConnectionError::Poisoned)?;
        state.current = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn reconnects_only_on_disconnect_and_never_reuses_generations() {
        let connects = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&connects);
        let reconnect =
            CapabilityReconnector::new(move || Ok(counter.fetch_add(1, Ordering::SeqCst)));
        let first = reconnect.current().expect("first");
        assert_eq!(first.generation(), 0);
        assert_eq!(*first.capability(), 0);
        assert_eq!(reconnect.current().expect("shared").generation(), 0);
        assert_eq!(connects.load(Ordering::SeqCst), 1);

        assert_eq!(
            reconnect
                .observe_error(
                    first.generation(),
                    &ConnectionError::Overloaded { capacity: 1 }
                )
                .expect("classify"),
            RetryDisposition::Backoff
        );
        assert_eq!(reconnect.current().expect("same").generation(), 0);

        assert_eq!(
            reconnect
                .observe_error(first.generation(), &ConnectionError::Disconnected)
                .expect("classify"),
            RetryDisposition::Reconnect
        );
        let second = reconnect.current().expect("second");
        assert_eq!(second.generation(), 1);
        assert_eq!(*second.capability(), 1);

        reconnect
            .observe_error(first.generation(), &ConnectionError::Disconnected)
            .expect("stale disconnect");
        assert_eq!(reconnect.current().expect("still second").generation(), 1);
        reconnect.reset().expect("reset");
        assert_eq!(reconnect.current().expect("third").generation(), 2);
    }

    #[test]
    fn concurrent_first_use_constructs_once() {
        let connects = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&connects);
        let reconnect = Arc::new(CapabilityReconnector::new(move || {
            Ok(counter.fetch_add(1, Ordering::SeqCst))
        }));
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let reconnect = Arc::clone(&reconnect);
                scope.spawn(move || {
                    assert_eq!(reconnect.current().expect("current").generation(), 0);
                });
            }
        });
        assert_eq!(connects.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn remote_exception_types_have_distinct_retry_policy() {
        let disconnected = RpcException::new("gone", ExceptionType::Disconnected);
        let overloaded = RpcException::new("busy", ExceptionType::Overloaded);
        let failed = RpcException::new("bad call", ExceptionType::Failed);
        assert_eq!(
            classify_exception(&disconnected),
            RetryDisposition::Reconnect
        );
        assert_eq!(classify_exception(&overloaded), RetryDisposition::Backoff);
        assert_eq!(classify_exception(&failed), RetryDisposition::Stop);
        assert_eq!(
            classify_connection_error(&ConnectionError::RemoteAbort(disconnected)),
            RetryDisposition::Reconnect
        );
        assert_eq!(
            classify_connection_error(&ConnectionError::RemoteAbort(overloaded)),
            RetryDisposition::Backoff
        );
    }
}
