//! Executor-neutral local capability clients, responses, and pipelines.

use std::any::Any;
use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::task::{Context, Poll, Waker};

use capnp_message::{OwnedMessage, OwnedPointerRef};
use capnp_schema::CompiledSchema;

use crate::{BoxFuture, LocalResponseFuture, LocalService, MessageFuture, RpcError, TypedReader};

const DEFAULT_CAPABILITY_LIMIT: usize = 4096;
static NEXT_SERVER_SET_ID: AtomicU64 = AtomicU64::new(1);

/// A stable, cloneable failure used by broken and disabled local clients.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CapabilityFailure {
    Broken(String),
    Disabled,
    Rejected(String),
}

impl fmt::Display for CapabilityFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Broken(reason) => write!(formatter, "broken capability: {reason}"),
            Self::Disabled => formatter.write_str("capability is disabled"),
            Self::Rejected(reason) => write!(formatter, "capability promise rejected: {reason}"),
        }
    }
}

impl std::error::Error for CapabilityFailure {}

/// A response message and its process-local capability table.
#[derive(Clone, Debug)]
pub struct LocalResponse {
    message: Arc<OwnedMessage>,
    capabilities: CapabilityList,
}

impl LocalResponse {
    pub fn new(message: Arc<OwnedMessage>) -> Self {
        Self {
            message,
            capabilities: CapabilityList::default(),
        }
    }

    pub fn with_capabilities(message: Arc<OwnedMessage>, capabilities: CapabilityList) -> Self {
        Self {
            message,
            capabilities,
        }
    }

    pub fn message(&self) -> &Arc<OwnedMessage> {
        &self.message
    }

    pub fn capabilities(&self) -> &CapabilityList {
        &self.capabilities
    }
}

/// A bounded capability-pointer table whose clones preserve client identity.
#[derive(Clone, Debug)]
pub struct CapabilityList {
    entries: Vec<Option<LocalClient>>,
    limit: usize,
}

impl Default for CapabilityList {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            limit: DEFAULT_CAPABILITY_LIMIT,
        }
    }
}

impl CapabilityList {
    pub fn new(length: usize, limit: usize) -> Result<Self, RpcError> {
        if length > limit {
            return Err(RpcError::CapabilityLimit {
                requested: length,
                limit,
            });
        }
        Ok(Self {
            entries: vec![None; length],
            limit,
        })
    }

    pub fn from_clients(
        clients: impl IntoIterator<Item = Option<LocalClient>>,
        limit: usize,
    ) -> Result<Self, RpcError> {
        let entries = clients.into_iter().collect::<Vec<_>>();
        if entries.len() > limit {
            return Err(RpcError::CapabilityLimit {
                requested: entries.len(),
                limit,
            });
        }
        Ok(Self { entries, limit })
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, index: usize) -> Result<Option<LocalClient>, RpcError> {
        self.entries
            .get(index)
            .cloned()
            .ok_or(RpcError::CapabilityIndex {
                index,
                length: self.entries.len(),
            })
    }

    pub fn set(&mut self, index: usize, client: Option<LocalClient>) -> Result<(), RpcError> {
        let length = self.entries.len();
        let entry = self
            .entries
            .get_mut(index)
            .ok_or(RpcError::CapabilityIndex { index, length })?;
        *entry = client;
        Ok(())
    }

    pub fn push(&mut self, client: Option<LocalClient>) -> Result<u32, RpcError> {
        if self.entries.len() >= self.limit {
            return Err(RpcError::CapabilityLimit {
                requested: self.entries.len().saturating_add(1),
                limit: self.limit,
            });
        }
        let index = u32::try_from(self.entries.len()).map_err(|_| RpcError::CapabilityLimit {
            requested: self.entries.len().saturating_add(1),
            limit: self.limit,
        })?;
        self.entries.push(client);
        Ok(index)
    }
}

/// A checked path through result pointer fields.
#[derive(Clone, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct PipelineTransform {
    pointer_fields: Vec<u16>,
}

impl PipelineTransform {
    pub fn root() -> Self {
        Self::default()
    }

    pub fn pointer_field(&self, index: u16) -> Self {
        let mut pointer_fields = self.pointer_fields.clone();
        pointer_fields.push(index);
        Self { pointer_fields }
    }

    pub fn pointer_fields(&self) -> &[u16] {
        &self.pointer_fields
    }

    pub fn capability(&self, message: &Arc<OwnedMessage>) -> Result<Option<u32>, RpcError> {
        let Some((&last, parents)) = self.pointer_fields.split_last() else {
            return Err(RpcError::PipelineExpectedCapability);
        };
        let mut structure = message.root_struct()?.into_root();
        for index in parents {
            structure = structure
                .child_struct(*index)?
                .ok_or(RpcError::PipelineExpectedStruct)?;
        }
        match structure.child_pointer(last)? {
            OwnedPointerRef::Null => Ok(None),
            OwnedPointerRef::Capability(value) => Ok(Some(value)),
            OwnedPointerRef::Struct(_) | OwnedPointerRef::List(_) => {
                Err(RpcError::PipelineExpectedCapability)
            }
        }
    }
}

/// Provisional capability paths published synchronously by a local call.
#[derive(Clone, Debug)]
pub struct PipelineBuilder {
    capabilities: BTreeMap<PipelineTransform, LocalClient>,
    limit: usize,
}

impl Default for PipelineBuilder {
    fn default() -> Self {
        Self::new(DEFAULT_CAPABILITY_LIMIT)
    }
}

impl PipelineBuilder {
    pub fn new(limit: usize) -> Self {
        Self {
            capabilities: BTreeMap::new(),
            limit,
        }
    }

    pub fn set_capability(
        &mut self,
        transform: PipelineTransform,
        client: LocalClient,
    ) -> Result<(), RpcError> {
        if self.capabilities.contains_key(&transform) {
            return Err(RpcError::DuplicatePipelinePath);
        }
        if self.capabilities.len() >= self.limit {
            return Err(RpcError::PipelineLimit {
                requested: self.capabilities.len().saturating_add(1),
                limit: self.limit,
            });
        }
        self.capabilities.insert(transform, client);
        Ok(())
    }

    fn get(&self, transform: &PipelineTransform) -> Option<LocalClient> {
        self.capabilities.get(transform).cloned()
    }
}

/// One local dispatch plus an optional provisional result pipeline.
pub struct LocalCall {
    response: LocalResponseFuture,
    pipeline: Option<PipelineBuilder>,
}

impl fmt::Debug for LocalCall {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalCall")
            .field("pipeline", &self.pipeline)
            .finish_non_exhaustive()
    }
}

impl LocalCall {
    pub fn new(response: LocalResponseFuture) -> Self {
        Self {
            response,
            pipeline: None,
        }
    }

    pub fn from_message_future(response: MessageFuture) -> Self {
        Self::new(Box::pin(
            async move { response.await.map(LocalResponse::new) },
        ))
    }

    pub fn set_pipeline(&mut self, pipeline: PipelineBuilder) -> Result<(), RpcError> {
        if self.pipeline.is_some() {
            return Err(RpcError::PipelineAlreadySet);
        }
        self.pipeline = Some(pipeline);
        Ok(())
    }

    pub fn with_pipeline(mut self, pipeline: PipelineBuilder) -> Result<Self, RpcError> {
        self.set_pipeline(pipeline)?;
        Ok(self)
    }

    #[doc(hidden)]
    pub fn into_response(self) -> LocalResponseFuture {
        self.response
    }
}

#[derive(Clone)]
struct RegisteredServer {
    set_id: u64,
    server: Arc<dyn Any + Send + Sync>,
}

struct SettledClient {
    service: Arc<dyn LocalService>,
    registration: Option<RegisteredServer>,
}

enum ClientKind {
    Settled(SettledClient),
    Promise(Arc<PromiseState>),
    Pipeline(PipelineSource),
    Failed(CapabilityFailure),
}

struct ClientCore {
    kind: ClientKind,
}

/// A thread-safe local client with stable process-local identity.
#[derive(Clone)]
pub struct LocalClient {
    schema: Arc<CompiledSchema>,
    core: Arc<ClientCore>,
}

impl fmt::Debug for LocalClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalClient")
            .field("identity", &Arc::as_ptr(&self.core))
            .finish_non_exhaustive()
    }
}

impl LocalClient {
    pub fn new(schema: Arc<CompiledSchema>, service: Arc<dyn LocalService>) -> Self {
        Self::settled(schema, service, None)
    }

    fn settled(
        schema: Arc<CompiledSchema>,
        service: Arc<dyn LocalService>,
        registration: Option<RegisteredServer>,
    ) -> Self {
        Self {
            schema,
            core: Arc::new(ClientCore {
                kind: ClientKind::Settled(SettledClient {
                    service,
                    registration,
                }),
            }),
        }
    }

    pub fn promise(schema: Arc<CompiledSchema>) -> (Self, PromiseClientResolver) {
        let state = Arc::new(PromiseState::new());
        let client = Self {
            schema,
            core: Arc::new(ClientCore {
                kind: ClientKind::Promise(Arc::clone(&state)),
            }),
        };
        let resolver = PromiseClientResolver {
            state,
            promise_core: Arc::downgrade(&client.core),
            resolved: false,
        };
        (client, resolver)
    }

    pub fn broken(schema: Arc<CompiledSchema>, reason: impl Into<String>) -> Self {
        Self::failed(schema, CapabilityFailure::Broken(reason.into()))
    }

    pub fn disabled(schema: Arc<CompiledSchema>) -> Self {
        Self::failed(schema, CapabilityFailure::Disabled)
    }

    fn failed(schema: Arc<CompiledSchema>, failure: CapabilityFailure) -> Self {
        Self {
            schema,
            core: Arc::new(ClientCore {
                kind: ClientKind::Failed(failure),
            }),
        }
    }

    fn from_pipeline(schema: Arc<CompiledSchema>, pipeline: PipelineSource) -> Self {
        Self {
            schema,
            core: Arc::new(ClientCore {
                kind: ClientKind::Pipeline(pipeline),
            }),
        }
    }

    pub fn schema(&self) -> &Arc<CompiledSchema> {
        &self.schema
    }

    pub fn same_identity(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.core, &other.core)
    }

    pub fn call<R, P>(
        &self,
        interface_id: u64,
        method_id: u16,
        params: Arc<OwnedMessage>,
        pipeline: P,
    ) -> PendingCall<R, P>
    where
        R: TypedReader,
        P: PipelineBinding,
    {
        let call = self.start_call(interface_id, method_id, params);
        let source = PipelineSource::root(Arc::clone(&self.schema), call.clone());
        PendingCall {
            call,
            schema: Arc::clone(&self.schema),
            pipeline: pipeline.bind(source),
            marker: PhantomData,
        }
    }

    pub fn call_streaming(
        &self,
        interface_id: u64,
        method_id: u16,
        params: Arc<OwnedMessage>,
    ) -> StreamingCall {
        // Starting the call remains synchronous so generated streaming calls
        // retain their send-now/E-order contract.
        let call = self.start_call(interface_id, method_id, params);
        StreamingCall {
            completion: Box::pin(async move {
                call.await_response().await?;
                Ok(())
            }),
        }
    }

    #[doc(hidden)]
    pub fn call_untyped(
        &self,
        interface_id: u64,
        method_id: u16,
        params: Arc<OwnedMessage>,
    ) -> UntypedPendingCall {
        let call = self.start_call(interface_id, method_id, params);
        UntypedPendingCall {
            pipeline: UntypedPipeline {
                source: PipelineSource::root(Arc::clone(&self.schema), call.clone()),
            },
            call,
        }
    }

    pub fn when_resolved(&self) -> BoxFuture<Result<LocalClient, RpcError>> {
        let mut current = self.clone();
        Box::pin(async move {
            let mut visited = BTreeMap::new();
            loop {
                if visited
                    .insert(Arc::as_ptr(&current.core) as usize, ())
                    .is_some()
                {
                    return Err(RpcError::PromiseCycle);
                }
                match &current.core.kind {
                    ClientKind::Settled(_) => return Ok(current),
                    ClientKind::Failed(failure) => {
                        return Err(RpcError::LocalCapability(failure.clone()));
                    }
                    ClientKind::Promise(state) => {
                        current = ResolutionFuture {
                            state: Arc::clone(state),
                        }
                        .await?;
                    }
                    ClientKind::Pipeline(pipeline) => {
                        current = pipeline.resolve_client().await?;
                    }
                }
            }
        })
    }

    fn try_resolved(&self) -> Option<LocalClient> {
        let mut current = self.clone();
        for _ in 0..64 {
            match &current.core.kind {
                ClientKind::Settled(_) => return Some(current),
                ClientKind::Failed(_) => return None,
                ClientKind::Promise(state) => current = state.try_resolved()?,
                ClientKind::Pipeline(pipeline) => current = pipeline.try_resolved_client()?,
            }
        }
        None
    }

    fn resolves_to_core(&self, target: &Arc<ClientCore>) -> bool {
        let mut current = self.clone();
        let mut visited = BTreeMap::new();
        loop {
            if Arc::ptr_eq(&current.core, target) {
                return true;
            }
            if visited
                .insert(Arc::as_ptr(&current.core) as usize, ())
                .is_some()
            {
                return false;
            }
            current = match &current.core.kind {
                ClientKind::Promise(state) => match state.try_resolved() {
                    Some(client) => client,
                    None => return false,
                },
                ClientKind::Pipeline(pipeline) => match pipeline.try_resolved_client() {
                    Some(client) => client,
                    None => return false,
                },
                ClientKind::Settled(_) | ClientKind::Failed(_) => return false,
            };
        }
    }

    fn start_call(
        &self,
        interface_id: u64,
        method_id: u16,
        params: Arc<OwnedMessage>,
    ) -> SharedCall {
        match &self.core.kind {
            ClientKind::Settled(settled) => {
                let LocalCall { response, pipeline } =
                    Arc::clone(&settled.service).dispatch_call(interface_id, method_id, params);
                SharedCall::new(response, pipeline.unwrap_or_default())
            }
            ClientKind::Failed(failure) => {
                SharedCall::failed(RpcError::LocalCapability(failure.clone()))
            }
            ClientKind::Pipeline(pipeline) => {
                let pipeline = pipeline.clone();
                SharedCall::new(
                    Box::pin(async move {
                        let target = pipeline.resolve_client().await?;
                        target
                            .start_call(interface_id, method_id, params)
                            .await_response()
                            .await
                    }),
                    PipelineBuilder::default(),
                )
            }
            ClientKind::Promise(state) => {
                state.enqueue(interface_id, method_id, params, Arc::clone(&self.schema))
            }
        }
    }
}

struct QueuedCall {
    interface_id: u64,
    method_id: u16,
    params: Arc<OwnedMessage>,
    deferred: DeferredCallResolver,
}

enum PromiseResolution {
    Pending,
    Resolved(Result<LocalClient, CapabilityFailure>),
}

struct PromiseStateInner {
    resolution: PromiseResolution,
    calls: VecDeque<QueuedCall>,
    waiters: Vec<Waker>,
}

struct PromiseState {
    inner: Mutex<PromiseStateInner>,
}

impl PromiseState {
    fn new() -> Self {
        Self {
            inner: Mutex::new(PromiseStateInner {
                resolution: PromiseResolution::Pending,
                calls: VecDeque::new(),
                waiters: Vec::new(),
            }),
        }
    }

    fn enqueue(
        &self,
        interface_id: u64,
        method_id: u16,
        params: Arc<OwnedMessage>,
        _schema: Arc<CompiledSchema>,
    ) -> SharedCall {
        let (future, deferred, state) = deferred_call();
        let outer = SharedCall::deferred(Box::pin(future), state);
        let immediate = {
            let mut inner = lock(&self.inner);
            match &inner.resolution {
                PromiseResolution::Pending => {
                    inner.calls.push_back(QueuedCall {
                        interface_id,
                        method_id,
                        params,
                        deferred,
                    });
                    return outer;
                }
                PromiseResolution::Resolved(resolution) => Some(resolution.clone()),
            }
        };
        if let Some(resolution) = immediate {
            assign_queued(
                QueuedCall {
                    interface_id,
                    method_id,
                    params,
                    deferred,
                },
                &resolution,
            );
        }
        outer
    }

    fn resolve(&self, resolution: Result<LocalClient, CapabilityFailure>) -> Result<(), RpcError> {
        let (calls, waiters) = {
            let mut inner = lock(&self.inner);
            if !matches!(inner.resolution, PromiseResolution::Pending) {
                return Err(RpcError::PromiseAlreadyResolved);
            }
            inner.resolution = PromiseResolution::Resolved(resolution.clone());
            (
                std::mem::take(&mut inner.calls),
                std::mem::take(&mut inner.waiters),
            )
        };
        for call in calls {
            assign_queued(call, &resolution);
        }
        for waiter in waiters {
            waiter.wake();
        }
        Ok(())
    }

    fn try_resolved(&self) -> Option<LocalClient> {
        match &lock(&self.inner).resolution {
            PromiseResolution::Resolved(Ok(client)) => Some(client.clone()),
            PromiseResolution::Pending | PromiseResolution::Resolved(Err(_)) => None,
        }
    }
}

fn assign_queued(call: QueuedCall, resolution: &Result<LocalClient, CapabilityFailure>) {
    let assigned = match resolution {
        Ok(client) => client.start_call(call.interface_id, call.method_id, call.params),
        Err(failure) => SharedCall::failed(RpcError::LocalCapability(failure.clone())),
    };
    call.deferred.assign(assigned);
}

struct ResolutionFuture {
    state: Arc<PromiseState>,
}

impl Future for ResolutionFuture {
    type Output = Result<LocalClient, RpcError>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let mut inner = lock(&self.state.inner);
        match &inner.resolution {
            PromiseResolution::Pending => {
                register_waker(&mut inner.waiters, context.waker());
                Poll::Pending
            }
            PromiseResolution::Resolved(Ok(client)) => Poll::Ready(Ok(client.clone())),
            PromiseResolution::Resolved(Err(failure)) => {
                Poll::Ready(Err(RpcError::LocalCapability(failure.clone())))
            }
        }
    }
}

/// One-shot authority for a local promise client.
///
/// Resolution consumes the authority, so a resolver cannot be used twice:
///
/// ```compile_fail
/// fn resolve_twice(
///     resolver: capnp_rpc::PromiseClientResolver,
///     client: capnp_rpc::LocalClient,
/// ) {
///     resolver.fulfill(client.clone()).unwrap();
///     resolver.fulfill(client).unwrap();
/// }
/// ```
pub struct PromiseClientResolver {
    state: Arc<PromiseState>,
    promise_core: Weak<ClientCore>,
    resolved: bool,
}

impl fmt::Debug for PromiseClientResolver {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PromiseClientResolver")
            .field("resolved", &self.resolved)
            .finish_non_exhaustive()
    }
}

impl PromiseClientResolver {
    pub fn fulfill(mut self, client: LocalClient) -> Result<(), RpcError> {
        if let Some(core) = self.promise_core.upgrade() {
            if client.resolves_to_core(&core) {
                return Err(RpcError::PromiseCycle);
            }
        }
        self.state.resolve(Ok(client))?;
        self.resolved = true;
        Ok(())
    }

    pub fn reject(mut self, reason: impl Into<String>) -> Result<(), RpcError> {
        self.state
            .resolve(Err(CapabilityFailure::Rejected(reason.into())))?;
        self.resolved = true;
        Ok(())
    }
}

impl Drop for PromiseClientResolver {
    fn drop(&mut self) {
        if !self.resolved {
            let _ = self.state.resolve(Err(CapabilityFailure::Rejected(
                "resolver dropped before fulfillment".to_owned(),
            )));
        }
    }
}

#[derive(Clone)]
struct SharedCall {
    inner: Arc<SharedCallInner>,
    pipeline: SharedPipeline,
}

#[derive(Clone)]
enum SharedPipeline {
    Local(Arc<Mutex<PipelineBuilder>>),
    Deferred(Arc<Mutex<DeferredCallState>>),
}

impl fmt::Debug for SharedCall {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("SharedCall").finish_non_exhaustive()
    }
}

struct SharedCallInner {
    state: Mutex<SharedCallState>,
}

enum SharedCallState {
    Pending {
        future: LocalResponseFuture,
        waiters: Vec<Waker>,
    },
    Polling {
        waiters: Vec<Waker>,
    },
    Ready(Result<LocalResponse, Arc<RpcError>>),
}

impl SharedCall {
    fn new(response: LocalResponseFuture, pipeline: PipelineBuilder) -> Self {
        Self {
            inner: Arc::new(SharedCallInner {
                state: Mutex::new(SharedCallState::Pending {
                    future: response,
                    waiters: Vec::new(),
                }),
            }),
            pipeline: SharedPipeline::Local(Arc::new(Mutex::new(pipeline))),
        }
    }

    fn deferred(response: LocalResponseFuture, state: Arc<Mutex<DeferredCallState>>) -> Self {
        Self {
            inner: Arc::new(SharedCallInner {
                state: Mutex::new(SharedCallState::Pending {
                    future: response,
                    waiters: Vec::new(),
                }),
            }),
            pipeline: SharedPipeline::Deferred(state),
        }
    }

    fn failed(error: RpcError) -> Self {
        Self {
            inner: Arc::new(SharedCallInner {
                state: Mutex::new(SharedCallState::Ready(Err(Arc::new(error)))),
            }),
            pipeline: SharedPipeline::Local(Arc::new(Mutex::new(PipelineBuilder::default()))),
        }
    }

    fn poll_response(&self, context: &mut Context<'_>) -> Poll<Result<LocalResponse, RpcError>> {
        let (mut future, mut waiters) = {
            let mut state = lock(&self.inner.state);
            match &mut *state {
                SharedCallState::Ready(result) => {
                    return Poll::Ready(clone_shared_result(result));
                }
                SharedCallState::Polling { waiters } => {
                    register_waker(waiters, context.waker());
                    return Poll::Pending;
                }
                SharedCallState::Pending { .. } => {}
            }
            let SharedCallState::Pending { future, waiters } = std::mem::replace(
                &mut *state,
                SharedCallState::Polling {
                    waiters: Vec::new(),
                },
            ) else {
                unreachable!("pending response state was replaced atomically")
            };
            (future, waiters)
        };
        register_waker(&mut waiters, context.waker());
        let polled = future.as_mut().poll(context);
        let (output, wake) = {
            let mut state = lock(&self.inner.state);
            let SharedCallState::Polling {
                waiters: concurrent_waiters,
            } = &mut *state
            else {
                unreachable!("only the polling task can complete this response")
            };
            waiters.append(concurrent_waiters);
            match polled {
                Poll::Pending => {
                    *state = SharedCallState::Pending { future, waiters };
                    return Poll::Pending;
                }
                Poll::Ready(result) => {
                    let shared = result.map_err(Arc::new);
                    let output = clone_shared_result(&shared);
                    *state = SharedCallState::Ready(shared);
                    (output, waiters)
                }
            }
        };
        for waiter in wake {
            waiter.wake();
        }
        Poll::Ready(output)
    }

    fn await_response(&self) -> SharedCallFuture {
        SharedCallFuture { call: self.clone() }
    }

    fn try_response(&self) -> Option<Result<LocalResponse, RpcError>> {
        match &*lock(&self.inner.state) {
            SharedCallState::Ready(result) => Some(clone_shared_result(result)),
            SharedCallState::Pending { .. } | SharedCallState::Polling { .. } => None,
        }
    }

    fn provisional(&self, transform: &PipelineTransform) -> Option<LocalClient> {
        match &self.pipeline {
            SharedPipeline::Local(pipeline) => lock(pipeline).get(transform),
            SharedPipeline::Deferred(state) => {
                let assigned = lock(state).assigned.clone()?;
                assigned.provisional(transform)
            }
        }
    }
}

fn clone_shared_result(
    result: &Result<LocalResponse, Arc<RpcError>>,
) -> Result<LocalResponse, RpcError> {
    result.clone().map_err(RpcError::Shared)
}

struct SharedCallFuture {
    call: SharedCall,
}

impl Future for SharedCallFuture {
    type Output = Result<LocalResponse, RpcError>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        self.call.poll_response(context)
    }
}

struct DeferredCallState {
    assigned: Option<SharedCall>,
    waiters: Vec<Waker>,
}

struct DeferredCallFuture {
    state: Arc<Mutex<DeferredCallState>>,
}

struct DeferredCallResolver {
    state: Arc<Mutex<DeferredCallState>>,
}

fn deferred_call() -> (
    DeferredCallFuture,
    DeferredCallResolver,
    Arc<Mutex<DeferredCallState>>,
) {
    let state = Arc::new(Mutex::new(DeferredCallState {
        assigned: None,
        waiters: Vec::new(),
    }));
    let future = DeferredCallFuture {
        state: Arc::clone(&state),
    };
    let resolver = DeferredCallResolver {
        state: Arc::clone(&state),
    };
    (future, resolver, state)
}

impl Future for DeferredCallFuture {
    type Output = Result<LocalResponse, RpcError>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let call = {
            let mut state = lock(&self.state);
            let Some(call) = state.assigned.clone() else {
                register_waker(&mut state.waiters, context.waker());
                return Poll::Pending;
            };
            call
        };
        call.poll_response(context)
    }
}

impl DeferredCallResolver {
    fn assign(self, call: SharedCall) {
        let waiters = {
            let mut state = lock(&self.state);
            if state.assigned.is_some() {
                return;
            }
            state.assigned = Some(call);
            std::mem::take(&mut state.waiters)
        };
        for waiter in waiters {
            waiter.wake();
        }
    }
}

/// A response-bound source propagated through generated pipeline values.
#[doc(hidden)]
#[derive(Clone)]
pub struct PipelineSource {
    schema: Arc<CompiledSchema>,
    call: SharedCall,
    transform: PipelineTransform,
}

impl fmt::Debug for PipelineSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PipelineSource")
            .field("transform", &self.transform)
            .finish_non_exhaustive()
    }
}

impl PipelineSource {
    fn root(schema: Arc<CompiledSchema>, call: SharedCall) -> Self {
        Self {
            schema,
            call,
            transform: PipelineTransform::root(),
        }
    }

    pub fn at(&self, transform: PipelineTransform) -> Self {
        Self {
            schema: Arc::clone(&self.schema),
            call: self.call.clone(),
            transform,
        }
    }

    fn client(&self) -> LocalClient {
        LocalClient::from_pipeline(Arc::clone(&self.schema), self.clone())
    }

    async fn resolve_client(&self) -> Result<LocalClient, RpcError> {
        PipelineClientFuture {
            source: self.clone(),
        }
        .await
    }

    fn try_resolved_client(&self) -> Option<LocalClient> {
        if let Some(client) = self.call.provisional(&self.transform) {
            return Some(client);
        }
        let response = self.call.try_response()?.ok()?;
        let index = self.transform.capability(response.message()).ok()??;
        response
            .capabilities()
            .get(usize::try_from(index).ok()?)
            .ok()?
    }
}

struct PipelineClientFuture {
    source: PipelineSource,
}

impl Future for PipelineClientFuture {
    type Output = Result<LocalClient, RpcError>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        if let Some(client) = self.source.call.provisional(&self.source.transform) {
            return Poll::Ready(Ok(client));
        }
        let response = match self.source.call.poll_response(context) {
            Poll::Ready(response) => response?,
            Poll::Pending => {
                // Polling a deferred response may have assigned a call with a
                // provisional pipeline. Recheck before sleeping on the final
                // response so promise pipelines cannot be stranded.
                return self
                    .source
                    .call
                    .provisional(&self.source.transform)
                    .map_or(Poll::Pending, |client| Poll::Ready(Ok(client)));
            }
        };
        let Some(index) = self.source.transform.capability(response.message())? else {
            return Poll::Ready(Ok(LocalClient::disabled(Arc::clone(&self.source.schema))));
        };
        Poll::Ready(
            response
                .capabilities()
                .get(
                    usize::try_from(index).map_err(|_| RpcError::CapabilityIndex {
                        index: usize::MAX,
                        length: response.capabilities().len(),
                    })?,
                )?
                .ok_or(RpcError::MissingCapability(index)),
        )
    }
}

/// Implemented by generated and dynamic pipeline values that can be attached
/// to a shared local response.
pub trait PipelineBinding: Send + 'static {
    #[doc(hidden)]
    fn bind(self, source: PipelineSource) -> Self;
}

/// A capability-valued pipeline path without a statically generated client.
#[derive(Clone, Debug)]
pub struct CapabilityPipeline {
    transform: PipelineTransform,
    source: Option<PipelineSource>,
}

impl CapabilityPipeline {
    pub fn new(transform: PipelineTransform) -> Self {
        Self {
            transform,
            source: None,
        }
    }

    #[doc(hidden)]
    pub fn from_parts(transform: PipelineTransform, source: Option<PipelineSource>) -> Self {
        Self { transform, source }
    }

    pub fn transform(&self) -> &PipelineTransform {
        &self.transform
    }

    pub fn resolve(&self, message: &Arc<OwnedMessage>) -> Result<Option<u32>, RpcError> {
        self.transform.capability(message)
    }

    pub fn client(&self) -> Result<LocalClient, RpcError> {
        self.source
            .as_ref()
            .map(|source| source.at(self.transform.clone()).client())
            .ok_or(RpcError::UnboundPipeline)
    }
}

impl PipelineBinding for CapabilityPipeline {
    fn bind(mut self, source: PipelineSource) -> Self {
        self.source = Some(source);
        self
    }
}

/// Conversion used by generated typed pipeline clients.
pub trait FromLocalClient: Sized {
    fn from_local_client(client: LocalClient) -> Self;
}

#[derive(Clone, Debug)]
pub struct TypedPipeline<T> {
    transform: PipelineTransform,
    source: Option<PipelineSource>,
    marker: PhantomData<fn() -> T>,
}

impl<T> TypedPipeline<T> {
    pub fn new(transform: PipelineTransform) -> Self {
        Self {
            transform,
            source: None,
            marker: PhantomData,
        }
    }

    #[doc(hidden)]
    pub fn from_parts(transform: PipelineTransform, source: Option<PipelineSource>) -> Self {
        Self {
            transform,
            source,
            marker: PhantomData,
        }
    }

    pub fn transform(&self) -> &PipelineTransform {
        &self.transform
    }

    pub fn resolve(&self, message: &Arc<OwnedMessage>) -> Result<Option<u32>, RpcError> {
        self.transform.capability(message)
    }
}

impl<T: FromLocalClient> TypedPipeline<T> {
    pub fn client(&self) -> Result<T, RpcError> {
        let client = self
            .source
            .as_ref()
            .map(|source| source.at(self.transform.clone()).client())
            .ok_or(RpcError::UnboundPipeline)?;
        Ok(T::from_local_client(client))
    }
}

impl<T: Send + 'static> PipelineBinding for TypedPipeline<T> {
    fn bind(mut self, source: PipelineSource) -> Self {
        self.source = Some(source);
        self
    }
}

/// Type-erased response pipeline used by schema-driven clients.
#[doc(hidden)]
#[derive(Clone, Debug)]
pub struct UntypedPipeline {
    source: PipelineSource,
}

impl UntypedPipeline {
    pub fn client(&self, transform: PipelineTransform) -> LocalClient {
        self.source.at(transform).client()
    }
}

/// Type-erased pending call used by schema-driven clients.
#[doc(hidden)]
pub struct UntypedPendingCall {
    call: SharedCall,
    pub pipeline: UntypedPipeline,
}

impl UntypedPendingCall {
    pub fn response(self) -> LocalResponseFuture {
        let call = self.call;
        Box::pin(async move { call.await_response().await })
    }
}

/// A response future and a pipeline that remain valid independently.
pub struct PendingCall<R, P> {
    call: SharedCall,
    schema: Arc<CompiledSchema>,
    pub pipeline: P,
    marker: PhantomData<fn() -> R>,
}

impl<R, P> PendingCall<R, P>
where
    R: TypedReader,
{
    pub fn response(self) -> BoxFuture<Result<R, RpcError>> {
        let schema = self.schema;
        let call = self.call;
        Box::pin(async move {
            let response = call.await_response().await?;
            R::from_message(schema, Arc::clone(response.message()))
        })
    }

    pub fn send_ignoring_result(self) -> BoxFuture<Result<(), RpcError>> {
        let call = self.call;
        Box::pin(async move {
            call.await_response().await?;
            Ok(())
        })
    }

    pub fn send_for_pipeline(self) -> P {
        self.pipeline
    }

    fn into_local_response(self) -> LocalResponseFuture {
        let call = self.call;
        Box::pin(async move { call.await_response().await })
    }
}

pub struct StreamingCall {
    completion: BoxFuture<Result<(), RpcError>>,
}

impl StreamingCall {
    pub fn completion(self) -> BoxFuture<Result<(), RpcError>> {
        self.completion
    }
}

/// Transfers a pending call's raw response/capability table without proxying.
pub fn tail_call<R, P>(call: PendingCall<R, P>) -> LocalResponseFuture
where
    R: TypedReader,
{
    call.into_local_response()
}

/// Starts and directly transfers an untyped local tail call.
pub fn direct_tail_call(
    client: &LocalClient,
    interface_id: u64,
    method_id: u16,
    params: Arc<OwnedMessage>,
) -> LocalResponseFuture {
    let call = client.start_call(interface_id, method_id, params);
    Box::pin(async move { call.await_response().await })
}

/// Reduces a future of a remote-style pending call to one pending response.
pub fn flatten_pending<R, P, F>(
    schema: Arc<CompiledSchema>,
    future: F,
    pipeline: P,
) -> PendingCall<R, P>
where
    R: TypedReader,
    P: PipelineBinding,
    F: Future<Output = Result<PendingCall<R, P>, RpcError>> + Send + 'static,
{
    let (deferred, resolver, state) = deferred_call();
    let response = Box::pin(async move {
        match future.await {
            Ok(call) => resolver.assign(call.call),
            Err(error) => resolver.assign(SharedCall::failed(error)),
        }
        deferred.await
    });
    let call = SharedCall::deferred(response, state);
    let source = PipelineSource::root(Arc::clone(&schema), call.clone());
    PendingCall {
        call,
        schema,
        pipeline: pipeline.bind(source),
        marker: PhantomData,
    }
}

/// A typed registry that can recover only servers it registered itself.
pub struct CapabilityServerSet<S: LocalService> {
    id: u64,
    schema: Arc<CompiledSchema>,
    registrations: Mutex<Vec<(Weak<S>, Weak<ClientCore>)>>,
}

impl<S: LocalService> fmt::Debug for CapabilityServerSet<S> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CapabilityServerSet")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl<S: LocalService> CapabilityServerSet<S> {
    pub fn new(schema: Arc<CompiledSchema>) -> Self {
        Self {
            id: NEXT_SERVER_SET_ID.fetch_add(1, Ordering::Relaxed),
            schema,
            registrations: Mutex::new(Vec::new()),
        }
    }

    pub fn add(&self, server: Arc<S>) -> LocalClient {
        let service: Arc<dyn LocalService> = server.clone();
        let erased: Arc<dyn Any + Send + Sync> = server.clone();
        let client = LocalClient::settled(
            Arc::clone(&self.schema),
            service,
            Some(RegisteredServer {
                set_id: self.id,
                server: erased,
            }),
        );
        lock(&self.registrations).push((Arc::downgrade(&server), Arc::downgrade(&client.core)));
        client
    }

    pub fn this_client(&self, server: &Arc<S>) -> Option<LocalClient> {
        let mut registrations = lock(&self.registrations);
        registrations
            .retain(|(server, client)| server.strong_count() != 0 && client.strong_count() != 0);
        registrations.iter().find_map(|(weak, core)| {
            weak.upgrade()
                .filter(|candidate| Arc::ptr_eq(candidate, server))
                .zip(core.upgrade())
                .map(|(_, core)| LocalClient {
                    schema: Arc::clone(&self.schema),
                    core,
                })
        })
    }

    pub fn try_get_local_server(&self, client: &LocalClient) -> Option<Arc<S>> {
        let client = client.try_resolved()?;
        let ClientKind::Settled(settled) = &client.core.kind else {
            return None;
        };
        let registration = settled.registration.as_ref()?;
        if registration.set_id != self.id {
            return None;
        }
        Arc::clone(&registration.server).downcast::<S>().ok()
    }

    pub fn get_local_server(
        &self,
        client: LocalClient,
    ) -> BoxFuture<Result<Option<Arc<S>>, RpcError>> {
        let id = self.id;
        Box::pin(async move {
            let client = client.when_resolved().await?;
            let ClientKind::Settled(settled) = &client.core.kind else {
                return Ok(None);
            };
            let Some(registration) = &settled.registration else {
                return Ok(None);
            };
            if registration.set_id != id {
                return Ok(None);
            }
            Ok(Arc::clone(&registration.server).downcast::<S>().ok())
        })
    }
}

fn register_waker(waiters: &mut Vec<Waker>, waker: &Waker) {
    if !waiters.iter().any(|existing| existing.will_wake(waker)) {
        waiters.push(waker.clone());
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
