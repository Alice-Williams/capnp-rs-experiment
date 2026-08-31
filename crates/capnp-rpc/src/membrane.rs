//! Recursive, bidirectional local capability membranes and revocation.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::task::{Context, Poll, Waker};

use capnp_message::OwnedMessage;
use capnp_schema::CompiledSchema;

use crate::local::{ClientResolution, SharedCallFuture, WeakLocalClient, deferred_client_call};
use crate::{
    BoxFuture, CapabilityFailure, LocalCall, LocalClient, LocalRequest, LocalResponse,
    LocalResponseFuture, LocalService, MessageFuture, PipelineBuilder, PipelineTransform, RpcError,
};

const DEFAULT_PIPELINE_LIMIT: usize = 4096;
static NEXT_MEMBRANE_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MembraneLimits {
    pub max_wrappers: usize,
    pub max_outstanding_calls: usize,
}

impl Default for MembraneLimits {
    fn default() -> Self {
        Self {
            max_wrappers: 4096,
            max_outstanding_calls: 4096,
        }
    }
}

/// A policy decision for one call crossing a membrane.
#[derive(Clone, Debug)]
pub enum MembraneDecision {
    /// Forward to the underlying target and recursively wrap capability tables.
    Forward,
    /// Redirect to a capability already on the caller's side. Capability tables
    /// are not automatically transformed for redirected calls.
    Redirect(LocalClient),
    /// Reject without dispatching the underlying target.
    Reject(CapabilityFailure),
}

/// Direction-aware policy for calls crossing a membrane.
///
/// Policies are shared across threads. A policy containing thread-local state
/// cannot be installed:
///
/// ```compile_fail
/// use std::rc::Rc;
/// struct ThreadLocalPolicy(Rc<()>);
/// impl capnp_rpc::MembranePolicy for ThreadLocalPolicy {}
/// ```
pub trait MembranePolicy: Send + Sync + 'static {
    fn inbound_call(
        &self,
        _interface_id: u64,
        _method_id: u16,
        _target: &LocalClient,
    ) -> MembraneDecision {
        MembraneDecision::Forward
    }

    fn outbound_call(
        &self,
        _interface_id: u64,
        _method_id: u16,
        _target: &LocalClient,
    ) -> MembraneDecision {
        MembraneDecision::Forward
    }

    /// Requests resolution of promise targets before a redirect is accepted.
    fn should_resolve_before_redirecting(&self) -> bool {
        false
    }
}

#[derive(Debug)]
struct PassthroughPolicy;

impl MembranePolicy for PassthroughPolicy {}

#[derive(Clone)]
pub struct Membrane {
    state: Arc<MembraneState>,
}

impl fmt::Debug for Membrane {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Membrane")
            .field("id", &self.state.id)
            .field("revoked", &self.state.failure().is_some())
            .finish()
    }
}

impl Membrane {
    pub fn new(policy: Arc<dyn MembranePolicy>) -> Self {
        Self::with_limits(policy, MembraneLimits::default())
    }

    pub fn with_limits(policy: Arc<dyn MembranePolicy>, limits: MembraneLimits) -> Self {
        Self {
            state: Arc::new(MembraneState::new(policy, None, limits)),
        }
    }

    fn with_usage(policy: Arc<dyn MembranePolicy>, usage: Arc<AtomicUsize>) -> Self {
        Self {
            state: Arc::new(MembraneState::new(
                policy,
                Some(usage),
                MembraneLimits::default(),
            )),
        }
    }

    /// Treats `inner` as inside and returns the outside view.
    pub fn wrap(&self, inner: LocalClient) -> LocalClient {
        self.state.wrap(inner, false)
    }

    /// Treats `outer` as outside and returns the inside view.
    pub fn reverse_wrap(&self, outer: LocalClient) -> LocalClient {
        self.state.wrap(outer, true)
    }

    pub fn revoke(&self, reason: impl Into<String>) -> Result<(), RpcError> {
        self.state.revoke(reason.into())
    }

    pub fn is_revoked(&self) -> bool {
        self.state.failure().is_some()
    }

    /// Copies an outside request into the membrane by preserving message bytes
    /// and reverse-wrapping every capability-table entry.
    pub fn copy_request_into(&self, request: &LocalRequest) -> Result<LocalRequest, RpcError> {
        self.state.map_request(request.clone(), true)
    }

    /// Copies an inside request out of the membrane.
    pub fn copy_request_out(&self, request: &LocalRequest) -> Result<LocalRequest, RpcError> {
        self.state.map_request(request.clone(), false)
    }

    /// Copies an outside response into the membrane.
    pub fn copy_response_into(&self, response: &LocalResponse) -> Result<LocalResponse, RpcError> {
        self.state.map_response(response.clone(), true)
    }

    /// Copies an inside response out of the membrane.
    pub fn copy_response_out(&self, response: &LocalResponse) -> Result<LocalResponse, RpcError> {
        self.state.map_response(response.clone(), false)
    }
}

struct RegistryEntry {
    underlying: WeakLocalClient,
    wrapper: WeakLocalClient,
    reverse: bool,
}

struct MembraneState {
    id: u64,
    policy: Arc<dyn MembranePolicy>,
    revoked: Mutex<Option<CapabilityFailure>>,
    waiters: Mutex<Vec<Weak<Mutex<Option<Waker>>>>>,
    registry: Mutex<Vec<RegistryEntry>>,
    usage: Option<Arc<AtomicUsize>>,
    limits: MembraneLimits,
}

impl MembraneState {
    fn new(
        policy: Arc<dyn MembranePolicy>,
        usage: Option<Arc<AtomicUsize>>,
        limits: MembraneLimits,
    ) -> Self {
        Self {
            id: NEXT_MEMBRANE_ID.fetch_add(1, Ordering::Relaxed),
            policy,
            revoked: Mutex::new(None),
            waiters: Mutex::new(Vec::new()),
            registry: Mutex::new(Vec::new()),
            usage,
            limits,
        }
    }

    fn wrap(self: &Arc<Self>, client: LocalClient, reverse: bool) -> LocalClient {
        let schema = Arc::clone(client.schema());
        self.try_wrap(client, reverse)
            .unwrap_or_else(|error| LocalClient::broken(schema, format!("{error}")))
    }

    fn try_wrap(
        self: &Arc<Self>,
        client: LocalClient,
        reverse: bool,
    ) -> Result<LocalClient, RpcError> {
        let mut registry = lock(&self.registry);
        registry.retain(|entry| entry.wrapper.upgrade().is_some());

        for entry in registry.iter() {
            if let Some(wrapper) = entry.wrapper.upgrade() {
                if wrapper.same_identity(&client) {
                    if entry.reverse == reverse {
                        return Ok(client);
                    }
                    if let Some(underlying) = entry.underlying.upgrade() {
                        return Ok(underlying);
                    }
                }
            }
            if entry.reverse == reverse {
                if let Some(underlying) = entry.underlying.upgrade() {
                    if underlying.same_identity(&client) {
                        if let Some(wrapper) = entry.wrapper.upgrade() {
                            return Ok(wrapper);
                        }
                    }
                }
            }
        }

        if registry.len() >= self.limits.max_wrappers {
            return Err(RpcError::MembraneLimit {
                resource: "wrappers",
                requested: registry.len().saturating_add(1),
                limit: self.limits.max_wrappers,
            });
        }

        let schema = Arc::clone(client.schema());
        let resolution = client.requires_resolution().then(|| {
            Arc::new(MembraneResolution {
                state: Arc::clone(self),
                target: client.clone(),
                reverse,
            }) as Arc<dyn ClientResolution>
        });
        let service = Arc::new(MembraneService {
            state: Arc::clone(self),
            target: client.clone(),
            reverse,
            _usage: self.use_token(),
        });
        let wrapper = match resolution {
            Some(resolution) => LocalClient::proxy(schema, service, resolution),
            None => LocalClient::new(schema, service),
        };
        registry.push(RegistryEntry {
            underlying: client.downgrade(),
            wrapper: wrapper.downgrade(),
            reverse,
        });
        Ok(wrapper)
    }

    fn map_request(
        self: &Arc<Self>,
        request: LocalRequest,
        reverse: bool,
    ) -> Result<LocalRequest, RpcError> {
        let (message, capabilities) = request.into_parts();
        let mut failure = None;
        let capabilities = capabilities.map_clients(|client| {
            let schema = Arc::clone(client.schema());
            match self.try_wrap(client, reverse) {
                Ok(client) => client,
                Err(error) => {
                    failure = Some(error);
                    LocalClient::disabled(schema)
                }
            }
        })?;
        if let Some(error) = failure {
            return Err(error);
        }
        Ok(LocalRequest::with_capabilities(message, capabilities))
    }

    fn map_response(
        self: &Arc<Self>,
        response: LocalResponse,
        reverse: bool,
    ) -> Result<LocalResponse, RpcError> {
        let mut failure = None;
        let capabilities = response.capabilities().map_clients(|client| {
            let schema = Arc::clone(client.schema());
            match self.try_wrap(client, reverse) {
                Ok(client) => client,
                Err(error) => {
                    failure = Some(error);
                    LocalClient::disabled(schema)
                }
            }
        })?;
        if let Some(error) = failure {
            return Err(error);
        }
        Ok(LocalResponse::with_capabilities(
            Arc::clone(response.message()),
            capabilities,
        ))
    }

    fn failure(&self) -> Option<CapabilityFailure> {
        lock(&self.revoked).clone()
    }

    fn revoke(&self, reason: String) -> Result<(), RpcError> {
        {
            let mut revoked = lock(&self.revoked);
            if revoked.is_some() {
                return Err(RpcError::MembraneAlreadyRevoked);
            }
            *revoked = Some(CapabilityFailure::Revoked(reason));
        }
        let waiters = {
            let mut waiters = lock(&self.waiters);
            let live = waiters.iter().filter_map(Weak::upgrade).collect::<Vec<_>>();
            waiters.clear();
            live
        };
        for waiter in waiters {
            if let Some(waker) = lock(&waiter).take() {
                waker.wake();
            }
        }
        Ok(())
    }

    fn watch(&self) -> Result<Arc<Mutex<Option<Waker>>>, RpcError> {
        let slot = Arc::new(Mutex::new(None));
        let mut waiters = lock(&self.waiters);
        waiters.retain(|waiter| waiter.strong_count() != 0);
        if waiters.len() >= self.limits.max_outstanding_calls {
            return Err(RpcError::MembraneLimit {
                resource: "outstanding calls",
                requested: waiters.len().saturating_add(1),
                limit: self.limits.max_outstanding_calls,
            });
        }
        waiters.push(Arc::downgrade(&slot));
        Ok(slot)
    }

    fn use_token(&self) -> Option<UseToken> {
        self.usage
            .as_ref()
            .map(|counter| UseToken::new(Arc::clone(counter)))
    }
}

struct MembraneResolution {
    state: Arc<MembraneState>,
    target: LocalClient,
    reverse: bool,
}

impl ClientResolution for MembraneResolution {
    fn resolve(&self) -> BoxFuture<Result<LocalClient, RpcError>> {
        let state = Arc::clone(&self.state);
        let target = self.target.clone();
        let reverse = self.reverse;
        Box::pin(async move {
            let resolved = target.when_resolved().await?;
            Ok(state.wrap(resolved, reverse))
        })
    }

    fn try_resolve(&self) -> Option<LocalClient> {
        self.target
            .try_resolved()
            .map(|resolved| self.state.wrap(resolved, self.reverse))
    }
}

struct MembraneService {
    state: Arc<MembraneState>,
    target: LocalClient,
    reverse: bool,
    _usage: Option<UseToken>,
}

impl MembraneService {
    fn start(&self, interface_id: u64, method_id: u16, request: LocalRequest) -> LocalCall {
        let revoked = self.state.failure();
        let policy_target = revoked.as_ref().map_or_else(
            || self.target.clone(),
            |failure| LocalClient::broken(Arc::clone(self.target.schema()), failure.to_string()),
        );
        let decision = if self.reverse {
            self.state
                .policy
                .outbound_call(interface_id, method_id, &policy_target)
        } else {
            self.state
                .policy
                .inbound_call(interface_id, method_id, &policy_target)
        };
        match decision {
            MembraneDecision::Reject(failure) => failed_call(failure),
            MembraneDecision::Redirect(client) => {
                if self.state.policy.should_resolve_before_redirecting()
                    && revoked.is_none()
                    && self.target.requires_resolution()
                {
                    let state = Arc::clone(&self.state);
                    let target = self.target.clone();
                    let reverse = self.reverse;
                    let resolved = Box::pin(async move {
                        let resolved = target.when_resolved().await?;
                        Ok(state.wrap(resolved, reverse))
                    });
                    return deferred_client_call(resolved, interface_id, method_id, request);
                }
                let shared = client.start_request(interface_id, method_id, request);
                local_call_from_shared(shared)
            }
            MembraneDecision::Forward => {
                if let Some(failure) = revoked {
                    return failed_call(failure);
                }
                let waiter = match self.state.watch() {
                    Ok(waiter) => waiter,
                    Err(error) => return LocalCall::new(Box::pin(async move { Err(error) })),
                };
                if let Some(failure) = self.state.failure() {
                    return failed_call(failure);
                }
                let request = match self.state.map_request(request, !self.reverse) {
                    Ok(request) => request,
                    Err(error) => return LocalCall::new(Box::pin(async move { Err(error) })),
                };
                let shared = self.target.start_request(interface_id, method_id, request);
                let response = MembraneResponseFuture {
                    inner: shared.await_response(),
                    state: Arc::clone(&self.state),
                    reverse: self.reverse,
                    waiter,
                    _usage: self.state.use_token(),
                };
                let pipeline_call = shared.clone();
                let pipeline_state = Arc::clone(&self.state);
                let reverse = self.reverse;
                let pipeline = PipelineBuilder::with_fallback(
                    DEFAULT_PIPELINE_LIMIT,
                    Arc::new(move |transform: &PipelineTransform| {
                        pipeline_call
                            .provisional(transform)
                            .map(|client| pipeline_state.wrap(client, reverse))
                    }),
                );
                LocalCall::from_parts(Box::pin(response), Some(pipeline))
            }
        }
    }
}

impl LocalService for MembraneService {
    fn dispatch(
        self: Arc<Self>,
        interface_id: u64,
        method_id: u16,
        params: Arc<OwnedMessage>,
    ) -> MessageFuture {
        let response = self
            .start(interface_id, method_id, LocalRequest::new(params))
            .into_response();
        Box::pin(async move {
            let response = response.await?;
            Ok(Arc::clone(response.message()))
        })
    }

    fn dispatch_request(
        self: Arc<Self>,
        interface_id: u64,
        method_id: u16,
        request: LocalRequest,
    ) -> LocalCall {
        self.start(interface_id, method_id, request)
    }
}

struct MembraneResponseFuture {
    inner: SharedCallFuture,
    state: Arc<MembraneState>,
    reverse: bool,
    waiter: Arc<Mutex<Option<Waker>>>,
    _usage: Option<UseToken>,
}

impl Future for MembraneResponseFuture {
    type Output = Result<LocalResponse, RpcError>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if let Some(failure) = this.state.failure() {
            return Poll::Ready(Err(RpcError::LocalCapability(failure)));
        }
        *lock(&this.waiter) = Some(context.waker().clone());
        if let Some(failure) = this.state.failure() {
            return Poll::Ready(Err(RpcError::LocalCapability(failure)));
        }
        match Pin::new(&mut this.inner).poll(context) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Ready(Ok(response)) => {
                *lock(&this.waiter) = None;
                Poll::Ready(this.state.map_response(response, this.reverse))
            }
        }
    }
}

fn local_call_from_shared(shared: crate::local::SharedCall) -> LocalCall {
    let response = {
        let shared = shared.clone();
        Box::pin(async move { shared.await_response().await }) as LocalResponseFuture
    };
    let pipeline_call = shared;
    let pipeline = PipelineBuilder::with_fallback(
        DEFAULT_PIPELINE_LIMIT,
        Arc::new(move |transform| pipeline_call.provisional(transform)),
    );
    LocalCall::from_parts(response, Some(pipeline))
}

fn failed_call(failure: CapabilityFailure) -> LocalCall {
    LocalCall::new(Box::pin(
        async move { Err(RpcError::LocalCapability(failure)) },
    ))
}

struct UseToken {
    counter: Arc<AtomicUsize>,
}

impl UseToken {
    fn new(counter: Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::Relaxed);
        Self { counter }
    }
}

impl Drop for UseToken {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::Release);
    }
}

/// A local server whose clients and outstanding calls can be revoked together.
pub struct RevocableServer<S: LocalService> {
    target: LocalClient,
    membrane: Membrane,
    usage: Arc<AtomicUsize>,
    marker: std::marker::PhantomData<fn() -> S>,
}

impl<S: LocalService> fmt::Debug for RevocableServer<S> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RevocableServer")
            .field("in_use", &self.is_in_use())
            .field("revoked", &self.membrane.is_revoked())
            .finish()
    }
}

impl<S: LocalService> RevocableServer<S> {
    pub fn new(schema: Arc<CompiledSchema>, server: Arc<S>) -> Self {
        let usage = Arc::new(AtomicUsize::new(0));
        Self {
            target: LocalClient::new(schema, server),
            membrane: Membrane::with_usage(Arc::new(PassthroughPolicy), Arc::clone(&usage)),
            usage,
            marker: std::marker::PhantomData,
        }
    }

    pub fn get_client(&self) -> LocalClient {
        self.membrane.wrap(self.target.clone())
    }

    pub fn revoke(&self) -> Result<(), RpcError> {
        self.membrane.revoke("")
    }

    pub fn is_in_use(&self) -> bool {
        self.usage.load(Ordering::Acquire) != 0
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
