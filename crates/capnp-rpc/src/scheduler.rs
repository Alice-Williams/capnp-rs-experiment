//! Executor-neutral server scheduling policies.
//!
//! These wrappers schedule application futures, never connection-actor work.
//! The actor remains the sole owner of protocol state and each completion still
//! returns through its generation-bearing token. Serial gates are FIFO and do
//! not hold a mutex while application code is polled.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::future::Future;
use std::hash::Hash;
use std::panic::{self, AssertUnwindSafe};
use std::pin::Pin;
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll, Wake, Waker};

use capnp_message::OwnedMessage;

use crate::{BoxFuture, LocalService, MessageFuture, RpcError};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SchedulerError {
    InvalidCapacity,
    Overloaded { capacity: usize },
    Disconnected,
    Poisoned,
    Panicked,
    Spawn(String),
}

impl fmt::Display for SchedulerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for SchedulerError {}

/// Runs each call as soon as the returned application future is polled.
pub struct Concurrent<S: ?Sized> {
    service: Arc<S>,
}

impl<S: ?Sized> Clone for Concurrent<S> {
    fn clone(&self) -> Self {
        Self {
            service: Arc::clone(&self.service),
        }
    }
}

impl<S: ?Sized> fmt::Debug for Concurrent<S> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Concurrent").finish_non_exhaustive()
    }
}

impl<S: ?Sized> Concurrent<S> {
    pub fn new(service: Arc<S>) -> Self {
        Self { service }
    }
}

impl<S: LocalService + ?Sized> LocalService for Concurrent<S> {
    fn dispatch(
        self: Arc<Self>,
        interface_id: u64,
        method_id: u16,
        params: Arc<OwnedMessage>,
    ) -> MessageFuture {
        Arc::clone(&self.service).dispatch(interface_id, method_id, params)
    }
}

/// Runs at most one call at a time, in FIFO permit order.
pub struct Serial<S: ?Sized> {
    service: Arc<S>,
    gate: Arc<SerialGate>,
}

impl<S: ?Sized> Clone for Serial<S> {
    fn clone(&self) -> Self {
        Self {
            service: Arc::clone(&self.service),
            gate: Arc::clone(&self.gate),
        }
    }
}

impl<S: ?Sized> fmt::Debug for Serial<S> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Serial").finish_non_exhaustive()
    }
}

impl<S: ?Sized> Serial<S> {
    pub fn new(service: Arc<S>) -> Self {
        Self {
            service,
            gate: Arc::new(SerialGate::new()),
        }
    }
}

impl<S: LocalService + ?Sized> LocalService for Serial<S> {
    fn dispatch(
        self: Arc<Self>,
        interface_id: u64,
        method_id: u16,
        params: Arc<OwnedMessage>,
    ) -> MessageFuture {
        let permit = self.gate.acquire();
        let service = Arc::clone(&self.service);
        Box::pin(async move {
            let _permit = permit.await.map_err(RpcError::Scheduler)?;
            service.dispatch(interface_id, method_id, params).await
        })
    }
}

/// Serializes equal keys while allowing different keys to overlap.
pub struct Keyed<S: ?Sized, K, F> {
    service: Arc<S>,
    key: F,
    gates: Mutex<HashMap<K, Weak<SerialGate>>>,
}

impl<S: ?Sized, K, F> fmt::Debug for Keyed<S, K, F> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let keys = self.gates.lock().map(|gates| gates.len()).ok();
        formatter
            .debug_struct("Keyed")
            .field("tracked_keys", &keys)
            .finish_non_exhaustive()
    }
}

impl<S: ?Sized, K, F> Keyed<S, K, F> {
    pub fn new(service: Arc<S>, key: F) -> Self {
        Self {
            service,
            key,
            gates: Mutex::new(HashMap::new()),
        }
    }
}

impl<S, K, F> LocalService for Keyed<S, K, F>
where
    S: LocalService + ?Sized,
    K: Clone + Eq + Hash + Send + Sync + 'static,
    F: Fn(u64, u16, &OwnedMessage) -> K + Send + Sync + 'static,
{
    fn dispatch(
        self: Arc<Self>,
        interface_id: u64,
        method_id: u16,
        params: Arc<OwnedMessage>,
    ) -> MessageFuture {
        let key = (self.key)(interface_id, method_id, &params);
        let gate = match self.gate_for(key) {
            Ok(gate) => gate,
            Err(error) => return ready_error(error),
        };
        let permit = gate.acquire();
        let service = Arc::clone(&self.service);
        Box::pin(async move {
            let _permit = permit.await.map_err(RpcError::Scheduler)?;
            service.dispatch(interface_id, method_id, params).await
        })
    }
}

impl<S: ?Sized, K, F> Keyed<S, K, F>
where
    K: Clone + Eq + Hash,
{
    fn gate_for(&self, key: K) -> Result<Arc<SerialGate>, SchedulerError> {
        let mut gates = self.gates.lock().map_err(|_| SchedulerError::Poisoned)?;
        gates.retain(|_, gate| gate.strong_count() != 0);
        if let Some(gate) = gates.get(&key).and_then(Weak::upgrade) {
            return Ok(gate);
        }
        let gate = Arc::new(SerialGate::new());
        gates.insert(key, Arc::downgrade(&gate));
        Ok(gate)
    }
}

/// A runtime that accepts owned, `Send` application tasks.
pub trait TaskExecutor: Send + Sync + 'static {
    fn spawn(&self, task: BoxFuture<()>) -> Result<(), SchedulerError>;
}

/// Adapts any fallible spawn callback to [`TaskExecutor`].
pub struct GenericExecutor<F> {
    spawn: F,
}

impl<F> GenericExecutor<F> {
    pub fn new(spawn: F) -> Self {
        Self { spawn }
    }
}

impl<F> fmt::Debug for GenericExecutor<F> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GenericExecutor")
            .finish_non_exhaustive()
    }
}

impl<F> TaskExecutor for GenericExecutor<F>
where
    F: Fn(BoxFuture<()>) -> Result<(), SchedulerError> + Send + Sync + 'static,
{
    fn spawn(&self, task: BoxFuture<()>) -> Result<(), SchedulerError> {
        (self.spawn)(task)
    }
}

/// Dependency-free Tokio adapter. Pass `|task| { tokio::spawn(task); }` from a
/// Tokio-enabled application; spawning is infallible from this boundary.
pub struct TokioExecutor<F> {
    spawn: F,
}

impl<F> TokioExecutor<F> {
    pub fn new(spawn: F) -> Self {
        Self { spawn }
    }
}

impl<F> fmt::Debug for TokioExecutor<F> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TokioExecutor")
            .finish_non_exhaustive()
    }
}

impl<F> TaskExecutor for TokioExecutor<F>
where
    F: Fn(BoxFuture<()>) + Send + Sync + 'static,
{
    fn spawn(&self, task: BoxFuture<()>) -> Result<(), SchedulerError> {
        (self.spawn)(task);
        Ok(())
    }
}

/// A small bounded executor used for CPU-bound handlers and deterministic
/// scheduling tests. Workers share one FIFO receiver and execute outside its
/// lock.
#[derive(Clone)]
pub struct ThreadPoolExecutor {
    sender: SyncSender<BoxFuture<()>>,
    capacity: usize,
}

impl fmt::Debug for ThreadPoolExecutor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ThreadPoolExecutor")
            .field("capacity", &self.capacity)
            .finish_non_exhaustive()
    }
}

impl ThreadPoolExecutor {
    pub fn new(workers: usize, capacity: usize) -> Result<Self, SchedulerError> {
        if workers == 0 || capacity == 0 {
            return Err(SchedulerError::InvalidCapacity);
        }
        let (sender, receiver) = mpsc::sync_channel::<BoxFuture<()>>(capacity);
        let receiver = Arc::new(Mutex::new(receiver));
        for index in 0..workers {
            let receiver = Arc::clone(&receiver);
            std::thread::Builder::new()
                .name(format!("capnp-rpc-worker-{index}"))
                .spawn(move || {
                    loop {
                        let task = match receiver.lock() {
                            Ok(receiver) => receiver.recv(),
                            Err(_) => return,
                        };
                        match task {
                            Ok(task) => {
                                let _ = panic::catch_unwind(AssertUnwindSafe(|| {
                                    run_to_completion(task)
                                }));
                            }
                            Err(_) => return,
                        }
                    }
                })
                .map_err(|error| SchedulerError::Spawn(error.to_string()))?;
        }
        Ok(Self { sender, capacity })
    }
}

impl TaskExecutor for ThreadPoolExecutor {
    fn spawn(&self, task: BoxFuture<()>) -> Result<(), SchedulerError> {
        match self.sender.try_send(task) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(SchedulerError::Overloaded {
                capacity: self.capacity,
            }),
            Err(TrySendError::Disconnected(_)) => Err(SchedulerError::Disconnected),
        }
    }
}

/// Moves service futures onto a selected executor while returning a `Send`
/// response future to the RPC caller.
pub struct ExecutorService<S: ?Sized, E> {
    service: Arc<S>,
    executor: E,
}

impl<S: ?Sized, E> ExecutorService<S, E> {
    pub fn new(service: Arc<S>, executor: E) -> Self {
        Self { service, executor }
    }
}

impl<S: ?Sized, E> fmt::Debug for ExecutorService<S, E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExecutorService")
            .finish_non_exhaustive()
    }
}

impl<S, E> LocalService for ExecutorService<S, E>
where
    S: LocalService + ?Sized,
    E: TaskExecutor,
{
    fn dispatch(
        self: Arc<Self>,
        interface_id: u64,
        method_id: u16,
        params: Arc<OwnedMessage>,
    ) -> MessageFuture {
        let response = Arc::new(ResponseCell::new());
        let service = Arc::clone(&self.service);
        let completion = Arc::clone(&response);
        let task = Box::pin(async move {
            let future = match panic::catch_unwind(AssertUnwindSafe(|| {
                service.dispatch(interface_id, method_id, params)
            })) {
                Ok(future) => future,
                Err(_) => {
                    completion.complete(Err(RpcError::Scheduler(SchedulerError::Panicked)));
                    return;
                }
            };
            let result = PanicBoundary { future }.await;
            completion.complete(result);
        });
        match panic::catch_unwind(AssertUnwindSafe(|| self.executor.spawn(task))) {
            Ok(Ok(())) => {}
            Ok(Err(error)) => response.complete(Err(RpcError::Scheduler(error))),
            Err(_) => response.complete(Err(RpcError::Scheduler(SchedulerError::Panicked))),
        }
        Box::pin(ResponseFuture { cell: response })
    }
}

struct LocalRequest {
    interface_id: u64,
    method_id: u16,
    params: Arc<OwnedMessage>,
    response: Arc<ResponseCell>,
}

impl Drop for LocalRequest {
    fn drop(&mut self) {
        self.response
            .complete(Err(RpcError::Scheduler(SchedulerError::Disconnected)));
    }
}

/// Isolates state constructed on a dedicated thread. The state itself need not
/// implement `Send`; only its factory and synchronous dispatch closure cross
/// the thread boundary.
#[derive(Clone)]
pub struct LocalServer {
    sender: SyncSender<LocalRequest>,
    capacity: usize,
}

impl fmt::Debug for LocalServer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalServer")
            .field("capacity", &self.capacity)
            .finish_non_exhaustive()
    }
}

impl LocalServer {
    pub fn spawn<S, Make, Dispatch>(
        name: impl Into<String>,
        capacity: usize,
        make_state: Make,
        mut dispatch: Dispatch,
    ) -> Result<Self, SchedulerError>
    where
        S: 'static,
        Make: FnOnce() -> S + Send + 'static,
        Dispatch: FnMut(&mut S, u64, u16, Arc<OwnedMessage>) -> Result<Arc<OwnedMessage>, RpcError>
            + Send
            + 'static,
    {
        if capacity == 0 {
            return Err(SchedulerError::InvalidCapacity);
        }
        let (sender, receiver) = mpsc::sync_channel::<LocalRequest>(capacity);
        std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                let mut state = make_state();
                while let Ok(request) = receiver.recv() {
                    let result = panic::catch_unwind(AssertUnwindSafe(|| {
                        dispatch(
                            &mut state,
                            request.interface_id,
                            request.method_id,
                            Arc::clone(&request.params),
                        )
                    }))
                    .unwrap_or(Err(RpcError::Scheduler(SchedulerError::Panicked)));
                    request.response.complete(result);
                }
            })
            .map_err(|error| SchedulerError::Spawn(error.to_string()))?;
        Ok(Self { sender, capacity })
    }
}

impl LocalService for LocalServer {
    fn dispatch(
        self: Arc<Self>,
        interface_id: u64,
        method_id: u16,
        params: Arc<OwnedMessage>,
    ) -> MessageFuture {
        let response = Arc::new(ResponseCell::new());
        let request = LocalRequest {
            interface_id,
            method_id,
            params,
            response: Arc::clone(&response),
        };
        match self.sender.try_send(request) {
            Ok(()) => {}
            Err(TrySendError::Full(request)) => {
                request
                    .response
                    .complete(Err(RpcError::Scheduler(SchedulerError::Overloaded {
                        capacity: self.capacity,
                    })));
            }
            Err(TrySendError::Disconnected(request)) => {
                request
                    .response
                    .complete(Err(RpcError::Scheduler(SchedulerError::Disconnected)));
            }
        }
        Box::pin(ResponseFuture { cell: response })
    }
}

fn ready_error(error: SchedulerError) -> MessageFuture {
    Box::pin(async move { Err(RpcError::Scheduler(error)) })
}

struct SerialGate {
    state: Mutex<GateState>,
}

struct GateState {
    active: bool,
    waiters: VecDeque<Arc<GateWaiter>>,
}

impl SerialGate {
    fn new() -> Self {
        Self {
            state: Mutex::new(GateState {
                active: false,
                waiters: VecDeque::new(),
            }),
        }
    }

    fn acquire(self: &Arc<Self>) -> GateAcquire {
        let waiter = Arc::new(GateWaiter {
            state: Mutex::new(WaiterState {
                status: WaiterStatus::Waiting,
                waker: None,
            }),
        });
        let error = match self.state.lock() {
            Ok(mut state) if !state.active => {
                state.active = true;
                if let Ok(mut waiter_state) = waiter.state.lock() {
                    waiter_state.status = WaiterStatus::Granted;
                    None
                } else {
                    state.active = false;
                    Some(SchedulerError::Poisoned)
                }
            }
            Ok(mut state) => {
                state.waiters.push_back(Arc::clone(&waiter));
                None
            }
            Err(_) => Some(SchedulerError::Poisoned),
        };
        GateAcquire {
            gate: Arc::clone(self),
            waiter,
            error,
            completed: false,
        }
    }

    fn release(&self) {
        let mut wake = None;
        let mut granted = false;
        if let Ok(mut state) = self.state.lock() {
            while let Some(waiter) = state.waiters.pop_front() {
                let Ok(mut waiter_state) = waiter.state.lock() else {
                    continue;
                };
                if waiter_state.status == WaiterStatus::Waiting {
                    waiter_state.status = WaiterStatus::Granted;
                    wake = waiter_state.waker.take();
                    granted = true;
                    break;
                }
            }
            if !granted {
                state.active = false;
            }
        }
        if let Some(waker) = wake {
            waker.wake();
        }
    }
}

struct GateWaiter {
    state: Mutex<WaiterState>,
}

struct WaiterState {
    status: WaiterStatus,
    waker: Option<Waker>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum WaiterStatus {
    Waiting,
    Granted,
    Canceled,
    Delivered,
}

struct GateAcquire {
    gate: Arc<SerialGate>,
    waiter: Arc<GateWaiter>,
    error: Option<SchedulerError>,
    completed: bool,
}

impl Future for GateAcquire {
    type Output = Result<GatePermit, SchedulerError>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        if let Some(error) = self.error.take() {
            self.completed = true;
            return Poll::Ready(Err(error));
        }
        let waiter = Arc::clone(&self.waiter);
        let mut state = match waiter.state.lock() {
            Ok(state) => state,
            Err(_) => {
                self.completed = true;
                return Poll::Ready(Err(SchedulerError::Poisoned));
            }
        };
        match state.status {
            WaiterStatus::Granted => {
                state.status = WaiterStatus::Delivered;
                drop(state);
                self.completed = true;
                Poll::Ready(Ok(GatePermit {
                    gate: Arc::clone(&self.gate),
                }))
            }
            WaiterStatus::Waiting => {
                state.waker = Some(context.waker().clone());
                Poll::Pending
            }
            WaiterStatus::Canceled | WaiterStatus::Delivered => {
                drop(state);
                self.completed = true;
                Poll::Ready(Err(SchedulerError::Disconnected))
            }
        }
    }
}

impl Drop for GateAcquire {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        let release = if let Ok(mut state) = self.waiter.state.lock() {
            match state.status {
                WaiterStatus::Waiting => {
                    state.status = WaiterStatus::Canceled;
                    false
                }
                WaiterStatus::Granted => {
                    state.status = WaiterStatus::Delivered;
                    true
                }
                WaiterStatus::Canceled | WaiterStatus::Delivered => false,
            }
        } else {
            false
        };
        if release {
            self.gate.release();
        }
    }
}

struct GatePermit {
    gate: Arc<SerialGate>,
}

impl Drop for GatePermit {
    fn drop(&mut self) {
        self.gate.release();
    }
}

struct ResponseCell {
    state: Mutex<ResponseState>,
}

struct ResponseState {
    outcome: Option<Result<Arc<OwnedMessage>, RpcError>>,
    waker: Option<Waker>,
}

impl ResponseCell {
    fn new() -> Self {
        Self {
            state: Mutex::new(ResponseState {
                outcome: None,
                waker: None,
            }),
        }
    }

    fn complete(&self, outcome: Result<Arc<OwnedMessage>, RpcError>) {
        let wake = match self.state.lock() {
            Ok(mut state) if state.outcome.is_none() => {
                state.outcome = Some(outcome);
                state.waker.take()
            }
            _ => None,
        };
        if let Some(waker) = wake {
            waker.wake();
        }
    }
}

struct ResponseFuture {
    cell: Arc<ResponseCell>,
}

struct PanicBoundary {
    future: MessageFuture,
}

impl Future for PanicBoundary {
    type Output = Result<Arc<OwnedMessage>, RpcError>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        panic::catch_unwind(AssertUnwindSafe(|| self.future.as_mut().poll(context))).unwrap_or(
            Poll::Ready(Err(RpcError::Scheduler(SchedulerError::Panicked))),
        )
    }
}

impl Future for ResponseFuture {
    type Output = Result<Arc<OwnedMessage>, RpcError>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = match self.cell.state.lock() {
            Ok(state) => state,
            Err(_) => return Poll::Ready(Err(RpcError::Scheduler(SchedulerError::Poisoned))),
        };
        if let Some(outcome) = state.outcome.take() {
            Poll::Ready(outcome)
        } else {
            state.waker = Some(context.waker().clone());
            Poll::Pending
        }
    }
}

struct ThreadWake(std::thread::Thread);

impl Wake for ThreadWake {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

fn run_to_completion<F: Future>(future: F) -> F::Output {
    let wake = Arc::new(ThreadWake(std::thread::current()));
    let waker = Waker::from(wake);
    let mut context = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => std::thread::park(),
        }
    }
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use capnp_message::{ExclusiveArena, ReaderLimits};

    fn message(value: u64) -> Arc<OwnedMessage> {
        let mut arena = ExclusiveArena::new(2, 16).expect("arena");
        arena
            .init_root_struct(1, 0)
            .expect("root")
            .set_u64(0, value, 0)
            .expect("value");
        OwnedMessage::new(arena.into_segments(), ReaderLimits::default()).expect("message")
    }

    struct ProbeService {
        active: AtomicUsize,
        maximum: AtomicUsize,
        keyed_active: Vec<AtomicUsize>,
        keyed_maximum: Vec<AtomicUsize>,
    }

    struct PanicService;

    impl LocalService for PanicService {
        fn dispatch(
            self: Arc<Self>,
            _interface_id: u64,
            _method_id: u16,
            _params: Arc<OwnedMessage>,
        ) -> MessageFuture {
            Box::pin(async move { panic!("scheduled handler panic") })
        }
    }

    struct SynchronousPanicService;

    impl LocalService for SynchronousPanicService {
        fn dispatch(
            self: Arc<Self>,
            _interface_id: u64,
            _method_id: u16,
            _params: Arc<OwnedMessage>,
        ) -> MessageFuture {
            panic!("synchronous dispatch panic")
        }
    }

    impl ProbeService {
        fn new() -> Self {
            Self {
                active: AtomicUsize::new(0),
                maximum: AtomicUsize::new(0),
                keyed_active: vec![AtomicUsize::new(0), AtomicUsize::new(0)],
                keyed_maximum: vec![AtomicUsize::new(0), AtomicUsize::new(0)],
            }
        }
    }

    impl LocalService for ProbeService {
        fn dispatch(
            self: Arc<Self>,
            _interface_id: u64,
            method_id: u16,
            _params: Arc<OwnedMessage>,
        ) -> MessageFuture {
            Box::pin(async move {
                let key = usize::from(method_id % 2);
                let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
                self.maximum.fetch_max(active, Ordering::SeqCst);
                let keyed = self.keyed_active[key].fetch_add(1, Ordering::SeqCst) + 1;
                self.keyed_maximum[key].fetch_max(keyed, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(10));
                self.keyed_active[key].fetch_sub(1, Ordering::SeqCst);
                self.active.fetch_sub(1, Ordering::SeqCst);
                Ok(message(u64::from(method_id)))
            })
        }
    }

    fn run_parallel(service: Arc<dyn LocalService>, methods: &[u16]) {
        std::thread::scope(|scope| {
            for method in methods.iter().copied() {
                let service = Arc::clone(&service);
                scope.spawn(move || {
                    run_to_completion(service.dispatch(1, method, message(0))).expect("response");
                });
            }
        });
    }

    #[test]
    fn concurrent_overlaps_and_serial_never_does() {
        let concurrent_probe = Arc::new(ProbeService::new());
        let concurrent: Arc<dyn LocalService> =
            Arc::new(Concurrent::new(Arc::clone(&concurrent_probe)));
        run_parallel(concurrent, &[0, 1, 2, 3]);
        assert!(concurrent_probe.maximum.load(Ordering::SeqCst) >= 2);

        let serial_probe = Arc::new(ProbeService::new());
        let serial: Arc<dyn LocalService> = Arc::new(Serial::new(Arc::clone(&serial_probe)));
        run_parallel(serial, &[0, 1, 2, 3]);
        assert_eq!(serial_probe.maximum.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn keyed_overlaps_different_keys_but_never_equal_keys() {
        let probe = Arc::new(ProbeService::new());
        let keyed: Arc<dyn LocalService> = Arc::new(Keyed::new(
            Arc::clone(&probe),
            |_interface, method, _params: &OwnedMessage| method % 2,
        ));
        run_parallel(keyed, &[0, 2, 1, 3]);
        assert!(probe.maximum.load(Ordering::SeqCst) >= 2);
        assert_eq!(probe.keyed_maximum[0].load(Ordering::SeqCst), 1);
        assert_eq!(probe.keyed_maximum[1].load(Ordering::SeqCst), 1);
    }

    #[test]
    fn serial_gate_is_fifo_and_skips_canceled_waiters() {
        let gate = Arc::new(SerialGate::new());
        let mut first = gate.acquire();
        let mut second = gate.acquire();
        let mut canceled = gate.acquire();
        let mut fourth = gate.acquire();
        let mut context = Context::from_waker(Waker::noop());
        let first = match Pin::new(&mut first).poll(&mut context) {
            Poll::Ready(Ok(permit)) => permit,
            _ => panic!("first permit"),
        };
        assert!(matches!(
            Pin::new(&mut second).poll(&mut context),
            Poll::Pending
        ));
        assert!(matches!(
            Pin::new(&mut canceled).poll(&mut context),
            Poll::Pending
        ));
        assert!(matches!(
            Pin::new(&mut fourth).poll(&mut context),
            Poll::Pending
        ));
        drop(canceled);
        drop(first);
        let second = match Pin::new(&mut second).poll(&mut context) {
            Poll::Ready(Ok(permit)) => permit,
            _ => panic!("second permit"),
        };
        assert!(matches!(
            Pin::new(&mut fourth).poll(&mut context),
            Poll::Pending
        ));
        drop(second);
        assert!(matches!(
            Pin::new(&mut fourth).poll(&mut context),
            Poll::Ready(Ok(_))
        ));
    }

    #[test]
    fn local_server_constructs_and_keeps_non_send_state_on_its_thread() {
        let server = LocalServer::spawn(
            "capnp-local-test",
            8,
            || Rc::new(Cell::new(0_u64)),
            |state, _interface, _method, _params| {
                state.set(state.get() + 1);
                Ok(message(state.get()))
            },
        )
        .expect("local server");
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<LocalServer>();
        let server: Arc<dyn LocalService> = Arc::new(server);
        run_to_completion(Arc::clone(&server).dispatch(1, 0, message(0))).expect("first");
        let second = run_to_completion(server.dispatch(1, 0, message(0))).expect("second");
        assert_eq!(
            second
                .root_struct()
                .expect("root")
                .root()
                .with_reader(|reader| {
                    reader
                        .data_section()
                        .expect("data")
                        .read_u64(0, 0)
                        .expect("value")
                })
                .expect("reader"),
            2
        );
    }

    #[test]
    fn executor_service_supports_bounded_pool_generic_and_tokio_callbacks() {
        let service = Arc::new(ProbeService::new());
        let pool = ThreadPoolExecutor::new(2, 8).expect("pool");
        let scheduled: Arc<dyn LocalService> =
            Arc::new(ExecutorService::new(Arc::clone(&service), pool.clone()));
        run_parallel(scheduled, &[0, 1, 2, 3]);
        assert!(service.maximum.load(Ordering::SeqCst) >= 2);

        let generic_pool = pool.clone();
        let generic = GenericExecutor::new(move |task| generic_pool.spawn(task));
        let generic_service =
            Arc::new(ExecutorService::new(Arc::new(ProbeService::new()), generic));
        run_to_completion(generic_service.dispatch(1, 7, message(0))).expect("generic");

        let tokio_style_pool = pool;
        let tokio = TokioExecutor::new(move |task| {
            tokio_style_pool.spawn(task).expect("tokio-style spawn");
        });
        let tokio_service = Arc::new(ExecutorService::new(Arc::new(ProbeService::new()), tokio));
        run_to_completion(tokio_service.dispatch(1, 8, message(0))).expect("tokio adapter");
    }

    #[test]
    fn executor_queue_rejects_overload_and_spawn_error_reaches_the_response() {
        let pool = ThreadPoolExecutor::new(1, 1).expect("pool");
        let (started_sender, started_receiver) = mpsc::sync_channel(0);
        let (release_sender, release_receiver) = mpsc::sync_channel(0);
        pool.spawn(Box::pin(async move {
            started_sender.send(()).expect("started");
            release_receiver.recv().expect("released");
        }))
        .expect("first task");
        started_receiver.recv().expect("worker started");
        pool.spawn(Box::pin(async {})).expect("queued task");
        assert!(matches!(
            pool.spawn(Box::pin(async {})),
            Err(SchedulerError::Overloaded { capacity: 1 })
        ));
        release_sender.send(()).expect("release worker");

        let failing =
            GenericExecutor::new(|_task| Err(SchedulerError::Spawn("test executor".to_owned())));
        let service = Arc::new(ExecutorService::new(Arc::new(ProbeService::new()), failing));
        assert!(matches!(
            run_to_completion(service.dispatch(1, 0, message(0))),
            Err(RpcError::Scheduler(SchedulerError::Spawn(reason))) if reason == "test executor"
        ));
    }

    #[test]
    fn local_server_queue_is_bounded_while_non_send_state_is_busy() {
        let (started_sender, started_receiver) = mpsc::sync_channel(0);
        let (release_sender, release_receiver) = mpsc::sync_channel(0);
        let server = LocalServer::spawn(
            "capnp-local-overload-test",
            1,
            move || (Rc::new(Cell::new(0_u64)), started_sender, release_receiver),
            |state, _interface, _method, _params| {
                if state.0.get() == 0 {
                    state.1.send(()).expect("started");
                    state.2.recv().expect("released");
                }
                state.0.set(state.0.get() + 1);
                Ok(message(state.0.get()))
            },
        )
        .expect("local server");
        let server = Arc::new(server);
        let first = Arc::clone(&server).dispatch(1, 0, message(0));
        started_receiver.recv().expect("handler started");
        let second = Arc::clone(&server).dispatch(1, 0, message(0));
        let overloaded = Arc::clone(&server).dispatch(1, 0, message(0));
        assert!(matches!(
            run_to_completion(overloaded),
            Err(RpcError::Scheduler(SchedulerError::Overloaded {
                capacity: 1
            }))
        ));
        release_sender.send(()).expect("release handler");
        run_to_completion(first).expect("first");
        run_to_completion(second).expect("second");
    }

    #[test]
    fn handler_panics_complete_waiters_and_workers_keep_running() {
        let pool = ThreadPoolExecutor::new(1, 4).expect("pool");
        let panicking = Arc::new(ExecutorService::new(Arc::new(PanicService), pool.clone()));
        assert!(matches!(
            run_to_completion(panicking.dispatch(1, 0, message(0))),
            Err(RpcError::Scheduler(SchedulerError::Panicked))
        ));
        let synchronous = Arc::new(ExecutorService::new(
            Arc::new(SynchronousPanicService),
            pool.clone(),
        ));
        assert!(matches!(
            run_to_completion(synchronous.dispatch(1, 0, message(0))),
            Err(RpcError::Scheduler(SchedulerError::Panicked))
        ));
        let healthy = Arc::new(ExecutorService::new(Arc::new(ProbeService::new()), pool));
        run_to_completion(healthy.dispatch(1, 0, message(0))).expect("worker survived panic");

        let local = LocalServer::spawn(
            "capnp-local-panic-test",
            2,
            || Rc::new(Cell::new(false)),
            |state, _interface, _method, _params| {
                if !state.get() {
                    state.set(true);
                    panic!("local handler panic");
                }
                Ok(message(1))
            },
        )
        .expect("local server");
        let local = Arc::new(local);
        assert!(matches!(
            run_to_completion(Arc::clone(&local).dispatch(1, 0, message(0))),
            Err(RpcError::Scheduler(SchedulerError::Panicked))
        ));
        run_to_completion(local.dispatch(1, 0, message(0))).expect("local worker survived panic");

        let panic_executor = GenericExecutor::new(|_task| panic!("executor panic"));
        let service = Arc::new(ExecutorService::new(
            Arc::new(ProbeService::new()),
            panic_executor,
        ));
        assert!(matches!(
            run_to_completion(service.dispatch(1, 0, message(0))),
            Err(RpcError::Scheduler(SchedulerError::Panicked))
        ));
    }
}
