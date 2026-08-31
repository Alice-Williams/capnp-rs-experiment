//! Per-capability streaming flow control compatible with pinned C++ behavior.
//!
//! `FlowController::send_now()` invokes the send closure immediately while a
//! per-stream gate preserves submission order. Its returned `FlowReady` future
//! says only when it is advisable to send the next message. Dropping that
//! future never cancels the already-recorded send. Acknowledgements drive fixed
//! or adaptive bandwidth-delay-product windows without holding a lock while a
//! waker is invoked.

use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

const DEFAULT_MIN_WINDOW: u64 = 64 * 1024;
const DEFAULT_MAX_WINDOW: u64 = 1024 * 1024 * 1024;
const STARTUP_EXIT_ROUNDS: u8 = 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FlowLimits {
    pub max_message_bytes: u64,
    pub max_in_flight_bytes: u64,
    pub max_blocked_senders: usize,
    pub min_window_bytes: u64,
    pub max_window_bytes: u64,
}

impl Default for FlowLimits {
    fn default() -> Self {
        Self {
            max_message_bytes: 64 * 1024 * 1024,
            max_in_flight_bytes: 1024 * 1024 * 1024 + 64 * 1024 * 1024,
            max_blocked_senders: 4096,
            min_window_bytes: DEFAULT_MIN_WINDOW,
            max_window_bytes: DEFAULT_MAX_WINDOW,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FlowError {
    InvalidLimits,
    MessageTooLarge { size: u64, limit: u64 },
    InFlightLimit { requested: u64, limit: u64 },
    BlockedSenderLimit { limit: usize },
    InvalidTimestamp,
    AckAlreadyCompleted,
    Closed,
    Failed(Arc<str>),
    Poisoned,
}

impl fmt::Display for FlowError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for FlowError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FlowMode {
    Fixed,
    Adaptive,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FlowStats {
    pub mode: FlowMode,
    pub window_bytes: u64,
    pub bytes_in_flight: u64,
    pub max_message_bytes_seen: u64,
    pub delivered_bytes: u64,
    pub outstanding_acks: usize,
    pub blocked_senders: usize,
    pub min_rtt: Option<Duration>,
    pub startup: bool,
    pub closed: bool,
}

#[derive(Clone)]
pub struct FlowController {
    inner: Arc<FlowInner>,
}

impl fmt::Debug for FlowController {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FlowController")
            .field("stats", &self.stats())
            .finish_non_exhaustive()
    }
}

impl FlowController {
    pub fn fixed(window_bytes: u64, limits: FlowLimits) -> Result<Self, FlowError> {
        Self::new(window_bytes, limits, ControllerMode::Fixed)
    }

    pub fn adaptive(initial_window_bytes: u64, limits: FlowLimits) -> Result<Self, FlowError> {
        Self::new(initial_window_bytes, limits, ControllerMode::Adaptive)
    }

    fn new(
        initial_window_bytes: u64,
        limits: FlowLimits,
        mode: ControllerMode,
    ) -> Result<Self, FlowError> {
        if initial_window_bytes == 0
            || limits.max_message_bytes == 0
            || limits.max_in_flight_bytes == 0
            || limits.max_blocked_senders == 0
            || limits.min_window_bytes == 0
            || limits.min_window_bytes > limits.max_window_bytes
            || initial_window_bytes > limits.max_window_bytes
        {
            return Err(FlowError::InvalidLimits);
        }
        Ok(Self {
            inner: Arc::new(FlowInner {
                send_gate: Mutex::new(()),
                state: Mutex::new(FlowState::new(initial_window_bytes, limits, mode)),
            }),
        })
    }

    /// Records and invokes one ordered send immediately. The returned future
    /// controls only when another send is advisable; it does not own the send.
    pub fn send_now<T>(
        &self,
        size_bytes: u64,
        sent_at: Duration,
        send: impl FnOnce() -> T,
    ) -> Result<(T, FlowSend), FlowError> {
        let _send_guard = self
            .inner
            .send_gate
            .lock()
            .map_err(|_| FlowError::Poisoned)?;
        let sent_micros = duration_micros(sent_at)?;
        let (id, blocked) = {
            let mut state = self.inner.state.lock().map_err(|_| FlowError::Poisoned)?;
            state.record_send(size_bytes, sent_micros)?
        };
        let output = send();
        let weak = Arc::downgrade(&self.inner);
        Ok((
            output,
            FlowSend {
                acknowledgement: FlowAck {
                    inner: weak.clone(),
                    send_id: Some(id),
                },
                ready: FlowReady {
                    inner: weak,
                    send_id: blocked.then_some(id),
                    immediate: (!blocked).then_some(Ok(())),
                },
            },
        ))
    }

    pub fn wait_all_acked(&self) -> AllAcked {
        AllAcked {
            inner: Arc::downgrade(&self.inner),
        }
    }

    pub fn close(&self) -> Result<(), FlowError> {
        let wakers = {
            let mut state = self.inner.state.lock().map_err(|_| FlowError::Poisoned)?;
            state.closed = true;
            state.take_all_wakers()
        };
        wake_all(wakers);
        Ok(())
    }

    pub fn stats(&self) -> Result<FlowStats, FlowError> {
        let state = self.inner.state.lock().map_err(|_| FlowError::Poisoned)?;
        Ok(state.stats())
    }
}

impl Drop for FlowController {
    fn drop(&mut self) {
        if Arc::strong_count(&self.inner) == 1 {
            let _ = self.close();
        }
    }
}

pub struct FlowSend {
    acknowledgement: FlowAck,
    ready: FlowReady,
}

impl fmt::Debug for FlowSend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("FlowSend").finish_non_exhaustive()
    }
}

impl FlowSend {
    pub fn into_parts(self) -> (FlowAck, FlowReady) {
        (self.acknowledgement, self.ready)
    }
}

pub struct FlowAck {
    inner: Weak<FlowInner>,
    send_id: Option<u64>,
}

impl fmt::Debug for FlowAck {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FlowAck")
            .field("pending", &self.send_id.is_some())
            .finish()
    }
}

impl FlowAck {
    pub fn acknowledge(mut self, acknowledged_at: Duration) -> Result<(), FlowError> {
        let id = self.send_id.take().ok_or(FlowError::AckAlreadyCompleted)?;
        let Some(inner) = self.inner.upgrade() else {
            return Ok(());
        };
        let acknowledged_micros = duration_micros(acknowledged_at)?;
        let wakers = {
            let mut state = inner.state.lock().map_err(|_| FlowError::Poisoned)?;
            state.acknowledge(id, acknowledged_micros)?
        };
        wake_all(wakers);
        Ok(())
    }

    pub fn fail(mut self, reason: impl Into<Arc<str>>) -> Result<(), FlowError> {
        let id = self.send_id.take().ok_or(FlowError::AckAlreadyCompleted)?;
        let Some(inner) = self.inner.upgrade() else {
            return Ok(());
        };
        let reason = reason.into();
        let wakers = {
            let mut state = inner.state.lock().map_err(|_| FlowError::Poisoned)?;
            state.fail(id, Arc::clone(&reason))?
        };
        wake_all(wakers);
        Ok(())
    }
}

pub struct FlowReady {
    inner: Weak<FlowInner>,
    send_id: Option<u64>,
    immediate: Option<Result<(), FlowError>>,
}

impl fmt::Debug for FlowReady {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FlowReady")
            .field("blocked", &self.send_id.is_some())
            .finish()
    }
}

impl Future for FlowReady {
    type Output = Result<(), FlowError>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        if let Some(outcome) = self.immediate.take() {
            return Poll::Ready(outcome);
        }
        let Some(id) = self.send_id else {
            return Poll::Ready(Ok(()));
        };
        let Some(inner) = self.inner.upgrade() else {
            self.send_id = None;
            return Poll::Ready(Ok(()));
        };
        let outcome = {
            let mut state = match inner.state.lock() {
                Ok(state) => state,
                Err(_) => return Poll::Ready(Err(FlowError::Poisoned)),
            };
            if state.closed {
                Some(Ok(()))
            } else if let Some(reason) = &state.failed {
                Some(Err(FlowError::Failed(Arc::clone(reason))))
            } else if let Some(waiter) = state.blocked.get_mut(&id) {
                *waiter = Some(context.waker().clone());
                None
            } else {
                Some(Ok(()))
            }
        };
        match outcome {
            Some(outcome) => {
                self.send_id = None;
                Poll::Ready(outcome)
            }
            None => Poll::Pending,
        }
    }
}

impl Drop for FlowReady {
    fn drop(&mut self) {
        let Some(id) = self.send_id.take() else {
            return;
        };
        if let Some(inner) = self.inner.upgrade() {
            if let Ok(mut state) = inner.state.lock() {
                state.blocked.remove(&id);
            }
        }
    }
}

pub struct AllAcked {
    inner: Weak<FlowInner>,
}

impl fmt::Debug for AllAcked {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("AllAcked").finish_non_exhaustive()
    }
}

impl Future for AllAcked {
    type Output = Result<(), FlowError>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let Some(inner) = self.inner.upgrade() else {
            return Poll::Ready(Ok(()));
        };
        let mut state = match inner.state.lock() {
            Ok(state) => state,
            Err(_) => return Poll::Ready(Err(FlowError::Poisoned)),
        };
        if let Some(reason) = &state.failed {
            Poll::Ready(Err(FlowError::Failed(Arc::clone(reason))))
        } else if state.closed || state.sends.is_empty() {
            Poll::Ready(Ok(()))
        } else {
            state.all_acked_waker = Some(context.waker().clone());
            Poll::Pending
        }
    }
}

struct FlowInner {
    send_gate: Mutex<()>,
    state: Mutex<FlowState>,
}

#[derive(Clone, Copy)]
enum ControllerMode {
    Fixed,
    Adaptive,
}

#[derive(Clone, Copy)]
struct SendSnapshot {
    sent_micros: u64,
    size: u64,
    delivered_at_send: u64,
    delivered_time_at_send: Option<u64>,
    window_at_send: u64,
    window_full_at_send: bool,
}

struct FlowState {
    mode: ControllerMode,
    limits: FlowLimits,
    window: u64,
    bytes_in_flight: u64,
    max_message_size: u64,
    delivered: u64,
    delivered_time: Option<u64>,
    first_ack: Option<(u64, u64)>,
    min_rtt_micros: Option<u64>,
    startup: bool,
    rounds_without_increase: u8,
    last_round_window: u64,
    round_start_micros: Option<u64>,
    next_send_id: u64,
    sends: BTreeMap<u64, SendSnapshot>,
    blocked: BTreeMap<u64, Option<Waker>>,
    all_acked_waker: Option<Waker>,
    failed: Option<Arc<str>>,
    closed: bool,
}

impl FlowState {
    fn new(window: u64, limits: FlowLimits, mode: ControllerMode) -> Self {
        Self {
            mode,
            limits,
            window,
            bytes_in_flight: 0,
            max_message_size: 0,
            delivered: 0,
            delivered_time: None,
            first_ack: None,
            min_rtt_micros: None,
            startup: matches!(mode, ControllerMode::Adaptive),
            rounds_without_increase: 0,
            last_round_window: 0,
            round_start_micros: None,
            next_send_id: 0,
            sends: BTreeMap::new(),
            blocked: BTreeMap::new(),
            all_acked_waker: None,
            failed: None,
            closed: false,
        }
    }

    fn record_send(&mut self, size: u64, sent_micros: u64) -> Result<(u64, bool), FlowError> {
        if self.closed {
            return Err(FlowError::Closed);
        }
        if let Some(reason) = &self.failed {
            return Err(FlowError::Failed(Arc::clone(reason)));
        }
        if size > self.limits.max_message_bytes {
            return Err(FlowError::MessageTooLarge {
                size,
                limit: self.limits.max_message_bytes,
            });
        }
        let in_flight = self
            .bytes_in_flight
            .checked_add(size)
            .ok_or(FlowError::InFlightLimit {
                requested: u64::MAX,
                limit: self.limits.max_in_flight_bytes,
            })?;
        if in_flight > self.limits.max_in_flight_bytes {
            return Err(FlowError::InFlightLimit {
                requested: in_flight,
                limit: self.limits.max_in_flight_bytes,
            });
        }
        let max_message_size = self.max_message_size.max(size);
        let blocked = !self.is_ready_with(in_flight, max_message_size);
        if blocked && self.blocked.len() >= self.limits.max_blocked_senders {
            return Err(FlowError::BlockedSenderLimit {
                limit: self.limits.max_blocked_senders,
            });
        }
        let id = self.next_send_id;
        self.next_send_id = self
            .next_send_id
            .checked_add(1)
            .ok_or(FlowError::InFlightLimit {
                requested: u64::MAX,
                limit: self.limits.max_in_flight_bytes,
            })?;
        self.max_message_size = max_message_size;
        self.bytes_in_flight = in_flight;
        let snapshot = SendSnapshot {
            sent_micros,
            size,
            delivered_at_send: self.delivered,
            delivered_time_at_send: self.delivered_time,
            window_at_send: self.window,
            window_full_at_send: blocked,
        };
        self.sends.insert(id, snapshot);
        if blocked {
            self.blocked.insert(id, None);
        }
        Ok((id, blocked))
    }

    fn acknowledge(&mut self, id: u64, ack_micros: u64) -> Result<Vec<Waker>, FlowError> {
        let snapshot = self
            .sends
            .remove(&id)
            .ok_or(FlowError::AckAlreadyCompleted)?;
        if ack_micros < snapshot.sent_micros {
            self.sends.insert(id, snapshot);
            return Err(FlowError::InvalidTimestamp);
        }
        self.bytes_in_flight = self
            .bytes_in_flight
            .checked_sub(snapshot.size)
            .ok_or(FlowError::AckAlreadyCompleted)?;
        self.delivered =
            self.delivered
                .checked_add(snapshot.size)
                .ok_or(FlowError::InFlightLimit {
                    requested: u64::MAX,
                    limit: self.limits.max_in_flight_bytes,
                })?;
        let rtt = ack_micros - snapshot.sent_micros;
        self.min_rtt_micros = Some(self.min_rtt_micros.map_or(rtt, |value| value.min(rtt)));
        if matches!(self.mode, ControllerMode::Adaptive) {
            self.update_adaptive_window(snapshot, ack_micros)?;
        }
        self.delivered_time = Some(ack_micros);
        if self.first_ack.is_none() {
            self.first_ack = Some((ack_micros, self.delivered));
        }
        let mut wakers = if self.is_ready() {
            self.take_blocked_wakers()
        } else {
            Vec::new()
        };
        if self.sends.is_empty() {
            wakers.extend(self.all_acked_waker.take());
        }
        Ok(wakers)
    }

    fn update_adaptive_window(
        &mut self,
        snapshot: SendSnapshot,
        ack_micros: u64,
    ) -> Result<(), FlowError> {
        let Some((first_time, first_delivered)) = self.first_ack else {
            return Ok(());
        };
        let (base_time, base_delivered) = snapshot
            .delivered_time_at_send
            .map_or((first_time, first_delivered), |time| {
                (time, snapshot.delivered_at_send)
            });
        let interval = ack_micros
            .checked_sub(base_time)
            .ok_or(FlowError::InvalidTimestamp)?;
        if interval == 0 {
            return Ok(());
        }
        let bytes_delivered = self
            .delivered
            .checked_sub(base_delivered)
            .ok_or(FlowError::InvalidTimestamp)?;
        let min_rtt = self.min_rtt_micros.unwrap_or(0);
        let growth = if self.startup { (2, 1) } else { (5, 4) };
        let mut new_window = if bytes_delivered > self.limits.max_window_bytes.saturating_mul(2) {
            self.limits.max_window_bytes
        } else {
            let bdp_growth =
                u128::from(bytes_delivered) * u128::from(min_rtt) * u128::from(growth.0)
                    / u128::from(interval)
                    / u128::from(growth.1);
            u64::try_from(bdp_growth).unwrap_or(u64::MAX)
        };
        new_window = new_window.min(apply_ratio(snapshot.window_at_send, growth.0, growth.1));
        if snapshot.window_full_at_send {
            new_window = new_window.max(apply_ratio(snapshot.window_at_send, 7, 8));
        } else {
            new_window = new_window.max(self.window);
        }
        self.window = new_window.clamp(self.limits.min_window_bytes, self.limits.max_window_bytes);
        if self.startup {
            let new_round = self
                .round_start_micros
                .is_none_or(|start| snapshot.sent_micros >= start);
            if new_round {
                if self.window > apply_ratio(self.last_round_window, 5, 4) {
                    self.rounds_without_increase = 0;
                } else {
                    self.rounds_without_increase = self.rounds_without_increase.saturating_add(1);
                    if self.rounds_without_increase >= STARTUP_EXIT_ROUNDS {
                        self.startup = false;
                    }
                }
                self.round_start_micros = Some(ack_micros);
                self.last_round_window = self.window;
            }
        }
        Ok(())
    }

    fn fail(&mut self, id: u64, reason: Arc<str>) -> Result<Vec<Waker>, FlowError> {
        let snapshot = self
            .sends
            .remove(&id)
            .ok_or(FlowError::AckAlreadyCompleted)?;
        self.bytes_in_flight = self
            .bytes_in_flight
            .checked_sub(snapshot.size)
            .ok_or(FlowError::AckAlreadyCompleted)?;
        self.failed = Some(reason);
        Ok(self.take_all_wakers())
    }

    fn is_ready(&self) -> bool {
        self.is_ready_with(self.bytes_in_flight, self.max_message_size)
    }

    fn is_ready_with(&self, bytes_in_flight: u64, max_message_size: u64) -> bool {
        let extended = self.window.saturating_add(max_message_size);
        match self.mode {
            ControllerMode::Fixed => {
                bytes_in_flight <= max_message_size || bytes_in_flight < extended
            }
            ControllerMode::Adaptive => bytes_in_flight < extended,
        }
    }

    fn take_blocked_wakers(&mut self) -> Vec<Waker> {
        core::mem::take(&mut self.blocked)
            .into_values()
            .flatten()
            .collect()
    }

    fn take_all_wakers(&mut self) -> Vec<Waker> {
        let mut wakers = self.take_blocked_wakers();
        wakers.extend(self.all_acked_waker.take());
        wakers
    }

    fn stats(&self) -> FlowStats {
        FlowStats {
            mode: match self.mode {
                ControllerMode::Fixed => FlowMode::Fixed,
                ControllerMode::Adaptive => FlowMode::Adaptive,
            },
            window_bytes: self.window,
            bytes_in_flight: self.bytes_in_flight,
            max_message_bytes_seen: self.max_message_size,
            delivered_bytes: self.delivered,
            outstanding_acks: self.sends.len(),
            blocked_senders: self.blocked.len(),
            min_rtt: self.min_rtt_micros.map(Duration::from_micros),
            startup: self.startup,
            closed: self.closed,
        }
    }
}

fn duration_micros(value: Duration) -> Result<u64, FlowError> {
    u64::try_from(value.as_micros()).map_err(|_| FlowError::InvalidTimestamp)
}

fn apply_ratio(value: u64, numerator: u64, denominator: u64) -> u64 {
    let scaled = u128::from(value) * u128::from(numerator) / u128::from(denominator);
    u64::try_from(scaled).unwrap_or(u64::MAX)
}

fn wake_all(wakers: Vec<Waker>) {
    for waker in wakers {
        waker.wake();
    }
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::Waker;

    const KIB: u64 = 1024;

    struct SimulatedLink {
        flow: FlowController,
        now_micros: u64,
        link_free_micros: u64,
        rtt_micros: u64,
        bytes_per_second: u64,
        acknowledgements: VecDeque<(u64, FlowAck)>,
        blocked: Option<FlowReady>,
        next_chunk: usize,
    }

    impl SimulatedLink {
        fn new() -> Self {
            Self {
                flow: FlowController::adaptive(256 * KIB, FlowLimits::default()).expect("flow"),
                now_micros: 0,
                link_free_micros: 0,
                rtt_micros: 100_000,
                bytes_per_second: 2 * 1024 * 1024,
                acknowledgements: VecDeque::new(),
                blocked: None,
                next_chunk: 0,
            }
        }

        fn send(&mut self, size: u64) -> bool {
            assert!(self.blocked.is_none(), "sent before readiness");
            let sent_at = self.now_micros;
            let (_, sent) = self
                .flow
                .send_now(size, Duration::from_micros(sent_at), || ())
                .expect("simulated send");
            let (ack, mut ready) = sent.into_parts();
            let serialization = size
                .checked_mul(1_000_000)
                .expect("bounded test size")
                .div_ceil(self.bytes_per_second);
            self.link_free_micros = self.link_free_micros.max(sent_at) + serialization;
            let ack_at = self.link_free_micros + self.rtt_micros;
            self.acknowledgements.push_back((ack_at, ack));
            if poll_ready(&mut ready).is_pending() {
                self.blocked = Some(ready);
                true
            } else {
                false
            }
        }

        fn send_next_chunk(&mut self) -> bool {
            const CHUNKS: [u64; 8] = [32 * KIB, 4 * KIB, 16_000, 12_345, 16, 9_999, 4_321, 8];
            let size = CHUNKS[self.next_chunk % CHUNKS.len()];
            self.next_chunk += 1;
            self.send(size)
        }

        fn acknowledge_next(&mut self) {
            let (ack_at, ack) = self
                .acknowledgements
                .pop_front()
                .expect("outstanding acknowledgement");
            self.now_micros = self.now_micros.max(ack_at);
            ack.acknowledge(Duration::from_micros(self.now_micros))
                .expect("acknowledge");
            if let Some(mut ready) = self.blocked.take() {
                match poll_ready(&mut ready) {
                    Poll::Ready(Ok(())) => {}
                    Poll::Pending => self.blocked = Some(ready),
                    Poll::Ready(Err(error)) => panic!("readiness failed: {error}"),
                }
            }
        }

        fn saturate_for(&mut self, duration_micros: u64) {
            let end = self.now_micros + duration_micros;
            while self.now_micros < end {
                if self.blocked.is_some() {
                    self.acknowledge_next();
                } else {
                    self.send_next_chunk();
                }
            }
        }

        fn drain(&mut self) {
            while !self.acknowledgements.is_empty() {
                self.acknowledge_next();
            }
            assert!(self.blocked.is_none());
        }
    }

    fn poll_ready(future: &mut FlowReady) -> Poll<Result<(), FlowError>> {
        let mut context = Context::from_waker(Waker::noop());
        Pin::new(future).poll(&mut context)
    }

    #[test]
    fn fixed_window_sends_now_blocks_at_extended_window_and_wakes_on_ack() {
        let flow = FlowController::fixed(256 * 1024, FlowLimits::default()).expect("flow");
        let sends = AtomicUsize::new(0);
        let mut acknowledgements = Vec::new();
        let mut final_ready = None;
        for index in 0..5 {
            let (_, sent) = flow
                .send_now(64 * 1024, Duration::from_millis(index), || {
                    sends.fetch_add(1, Ordering::SeqCst);
                })
                .expect("send");
            let (ack, mut ready) = sent.into_parts();
            if index < 4 {
                assert_eq!(poll_ready(&mut ready), Poll::Ready(Ok(())));
            } else {
                assert_eq!(poll_ready(&mut ready), Poll::Pending);
                final_ready = Some(ready);
            }
            acknowledgements.push(ack);
        }
        assert_eq!(sends.load(Ordering::SeqCst), 5);
        acknowledgements
            .remove(0)
            .acknowledge(Duration::from_millis(100))
            .expect("ack");
        assert_eq!(
            poll_ready(final_ready.as_mut().expect("blocked ready")),
            Poll::Ready(Ok(()))
        );
    }

    #[test]
    fn dropped_ready_does_not_cancel_send_and_close_wakes_blocked_sender() {
        let flow = FlowController::fixed(1, FlowLimits::default()).expect("flow");
        let (_, first) = flow.send_now(2, Duration::ZERO, || ()).expect("first send");
        let (first_ack, first_ready) = first.into_parts();
        drop(first_ready);
        assert_eq!(flow.stats().expect("stats").bytes_in_flight, 2);
        let (_, second) = flow
            .send_now(2, Duration::from_micros(1), || ())
            .expect("second send");
        let (_second_ack, mut second_ready) = second.into_parts();
        assert_eq!(poll_ready(&mut second_ready), Poll::Pending);
        flow.close().expect("close");
        assert_eq!(poll_ready(&mut second_ready), Poll::Ready(Ok(())));
        drop(first_ack);
    }

    #[test]
    fn acknowledgement_failure_poisoning_is_sticky() {
        let flow = FlowController::fixed(1, FlowLimits::default()).expect("flow");
        let (_, first) = flow.send_now(2, Duration::ZERO, || ()).expect("first send");
        let (_first_ack, _first_ready) = first.into_parts();
        let (_, second) = flow
            .send_now(2, Duration::from_micros(1), || ())
            .expect("second send");
        let (ack, mut ready) = second.into_parts();
        assert_eq!(poll_ready(&mut ready), Poll::Pending);
        ack.fail(Arc::<str>::from("peer failed")).expect("failure");
        assert!(matches!(
            poll_ready(&mut ready),
            Poll::Ready(Err(FlowError::Failed(reason))) if &*reason == "peer failed"
        ));
        assert!(matches!(
            flow.send_now(1, Duration::from_micros(2), || ()),
            Err(FlowError::Failed(reason)) if &*reason == "peer failed"
        ));
    }

    #[test]
    fn limits_reject_before_the_send_closure_runs() {
        let limits = FlowLimits {
            max_message_bytes: 8,
            max_in_flight_bytes: 8,
            ..FlowLimits::default()
        };
        let flow = FlowController::fixed(4, limits).expect("flow");
        let sends = AtomicUsize::new(0);
        assert!(matches!(
            flow.send_now(9, Duration::ZERO, || {
                sends.fetch_add(1, Ordering::SeqCst);
            }),
            Err(FlowError::MessageTooLarge { size: 9, limit: 8 })
        ));
        assert_eq!(sends.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn adaptive_controller_tracks_bdp_and_preserves_app_limited_window() {
        let mut link = SimulatedLink::new();
        for _ in 0..5 {
            link.send(64 * KIB);
        }
        assert!(link.blocked.is_some(), "fifth initial chunk must block");
        link.acknowledge_next();
        assert!(
            link.blocked.is_none(),
            "one acknowledgement releases sender"
        );

        let initial_window = link.flow.stats().expect("stats").window_bytes;
        link.saturate_for(5 * link.rtt_micros);
        let startup_window = link.flow.stats().expect("stats").window_bytes;
        assert!(
            startup_window > initial_window,
            "startup did not grow the window"
        );
        link.saturate_for(50 * link.rtt_micros);
        link.drain();
        let converged = link.flow.stats().expect("stats").window_bytes;
        let bdp = link.bytes_per_second * link.rtt_micros / 1_000_000;
        assert!(converged >= bdp / 2, "window {converged} below BDP {bdp}");
        assert!(converged <= bdp * 2, "window {converged} above BDP {bdp}");

        for _ in 0..20 {
            assert!(!link.send(1024));
            link.acknowledge_next();
        }
        let app_limited = link.flow.stats().expect("stats").window_bytes;
        assert!(
            app_limited >= converged,
            "app-limited traffic shrank window"
        );

        link.bytes_per_second /= 4;
        link.saturate_for(200 * link.rtt_micros);
        link.drain();
        let reduced = link.flow.stats().expect("stats").window_bytes;
        assert!(
            reduced < app_limited,
            "bandwidth loss did not shrink window"
        );
        assert!(reduced >= DEFAULT_MIN_WINDOW);

        link.bytes_per_second = 1024;
        link.saturate_for(500 * link.rtt_micros);
        link.drain();
        assert_eq!(
            link.flow.stats().expect("stats").window_bytes,
            DEFAULT_MIN_WINDOW,
            "adaptive decay must clamp at the configured minimum"
        );
    }

    #[test]
    fn blocked_sender_limit_is_transactional() {
        let limits = FlowLimits {
            max_blocked_senders: 1,
            ..FlowLimits::default()
        };
        let flow = FlowController::fixed(1, limits).expect("flow");
        let (_, first) = flow.send_now(2, Duration::ZERO, || ()).expect("first");
        let (_first_ack, _first_ready) = first.into_parts();
        let (_, second) = flow
            .send_now(2, Duration::from_micros(1), || ())
            .expect("second");
        let (_second_ack, _second_ready) = second.into_parts();
        let before = flow.stats().expect("stats");
        assert!(matches!(
            flow.send_now(2, Duration::from_micros(2), || ()),
            Err(FlowError::BlockedSenderLimit { limit: 1 })
        ));
        assert_eq!(flow.stats().expect("stats"), before);
    }

    #[test]
    fn independent_streams_do_not_share_backpressure() {
        let first = FlowController::fixed(1, FlowLimits::default()).expect("first flow");
        let second = FlowController::fixed(1, FlowLimits::default()).expect("second flow");
        let (_, send) = first.send_now(2, Duration::ZERO, || ()).expect("send");
        let (_ack, _ready) = send.into_parts();
        let (_, send) = first
            .send_now(2, Duration::from_micros(1), || ())
            .expect("send");
        let (_ack, mut blocked) = send.into_parts();
        assert_eq!(poll_ready(&mut blocked), Poll::Pending);
        let (_, send) = second.send_now(1, Duration::ZERO, || ()).expect("send");
        let (_ack, mut ready) = send.into_parts();
        assert_eq!(poll_ready(&mut ready), Poll::Ready(Ok(())));
    }

    #[test]
    fn wait_all_acked_completes_on_ack_or_close() {
        let flow = FlowController::fixed(1, FlowLimits::default()).expect("flow");
        let (_, send) = flow.send_now(2, Duration::ZERO, || ()).expect("send");
        let (ack, _ready) = send.into_parts();
        let mut all_acked = flow.wait_all_acked();
        let mut context = Context::from_waker(Waker::noop());
        assert_eq!(Pin::new(&mut all_acked).poll(&mut context), Poll::Pending);
        ack.acknowledge(Duration::from_micros(1)).expect("ack");
        assert_eq!(
            Pin::new(&mut all_acked).poll(&mut context),
            Poll::Ready(Ok(()))
        );

        let (_, send) = flow
            .send_now(2, Duration::from_micros(2), || ())
            .expect("send");
        let (_ack, _ready) = send.into_parts();
        let mut closed = flow.wait_all_acked();
        assert_eq!(Pin::new(&mut closed).poll(&mut context), Poll::Pending);
        flow.close().expect("close");
        assert_eq!(
            Pin::new(&mut closed).poll(&mut context),
            Poll::Ready(Ok(()))
        );
    }
}
