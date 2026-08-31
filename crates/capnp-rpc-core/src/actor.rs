//! Single-owner two-party Level-1 connection actor.
//!
//! The actor alone mutates protocol tables. Thread-safe handles only append to
//! a bounded mailbox. Application work leaves the actor as a `Dispatch` effect
//! and returns through a generation-bearing completion token, so handlers may
//! run concurrently and finish out of order without sharing table state.
//! M36 adds actor-owned promise import/export resolution, permanently frozen
//! forwarding routes, and bounded loopback embargoes. A call already ordered
//! before `Resolve` is routed before that message is emitted; a loopback route
//! cannot dispatch locally until the matching receiver disembargo arrives.
//! M37 adds aggregate incoming-call byte accounting to the existing
//! answer-count bound. Streaming flow controllers live in `capnp-rpc`;
//! M38 adds question-lease cancellation, application cancellation opt-out,
//! legacy deferred `Finish` handling, and complete disconnect propagation.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use crate::OwnedResource;
use crate::capability::{
    CapabilityStats, CapabilityTables, HostedCapability, OutgoingCapability, PromiseCapability,
    ReceivedCapability,
};
use crate::level0::{
    CallTarget, CapDescriptor, DisembargoContext, DisembargoMessage, FinishMessage, HandlerResult,
    Payload, PipelineOp, PromiseResolution, PromisedAnswer, ResolveMessage, ReturnPayload,
    SendResultsTo, encode_bootstrap, encode_call_payload_with_options, encode_call_with_options,
    encode_disembargo, encode_finish_with_release, encode_release, encode_resolve, encode_return,
};
use crate::protocol::{
    ExceptionType, ProtocolLimits, ProtocolMessage, RpcException, encode_abort,
    encode_unimplemented, message_bytes, read_protocol_message_with_limits, read_protocol_struct,
};
use capnp_message::{OwnedMessage, OwnedPointerRef};
use capnp_schema::DynamicAnyPointer;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct QuestionKey {
    id: u32,
    generation: u64,
}

impl QuestionKey {
    pub const fn id(self) -> u32 {
        self.id
    }

    pub const fn generation(self) -> u64 {
        self.generation
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AnswerKey {
    id: u32,
    generation: u64,
}

impl AnswerKey {
    pub const fn id(self) -> u32 {
        self.id
    }

    pub const fn generation(self) -> u64 {
        self.generation
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ActorLimits {
    pub mailbox_capacity: usize,
    pub max_questions: usize,
    pub max_answers: usize,
    pub max_incoming_call_bytes: u64,
    pub max_imports: usize,
    pub max_exports: usize,
    pub max_embargoes: usize,
    pub max_embargoed_calls: usize,
}

impl Default for ActorLimits {
    fn default() -> Self {
        Self {
            mailbox_capacity: 256,
            max_questions: 4096,
            max_answers: 4096,
            max_incoming_call_bytes: 256 * 1024 * 1024,
            max_imports: 4096,
            max_exports: 4096,
            max_embargoes: 4096,
            max_embargoed_calls: 4096,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ConnectionStats {
    pub active_questions: usize,
    pub active_answers: usize,
    pub incoming_call_bytes: u64,
    pub allocated_questions: u64,
    pub reused_question_ids: u64,
    pub dispatched_handlers: u64,
    pub completed_handlers: u64,
    pub stale_handler_completions: u64,
    pub active_imports: usize,
    pub active_exports: usize,
    pub import_references: u64,
    pub export_references: u64,
    pub active_embargoes: usize,
    pub queued_embargo_calls: usize,
}

#[derive(Clone, Debug)]
pub enum ConnectionError {
    Overloaded { capacity: usize },
    QuestionLimit { limit: usize },
    AnswerLimit { limit: usize },
    IncomingCallByteLimit { requested: u64, limit: u64 },
    DuplicateAnswer(u32),
    UnknownQuestion(u32),
    StaleTarget(QuestionKey),
    StaleAnswer(AnswerKey),
    GenerationExhausted,
    Unimplemented,
    Canceled,
    Disconnected,
    RemoteAbort(RpcException),
    Protocol(String),
    Wire(String),
    Capability(String),
    Poisoned,
    PolledAfterCompletion,
}

impl fmt::Display for ConnectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for ConnectionError {}

#[derive(Clone)]
pub struct ConnectionHandle {
    mailbox: Arc<SharedMailbox>,
}

impl fmt::Debug for ConnectionHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConnectionHandle")
            .field("closed", &self.mailbox.closed.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

impl ConnectionHandle {
    /// Creates a connection-local promise and its one-shot resolver. The
    /// promise must be exported in a payload before the resolver is consumed.
    pub fn new_promise(&self) -> Result<(PromiseCapability, PromiseResolver), ConnectionError> {
        let capability = PromiseCapability::new()
            .map_err(|error| ConnectionError::Capability(error.to_string()))?;
        Ok((
            capability.clone(),
            PromiseResolver {
                handle: self.clone(),
                capability,
            },
        ))
    }

    pub fn bootstrap(&self) -> Result<QuestionFuture, ConnectionError> {
        let cell = Arc::new(QuestionCell::new());
        self.submit(ActorCommand::StartBootstrap {
            cell: Arc::clone(&cell),
        })?;
        Ok(QuestionFuture::new(self.clone(), cell))
    }

    pub fn call(
        &self,
        target: &QuestionTarget,
        interface_id: u64,
        method_id: u16,
        params: Arc<OwnedMessage>,
    ) -> Result<QuestionFuture, ConnectionError> {
        let cell = Arc::new(QuestionCell::new());
        self.submit(ActorCommand::StartCall {
            target: OutgoingCallTarget::Bootstrap(target.clone()),
            interface_id,
            method_id,
            params,
            capabilities: Vec::new(),
            cell: Arc::clone(&cell),
        })?;
        Ok(QuestionFuture::new(self.clone(), cell))
    }

    pub fn call_imported(
        &self,
        import_id: u32,
        interface_id: u64,
        method_id: u16,
        params: Arc<OwnedMessage>,
        capabilities: Vec<OutgoingCapability>,
    ) -> Result<QuestionFuture, ConnectionError> {
        let cell = Arc::new(QuestionCell::new());
        self.submit(ActorCommand::StartCall {
            target: OutgoingCallTarget::Imported(import_id),
            interface_id,
            method_id,
            params,
            capabilities,
            cell: Arc::clone(&cell),
        })?;
        Ok(QuestionFuture::new(self.clone(), cell))
    }

    pub fn receive(&self, message: Arc<OwnedMessage>) -> Result<(), ConnectionError> {
        self.receive_with_resources(message, Vec::new())
    }

    pub fn receive_with_resources(
        &self,
        message: Arc<OwnedMessage>,
        resources: Vec<OwnedResource>,
    ) -> Result<(), ConnectionError> {
        self.submit(ActorCommand::Incoming { message, resources })
    }

    pub fn shutdown(&self) -> Result<(), ConnectionError> {
        self.mailbox.submit_shutdown()
    }

    fn submit(&self, command: ActorCommand) -> Result<(), ConnectionError> {
        self.mailbox.submit(command)
    }
}

/// A one-shot authority to settle a promise exported by this connection.
pub struct PromiseResolver {
    handle: ConnectionHandle,
    capability: PromiseCapability,
}

impl fmt::Debug for PromiseResolver {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PromiseResolver")
            .field("identity", &self.capability.identity())
            .finish_non_exhaustive()
    }
}

impl PromiseResolver {
    pub fn resolve_to_hosted(self, capability: HostedCapability) -> Result<(), ConnectionError> {
        self.resolve(LocalPromiseResolution::Hosted(capability))
    }

    pub fn resolve_to_import(self, import_id: u32) -> Result<(), ConnectionError> {
        self.resolve(LocalPromiseResolution::Imported(import_id))
    }

    pub fn reject(self, exception: RpcException) -> Result<(), ConnectionError> {
        self.resolve(LocalPromiseResolution::Exception(exception))
    }

    fn resolve(self, resolution: LocalPromiseResolution) -> Result<(), ConnectionError> {
        self.handle.submit(ActorCommand::ResolvePromise {
            identity: self.capability.identity(),
            resolution,
        })
    }
}

#[derive(Clone)]
pub struct QuestionTarget {
    lease: Arc<QuestionLease>,
    transform: Vec<PipelineOp>,
}

impl fmt::Debug for QuestionTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("QuestionTarget")
            .field("key", &self.key())
            .field("transform", &self.transform)
            .finish()
    }
}

impl QuestionTarget {
    pub fn key(&self) -> Option<QuestionKey> {
        self.lease.cell.active_key().ok().flatten()
    }

    pub fn pointer_field(&self, index: u16) -> Self {
        let mut transform = self.transform.clone();
        transform.push(PipelineOp::GetPointerField(index));
        Self {
            lease: Arc::clone(&self.lease),
            transform,
        }
    }

    pub fn noop(&self) -> Self {
        let mut transform = self.transform.clone();
        transform.push(PipelineOp::Noop);
        Self {
            lease: Arc::clone(&self.lease),
            transform,
        }
    }

    pub fn as_outgoing_capability(&self) -> Result<OutgoingCapability, ConnectionError> {
        let key = self
            .lease
            .cell
            .active_key()?
            .ok_or(ConnectionError::Disconnected)?;
        Ok(OutgoingCapability::ReceiverAnswer(PromisedAnswer {
            question_id: key.id,
            transform: self.transform.clone(),
        }))
    }
}

pub struct QuestionFuture {
    lease: Arc<QuestionLease>,
}

impl fmt::Debug for QuestionFuture {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("QuestionFuture")
            .finish_non_exhaustive()
    }
}

impl QuestionFuture {
    fn new(handle: ConnectionHandle, cell: Arc<QuestionCell>) -> Self {
        Self {
            lease: Arc::new(QuestionLease {
                handle,
                cell,
                cancel_sent: AtomicBool::new(false),
            }),
        }
    }

    pub fn target(&self) -> QuestionTarget {
        QuestionTarget {
            lease: Arc::clone(&self.lease),
            transform: Vec::new(),
        }
    }

    /// Requests cancellation and consumes the response future. The question ID
    /// remains reserved until the peer's `Return` arrives or the connection
    /// disconnects.
    pub fn cancel(self) -> Result<(), ConnectionError> {
        self.lease.request_cancel()
    }
}

impl Future for QuestionFuture {
    type Output = Result<ReturnPayload, ConnectionError>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        self.lease.cell.poll(context)
    }
}

struct QuestionLease {
    handle: ConnectionHandle,
    cell: Arc<QuestionCell>,
    cancel_sent: AtomicBool,
}

impl QuestionLease {
    fn request_cancel(&self) -> Result<(), ConnectionError> {
        if self.cancel_sent.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        self.handle
            .mailbox
            .submit_lifecycle(ActorCommand::CancelQuestion {
                cell: Arc::clone(&self.cell),
            })
    }
}

impl Drop for QuestionLease {
    fn drop(&mut self) {
        let _ = self.request_cancel();
    }
}

#[derive(Clone, Debug)]
pub enum IncomingRequest {
    Bootstrap,
    Call {
        target: IncomingCallTarget,
        interface_id: u64,
        method_id: u16,
        params: Payload,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IncomingCallTarget {
    BootstrapAnswer(AnswerKey),
    Hosted(HostedCapability),
}

pub struct CompletionToken {
    handle: ConnectionHandle,
    answer: AnswerKey,
    cancellation: CancellationSignal,
}

/// Cooperative cancellation state for one dispatched incoming call.
#[derive(Clone)]
pub struct CancellationSignal {
    state: Arc<AtomicU8>,
}

impl fmt::Debug for CancellationSignal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CancellationSignal")
            .field("canceled", &self.is_canceled())
            .field("allowed", &self.is_allowed())
            .finish()
    }
}

impl CancellationSignal {
    const ALLOWED: u8 = 0;
    const DISALLOWED: u8 = 1;
    const CANCELED: u8 = 2;

    fn new() -> Self {
        Self {
            state: Arc::new(AtomicU8::new(Self::ALLOWED)),
        }
    }

    pub fn is_canceled(&self) -> bool {
        self.state.load(Ordering::Acquire) == Self::CANCELED
    }

    pub fn is_allowed(&self) -> bool {
        self.state.load(Ordering::Acquire) == Self::ALLOWED
    }

    /// Opts out before cancellation wins the race. Returns false if the call
    /// was already canceled.
    pub fn disallow(&self) -> bool {
        self.state
            .compare_exchange(
                Self::ALLOWED,
                Self::DISALLOWED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
            || self.state.load(Ordering::Acquire) == Self::DISALLOWED
    }

    fn cancel_if_allowed(&self) -> bool {
        self.state
            .compare_exchange(
                Self::ALLOWED,
                Self::CANCELED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    fn force_cancel(&self) {
        self.state.store(Self::CANCELED, Ordering::Release);
    }
}

/// Completion authority for a call shortened back to a local capability.
pub struct LocalCompletionToken {
    handle: ConnectionHandle,
    question: QuestionKey,
}

impl fmt::Debug for LocalCompletionToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalCompletionToken")
            .field("question", &self.question)
            .finish_non_exhaustive()
    }
}

impl LocalCompletionToken {
    pub fn complete(self, result: HandlerResult) -> Result<(), ConnectionError> {
        self.handle.submit(ActorCommand::LocalCallComplete {
            question: self.question,
            result,
            capabilities: Vec::new(),
        })
    }

    pub fn complete_with_capabilities(
        self,
        content: Arc<OwnedMessage>,
        capabilities: Vec<OutgoingCapability>,
    ) -> Result<(), ConnectionError> {
        self.handle.submit(ActorCommand::LocalCallComplete {
            question: self.question,
            result: HandlerResult::Results(content),
            capabilities,
        })
    }
}

impl fmt::Debug for CompletionToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CompletionToken")
            .field("answer", &self.answer)
            .finish_non_exhaustive()
    }
}

impl CompletionToken {
    pub const fn answer_key(&self) -> AnswerKey {
        self.answer
    }

    pub fn cancellation(&self) -> CancellationSignal {
        self.cancellation.clone()
    }

    pub fn disallow_cancellation(&self) -> bool {
        self.cancellation.disallow()
    }

    pub fn complete(self, result: HandlerResult) -> Result<(), ConnectionError> {
        self.handle.submit(ActorCommand::HandlerComplete {
            answer: self.answer,
            result,
            capabilities: Vec::new(),
        })
    }

    pub fn complete_with_capabilities(
        self,
        content: Arc<OwnedMessage>,
        capabilities: Vec<OutgoingCapability>,
    ) -> Result<(), ConnectionError> {
        self.handle.submit(ActorCommand::HandlerComplete {
            answer: self.answer,
            result: HandlerResult::Results(content),
            capabilities,
        })
    }

    /// Performs a Level-1 tail call back to a capability imported from this
    /// peer. The actor emits the redirected call before the routing `Return`.
    pub fn tail_call_imported(
        self,
        import_id: u32,
        interface_id: u64,
        method_id: u16,
        params: Arc<OwnedMessage>,
        capabilities: Vec<OutgoingCapability>,
    ) -> Result<(), ConnectionError> {
        self.handle.submit(ActorCommand::StartTailCall {
            answer: self.answer,
            import_id,
            interface_id,
            method_id,
            params,
            capabilities,
        })
    }
}

#[derive(Debug)]
pub enum ActorEffect {
    Send(Arc<OwnedMessage>),
    SendWithResources {
        message: Arc<OwnedMessage>,
        resources: Vec<OwnedResource>,
    },
    Dispatch {
        request: IncomingRequest,
        completion: CompletionToken,
    },
    DispatchLocal {
        request: IncomingRequest,
        completion: LocalCompletionToken,
    },
    CloseTransport,
}

#[derive(Clone, Debug)]
enum LocalPromiseResolution {
    Hosted(HostedCapability),
    Imported(u32),
    Exception(RpcException),
}

#[derive(Clone, Debug)]
enum FrozenPromiseRoute {
    Hosted(HostedCapability),
    Imported(u32),
    Exception(RpcException),
}

#[derive(Clone, Debug)]
enum ImportPromiseState {
    Unresolved,
    Remote(u32),
    PromisedAnswer(PromisedAnswer),
    Loopback {
        capability: HostedCapability,
        embargo_id: u32,
    },
    Local(HostedCapability),
    Broken(RpcException),
}

#[derive(Clone, Debug)]
struct PendingPromiseCall {
    answer: AnswerKey,
    interface_id: u64,
    method_id: u16,
    params: Payload,
}

enum TailCallParams {
    Owned(Arc<OwnedMessage>),
    Dynamic(Payload),
}

#[derive(Clone, Debug)]
struct ExportPromiseState {
    identity: u64,
    route: Option<FrozenPromiseRoute>,
    queued_calls: VecDeque<PendingPromiseCall>,
}

#[derive(Clone)]
struct PendingLocalCall {
    interface_id: u64,
    method_id: u16,
    params: Arc<OwnedMessage>,
    capabilities: Vec<OutgoingCapability>,
    cell: Arc<QuestionCell>,
}

#[derive(Clone)]
struct EmbargoState {
    promise_id: u32,
    capability: HostedCapability,
    queued_calls: VecDeque<PendingLocalCall>,
}

/// The only owner of a connection's ordered protocol state.
pub struct ConnectionActor {
    mailbox: Arc<SharedMailbox>,
    handle: ConnectionHandle,
    protocol_limits: ProtocolLimits,
    questions: QuestionTable,
    answers: AnswerTable,
    capabilities: CapabilityTables,
    promise_imports: BTreeMap<u32, ImportPromiseState>,
    promise_exports: BTreeMap<u32, ExportPromiseState>,
    embargoes: BTreeMap<u32, EmbargoState>,
    max_embargoes: usize,
    max_embargoed_calls: usize,
    embargoed_calls: usize,
    effects: VecDeque<ActorEffect>,
    deferred_finishes: VecDeque<FinishMessage>,
    yield_deferred_finish: bool,
    stats: ConnectionStats,
    terminal: bool,
}

impl fmt::Debug for ConnectionActor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConnectionActor")
            .field("stats", &self.stats())
            .field("terminal", &self.terminal)
            .finish_non_exhaustive()
    }
}

impl ConnectionActor {
    pub fn new(limits: ActorLimits, protocol_limits: ProtocolLimits) -> (ConnectionHandle, Self) {
        let mailbox = Arc::new(SharedMailbox::new(limits.mailbox_capacity));
        let handle = ConnectionHandle {
            mailbox: Arc::clone(&mailbox),
        };
        (
            handle.clone(),
            Self {
                mailbox,
                handle,
                protocol_limits,
                questions: QuestionTable::new(limits.max_questions),
                answers: AnswerTable::new(limits.max_answers, limits.max_incoming_call_bytes),
                capabilities: CapabilityTables::new(limits.max_imports, limits.max_exports),
                promise_imports: BTreeMap::new(),
                promise_exports: BTreeMap::new(),
                embargoes: BTreeMap::new(),
                max_embargoes: limits.max_embargoes,
                max_embargoed_calls: limits.max_embargoed_calls,
                embargoed_calls: 0,
                effects: VecDeque::new(),
                deferred_finishes: VecDeque::new(),
                yield_deferred_finish: false,
                stats: ConnectionStats::default(),
                terminal: false,
            },
        )
    }

    pub fn stats(&self) -> ConnectionStats {
        let CapabilityStats {
            active_imports,
            active_exports,
            import_references,
            export_references,
        } = self.capabilities.stats();
        ConnectionStats {
            active_questions: self.questions.len(),
            active_answers: self.answers.len(),
            incoming_call_bytes: self.answers.incoming_bytes(),
            active_imports,
            active_exports,
            import_references,
            export_references,
            active_embargoes: self.embargoes.len(),
            queued_embargo_calls: self.embargoed_calls,
            ..self.stats
        }
    }

    /// Releases locally-held import references and schedules a batched wire
    /// `Release` message without involving application locks.
    pub fn release_import(&mut self, id: u32, count: u32) -> Result<(), ConnectionError> {
        self.release_import_inner(id, count, &mut BTreeSet::new())
    }

    fn release_import_inner(
        &mut self,
        id: u32,
        count: u32,
        releasing: &mut BTreeSet<u32>,
    ) -> Result<(), ConnectionError> {
        if !releasing.insert(id) {
            return Ok(());
        }
        let snapshot = self.capabilities.clone();
        let release = self
            .capabilities
            .release_import(id, count)
            .map_err(|error| ConnectionError::Capability(error.to_string()))?;
        let message =
            match encode_release(release.id, release.reference_count, self.protocol_limits) {
                Ok(message) => message,
                Err(error) => {
                    self.capabilities = snapshot;
                    return Err(ConnectionError::Wire(error.to_string()));
                }
            };
        self.effects.push_back(ActorEffect::Send(message));
        if !self.capabilities.contains_import(id) {
            if let Some(ImportPromiseState::Remote(target)) = self.promise_imports.remove(&id) {
                if self.capabilities.contains_import(target) {
                    self.release_import_inner(target, 1, releasing)?;
                }
            }
        }
        Ok(())
    }

    /// Processes ordered commands until one externally-visible effect is ready.
    pub fn poll_next_effect(&mut self, context: &mut Context<'_>) -> Poll<Option<ActorEffect>> {
        loop {
            if let Some(effect) = self.effects.pop_front() {
                return Poll::Ready(Some(effect));
            }
            if self.yield_deferred_finish {
                self.yield_deferred_finish = false;
                context.waker().wake_by_ref();
                return Poll::Pending;
            }
            if let Some(finish) = self.deferred_finishes.pop_front() {
                self.finish_incoming(finish);
                continue;
            }
            let command = match self.mailbox.pop_or_register(context.waker()) {
                Ok(Some(command)) => command,
                Ok(None) if self.terminal => return Poll::Ready(None),
                Ok(None) => return Poll::Pending,
                Err(error) => {
                    self.transition_terminal(error, false);
                    continue;
                }
            };
            self.process(command);
        }
    }

    fn process(&mut self, command: ActorCommand) {
        if self.terminal {
            complete_rejected(command, ConnectionError::Disconnected);
            return;
        }
        match command {
            ActorCommand::StartBootstrap { cell } => self.start_bootstrap(cell),
            ActorCommand::StartCall {
                target,
                interface_id,
                method_id,
                params,
                capabilities,
                cell,
            } => self.start_call(target, interface_id, method_id, params, capabilities, cell),
            ActorCommand::Incoming { message, resources } => self.incoming(message, resources),
            ActorCommand::StartTailCall {
                answer,
                import_id,
                interface_id,
                method_id,
                params,
                capabilities,
            } => self.start_tail_call(
                answer,
                import_id,
                interface_id,
                method_id,
                params,
                capabilities,
            ),
            ActorCommand::HandlerComplete {
                answer,
                result,
                capabilities,
            } => self.handler_complete(answer, result, capabilities),
            ActorCommand::ResolvePromise {
                identity,
                resolution,
            } => self.resolve_promise(identity, resolution),
            ActorCommand::LocalCallComplete {
                question,
                result,
                capabilities,
            } => self.local_call_complete(question, result, capabilities),
            ActorCommand::CancelQuestion { cell } => self.cancel_question(&cell),
            ActorCommand::Shutdown => {
                self.transition_terminal(ConnectionError::Disconnected, false)
            }
        }
    }

    fn start_bootstrap(&mut self, cell: Arc<QuestionCell>) {
        let key = match self.questions.allocate(QuestionState {
            cell: Arc::clone(&cell),
            param_exports: Vec::new(),
            is_tail_call: false,
            sent_to_peer: true,
            canceled: false,
        }) {
            Ok(key) => key,
            Err(error) => {
                cell.complete(Err(error));
                return;
            }
        };
        self.record_question_allocation(key);
        if let Err(error) = cell.assign(key) {
            if let Some(question) = self.questions.remove(key) {
                let _ = self
                    .capabilities
                    .apply_implicit_releases(&question.param_exports);
            }
            self.protocol_failure(error);
            return;
        }
        match encode_bootstrap(key.id, self.protocol_limits) {
            Ok(message) => self.effects.push_back(ActorEffect::Send(message)),
            Err(error) => {
                if let Some(question) = self.questions.remove(key) {
                    question
                        .cell
                        .complete(Err(ConnectionError::Wire(error.to_string())));
                }
            }
        }
    }

    fn start_call(
        &mut self,
        target: OutgoingCallTarget,
        interface_id: u64,
        method_id: u16,
        params: Arc<OwnedMessage>,
        capabilities: Vec<OutgoingCapability>,
        cell: Arc<QuestionCell>,
    ) {
        if let OutgoingCallTarget::Imported(id) = &target {
            match self.promise_imports.get(id).cloned() {
                Some(ImportPromiseState::Loopback {
                    capability,
                    embargo_id,
                }) => {
                    if self.embargoed_calls >= self.max_embargoed_calls {
                        cell.complete(Err(ConnectionError::Overloaded {
                            capacity: self.max_embargoed_calls,
                        }));
                        return;
                    }
                    let Some(embargo) = self.embargoes.get_mut(&embargo_id) else {
                        cell.complete(Err(ConnectionError::Protocol(format!(
                            "promise import {id} references missing embargo {embargo_id}"
                        ))));
                        return;
                    };
                    debug_assert_eq!(embargo.capability, capability);
                    embargo.queued_calls.push_back(PendingLocalCall {
                        interface_id,
                        method_id,
                        params,
                        capabilities,
                        cell,
                    });
                    self.embargoed_calls = match self.embargoed_calls.checked_add(1) {
                        Some(count) => count,
                        None => {
                            self.protocol_failure(ConnectionError::GenerationExhausted);
                            return;
                        }
                    };
                    return;
                }
                Some(ImportPromiseState::Local(capability)) if capabilities.is_empty() => {
                    self.start_local_call(
                        capability,
                        interface_id,
                        method_id,
                        params,
                        capabilities,
                        cell,
                    );
                    return;
                }
                Some(ImportPromiseState::Broken(exception)) => {
                    cell.complete(Ok(ReturnPayload::Exception(exception)));
                    return;
                }
                Some(
                    ImportPromiseState::Unresolved
                    | ImportPromiseState::Remote(_)
                    | ImportPromiseState::PromisedAnswer(_)
                    | ImportPromiseState::Local(_),
                )
                | None => {}
            }
        }
        let wire_target = match target {
            OutgoingCallTarget::Bootstrap(target) => match target.lease.cell.active_key() {
                Ok(Some(key)) if self.questions.contains(key) => {
                    if target.transform.is_empty() {
                        CallTarget::BootstrapAnswer(key.id)
                    } else {
                        CallTarget::PromisedAnswer(PromisedAnswer {
                            question_id: key.id,
                            transform: target.transform,
                        })
                    }
                }
                Ok(Some(key)) => {
                    cell.complete(Err(ConnectionError::StaleTarget(key)));
                    return;
                }
                Ok(None) => {
                    cell.complete(Err(ConnectionError::Disconnected));
                    return;
                }
                Err(error) => {
                    cell.complete(Err(error));
                    return;
                }
            },
            OutgoingCallTarget::Imported(id) if self.capabilities.contains_import(id) => {
                let routed_id = match self.promise_imports.get(&id) {
                    Some(ImportPromiseState::Remote(routed_id)) => *routed_id,
                    _ => id,
                };
                match self.promise_imports.get(&id) {
                    Some(ImportPromiseState::PromisedAnswer(target)) => {
                        CallTarget::PromisedAnswer(target.clone())
                    }
                    _ => CallTarget::ImportedCap(routed_id),
                }
            }
            OutgoingCallTarget::Imported(id) => {
                cell.complete(Err(ConnectionError::Capability(format!(
                    "unknown import {id}"
                ))));
                return;
            }
        };
        let (descriptors, resources) = match self.describe_outgoing_capabilities(&capabilities) {
            Ok(described) => described,
            Err(error) => {
                cell.complete(Err(ConnectionError::Capability(error.to_string())));
                return;
            }
        };
        let param_exports = sender_hosted_ids(&descriptors);
        let key = match self.questions.allocate(QuestionState {
            cell: Arc::clone(&cell),
            param_exports: param_exports.clone(),
            is_tail_call: false,
            sent_to_peer: true,
            canceled: false,
        }) {
            Ok(key) => key,
            Err(error) => {
                let _ = self.capabilities.apply_implicit_releases(&param_exports);
                cell.complete(Err(error));
                return;
            }
        };
        self.record_question_allocation(key);
        if let Err(error) = cell.assign(key) {
            if let Some(question) = self.questions.remove(key) {
                let _ = self
                    .capabilities
                    .apply_implicit_releases(&question.param_exports);
            }
            self.protocol_failure(error);
            return;
        }
        match crate::encode_call_with_capabilities(
            key.id,
            wire_target,
            interface_id,
            method_id,
            &params,
            &descriptors,
            self.protocol_limits,
        ) {
            Ok(message) => self.send_with_resources(message, resources),
            Err(error) => {
                if let Some(question) = self.questions.remove(key) {
                    let _ = self
                        .capabilities
                        .apply_implicit_releases(&question.param_exports);
                    question
                        .cell
                        .complete(Err(ConnectionError::Wire(error.to_string())));
                }
            }
        }
    }

    fn start_local_call(
        &mut self,
        capability: HostedCapability,
        interface_id: u64,
        method_id: u16,
        params: Arc<OwnedMessage>,
        capabilities: Vec<OutgoingCapability>,
        cell: Arc<QuestionCell>,
    ) {
        debug_assert!(capabilities.is_empty());
        let key = match self.questions.allocate(QuestionState {
            cell: Arc::clone(&cell),
            param_exports: Vec::new(),
            is_tail_call: false,
            sent_to_peer: false,
            canceled: false,
        }) {
            Ok(key) => key,
            Err(error) => {
                cell.complete(Err(error));
                return;
            }
        };
        self.record_question_allocation(key);
        if let Err(error) = cell.assign(key) {
            let _ = self.questions.remove(key);
            self.protocol_failure(error);
            return;
        }
        let content = match params.root_pointer() {
            Ok(OwnedPointerRef::Null) => DynamicAnyPointer::Null,
            Ok(OwnedPointerRef::Struct(value)) => DynamicAnyPointer::Struct(value),
            Ok(OwnedPointerRef::List(value)) => DynamicAnyPointer::List(value),
            Ok(OwnedPointerRef::Capability(value)) => DynamicAnyPointer::Capability(value),
            Err(error) => {
                let _ = self.questions.remove(key);
                cell.complete(Err(ConnectionError::Protocol(error.to_string())));
                return;
            }
        };
        self.stats.dispatched_handlers = self.stats.dispatched_handlers.saturating_add(1);
        self.effects.push_back(ActorEffect::DispatchLocal {
            request: IncomingRequest::Call {
                target: IncomingCallTarget::Hosted(capability),
                interface_id,
                method_id,
                params: Payload {
                    content,
                    cap_table: Vec::new(),
                },
            },
            completion: LocalCompletionToken {
                handle: self.handle.clone(),
                question: key,
            },
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn start_tail_call(
        &mut self,
        answer: AnswerKey,
        import_id: u32,
        interface_id: u64,
        method_id: u16,
        params: Arc<OwnedMessage>,
        capabilities: Vec<OutgoingCapability>,
    ) {
        self.start_tail_call_common(
            answer,
            import_id,
            interface_id,
            method_id,
            TailCallParams::Owned(params),
            capabilities,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn start_tail_call_common(
        &mut self,
        answer: AnswerKey,
        import_id: u32,
        interface_id: u64,
        method_id: u16,
        params: TailCallParams,
        capabilities: Vec<OutgoingCapability>,
    ) {
        if !self.answers.is_in_progress(answer) {
            self.stats.stale_handler_completions =
                self.stats.stale_handler_completions.saturating_add(1);
            return;
        }
        if !self.capabilities.contains_import(import_id) {
            self.fail_pipeline_call(answer, format!("unknown tail-call import {import_id}"));
            return;
        }
        let (descriptors, resources) = match self.describe_outgoing_capabilities(&capabilities) {
            Ok(described) => described,
            Err(error) => {
                self.fail_pipeline_call(answer, error.to_string());
                return;
            }
        };
        let param_exports = sender_hosted_ids(&descriptors);
        let cell = Arc::new(QuestionCell::new());
        let key = match self.questions.allocate(QuestionState {
            cell: Arc::clone(&cell),
            param_exports: param_exports.clone(),
            is_tail_call: true,
            sent_to_peer: true,
            canceled: false,
        }) {
            Ok(key) => key,
            Err(error) => {
                let _ = self.capabilities.apply_implicit_releases(&param_exports);
                self.fail_pipeline_call(answer, error.to_string());
                return;
            }
        };
        self.record_question_allocation(key);
        if let Err(error) = cell.assign(key) {
            let _ = self.questions.remove(key);
            let _ = self.capabilities.apply_implicit_releases(&param_exports);
            self.protocol_failure(error);
            return;
        }
        let call = match &params {
            TailCallParams::Owned(params) => encode_call_with_options(
                key.id,
                CallTarget::ImportedCap(import_id),
                interface_id,
                method_id,
                params,
                &descriptors,
                SendResultsTo::Yourself,
                self.protocol_limits,
            ),
            TailCallParams::Dynamic(params) => encode_call_payload_with_options(
                key.id,
                CallTarget::ImportedCap(import_id),
                interface_id,
                method_id,
                params,
                &descriptors,
                SendResultsTo::Yourself,
                self.protocol_limits,
            ),
        };
        let returned = encode_return(
            answer.id,
            &HandlerResult::TakeFromOtherQuestion(key.id),
            self.protocol_limits,
        );
        let (call, returned) = match (call, returned) {
            (Ok(call), Ok(returned)) => (call, returned),
            (call, returned) => {
                let _ = self.questions.remove(key);
                let _ = self.capabilities.apply_implicit_releases(&param_exports);
                let reason = call.err().or_else(|| returned.err()).map_or_else(
                    || "tail call encoding failed".to_owned(),
                    |error| error.to_string(),
                );
                self.protocol_failure(ConnectionError::Wire(reason));
                return;
            }
        };
        if !self.answers.mark_returned(answer) {
            let _ = self.questions.remove(key);
            let _ = self.capabilities.apply_implicit_releases(&param_exports);
            self.stats.stale_handler_completions =
                self.stats.stale_handler_completions.saturating_add(1);
            return;
        }
        if !self.answers.record_tail_question(answer, key) {
            let _ = self.questions.remove(key);
            let _ = self.capabilities.apply_implicit_releases(&param_exports);
            self.protocol_failure(ConnectionError::StaleAnswer(answer));
            return;
        }
        let original_param_imports = self.answers.param_imports(answer).unwrap_or_default();
        if let Err(error) = self.apply_implicit_import_releases(&original_param_imports) {
            self.protocol_failure(error);
            return;
        }
        self.send_with_resources(call, resources);
        self.effects.push_back(ActorEffect::Send(returned));
    }

    fn incoming(&mut self, raw: Arc<OwnedMessage>, resources: Vec<OwnedResource>) {
        let raw_bytes = match message_bytes(&raw)
            .map_err(|error| ConnectionError::Protocol(error.to_string()))
            .and_then(|bytes| {
                u64::try_from(bytes).map_err(|_| {
                    ConnectionError::Protocol("incoming message size does not fit u64".to_owned())
                })
            }) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.protocol_failure(error);
                return;
            }
        };
        let mut message =
            match read_protocol_message_with_limits(Arc::clone(&raw), self.protocol_limits) {
                Ok(message) => message,
                Err(error) => {
                    self.protocol_failure(ConnectionError::Protocol(error.to_string()));
                    return;
                }
            };
        message.bind_resources(resources);
        match message {
            ProtocolMessage::Bootstrap(message) => {
                let answer = match self.answers.insert(
                    message.question_id,
                    AnswerKind::Bootstrap,
                    Vec::new(),
                    false,
                    0,
                ) {
                    Ok(answer) => answer,
                    Err(error) => {
                        self.protocol_failure(error);
                        return;
                    }
                };
                self.dispatch(IncomingRequest::Bootstrap, answer);
            }
            ProtocolMessage::Call(message) => {
                if let Err(error) = self.answers.check_insert(message.question_id, raw_bytes) {
                    self.protocol_failure(error);
                    return;
                }
                let (target, promised_answer, promise_export) = match message.target {
                    CallTarget::BootstrapAnswer(target_id) => {
                        let Some(target) = self.answers.key_for_id(target_id) else {
                            self.protocol_failure(ConnectionError::Protocol(format!(
                                "call targets unknown bootstrap answer {target_id}"
                            )));
                            return;
                        };
                        if self.answers.is_bootstrap(target) {
                            (
                                Some(IncomingCallTarget::BootstrapAnswer(target)),
                                None,
                                None,
                            )
                        } else {
                            (
                                None,
                                Some(PromisedAnswer {
                                    question_id: target_id,
                                    transform: Vec::new(),
                                }),
                                None,
                            )
                        }
                    }
                    CallTarget::ImportedCap(export_id) => {
                        match self
                            .capabilities
                            .receive(&CapDescriptor::ReceiverHosted(export_id))
                        {
                            Ok(ReceivedCapability::Hosted(capability)) => {
                                (Some(IncomingCallTarget::Hosted(capability)), None, None)
                            }
                            Ok(ReceivedCapability::ExportedPromise(_)) => {
                                (None, None, Some(export_id))
                            }
                            Ok(_) => {
                                self.protocol_failure(ConnectionError::Protocol(
                                    "importedCap did not resolve to a hosted capability".to_owned(),
                                ));
                                return;
                            }
                            Err(error) => {
                                self.protocol_failure(ConnectionError::Capability(
                                    error.to_string(),
                                ));
                                return;
                            }
                        }
                    }
                    CallTarget::PromisedAnswer(promised_answer) => {
                        (None, Some(promised_answer), None)
                    }
                };
                let param_imports = match self.receive_cap_table(&message.params.cap_table) {
                    Ok(imports) => imports,
                    Err(error) => {
                        self.protocol_failure(error);
                        return;
                    }
                };
                let answer = match self.answers.insert(
                    message.question_id,
                    AnswerKind::Call,
                    param_imports,
                    message.send_results_to == SendResultsTo::Yourself,
                    raw_bytes,
                ) {
                    Ok(answer) => answer,
                    Err(error) => {
                        self.protocol_failure(error);
                        return;
                    }
                };
                if let Some(target) = target {
                    self.dispatch(
                        IncomingRequest::Call {
                            target,
                            interface_id: message.interface_id,
                            method_id: message.method_id,
                            params: message.params,
                        },
                        answer,
                    );
                } else if let Some(promised_answer) = promised_answer {
                    let pending = PendingPipelineCall {
                        answer,
                        transform: promised_answer.transform,
                        interface_id: message.interface_id,
                        method_id: message.method_id,
                        params: message.params,
                    };
                    match self
                        .answers
                        .queue_pipeline_call(promised_answer.question_id, pending.clone())
                    {
                        Ok(Some(pipeline)) => self.route_pipeline_call(pending, &pipeline),
                        Ok(None) => {}
                        Err(error) => self.fail_pipeline_call(answer, error.to_string()),
                    }
                } else if let Some(export_id) = promise_export {
                    self.route_exported_promise_call(
                        export_id,
                        PendingPromiseCall {
                            answer,
                            interface_id: message.interface_id,
                            method_id: message.method_id,
                            params: message.params,
                        },
                    );
                }
            }
            ProtocolMessage::Return(message) => {
                if self.questions.is_canceled_id(message.answer_id) {
                    let Some((_key, question)) = self.questions.remove_id(message.answer_id) else {
                        self.protocol_failure(ConnectionError::UnknownQuestion(message.answer_id));
                        return;
                    };
                    if message.release_param_caps {
                        if let Err(error) = self
                            .capabilities
                            .apply_implicit_releases(&question.param_exports)
                        {
                            self.protocol_failure(ConnectionError::Capability(error.to_string()));
                            return;
                        }
                    }
                    if let ReturnPayload::TakeFromOtherQuestion(answer_id) = message.payload {
                        if let Some(answer) = self.answers.finish(answer_id, true) {
                            self.finish_answer(answer);
                        }
                    }
                    return;
                }
                let has_result_caps = if let ReturnPayload::Results(payload) = &message.payload {
                    if let Err(error) = self.receive_cap_table(&payload.cap_table) {
                        self.protocol_failure(error);
                        return;
                    }
                    !payload.cap_table.is_empty()
                } else {
                    false
                };
                if message.no_finish_needed && has_result_caps {
                    self.protocol_failure(ConnectionError::Protocol(
                        "return.noFinishNeeded cannot retain result capabilities".to_owned(),
                    ));
                    return;
                }
                let tail_routing =
                    matches!(message.payload, ReturnPayload::TakeFromOtherQuestion(_));
                let tail_results_elsewhere =
                    matches!(message.payload, ReturnPayload::ResultsSentElsewhere)
                        && self.questions.is_tail_call_id(message.answer_id);
                let removed = if tail_routing || tail_results_elsewhere {
                    self.questions.reserve_id(message.answer_id)
                } else {
                    self.questions.remove_id(message.answer_id)
                };
                let Some((key, question)) = removed else {
                    self.protocol_failure(ConnectionError::UnknownQuestion(message.answer_id));
                    return;
                };
                if message.release_param_caps {
                    if let Err(error) = self
                        .capabilities
                        .apply_implicit_releases(&question.param_exports)
                    {
                        self.protocol_failure(ConnectionError::Capability(error.to_string()));
                        return;
                    }
                }
                if let ReturnPayload::TakeFromOtherQuestion(answer_id) = &message.payload {
                    let answer_id = *answer_id;
                    if question.is_tail_call {
                        question.cell.complete(Err(ConnectionError::Protocol(
                            "tail call returned takeFromOtherQuestion".to_owned(),
                        )));
                    } else if let Some(response) = self.answers.take_redirected_response(answer_id)
                    {
                        self.complete_redirect_waiter(key, question, response);
                    } else if self.answers.can_wait_for_redirect(answer_id) {
                        if let Err((error, (key, question))) =
                            self.answers.wait_for_redirect(answer_id, (key, question))
                        {
                            let _ = self.questions.release_reserved(key);
                            question.cell.complete(Err(error.clone()));
                            self.protocol_failure(error);
                        }
                    } else {
                        let error = ConnectionError::Protocol(format!(
                            "takeFromOtherQuestion references unavailable answer {answer_id}"
                        ));
                        let _ = self.questions.release_reserved(key);
                        question.cell.complete(Err(error.clone()));
                        self.protocol_failure(error);
                    }
                    return;
                }
                if tail_results_elsewhere {
                    question
                        .cell
                        .complete(Ok(ReturnPayload::ResultsSentElsewhere));
                    match self.answers.record_tail_results_elsewhere(key) {
                        Ok(Some(answer)) => self.finish_answer(answer),
                        Ok(None) => {}
                        Err(error) => {
                            let _ = self.questions.release_reserved(key);
                            self.protocol_failure(error);
                        }
                    }
                    return;
                }
                let outcome = match message.payload {
                    ReturnPayload::ResultsSentElsewhere => Err(ConnectionError::Protocol(
                        "resultsSentElsewhere returned for a non-tail call".to_owned(),
                    )),
                    ReturnPayload::TakeFromOtherQuestion(_) => Err(ConnectionError::Protocol(
                        "unreachable duplicate tail-routing branch".to_owned(),
                    )),
                    ReturnPayload::Results(_) | ReturnPayload::Exception(_)
                        if question.is_tail_call =>
                    {
                        Err(ConnectionError::Protocol(
                            "tail call returned results to the wrong endpoint".to_owned(),
                        ))
                    }
                    payload => Ok(payload),
                };
                question.cell.complete(outcome);
                if !message.no_finish_needed {
                    match encode_finish_with_release(key.id, !has_result_caps, self.protocol_limits)
                    {
                        Ok(finish) => self.effects.push_back(ActorEffect::Send(finish)),
                        Err(error) => {
                            self.protocol_failure(ConnectionError::Wire(error.to_string()))
                        }
                    }
                }
            }
            ProtocolMessage::Finish(message) => {
                if message.require_early_cancellation_workaround {
                    self.deferred_finishes.push_back(message);
                    self.yield_deferred_finish = true;
                } else {
                    self.finish_incoming(message);
                }
            }
            ProtocolMessage::Release(release) => {
                if let Err(error) = self.capabilities.apply_release(release) {
                    self.protocol_failure(ConnectionError::Capability(error.to_string()));
                }
            }
            ProtocolMessage::Resolve(message) => self.incoming_resolve(message),
            ProtocolMessage::Disembargo(message)
                if matches!(message.context, DisembargoContext::Accept(_)) =>
            {
                self.reply_unimplemented(&raw);
            }
            ProtocolMessage::Disembargo(message) => self.incoming_disembargo(message),
            ProtocolMessage::Provide(_)
            | ProtocolMessage::Accept(_)
            | ProtocolMessage::ThirdPartyAnswer(_) => self.reply_unimplemented(&raw),
            ProtocolMessage::Abort(exception) => {
                self.transition_terminal(ConnectionError::RemoteAbort(exception), false);
            }
            ProtocolMessage::Unimplemented(Some(nested)) => {
                self.handle_unimplemented(nested);
            }
            ProtocolMessage::Unimplemented(None) => {}
            ProtocolMessage::Unsupported { .. } => self.reply_unimplemented(&raw),
        }
    }

    fn reply_unimplemented(&mut self, raw: &Arc<OwnedMessage>) {
        match encode_unimplemented(raw, self.protocol_limits) {
            Ok(message) => self.effects.push_back(ActorEffect::Send(message)),
            Err(error) => self.protocol_failure(ConnectionError::Wire(error.to_string())),
        }
    }

    fn incoming_resolve(&mut self, message: ResolveMessage) {
        let Some(state) = self.promise_imports.get(&message.promise_id) else {
            self.release_abandoned_resolution(message.resolution);
            return;
        };
        if !matches!(state, ImportPromiseState::Unresolved) {
            self.protocol_failure(ConnectionError::Protocol(format!(
                "promise import {} resolved more than once",
                message.promise_id
            )));
            return;
        }

        let next = match message.resolution {
            PromiseResolution::Exception(exception) => ImportPromiseState::Broken(exception),
            PromiseResolution::Cap(descriptor) => match descriptor.descriptor() {
                CapDescriptor::SenderHosted(id) => match self.capabilities.receive(&descriptor) {
                    Ok(ReceivedCapability::Imported(_)) => ImportPromiseState::Remote(*id),
                    Ok(_) => unreachable!("senderHosted has a fixed receive kind"),
                    Err(error) => {
                        self.protocol_failure(ConnectionError::Capability(error.to_string()));
                        return;
                    }
                },
                CapDescriptor::SenderPromise(id) => {
                    if *id == message.promise_id {
                        self.protocol_failure(ConnectionError::Protocol(format!(
                            "promise import {id} resolved to itself"
                        )));
                        return;
                    }
                    match self.capabilities.receive(&descriptor) {
                        Ok(ReceivedCapability::PromiseImported(_)) => {
                            self.promise_imports
                                .entry(*id)
                                .or_insert(ImportPromiseState::Unresolved);
                            ImportPromiseState::Remote(*id)
                        }
                        Ok(_) => unreachable!("senderPromise has a fixed receive kind"),
                        Err(error) => {
                            self.protocol_failure(ConnectionError::Capability(error.to_string()));
                            return;
                        }
                    }
                }
                CapDescriptor::ReceiverHosted(_export_id) => {
                    let capability = match self.capabilities.receive(&descriptor) {
                        Ok(ReceivedCapability::Hosted(capability)) => capability,
                        Ok(ReceivedCapability::ExportedPromise(_)) => {
                            // Keeping the original peer route is always a valid
                            // non-shortened path. The peer's frozen export route
                            // performs the eventual promise-to-promise forwarding.
                            self.promise_imports.insert(
                                message.promise_id,
                                ImportPromiseState::Remote(message.promise_id),
                            );
                            return;
                        }
                        Ok(_) => unreachable!("receiverHosted has a fixed receive kind"),
                        Err(error) => {
                            self.protocol_failure(ConnectionError::Capability(error.to_string()));
                            return;
                        }
                    };
                    if self.embargoes.len() >= self.max_embargoes {
                        self.protocol_failure(ConnectionError::Protocol(format!(
                            "embargo limit {} exceeded",
                            self.max_embargoes
                        )));
                        return;
                    }
                    let embargo_id = match lowest_free_actor_id(&self.embargoes) {
                        Some(id) => id,
                        None => {
                            self.protocol_failure(ConnectionError::GenerationExhausted);
                            return;
                        }
                    };
                    self.embargoes.insert(
                        embargo_id,
                        EmbargoState {
                            promise_id: message.promise_id,
                            capability: capability.clone(),
                            queued_calls: VecDeque::new(),
                        },
                    );
                    let disembargo = encode_disembargo(
                        &CallTarget::ImportedCap(message.promise_id),
                        DisembargoContext::SenderLoopback(embargo_id),
                        self.protocol_limits,
                    );
                    match disembargo {
                        Ok(disembargo) => self.effects.push_back(ActorEffect::Send(disembargo)),
                        Err(error) => {
                            self.embargoes.remove(&embargo_id);
                            self.protocol_failure(ConnectionError::Wire(error.to_string()));
                            return;
                        }
                    }
                    ImportPromiseState::Loopback {
                        capability,
                        embargo_id,
                    }
                }
                CapDescriptor::None => ImportPromiseState::Broken(RpcException::new(
                    "promise resolved to null",
                    ExceptionType::Failed,
                )),
                CapDescriptor::ReceiverAnswer(target) => {
                    if !self.questions.contains_id(target.question_id)
                        || target.transform.len() > self.protocol_limits.max_pipeline_ops
                    {
                        self.protocol_failure(ConnectionError::Protocol(format!(
                            "promise resolution references unavailable receiverAnswer {}",
                            target.question_id
                        )));
                        return;
                    }
                    ImportPromiseState::PromisedAnswer(target.clone())
                }
                CapDescriptor::ThirdPartyHosted(third_party) => {
                    match self.capabilities.receive(&descriptor) {
                        Ok(ReceivedCapability::Imported(_)) => {
                            ImportPromiseState::Remote(third_party.vine_id)
                        }
                        Ok(_) => unreachable!("thirdPartyHosted uses its vine import"),
                        Err(error) => {
                            self.protocol_failure(ConnectionError::Capability(error.to_string()));
                            return;
                        }
                    }
                }
                CapDescriptor::Attached { .. } => unreachable!("descriptor() removes attachment"),
            },
        };
        self.promise_imports.insert(message.promise_id, next);
    }

    fn release_abandoned_resolution(&mut self, resolution: PromiseResolution) {
        let PromiseResolution::Cap(descriptor) = resolution else {
            return;
        };
        match self.capabilities.receive(&descriptor) {
            Ok(ReceivedCapability::Imported(id) | ReceivedCapability::PromiseImported(id)) => {
                if let Err(error) = self.release_import(id, 1) {
                    self.protocol_failure(error);
                }
            }
            Ok(_) => {}
            Err(error) => self.protocol_failure(ConnectionError::Capability(error.to_string())),
        }
    }

    fn incoming_disembargo(&mut self, message: DisembargoMessage) {
        match message.context {
            DisembargoContext::SenderLoopback(id) => {
                let CallTarget::ImportedCap(export_id) = message.target else {
                    self.protocol_failure(ConnectionError::Protocol(
                        "senderLoopback requires an importedCap target".to_owned(),
                    ));
                    return;
                };
                let Some(state) = self.promise_exports.get(&export_id) else {
                    self.protocol_failure(ConnectionError::Protocol(format!(
                        "senderLoopback targets unknown promise export {export_id}"
                    )));
                    return;
                };
                let Some(FrozenPromiseRoute::Imported(import_id)) = state.route else {
                    self.protocol_failure(ConnectionError::Protocol(format!(
                        "senderLoopback target {export_id} does not resolve back to its sender"
                    )));
                    return;
                };
                if !self.capabilities.contains_import(import_id) {
                    self.protocol_failure(ConnectionError::Protocol(format!(
                        "senderLoopback route references released import {import_id}"
                    )));
                    return;
                }
                match encode_disembargo(
                    &CallTarget::ImportedCap(export_id),
                    DisembargoContext::ReceiverLoopback(id),
                    self.protocol_limits,
                ) {
                    Ok(reply) => self.effects.push_back(ActorEffect::Send(reply)),
                    Err(error) => self.protocol_failure(ConnectionError::Wire(error.to_string())),
                }
            }
            DisembargoContext::ReceiverLoopback(id) => {
                let Some(embargo) = self.embargoes.remove(&id) else {
                    self.protocol_failure(ConnectionError::Protocol(format!(
                        "receiverLoopback references unknown embargo {id}"
                    )));
                    return;
                };
                let Some(remaining) = self.embargoed_calls.checked_sub(embargo.queued_calls.len())
                else {
                    self.protocol_failure(ConnectionError::Protocol(
                        "embargoed-call accounting underflow".to_owned(),
                    ));
                    return;
                };
                self.embargoed_calls = remaining;
                let matches = self
                    .promise_imports
                    .get(&embargo.promise_id)
                    .is_some_and(|state| {
                        matches!(
                            state,
                            ImportPromiseState::Loopback { embargo_id, .. } if *embargo_id == id
                        )
                    });
                if !matches {
                    self.protocol_failure(ConnectionError::Protocol(format!(
                        "receiverLoopback {id} does not match its promise route"
                    )));
                    return;
                }
                self.promise_imports.insert(
                    embargo.promise_id,
                    ImportPromiseState::Local(embargo.capability.clone()),
                );
                for call in embargo.queued_calls {
                    if call.capabilities.is_empty() {
                        self.start_local_call(
                            embargo.capability.clone(),
                            call.interface_id,
                            call.method_id,
                            call.params,
                            call.capabilities,
                            call.cell,
                        );
                    } else {
                        self.start_call(
                            OutgoingCallTarget::Imported(embargo.promise_id),
                            call.interface_id,
                            call.method_id,
                            call.params,
                            call.capabilities,
                            call.cell,
                        );
                    }
                }
            }
            DisembargoContext::Accept(_) => {
                self.protocol_failure(ConnectionError::Protocol(
                    "accept disembargo bypassed Level-3 dispatch".to_owned(),
                ));
            }
        }
    }

    fn handle_unimplemented(&mut self, nested: capnp_schema::DynamicStruct) {
        let question_id = match read_protocol_struct(nested) {
            Ok(ProtocolMessage::Bootstrap(message)) => Some(message.question_id),
            Ok(ProtocolMessage::Call(message)) => Some(message.question_id),
            Ok(_) => None,
            Err(error) => {
                self.protocol_failure(ConnectionError::Protocol(error.to_string()));
                return;
            }
        };
        if let Some(id) = question_id {
            if let Some((_key, question)) = self.questions.remove_id(id) {
                question.cell.complete(Err(ConnectionError::Unimplemented));
            }
        }
    }

    fn dispatch(&mut self, request: IncomingRequest, answer: AnswerKey) {
        let Some(cancellation) = self.answers.cancellation(answer) else {
            self.protocol_failure(ConnectionError::StaleAnswer(answer));
            return;
        };
        self.stats.dispatched_handlers = self.stats.dispatched_handlers.saturating_add(1);
        self.effects.push_back(ActorEffect::Dispatch {
            request,
            completion: CompletionToken {
                handle: self.handle.clone(),
                answer,
                cancellation,
            },
        });
    }

    fn receive_cap_table(
        &mut self,
        descriptors: &[CapDescriptor],
    ) -> Result<Vec<u32>, ConnectionError> {
        let mut imports = Vec::new();
        for descriptor in descriptors {
            let received = self
                .capabilities
                .receive(descriptor)
                .map_err(|error| ConnectionError::Capability(error.to_string()))?;
            match received {
                ReceivedCapability::Imported(id) => imports.push(id),
                ReceivedCapability::PromiseImported(id) => {
                    match self.promise_imports.get(&id) {
                        Some(ImportPromiseState::Unresolved) | None => {
                            self.promise_imports
                                .entry(id)
                                .or_insert(ImportPromiseState::Unresolved);
                        }
                        Some(_) => {
                            return Err(ConnectionError::Protocol(format!(
                                "resolved promise import {id} was described again"
                            )));
                        }
                    }
                    imports.push(id);
                }
                ReceivedCapability::None
                | ReceivedCapability::Hosted(_)
                | ReceivedCapability::ExportedPromise(_)
                | ReceivedCapability::ReceiverAnswer(_) => {}
            }
        }
        Ok(imports)
    }

    fn apply_implicit_import_releases(&mut self, ids: &[u32]) -> Result<(), ConnectionError> {
        self.capabilities
            .apply_implicit_import_releases(ids)
            .map_err(|error| ConnectionError::Capability(error.to_string()))?;
        let unique = ids.iter().copied().collect::<BTreeSet<_>>();
        for id in unique {
            if self.capabilities.contains_import(id) {
                continue;
            }
            if let Some(ImportPromiseState::Remote(target)) = self.promise_imports.remove(&id) {
                if self.capabilities.contains_import(target) {
                    self.release_import(target, 1)?;
                }
            }
        }
        Ok(())
    }

    fn describe_outgoing_capabilities(
        &mut self,
        capabilities: &[OutgoingCapability],
    ) -> Result<(Vec<CapDescriptor>, Vec<OwnedResource>), ConnectionError> {
        for capability in capabilities {
            let capability = capability.capability();
            if let OutgoingCapability::ReceiverAnswer(promised_answer) = capability {
                if !self.questions.contains_id(promised_answer.question_id) {
                    return Err(ConnectionError::Protocol(format!(
                        "receiverAnswer references inactive question {}",
                        promised_answer.question_id
                    )));
                }
                if promised_answer.transform.len() > self.protocol_limits.max_pipeline_ops {
                    return Err(ConnectionError::Protocol(
                        "receiverAnswer transform exceeds protocol limit".to_owned(),
                    ));
                }
            }
            if let OutgoingCapability::Promise(promise) = capability {
                if let Some(export_id) = self.capabilities.promise_export_id(promise.identity()) {
                    if self
                        .promise_exports
                        .get(&export_id)
                        .is_some_and(|state| state.route.is_some())
                    {
                        return Err(ConnectionError::Protocol(
                            "a resolved promise cannot be described as senderPromise again"
                                .to_owned(),
                        ));
                    }
                }
            }
        }
        let (descriptors, resources) = self
            .capabilities
            .describe_all_with_resources(capabilities)
            .map_err(|error| ConnectionError::Capability(error.to_string()))?;
        for (capability, descriptor) in capabilities.iter().zip(&descriptors) {
            if let (OutgoingCapability::Promise(promise), CapDescriptor::SenderPromise(id)) =
                (capability.capability(), descriptor.descriptor())
            {
                self.promise_exports
                    .entry(*id)
                    .or_insert_with(|| ExportPromiseState {
                        identity: promise.identity(),
                        route: None,
                        queued_calls: VecDeque::new(),
                    });
            }
        }
        Ok((descriptors, resources))
    }

    fn resolve_promise(&mut self, identity: u64, resolution: LocalPromiseResolution) {
        let Some(export_id) = self
            .promise_exports
            .iter()
            .find_map(|(id, state)| (state.identity == identity).then_some(*id))
        else {
            self.protocol_failure(ConnectionError::Capability(format!(
                "promise identity {identity} was resolved before being exported"
            )));
            return;
        };
        if self
            .promise_exports
            .get(&export_id)
            .is_some_and(|state| state.route.is_some())
        {
            self.protocol_failure(ConnectionError::Protocol(format!(
                "promise export {export_id} resolved more than once"
            )));
            return;
        }

        let live_export = self.capabilities.contains_export(export_id);
        let (wire, frozen) = match resolution {
            LocalPromiseResolution::Hosted(capability) => {
                let descriptor = if live_export {
                    match self
                        .capabilities
                        .describe(&OutgoingCapability::Hosted(capability.clone()))
                    {
                        Ok(descriptor @ CapDescriptor::SenderHosted(_)) => Some(descriptor),
                        Ok(_) => unreachable!("hosted capability has a fixed descriptor kind"),
                        Err(error) => {
                            self.protocol_failure(ConnectionError::Capability(error.to_string()));
                            return;
                        }
                    }
                } else {
                    None
                };
                (
                    descriptor.map(PromiseResolution::Cap),
                    FrozenPromiseRoute::Hosted(capability),
                )
            }
            LocalPromiseResolution::Imported(import_id) => {
                if !self.capabilities.contains_import(import_id) {
                    self.protocol_failure(ConnectionError::Capability(format!(
                        "promise resolution references unknown import {import_id}"
                    )));
                    return;
                }
                (
                    live_export.then_some(PromiseResolution::Cap(CapDescriptor::ReceiverHosted(
                        import_id,
                    ))),
                    FrozenPromiseRoute::Imported(import_id),
                )
            }
            LocalPromiseResolution::Exception(exception) => (
                live_export.then(|| PromiseResolution::Exception(exception.clone())),
                FrozenPromiseRoute::Exception(exception),
            ),
        };
        let queued = {
            let state = self
                .promise_exports
                .get_mut(&export_id)
                .expect("promise export found above");
            state.route = Some(frozen.clone());
            core::mem::take(&mut state.queued_calls)
        };
        for call in queued {
            self.route_frozen_promise_call(call, &frozen);
        }
        if let Some(wire) = wire {
            match encode_resolve(export_id, &wire, self.protocol_limits) {
                Ok(message) => self.effects.push_back(ActorEffect::Send(message)),
                Err(error) => self.protocol_failure(ConnectionError::Wire(error.to_string())),
            }
        } else {
            self.promise_exports.remove(&export_id);
            self.capabilities.release_promise_reservation(export_id);
        }
    }

    fn route_exported_promise_call(&mut self, export_id: u32, call: PendingPromiseCall) {
        let Some(state) = self.promise_exports.get_mut(&export_id) else {
            self.protocol_failure(ConnectionError::Protocol(format!(
                "call targets unknown promise export {export_id}"
            )));
            return;
        };
        if let Some(route) = state.route.clone() {
            self.route_frozen_promise_call(call, &route);
        } else {
            state.queued_calls.push_back(call);
        }
    }

    fn route_frozen_promise_call(&mut self, call: PendingPromiseCall, route: &FrozenPromiseRoute) {
        match route {
            FrozenPromiseRoute::Hosted(capability) => self.dispatch(
                IncomingRequest::Call {
                    target: IncomingCallTarget::Hosted(capability.clone()),
                    interface_id: call.interface_id,
                    method_id: call.method_id,
                    params: call.params,
                },
                call.answer,
            ),
            FrozenPromiseRoute::Imported(import_id) => {
                let capabilities = match self.forwarding_capabilities(&call.params.cap_table) {
                    Ok(capabilities) => capabilities,
                    Err(error) => {
                        self.fail_pipeline_call(call.answer, error.to_string());
                        return;
                    }
                };
                self.start_tail_call_common(
                    call.answer,
                    *import_id,
                    call.interface_id,
                    call.method_id,
                    TailCallParams::Dynamic(call.params),
                    capabilities,
                );
            }
            FrozenPromiseRoute::Exception(exception) => {
                self.fail_pipeline_call(call.answer, exception.reason.clone());
            }
        }
    }

    fn forwarding_capabilities(
        &mut self,
        descriptors: &[CapDescriptor],
    ) -> Result<Vec<OutgoingCapability>, ConnectionError> {
        descriptors
            .iter()
            .map(|descriptor| {
                let capability = match descriptor.descriptor() {
                    CapDescriptor::None => Ok(OutgoingCapability::None),
                    CapDescriptor::SenderHosted(id) | CapDescriptor::SenderPromise(id) => {
                        Ok(OutgoingCapability::ReceiverHosted(*id))
                    }
                    CapDescriptor::ReceiverHosted(_) => {
                        match self.capabilities.receive(descriptor) {
                            Ok(ReceivedCapability::Hosted(capability)) => {
                                Ok(OutgoingCapability::Hosted(capability))
                            }
                            Ok(ReceivedCapability::ExportedPromise(capability)) => {
                                Ok(OutgoingCapability::Promise(capability))
                            }
                            Ok(_) => unreachable!("receiverHosted has a fixed receive kind"),
                            Err(error) => Err(ConnectionError::Capability(error.to_string())),
                        }
                    }
                    CapDescriptor::ReceiverAnswer(target) => {
                        if !self.questions.contains_id(target.question_id) {
                            return Err(ConnectionError::Protocol(format!(
                                "forwarded receiverAnswer references inactive question {}",
                                target.question_id
                            )));
                        }
                        Ok(OutgoingCapability::ReceiverAnswer(target.clone()))
                    }
                    CapDescriptor::ThirdPartyHosted(third_party) => {
                        // Until direct pickup succeeds, forwarding through the
                        // sender's vine is the protocol-defined safe fallback.
                        Ok(OutgoingCapability::ReceiverHosted(third_party.vine_id))
                    }
                    CapDescriptor::Attached { .. } => {
                        unreachable!("descriptor() removes attachment")
                    }
                }?;
                Ok(match descriptor.attached_resource() {
                    Some(resource) => OutgoingCapability::Attached {
                        capability: Box::new(capability),
                        resource: resource.clone(),
                    },
                    None => capability,
                })
            })
            .collect()
    }

    fn local_call_complete(
        &mut self,
        question: QuestionKey,
        result: HandlerResult,
        capabilities: Vec<OutgoingCapability>,
    ) {
        let Some(question_state) = self.questions.remove(question) else {
            self.stats.stale_handler_completions =
                self.stats.stale_handler_completions.saturating_add(1);
            return;
        };
        self.stats.completed_handlers = self.stats.completed_handlers.saturating_add(1);
        question_state
            .cell
            .complete(redirected_response_payload(RedirectedResponse {
                result,
                capabilities,
            }));
    }

    fn cancel_question(&mut self, cell: &Arc<QuestionCell>) {
        let Ok(Some(key)) = cell.active_key() else {
            return;
        };
        let Some(sent_to_peer) = self.questions.mark_canceled(key) else {
            return;
        };
        cell.complete(Err(ConnectionError::Canceled));
        if sent_to_peer {
            match encode_finish_with_release(key.id, true, self.protocol_limits) {
                Ok(finish) => self.effects.push_back(ActorEffect::Send(finish)),
                Err(error) => self.protocol_failure(ConnectionError::Wire(error.to_string())),
            }
        } else {
            let _ = self.questions.remove(key);
        }
    }

    fn route_pipeline_call(&mut self, pending: PendingPipelineCall, pipeline: &PipelineSnapshot) {
        match resolve_pipeline_target(pipeline, &pending.transform) {
            Ok(target) => self.dispatch(
                IncomingRequest::Call {
                    target,
                    interface_id: pending.interface_id,
                    method_id: pending.method_id,
                    params: pending.params,
                },
                pending.answer,
            ),
            Err(error) => self.fail_pipeline_call(pending.answer, error.to_string()),
        }
    }

    fn fail_pipeline_call(&mut self, answer: AnswerKey, reason: String) {
        if !self.answers.mark_returned(answer) {
            self.stats.stale_handler_completions =
                self.stats.stale_handler_completions.saturating_add(1);
            return;
        }
        let param_imports = self.answers.param_imports(answer).unwrap_or_default();
        let result = HandlerResult::Exception(RpcException::new(reason, ExceptionType::Failed));
        match encode_return(answer.id, &result, self.protocol_limits) {
            Ok(message) => {
                if let Err(error) = self.apply_implicit_import_releases(&param_imports) {
                    self.protocol_failure(error);
                    return;
                }
                self.effects.push_back(ActorEffect::Send(message));
            }
            Err(error) => self.protocol_failure(ConnectionError::Wire(error.to_string())),
        }
    }

    fn handler_complete_redirected(
        &mut self,
        answer: AnswerKey,
        result: HandlerResult,
        capabilities: Vec<OutgoingCapability>,
        pipeline: Option<PipelineSnapshot>,
    ) {
        let redirect_waiter = match self.answers.store_redirected_response(
            answer,
            RedirectedResponse {
                result,
                capabilities,
            },
        ) {
            Ok(waiter) => waiter,
            Err(error) => {
                self.protocol_failure(error);
                return;
            }
        };
        let param_imports = self.answers.param_imports(answer).unwrap_or_default();
        let returned = match encode_return(
            answer.id,
            &HandlerResult::ResultsSentElsewhere,
            self.protocol_limits,
        ) {
            Ok(returned) => returned,
            Err(error) => {
                self.protocol_failure(ConnectionError::Wire(error.to_string()));
                return;
            }
        };
        if let Err(error) = self.apply_implicit_import_releases(&param_imports) {
            self.protocol_failure(error);
            return;
        }
        let queued = if let Some(pipeline) = pipeline {
            self.answers
                .resolve_pipeline(answer, pipeline.clone())
                .map(|queued| (queued, Some(pipeline)))
        } else {
            self.answers
                .fail_pipeline(answer)
                .map(|queued| (queued, None))
        };
        if let Some((queued, pipeline)) = queued {
            for pending in queued {
                if let Some(pipeline) = &pipeline {
                    self.route_pipeline_call(pending, pipeline);
                } else {
                    self.fail_pipeline_call(
                        pending.answer,
                        "redirected pipeline source returned no capability results".to_owned(),
                    );
                }
            }
        }
        self.stats.completed_handlers = self.stats.completed_handlers.saturating_add(1);
        self.effects.push_back(ActorEffect::Send(returned));
        if let Some((question_key, question)) = redirect_waiter {
            let Some(response) = self.answers.take_redirected_response(answer.id) else {
                self.protocol_failure(ConnectionError::Protocol(
                    "redirected response disappeared before waiter completion".to_owned(),
                ));
                return;
            };
            self.complete_redirect_waiter(question_key, question, response);
        }
    }

    fn finish_incoming(&mut self, message: FinishMessage) {
        if let Some(answer) = self
            .answers
            .finish(message.question_id, message.release_result_caps)
        {
            self.finish_answer(answer);
        }
    }

    fn finish_answer(&mut self, answer: AnswerState) {
        if answer.finish_release_result_caps {
            if let Err(error) = self
                .capabilities
                .apply_implicit_releases(&answer.result_exports)
            {
                self.protocol_failure(ConnectionError::Capability(error.to_string()));
                return;
            }
        }
        if let Some(tail_question) = answer.tail_question {
            if !self.questions.release_reserved(tail_question) {
                self.protocol_failure(ConnectionError::StaleTarget(tail_question));
                return;
            }
            match encode_finish_with_release(tail_question.id, true, self.protocol_limits) {
                Ok(finish) => self.effects.push_back(ActorEffect::Send(finish)),
                Err(error) => self.protocol_failure(ConnectionError::Wire(error.to_string())),
            }
        }
    }

    fn complete_redirect_waiter(
        &mut self,
        key: QuestionKey,
        question: QuestionState,
        response: RedirectedResponse,
    ) {
        if !self.questions.release_reserved(key) {
            self.protocol_failure(ConnectionError::StaleTarget(key));
            return;
        }
        question
            .cell
            .complete(redirected_response_payload(response));
        match encode_finish_with_release(key.id, true, self.protocol_limits) {
            Ok(finish) => self.effects.push_back(ActorEffect::Send(finish)),
            Err(error) => self.protocol_failure(ConnectionError::Wire(error.to_string())),
        }
    }

    fn handler_complete(
        &mut self,
        answer: AnswerKey,
        result: HandlerResult,
        capabilities: Vec<OutgoingCapability>,
    ) {
        if matches!(result, HandlerResult::ResultsWithCapabilities { .. }) {
            self.protocol_failure(ConnectionError::Protocol(
                "raw capability descriptors cannot bypass actor accounting".to_owned(),
            ));
            return;
        }
        let pipeline = match &result {
            HandlerResult::Results(content) => Some(PipelineSnapshot {
                content: Arc::clone(content),
                capabilities: capabilities.clone(),
            }),
            _ => None,
        };
        if !self.answers.mark_returned(answer) {
            self.stats.stale_handler_completions =
                self.stats.stale_handler_completions.saturating_add(1);
            return;
        }
        if self.answers.redirect_results(answer) {
            self.handler_complete_redirected(answer, result, capabilities, pipeline);
            return;
        }
        let (descriptors, resources) = match self.describe_outgoing_capabilities(&capabilities) {
            Ok(described) => described,
            Err(error) => {
                self.protocol_failure(ConnectionError::Capability(error.to_string()));
                return;
            }
        };
        let result_exports = sender_hosted_ids(&descriptors);
        if !self
            .answers
            .record_result_exports(answer, result_exports.clone())
        {
            let _ = self.capabilities.apply_implicit_releases(&result_exports);
            self.protocol_failure(ConnectionError::StaleAnswer(answer));
            return;
        }
        let result = match (result, descriptors.is_empty()) {
            (HandlerResult::Results(content), false) => HandlerResult::ResultsWithCapabilities {
                content,
                cap_table: descriptors,
            },
            (result, true) => result,
            (result, false) => {
                let _ = self.capabilities.apply_implicit_releases(&result_exports);
                self.protocol_failure(ConnectionError::Protocol(format!(
                    "capabilities cannot accompany handler result {result:?}"
                )));
                return;
            }
        };
        let param_imports = self.answers.param_imports(answer).unwrap_or_default();
        let finish = self.answers.finish_state(answer);
        match encode_return(answer.id, &result, self.protocol_limits) {
            Ok(message) => {
                if let Err(error) = self.apply_implicit_import_releases(&param_imports) {
                    self.protocol_failure(error);
                    return;
                }
                let queued = if let Some(pipeline) = pipeline {
                    self.answers
                        .resolve_pipeline(answer, pipeline.clone())
                        .map(|queued| (queued, Some(pipeline)))
                } else {
                    self.answers
                        .fail_pipeline(answer)
                        .map(|queued| (queued, None))
                };
                if let Some((queued, pipeline)) = queued {
                    for pending in queued {
                        if let Some(pipeline) = &pipeline {
                            self.route_pipeline_call(pending, pipeline);
                        } else {
                            self.fail_pipeline_call(
                                pending.answer,
                                "pipeline source returned no capability results".to_owned(),
                            );
                        }
                    }
                }
                self.stats.completed_handlers = self.stats.completed_handlers.saturating_add(1);
                if let Some(release_result_caps) = finish {
                    if release_result_caps {
                        if let Err(error) =
                            self.capabilities.apply_implicit_releases(&result_exports)
                        {
                            self.protocol_failure(ConnectionError::Capability(error.to_string()));
                            return;
                        }
                    }
                    let _ = self.answers.remove_id(answer.id);
                } else {
                    self.send_with_resources(message, resources);
                }
            }
            Err(error) => {
                let _ = self.capabilities.apply_implicit_releases(&result_exports);
                self.protocol_failure(ConnectionError::Wire(error.to_string()));
            }
        }
    }

    fn record_question_allocation(&mut self, key: QuestionKey) {
        self.stats.allocated_questions = self.stats.allocated_questions.saturating_add(1);
        if key.generation != 0 {
            self.stats.reused_question_ids = self.stats.reused_question_ids.saturating_add(1);
        }
    }

    fn send_with_resources(&mut self, message: Arc<OwnedMessage>, resources: Vec<OwnedResource>) {
        if resources.is_empty() {
            self.effects.push_back(ActorEffect::Send(message));
        } else {
            self.effects
                .push_back(ActorEffect::SendWithResources { message, resources });
        }
    }

    fn protocol_failure(&mut self, error: ConnectionError) {
        let reason = error.to_string();
        if let Ok(abort) = encode_abort(
            &RpcException::new(reason, ExceptionType::Failed),
            self.protocol_limits,
        ) {
            self.effects.push_back(ActorEffect::Send(abort));
        }
        self.transition_terminal(error, false);
    }

    fn transition_terminal(&mut self, error: ConnectionError, send_abort: bool) {
        if self.terminal {
            return;
        }
        if send_abort {
            let reason = error.to_string();
            if let Ok(abort) = encode_abort(
                &RpcException::new(reason, ExceptionType::Failed),
                self.protocol_limits,
            ) {
                self.effects.push_back(ActorEffect::Send(abort));
            }
        }
        self.terminal = true;
        self.mailbox.closed.store(true, Ordering::Release);
        for question in self.questions.drain() {
            question.cell.complete(Err(error.clone()));
        }
        for question in self.answers.drain_redirect_waiters() {
            question.cell.complete(Err(error.clone()));
        }
        self.answers.clear();
        self.deferred_finishes.clear();
        self.yield_deferred_finish = false;
        self.capabilities.clear();
        self.promise_imports.clear();
        self.promise_exports.clear();
        for embargo in core::mem::take(&mut self.embargoes).into_values() {
            for call in embargo.queued_calls {
                call.cell.complete(Err(error.clone()));
            }
        }
        self.embargoed_calls = 0;
        if let Ok(commands) = self.mailbox.drain() {
            for command in commands {
                complete_rejected(command, error.clone());
            }
        }
        self.effects.push_back(ActorEffect::CloseTransport);
    }
}

impl Drop for ConnectionActor {
    fn drop(&mut self) {
        self.transition_terminal(ConnectionError::Disconnected, false);
    }
}

enum ActorCommand {
    StartBootstrap {
        cell: Arc<QuestionCell>,
    },
    StartCall {
        target: OutgoingCallTarget,
        interface_id: u64,
        method_id: u16,
        params: Arc<OwnedMessage>,
        capabilities: Vec<OutgoingCapability>,
        cell: Arc<QuestionCell>,
    },
    Incoming {
        message: Arc<OwnedMessage>,
        resources: Vec<OwnedResource>,
    },
    StartTailCall {
        answer: AnswerKey,
        import_id: u32,
        interface_id: u64,
        method_id: u16,
        params: Arc<OwnedMessage>,
        capabilities: Vec<OutgoingCapability>,
    },
    HandlerComplete {
        answer: AnswerKey,
        result: HandlerResult,
        capabilities: Vec<OutgoingCapability>,
    },
    ResolvePromise {
        identity: u64,
        resolution: LocalPromiseResolution,
    },
    LocalCallComplete {
        question: QuestionKey,
        result: HandlerResult,
        capabilities: Vec<OutgoingCapability>,
    },
    CancelQuestion {
        cell: Arc<QuestionCell>,
    },
    Shutdown,
}

enum OutgoingCallTarget {
    Bootstrap(QuestionTarget),
    Imported(u32),
}

fn sender_hosted_ids(descriptors: &[CapDescriptor]) -> Vec<u32> {
    descriptors
        .iter()
        .filter_map(|descriptor| match descriptor.descriptor() {
            CapDescriptor::SenderHosted(id) | CapDescriptor::SenderPromise(id) => Some(*id),
            CapDescriptor::None
            | CapDescriptor::ReceiverHosted(_)
            | CapDescriptor::ReceiverAnswer(_) => None,
            CapDescriptor::ThirdPartyHosted(third_party) => Some(third_party.vine_id),
            CapDescriptor::Attached { .. } => unreachable!("descriptor() removes attachment"),
        })
        .collect()
}

fn lowest_free_actor_id<T>(values: &BTreeMap<u32, T>) -> Option<u32> {
    let mut candidate = 0_u32;
    for id in values.keys().copied() {
        if id != candidate {
            break;
        }
        candidate = candidate.checked_add(1)?;
    }
    Some(candidate)
}

fn resolve_pipeline_target(
    pipeline: &PipelineSnapshot,
    transform: &[PipelineOp],
) -> Result<IncomingCallTarget, ConnectionError> {
    let mut pointer = pipeline
        .content
        .root_pointer()
        .map_err(|error| ConnectionError::Protocol(error.to_string()))?;
    for operation in transform {
        match operation {
            PipelineOp::Noop => {}
            PipelineOp::GetPointerField(index) => {
                let OwnedPointerRef::Struct(structure) = pointer else {
                    return Err(ConnectionError::Protocol(
                        "pipeline getPointerField requires a struct".to_owned(),
                    ));
                };
                pointer = structure
                    .child_pointer(*index)
                    .map_err(|error| ConnectionError::Protocol(error.to_string()))?;
            }
        }
    }
    let OwnedPointerRef::Capability(index) = pointer else {
        return Err(ConnectionError::Protocol(
            "pipeline transform did not resolve to a capability".to_owned(),
        ));
    };
    let index = usize::try_from(index)
        .map_err(|_| ConnectionError::Protocol("capability index overflow".to_owned()))?;
    match pipeline
        .capabilities
        .get(index)
        .map(OutgoingCapability::capability)
    {
        Some(OutgoingCapability::Hosted(capability)) => {
            Ok(IncomingCallTarget::Hosted(capability.clone()))
        }
        Some(OutgoingCapability::None) | None => Err(ConnectionError::Protocol(format!(
            "pipeline capability index {index} is absent"
        ))),
        Some(OutgoingCapability::Promise(_))
        | Some(OutgoingCapability::ReceiverHosted(_))
        | Some(OutgoingCapability::ReceiverAnswer(_)) => Err(ConnectionError::Protocol(
            "pipeline target requires Level-1 tail routing".to_owned(),
        )),
        Some(OutgoingCapability::Attached { .. }) => {
            unreachable!("capability() removes attachment")
        }
    }
}

fn redirected_response_payload(
    response: RedirectedResponse,
) -> Result<ReturnPayload, ConnectionError> {
    match response.result {
        HandlerResult::Results(content) => {
            let content = match content
                .root_pointer()
                .map_err(|error| ConnectionError::Protocol(error.to_string()))?
            {
                OwnedPointerRef::Null => DynamicAnyPointer::Null,
                OwnedPointerRef::Struct(value) => DynamicAnyPointer::Struct(value),
                OwnedPointerRef::List(value) => DynamicAnyPointer::List(value),
                OwnedPointerRef::Capability(value) => DynamicAnyPointer::Capability(value),
            };
            Ok(ReturnPayload::LocalResults {
                content,
                capabilities: response.capabilities,
            })
        }
        HandlerResult::Exception(exception) => Ok(ReturnPayload::Exception(exception)),
        HandlerResult::Canceled => Ok(ReturnPayload::Canceled),
        HandlerResult::ResultsWithCapabilities { .. }
        | HandlerResult::ResultsSentElsewhere
        | HandlerResult::TakeFromOtherQuestion(_) => Err(ConnectionError::Protocol(
            "invalid redirected response variant".to_owned(),
        )),
    }
}

fn complete_rejected(command: ActorCommand, error: ConnectionError) {
    match command {
        ActorCommand::StartBootstrap { cell } | ActorCommand::StartCall { cell, .. } => {
            cell.complete(Err(error));
        }
        ActorCommand::Incoming { .. }
        | ActorCommand::StartTailCall { .. }
        | ActorCommand::HandlerComplete { .. }
        | ActorCommand::ResolvePromise { .. }
        | ActorCommand::LocalCallComplete { .. }
        | ActorCommand::CancelQuestion { .. }
        | ActorCommand::Shutdown => {}
    }
}

struct SharedMailbox {
    capacity: usize,
    closed: AtomicBool,
    shutdown_queued: AtomicBool,
    state: Mutex<MailboxState>,
}

struct MailboxState {
    commands: VecDeque<ActorCommand>,
    actor_waker: Option<Waker>,
}

impl SharedMailbox {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            closed: AtomicBool::new(false),
            shutdown_queued: AtomicBool::new(false),
            state: Mutex::new(MailboxState {
                commands: VecDeque::new(),
                actor_waker: None,
            }),
        }
    }

    fn submit(&self, command: ActorCommand) -> Result<(), ConnectionError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(ConnectionError::Disconnected);
        }
        let mut state = self.state.lock().map_err(|_| ConnectionError::Poisoned)?;
        if self.closed.load(Ordering::Acquire) {
            return Err(ConnectionError::Disconnected);
        }
        if state.commands.len() >= self.capacity {
            return Err(ConnectionError::Overloaded {
                capacity: self.capacity,
            });
        }
        state.commands.push_back(command);
        let actor_waker = state.actor_waker.take();
        drop(state);
        if let Some(waker) = actor_waker {
            waker.wake();
        }
        Ok(())
    }

    fn submit_lifecycle(&self, command: ActorCommand) -> Result<(), ConnectionError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(ConnectionError::Disconnected);
        }
        let mut state = self.state.lock().map_err(|_| ConnectionError::Poisoned)?;
        if self.closed.load(Ordering::Acquire) {
            return Err(ConnectionError::Disconnected);
        }
        state.commands.push_back(command);
        let actor_waker = state.actor_waker.take();
        drop(state);
        if let Some(waker) = actor_waker {
            waker.wake();
        }
        Ok(())
    }

    fn submit_shutdown(&self) -> Result<(), ConnectionError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(ConnectionError::Disconnected);
        }
        if self.shutdown_queued.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        self.submit_lifecycle(ActorCommand::Shutdown)
    }

    fn pop_or_register(&self, waker: &Waker) -> Result<Option<ActorCommand>, ConnectionError> {
        let mut state = self.state.lock().map_err(|_| ConnectionError::Poisoned)?;
        if let Some(command) = state.commands.pop_front() {
            return Ok(Some(command));
        }
        state.actor_waker = Some(waker.clone());
        Ok(None)
    }

    fn drain(&self) -> Result<VecDeque<ActorCommand>, ConnectionError> {
        let mut state = self.state.lock().map_err(|_| ConnectionError::Poisoned)?;
        Ok(std::mem::take(&mut state.commands))
    }
}

struct QuestionCell {
    state: Mutex<QuestionCellState>,
}

struct QuestionCellState {
    key: Option<QuestionKey>,
    active: bool,
    outcome: Option<Result<ReturnPayload, ConnectionError>>,
    delivered: bool,
    waker: Option<Waker>,
}

impl QuestionCell {
    fn new() -> Self {
        Self {
            state: Mutex::new(QuestionCellState {
                key: None,
                active: false,
                outcome: None,
                delivered: false,
                waker: None,
            }),
        }
    }

    fn assign(&self, key: QuestionKey) -> Result<(), ConnectionError> {
        let mut state = self.state.lock().map_err(|_| ConnectionError::Poisoned)?;
        state.key = Some(key);
        state.active = true;
        Ok(())
    }

    fn active_key(&self) -> Result<Option<QuestionKey>, ConnectionError> {
        let state = self.state.lock().map_err(|_| ConnectionError::Poisoned)?;
        Ok(state.active.then_some(state.key).flatten())
    }

    fn complete(&self, outcome: Result<ReturnPayload, ConnectionError>) {
        let waker = if let Ok(mut state) = self.state.lock() {
            if state.outcome.is_some() || state.delivered {
                return;
            }
            state.active = false;
            state.outcome = Some(outcome);
            state.waker.take()
        } else {
            None
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    fn poll(&self, context: &mut Context<'_>) -> Poll<Result<ReturnPayload, ConnectionError>> {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(_) => return Poll::Ready(Err(ConnectionError::Poisoned)),
        };
        if let Some(outcome) = state.outcome.take() {
            state.delivered = true;
            return Poll::Ready(outcome);
        }
        if state.delivered {
            return Poll::Ready(Err(ConnectionError::PolledAfterCompletion));
        }
        state.waker = Some(context.waker().clone());
        Poll::Pending
    }
}

struct QuestionState {
    cell: Arc<QuestionCell>,
    param_exports: Vec<u32>,
    is_tail_call: bool,
    sent_to_peer: bool,
    canceled: bool,
}

struct QuestionSlot {
    generation: u64,
    value: Option<QuestionState>,
    reserved: bool,
}

struct QuestionTable {
    slots: Vec<QuestionSlot>,
    max: usize,
    len: usize,
}

impl QuestionTable {
    fn new(max: usize) -> Self {
        Self {
            slots: Vec::new(),
            max: max.min(u32::MAX as usize),
            len: 0,
        }
    }

    fn len(&self) -> usize {
        self.len
    }

    fn allocate(&mut self, value: QuestionState) -> Result<QuestionKey, ConnectionError> {
        let mut exhausted_slot = false;
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if slot.value.is_none() && !slot.reserved {
                if slot.generation == u64::MAX {
                    exhausted_slot = true;
                    continue;
                }
                let id = u32::try_from(index).map_err(|_| ConnectionError::GenerationExhausted)?;
                let key = QuestionKey {
                    id,
                    generation: slot.generation,
                };
                slot.value = Some(value);
                self.len += 1;
                return Ok(key);
            }
        }
        if self.slots.len() >= self.max {
            if exhausted_slot {
                return Err(ConnectionError::GenerationExhausted);
            }
            return Err(ConnectionError::QuestionLimit { limit: self.max });
        }
        let id =
            u32::try_from(self.slots.len()).map_err(|_| ConnectionError::GenerationExhausted)?;
        self.slots.push(QuestionSlot {
            generation: 0,
            value: Some(value),
            reserved: false,
        });
        self.len += 1;
        Ok(QuestionKey { id, generation: 0 })
    }

    fn contains(&self, key: QuestionKey) -> bool {
        usize::try_from(key.id)
            .ok()
            .and_then(|index| self.slots.get(index))
            .is_some_and(|slot| slot.generation == key.generation && slot.value.is_some())
    }

    fn contains_id(&self, id: u32) -> bool {
        usize::try_from(id)
            .ok()
            .and_then(|index| self.slots.get(index))
            .is_some_and(|slot| slot.value.is_some() || slot.reserved)
    }

    fn is_tail_call_id(&self, id: u32) -> bool {
        self.slots
            .get(usize::try_from(id).unwrap_or(usize::MAX))
            .and_then(|slot| slot.value.as_ref())
            .is_some_and(|question| question.is_tail_call)
    }

    fn is_canceled_id(&self, id: u32) -> bool {
        self.slots
            .get(usize::try_from(id).unwrap_or(usize::MAX))
            .and_then(|slot| slot.value.as_ref())
            .is_some_and(|question| question.canceled)
    }

    fn mark_canceled(&mut self, key: QuestionKey) -> Option<bool> {
        let value = usize::try_from(key.id)
            .ok()
            .and_then(|index| self.slots.get_mut(index))?;
        if value.generation != key.generation {
            return None;
        }
        let question = value.value.as_mut()?;
        if question.canceled {
            return None;
        }
        question.canceled = true;
        Some(question.sent_to_peer)
    }

    fn remove(&mut self, key: QuestionKey) -> Option<QuestionState> {
        let slot = usize::try_from(key.id)
            .ok()
            .and_then(|index| self.slots.get_mut(index))?;
        if slot.generation != key.generation {
            return None;
        }
        let next_generation = slot.generation.checked_add(1)?;
        let value = slot.value.take()?;
        slot.generation = next_generation;
        self.len -= 1;
        Some(value)
    }

    fn remove_id(&mut self, id: u32) -> Option<(QuestionKey, QuestionState)> {
        let slot = usize::try_from(id)
            .ok()
            .and_then(|index| self.slots.get_mut(index))?;
        let key = QuestionKey {
            id,
            generation: slot.generation,
        };
        let next_generation = slot.generation.checked_add(1)?;
        let value = slot.value.take()?;
        slot.generation = next_generation;
        self.len -= 1;
        Some((key, value))
    }

    fn reserve_id(&mut self, id: u32) -> Option<(QuestionKey, QuestionState)> {
        let slot = usize::try_from(id)
            .ok()
            .and_then(|index| self.slots.get_mut(index))?;
        if slot.reserved {
            return None;
        }
        let key = QuestionKey {
            id,
            generation: slot.generation,
        };
        let value = slot.value.take()?;
        slot.reserved = true;
        Some((key, value))
    }

    fn release_reserved(&mut self, key: QuestionKey) -> bool {
        let Some(slot) = usize::try_from(key.id)
            .ok()
            .and_then(|index| self.slots.get_mut(index))
        else {
            return false;
        };
        if slot.generation != key.generation || !slot.reserved || slot.value.is_some() {
            return false;
        }
        let Some(next_generation) = slot.generation.checked_add(1) else {
            return false;
        };
        slot.generation = next_generation;
        slot.reserved = false;
        self.len -= 1;
        true
    }

    fn drain(&mut self) -> Vec<QuestionState> {
        let mut output = Vec::with_capacity(self.len);
        for slot in &mut self.slots {
            if let Some(value) = slot.value.take() {
                output.push(value);
                slot.generation = slot.generation.saturating_add(1);
            }
            if slot.reserved {
                slot.reserved = false;
                slot.generation = slot.generation.saturating_add(1);
            }
        }
        self.len = 0;
        output
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AnswerKind {
    Bootstrap,
    Call,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AnswerPhase {
    InProgress,
    Returned,
}

#[derive(Clone)]
struct PipelineSnapshot {
    content: Arc<OwnedMessage>,
    capabilities: Vec<OutgoingCapability>,
}

#[derive(Clone)]
struct RedirectedResponse {
    result: HandlerResult,
    capabilities: Vec<OutgoingCapability>,
}

#[derive(Clone)]
struct PendingPipelineCall {
    answer: AnswerKey,
    transform: Vec<PipelineOp>,
    interface_id: u64,
    method_id: u16,
    params: Payload,
}

struct AnswerState {
    kind: AnswerKind,
    phase: AnswerPhase,
    result_exports: Vec<u32>,
    param_imports: Vec<u32>,
    pipeline: Option<PipelineSnapshot>,
    queued_pipeline_calls: VecDeque<PendingPipelineCall>,
    finish_received: bool,
    finish_release_result_caps: bool,
    redirect_results: bool,
    redirected_response: Option<RedirectedResponse>,
    redirect_waiter: Option<(QuestionKey, QuestionState)>,
    tail_question: Option<QuestionKey>,
    tail_results_elsewhere: bool,
    incoming_bytes: u64,
    cancellation: CancellationSignal,
}

struct AnswerTable {
    values: BTreeMap<u32, (u64, AnswerState)>,
    next_generation: u64,
    max: usize,
    incoming_bytes: u64,
    max_incoming_bytes: u64,
}

impl AnswerTable {
    fn new(max: usize, max_incoming_bytes: u64) -> Self {
        Self {
            values: BTreeMap::new(),
            next_generation: 0,
            max,
            incoming_bytes: 0,
            max_incoming_bytes,
        }
    }

    fn len(&self) -> usize {
        self.values.len()
    }

    fn incoming_bytes(&self) -> u64 {
        self.incoming_bytes
    }

    fn check_insert(&self, id: u32, incoming_bytes: u64) -> Result<u64, ConnectionError> {
        if self.values.contains_key(&id) {
            return Err(ConnectionError::DuplicateAnswer(id));
        }
        if self.values.len() >= self.max {
            return Err(ConnectionError::AnswerLimit { limit: self.max });
        }
        if self.next_generation == u64::MAX {
            return Err(ConnectionError::GenerationExhausted);
        }
        let requested = self.incoming_bytes.checked_add(incoming_bytes).ok_or(
            ConnectionError::IncomingCallByteLimit {
                requested: u64::MAX,
                limit: self.max_incoming_bytes,
            },
        )?;
        if requested > self.max_incoming_bytes {
            return Err(ConnectionError::IncomingCallByteLimit {
                requested,
                limit: self.max_incoming_bytes,
            });
        }
        Ok(requested)
    }

    fn insert(
        &mut self,
        id: u32,
        kind: AnswerKind,
        param_imports: Vec<u32>,
        redirect_results: bool,
        incoming_bytes: u64,
    ) -> Result<AnswerKey, ConnectionError> {
        let requested = self.check_insert(id, incoming_bytes)?;
        let generation = self.next_generation;
        self.next_generation = self
            .next_generation
            .checked_add(1)
            .ok_or(ConnectionError::GenerationExhausted)?;
        self.values.insert(
            id,
            (
                generation,
                AnswerState {
                    kind,
                    phase: AnswerPhase::InProgress,
                    result_exports: Vec::new(),
                    param_imports,
                    pipeline: None,
                    queued_pipeline_calls: VecDeque::new(),
                    finish_received: false,
                    finish_release_result_caps: true,
                    redirect_results,
                    redirected_response: None,
                    redirect_waiter: None,
                    tail_question: None,
                    tail_results_elsewhere: false,
                    incoming_bytes,
                    cancellation: CancellationSignal::new(),
                },
            ),
        );
        self.incoming_bytes = requested;
        Ok(AnswerKey { id, generation })
    }

    fn key_for_id(&self, id: u32) -> Option<AnswerKey> {
        self.values.get(&id).map(|(generation, _)| AnswerKey {
            id,
            generation: *generation,
        })
    }

    fn is_bootstrap(&self, key: AnswerKey) -> bool {
        self.values.get(&key.id).is_some_and(|(generation, value)| {
            *generation == key.generation && value.kind == AnswerKind::Bootstrap
        })
    }

    fn mark_returned(&mut self, key: AnswerKey) -> bool {
        let Some((generation, value)) = self.values.get_mut(&key.id) else {
            return false;
        };
        if *generation != key.generation || value.phase != AnswerPhase::InProgress {
            return false;
        }
        value.phase = AnswerPhase::Returned;
        true
    }

    fn is_in_progress(&self, key: AnswerKey) -> bool {
        self.values.get(&key.id).is_some_and(|(generation, value)| {
            *generation == key.generation && value.phase == AnswerPhase::InProgress
        })
    }

    fn cancellation(&self, key: AnswerKey) -> Option<CancellationSignal> {
        self.values.get(&key.id).and_then(|(generation, value)| {
            (*generation == key.generation).then(|| value.cancellation.clone())
        })
    }

    fn redirect_results(&self, key: AnswerKey) -> bool {
        self.values.get(&key.id).is_some_and(|(generation, value)| {
            *generation == key.generation && value.redirect_results
        })
    }

    fn record_tail_question(&mut self, key: AnswerKey, tail_question: QuestionKey) -> bool {
        let Some((generation, answer)) = self.values.get_mut(&key.id) else {
            return false;
        };
        if *generation != key.generation
            || answer.phase != AnswerPhase::Returned
            || answer.tail_question.is_some()
        {
            return false;
        }
        answer.tail_question = Some(tail_question);
        true
    }

    fn record_tail_results_elsewhere(
        &mut self,
        tail_question: QuestionKey,
    ) -> Result<Option<AnswerState>, ConnectionError> {
        let original_id = self
            .values
            .iter()
            .find_map(|(id, (_, answer))| {
                (answer.tail_question == Some(tail_question)).then_some(*id)
            })
            .ok_or_else(|| {
                ConnectionError::Protocol(format!(
                    "resultsSentElsewhere references untracked tail question {}",
                    tail_question.id
                ))
            })?;
        let (_, answer) = self
            .values
            .get_mut(&original_id)
            .ok_or(ConnectionError::UnknownQuestion(original_id))?;
        answer.tail_results_elsewhere = true;
        if answer.finish_received {
            Ok(self.remove_id(original_id))
        } else {
            Ok(None)
        }
    }

    fn store_redirected_response(
        &mut self,
        key: AnswerKey,
        response: RedirectedResponse,
    ) -> Result<Option<(QuestionKey, QuestionState)>, ConnectionError> {
        let Some((generation, value)) = self.values.get_mut(&key.id) else {
            return Err(ConnectionError::StaleAnswer(key));
        };
        if *generation != key.generation || !value.redirect_results {
            return Err(ConnectionError::StaleAnswer(key));
        }
        value.redirected_response = Some(response);
        Ok(value.redirect_waiter.take())
    }

    fn take_redirected_response(&mut self, id: u32) -> Option<RedirectedResponse> {
        self.values
            .get_mut(&id)
            .and_then(|(_, value)| value.redirected_response.take())
    }

    fn wait_for_redirect(
        &mut self,
        id: u32,
        waiter: (QuestionKey, QuestionState),
    ) -> Result<(), (ConnectionError, (QuestionKey, QuestionState))> {
        let Some((_, answer)) = self.values.get_mut(&id) else {
            return Err((
                ConnectionError::Protocol(format!(
                    "takeFromOtherQuestion references unknown answer {id}"
                )),
                waiter,
            ));
        };
        if !answer.redirect_results || answer.redirect_waiter.is_some() {
            return Err((
                ConnectionError::Protocol(format!(
                    "takeFromOtherQuestion references unavailable answer {id}"
                )),
                waiter,
            ));
        }
        answer.redirect_waiter = Some(waiter);
        Ok(())
    }

    fn can_wait_for_redirect(&self, id: u32) -> bool {
        self.values
            .get(&id)
            .is_some_and(|(_, answer)| answer.redirect_results && answer.redirect_waiter.is_none())
    }

    fn drain_redirect_waiters(&mut self) -> Vec<QuestionState> {
        self.values
            .values_mut()
            .filter_map(|(_, answer)| answer.redirect_waiter.take().map(|(_, state)| state))
            .collect()
    }

    fn record_result_exports(&mut self, key: AnswerKey, exports: Vec<u32>) -> bool {
        let Some((generation, value)) = self.values.get_mut(&key.id) else {
            return false;
        };
        if *generation != key.generation || value.phase != AnswerPhase::Returned {
            return false;
        }
        value.result_exports = exports;
        true
    }

    fn param_imports(&self, key: AnswerKey) -> Option<Vec<u32>> {
        self.values.get(&key.id).and_then(|(generation, value)| {
            (*generation == key.generation).then(|| value.param_imports.clone())
        })
    }

    fn finish_state(&self, key: AnswerKey) -> Option<bool> {
        self.values.get(&key.id).and_then(|(generation, value)| {
            (*generation == key.generation && value.finish_received)
                .then_some(value.finish_release_result_caps)
        })
    }

    fn queue_pipeline_call(
        &mut self,
        source_id: u32,
        call: PendingPipelineCall,
    ) -> Result<Option<PipelineSnapshot>, ConnectionError> {
        let (_, source) = self.values.get_mut(&source_id).ok_or_else(|| {
            ConnectionError::Protocol(format!(
                "promised answer references unknown answer {source_id}"
            ))
        })?;
        if let Some(pipeline) = &source.pipeline {
            Ok(Some(pipeline.clone()))
        } else if source.phase == AnswerPhase::Returned {
            Err(ConnectionError::Protocol(format!(
                "answer {source_id} returned without a capability pipeline"
            )))
        } else {
            source.queued_pipeline_calls.push_back(call);
            Ok(None)
        }
    }

    fn resolve_pipeline(
        &mut self,
        key: AnswerKey,
        pipeline: PipelineSnapshot,
    ) -> Option<VecDeque<PendingPipelineCall>> {
        let (generation, answer) = self.values.get_mut(&key.id)?;
        if *generation != key.generation {
            return None;
        }
        answer.pipeline = Some(pipeline);
        Some(core::mem::take(&mut answer.queued_pipeline_calls))
    }

    fn fail_pipeline(&mut self, key: AnswerKey) -> Option<VecDeque<PendingPipelineCall>> {
        let (generation, answer) = self.values.get_mut(&key.id)?;
        if *generation != key.generation {
            return None;
        }
        Some(core::mem::take(&mut answer.queued_pipeline_calls))
    }

    fn remove_id(&mut self, id: u32) -> Option<AnswerState> {
        let (_, value) = self.values.remove(&id)?;
        self.incoming_bytes = self
            .incoming_bytes
            .checked_sub(value.incoming_bytes)
            .expect("answer byte accounting invariant");
        Some(value)
    }

    fn finish(&mut self, id: u32, release_result_caps: bool) -> Option<AnswerState> {
        for (_, value) in self.values.values_mut() {
            value
                .queued_pipeline_calls
                .retain(|pending| pending.answer.id != id);
        }
        let (_, value) = self.values.get_mut(&id)?;
        value.finish_received = true;
        value.finish_release_result_caps = release_result_caps;
        let keep_for_pipeline = !value.queued_pipeline_calls.is_empty();
        let keep_for_tail = value.tail_question.is_some() && !value.tail_results_elsewhere;
        if keep_for_pipeline || keep_for_tail {
            None
        } else if value.phase == AnswerPhase::InProgress && !value.cancellation.cancel_if_allowed()
        {
            // The application opted out after dispatch. Keep the entry until
            // its completion arrives, but suppress the Return because Finish
            // already released the caller's interest.
            None
        } else {
            self.remove_id(id)
        }
    }

    fn clear(&mut self) {
        for (_, answer) in self.values.values() {
            answer.cancellation.force_cancel();
        }
        self.values.clear();
        self.incoming_bytes = 0;
    }
}

fn _assert_public_send_traits() {
    fn assert_send<T: Send>() {}
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ConnectionHandle>();
    assert_send::<ConnectionActor>();
    assert_send::<QuestionFuture>();
    assert_send::<CompletionToken>();
    assert_send::<CancellationSignal>();
    assert_send::<LocalCompletionToken>();
    assert_send::<PromiseResolver>();
    assert_send_sync::<HostedCapability>();
    assert_send_sync::<OutgoingCapability>();
    assert_send::<CapabilityTables>();
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;
    use crate::{AttachedResource, read_protocol_message};
    use std::sync::mpsc;
    use std::task::Waker;

    use capnp_message::{ExclusiveArena, ReaderLimits};

    fn message(value: u64) -> Arc<OwnedMessage> {
        let mut arena = ExclusiveArena::new(2, 16).expect("arena");
        arena
            .init_root_struct(1, 0)
            .expect("root")
            .set_u64(0, value, 0)
            .expect("value");
        OwnedMessage::new(arena.into_segments(), ReaderLimits::default()).expect("owned")
    }

    fn capability_result(index: u32) -> Arc<OwnedMessage> {
        let mut arena = ExclusiveArena::new(2, 16).expect("arena");
        arena
            .init_root_struct(0, 1)
            .expect("root")
            .set_capability(0, index)
            .expect("capability");
        OwnedMessage::new(arena.into_segments(), ReaderLimits::default()).expect("owned")
    }

    fn next(actor: &mut ConnectionActor) -> Poll<Option<ActorEffect>> {
        let mut context = Context::from_waker(Waker::noop());
        actor.poll_next_effect(&mut context)
    }

    fn send_to(effect: ActorEffect, peer: &ConnectionHandle) {
        match effect {
            ActorEffect::Send(message) => peer.receive(message),
            ActorEffect::SendWithResources { message, resources } => {
                peer.receive_with_resources(message, resources)
            }
            _ => panic!("send effect"),
        }
        .expect("peer accepts wire message");
    }

    fn poll_future(future: &mut QuestionFuture) -> Poll<Result<ReturnPayload, ConnectionError>> {
        let mut context = Context::from_waker(Waker::noop());
        Pin::new(future).poll(&mut context)
    }

    #[test]
    fn promise_resolution_freezes_the_original_remote_route() {
        let (handle, mut actor) =
            ConnectionActor::new(ActorLimits::default(), ProtocolLimits::default());
        let (promise, resolver) = handle.new_promise().expect("promise");
        assert_eq!(
            actor
                .describe_outgoing_capabilities(&[OutgoingCapability::Promise(promise)])
                .expect("export promise")
                .0,
            vec![CapDescriptor::SenderPromise(0)]
        );
        assert_eq!(
            actor.capabilities.receive(&CapDescriptor::SenderPromise(7)),
            Ok(ReceivedCapability::PromiseImported(7))
        );
        actor
            .promise_imports
            .insert(7, ImportPromiseState::Unresolved);

        resolver.resolve_to_import(7).expect("resolve queued");
        let Poll::Ready(Some(ActorEffect::Send(resolve))) = next(&mut actor) else {
            panic!("resolve message")
        };
        assert!(matches!(
            read_protocol_message(resolve).expect("decode resolve"),
            ProtocolMessage::Resolve(ResolveMessage {
                promise_id: 0,
                resolution: PromiseResolution::Cap(CapDescriptor::ReceiverHosted(7)),
            })
        ));

        handle
            .receive(
                encode_resolve(
                    7,
                    &PromiseResolution::Cap(CapDescriptor::SenderHosted(8)),
                    ProtocolLimits::default(),
                )
                .expect("resolve chained promise"),
            )
            .expect("incoming resolve queued");
        assert!(matches!(next(&mut actor), Poll::Pending));

        handle
            .receive(
                crate::encode_call(
                    41,
                    CallTarget::ImportedCap(0),
                    0xabc,
                    3,
                    &message(123),
                    ProtocolLimits::default(),
                )
                .expect("promise call"),
            )
            .expect("call queued");
        let Poll::Ready(Some(ActorEffect::Send(forwarded))) = next(&mut actor) else {
            panic!("forwarded tail call")
        };
        let ProtocolMessage::Call(forwarded) =
            read_protocol_message(forwarded).expect("decode forwarded call")
        else {
            panic!("call")
        };
        assert_eq!(forwarded.target, CallTarget::ImportedCap(7));
        assert_eq!(forwarded.send_results_to, SendResultsTo::Yourself);
        let Poll::Ready(Some(ActorEffect::Send(returned))) = next(&mut actor) else {
            panic!("tail-routing return")
        };
        assert!(matches!(
            read_protocol_message(returned).expect("decode return"),
            ProtocolMessage::Return(crate::ReturnMessage {
                payload: ReturnPayload::TakeFromOtherQuestion(_),
                ..
            })
        ));
    }

    #[test]
    fn release_before_resolve_suppresses_resolve_and_releases_the_id_tombstone() {
        let (handle, mut actor) =
            ConnectionActor::new(ActorLimits::default(), ProtocolLimits::default());
        let (promise, resolver) = handle.new_promise().expect("promise");
        actor
            .describe_outgoing_capabilities(&[OutgoingCapability::Promise(promise)])
            .expect("export promise");
        handle
            .receive(encode_release(0, 1, ProtocolLimits::default()).expect("release message"))
            .expect("release queued");
        assert!(matches!(next(&mut actor), Poll::Pending));
        assert!(!actor.capabilities.contains_export(0));

        resolver
            .resolve_to_hosted(HostedCapability::new().expect("late hosted resolution"))
            .expect("late resolution queued");
        assert!(matches!(next(&mut actor), Poll::Pending));
        assert!(!actor.terminal);
        assert!(!actor.promise_exports.contains_key(&0));
        assert_eq!(actor.stats().active_exports, 0);

        let hosted = HostedCapability::new().expect("hosted");
        assert_eq!(
            actor
                .describe_outgoing_capabilities(&[OutgoingCapability::Hosted(hosted)])
                .expect("freed ID reusable")
                .0,
            vec![CapDescriptor::SenderHosted(0)]
        );
    }

    #[test]
    fn loopback_resolution_queues_calls_until_receiver_disembargo() {
        let (handle, mut actor) =
            ConnectionActor::new(ActorLimits::default(), ProtocolLimits::default());
        let hosted = HostedCapability::new().expect("hosted");
        assert_eq!(
            actor
                .describe_outgoing_capabilities(&[OutgoingCapability::Hosted(hosted.clone())])
                .expect("export hosted")
                .0,
            vec![CapDescriptor::SenderHosted(0)]
        );
        assert_eq!(
            actor.capabilities.receive(&CapDescriptor::SenderPromise(7)),
            Ok(ReceivedCapability::PromiseImported(7))
        );
        actor
            .promise_imports
            .insert(7, ImportPromiseState::Unresolved);

        handle
            .receive(
                encode_resolve(
                    7,
                    &PromiseResolution::Cap(CapDescriptor::ReceiverHosted(0)),
                    ProtocolLimits::default(),
                )
                .expect("loopback resolve"),
            )
            .expect("resolve queued");
        let Poll::Ready(Some(ActorEffect::Send(disembargo))) = next(&mut actor) else {
            panic!("sender disembargo")
        };
        assert!(matches!(
            read_protocol_message(disembargo).expect("decode disembargo"),
            ProtocolMessage::Disembargo(DisembargoMessage {
                target: CallTarget::ImportedCap(7),
                context: DisembargoContext::SenderLoopback(0),
            })
        ));

        let mut future = handle
            .call_imported(7, 0xabc, 5, message(99), Vec::new())
            .expect("loopback call queued");
        assert!(matches!(next(&mut actor), Poll::Pending));
        assert!(matches!(poll_future(&mut future), Poll::Pending));
        assert_eq!(actor.stats().queued_embargo_calls, 1);

        handle
            .receive(
                encode_disembargo(
                    &CallTarget::ImportedCap(7),
                    DisembargoContext::ReceiverLoopback(0),
                    ProtocolLimits::default(),
                )
                .expect("receiver disembargo"),
            )
            .expect("receiver disembargo queued");
        let Poll::Ready(Some(ActorEffect::DispatchLocal {
            request:
                IncomingRequest::Call {
                    target: IncomingCallTarget::Hosted(target),
                    method_id: 5,
                    ..
                },
            completion,
        })) = next(&mut actor)
        else {
            panic!("local dispatch after receiver disembargo")
        };
        assert_eq!(target, hosted);
        completion
            .complete(HandlerResult::Results(message(100)))
            .expect("local completion queued");
        assert!(matches!(next(&mut actor), Poll::Pending));
        assert!(matches!(
            poll_future(&mut future),
            Poll::Ready(Ok(ReturnPayload::LocalResults { .. }))
        ));
        assert_eq!(actor.stats().active_embargoes, 0);
        assert_eq!(actor.stats().queued_embargo_calls, 0);
    }

    #[test]
    fn embargoed_call_limit_rejects_without_growing_the_queue() {
        let limits = ActorLimits {
            max_embargoed_calls: 0,
            ..ActorLimits::default()
        };
        let (handle, mut actor) = ConnectionActor::new(limits, ProtocolLimits::default());
        let hosted = HostedCapability::new().expect("hosted");
        actor.promise_imports.insert(
            7,
            ImportPromiseState::Loopback {
                capability: hosted.clone(),
                embargo_id: 0,
            },
        );
        actor.embargoes.insert(
            0,
            EmbargoState {
                promise_id: 7,
                capability: hosted,
                queued_calls: VecDeque::new(),
            },
        );
        let mut future = handle
            .call_imported(7, 1, 2, message(3), Vec::new())
            .expect("mailbox accepts call");
        assert!(matches!(next(&mut actor), Poll::Pending));
        assert!(matches!(
            poll_future(&mut future),
            Poll::Ready(Err(ConnectionError::Overloaded { capacity: 0 }))
        ));
        assert_eq!(actor.stats().queued_embargo_calls, 0);
    }

    #[test]
    fn calls_received_before_resolution_route_before_the_resolve_message() {
        let (handle, mut actor) =
            ConnectionActor::new(ActorLimits::default(), ProtocolLimits::default());
        let (promise, resolver) = handle.new_promise().expect("promise");
        actor
            .describe_outgoing_capabilities(&[OutgoingCapability::Promise(promise)])
            .expect("export promise");
        handle
            .receive(
                crate::encode_call(
                    55,
                    CallTarget::ImportedCap(0),
                    0xabc,
                    9,
                    &message(1),
                    ProtocolLimits::default(),
                )
                .expect("call before resolve"),
            )
            .expect("call queued");
        assert!(matches!(next(&mut actor), Poll::Pending));

        let hosted = HostedCapability::new().expect("hosted");
        resolver
            .resolve_to_hosted(hosted.clone())
            .expect("resolution queued");
        let Poll::Ready(Some(ActorEffect::Dispatch {
            request:
                IncomingRequest::Call {
                    target: IncomingCallTarget::Hosted(target),
                    method_id: 9,
                    ..
                },
            ..
        })) = next(&mut actor)
        else {
            panic!("queued call dispatches first")
        };
        assert_eq!(target, hosted);
        let Poll::Ready(Some(ActorEffect::Send(resolve))) = next(&mut actor) else {
            panic!("resolve follows prior call dispatch")
        };
        assert!(matches!(
            read_protocol_message(resolve).expect("decode resolve"),
            ProtocolMessage::Resolve(ResolveMessage {
                promise_id: 0,
                resolution: PromiseResolution::Cap(CapDescriptor::SenderHosted(1)),
            })
        ));
    }

    #[test]
    fn resolve_for_released_import_releases_its_resolution_capability() {
        let (_handle, mut actor) =
            ConnectionActor::new(ActorLimits::default(), ProtocolLimits::default());
        assert_eq!(
            actor.capabilities.receive(&CapDescriptor::SenderPromise(7)),
            Ok(ReceivedCapability::PromiseImported(7))
        );
        actor
            .promise_imports
            .insert(7, ImportPromiseState::Unresolved);
        actor.release_import(7, 1).expect("release promise import");
        let Poll::Ready(Some(ActorEffect::Send(release))) = next(&mut actor) else {
            panic!("promise release")
        };
        assert!(matches!(
            read_protocol_message(release).expect("decode release"),
            ProtocolMessage::Release(crate::ReleaseMessage {
                id: 7,
                reference_count: 1
            })
        ));

        actor
            .handle
            .receive(
                encode_resolve(
                    7,
                    &PromiseResolution::Cap(CapDescriptor::SenderHosted(8)),
                    ProtocolLimits::default(),
                )
                .expect("late resolve"),
            )
            .expect("late resolve queued");
        let Poll::Ready(Some(ActorEffect::Send(release))) = next(&mut actor) else {
            panic!("resolution capability release")
        };
        assert!(matches!(
            read_protocol_message(release).expect("decode release"),
            ProtocolMessage::Release(crate::ReleaseMessage {
                id: 8,
                reference_count: 1
            })
        ));
        assert!(!actor.terminal);
    }

    #[test]
    fn broken_promise_short_circuits_calls_and_duplicate_resolve_is_fatal() {
        let (handle, mut actor) =
            ConnectionActor::new(ActorLimits::default(), ProtocolLimits::default());
        actor
            .capabilities
            .receive(&CapDescriptor::SenderPromise(7))
            .expect("promise import");
        actor
            .promise_imports
            .insert(7, ImportPromiseState::Unresolved);
        let broken = RpcException::new("broken promise", ExceptionType::Failed);
        handle
            .receive(
                encode_resolve(
                    7,
                    &PromiseResolution::Exception(broken.clone()),
                    ProtocolLimits::default(),
                )
                .expect("broken resolve"),
            )
            .expect("broken resolve queued");
        assert!(matches!(next(&mut actor), Poll::Pending));
        let mut future = handle
            .call_imported(7, 1, 2, message(3), Vec::new())
            .expect("broken call accepted");
        assert!(matches!(next(&mut actor), Poll::Pending));
        assert!(matches!(
            poll_future(&mut future),
            Poll::Ready(Ok(ReturnPayload::Exception(exception))) if exception == broken
        ));

        handle
            .receive(
                encode_resolve(
                    7,
                    &PromiseResolution::Exception(RpcException::new(
                        "duplicate",
                        ExceptionType::Failed,
                    )),
                    ProtocolLimits::default(),
                )
                .expect("duplicate resolve"),
            )
            .expect("duplicate resolve queued");
        assert!(matches!(
            next(&mut actor),
            Poll::Ready(Some(ActorEffect::Send(_)))
        ));
        assert!(matches!(
            next(&mut actor),
            Poll::Ready(Some(ActorEffect::CloseTransport))
        ));
        assert!(actor.terminal);
    }

    #[test]
    fn concurrent_handlers_finish_out_of_order_and_tables_reuse_safely() {
        let (left_handle, mut left) =
            ConnectionActor::new(ActorLimits::default(), ProtocolLimits::default());
        let (right_handle, mut right) =
            ConnectionActor::new(ActorLimits::default(), ProtocolLimits::default());

        let mut bootstrap = left_handle.bootstrap().expect("bootstrap queued");
        let target = bootstrap.target();
        let Poll::Ready(Some(bootstrap_wire)) = next(&mut left) else {
            panic!("bootstrap wire")
        };
        let first_key = target.key().expect("actor assigned ID");
        assert_eq!(
            first_key,
            QuestionKey {
                id: 0,
                generation: 0
            }
        );
        send_to(bootstrap_wire, &right_handle);
        let Poll::Ready(Some(ActorEffect::Dispatch {
            request: IncomingRequest::Bootstrap,
            completion: bootstrap_completion,
        })) = next(&mut right)
        else {
            panic!("bootstrap dispatch")
        };

        let mut call = left_handle
            .call(&target, 0xfeed, 4, message(11))
            .expect("call queued");
        let Poll::Ready(Some(call_wire)) = next(&mut left) else {
            panic!("call wire")
        };
        send_to(call_wire, &right_handle);
        let Poll::Ready(Some(ActorEffect::Dispatch {
            request:
                IncomingRequest::Call {
                    interface_id: 0xfeed,
                    method_id: 4,
                    ..
                },
            completion: call_completion,
        })) = next(&mut right)
        else {
            panic!("call dispatch")
        };

        let (call_done, release_bootstrap) = mpsc::sync_channel(0);
        std::thread::scope(|scope| {
            scope.spawn(move || {
                call_completion
                    .complete(HandlerResult::Results(message(22)))
                    .expect("call completion queued");
                call_done.send(()).expect("release bootstrap handler");
            });
            scope.spawn(move || {
                release_bootstrap.recv().expect("call completed first");
                bootstrap_completion
                    .complete(HandlerResult::Results(message(33)))
                    .expect("bootstrap completion queued");
            });
        });
        let Poll::Ready(Some(call_return)) = next(&mut right) else {
            panic!("call return")
        };
        send_to(call_return, &left_handle);
        let Poll::Ready(Some(call_finish)) = next(&mut left) else {
            panic!("call finish")
        };
        assert!(matches!(
            poll_future(&mut call),
            Poll::Ready(Ok(ReturnPayload::Results(_)))
        ));
        send_to(call_finish, &right_handle);
        let Poll::Ready(Some(bootstrap_return)) = next(&mut right) else {
            panic!("bootstrap return")
        };
        send_to(bootstrap_return, &left_handle);
        let Poll::Ready(Some(bootstrap_finish)) = next(&mut left) else {
            panic!("bootstrap finish")
        };
        assert!(matches!(
            poll_future(&mut bootstrap),
            Poll::Ready(Ok(ReturnPayload::Results(_)))
        ));
        send_to(bootstrap_finish, &right_handle);
        assert!(matches!(next(&mut right), Poll::Pending));
        assert_eq!(left.stats().active_questions, 0);
        assert_eq!(right.stats().active_answers, 0);
        assert_eq!(right.stats().completed_handlers, 2);

        let reused = left_handle.bootstrap().expect("reused bootstrap queued");
        let reused_target = reused.target();
        assert!(matches!(
            next(&mut left),
            Poll::Ready(Some(ActorEffect::Send(_)))
        ));
        assert_eq!(
            reused_target.key(),
            Some(QuestionKey {
                id: 0,
                generation: 1
            })
        );
        assert_eq!(left.stats().reused_question_ids, 1);
    }

    #[test]
    fn mailbox_overload_is_immediate_and_shutdown_wakes_waiters() {
        let limits = ActorLimits {
            mailbox_capacity: 1,
            ..ActorLimits::default()
        };
        let (handle, mut actor) = ConnectionActor::new(limits, ProtocolLimits::default());
        let mut pending = handle.bootstrap().expect("first fits");
        assert!(matches!(
            handle.bootstrap(),
            Err(ConnectionError::Overloaded { capacity: 1 })
        ));
        assert!(matches!(
            next(&mut actor),
            Poll::Ready(Some(ActorEffect::Send(_)))
        ));
        handle.shutdown().expect("shutdown fits");
        handle.shutdown().expect("duplicate shutdown is idempotent");
        assert!(matches!(
            next(&mut actor),
            Poll::Ready(Some(ActorEffect::CloseTransport))
        ));
        assert!(matches!(
            poll_future(&mut pending),
            Poll::Ready(Err(ConnectionError::Disconnected))
        ));
        assert!(matches!(
            handle.bootstrap(),
            Err(ConnectionError::Disconnected)
        ));
    }

    #[test]
    fn hosted_callback_and_batched_release_use_actor_owned_tables() {
        let (owner_handle, mut owner) =
            ConnectionActor::new(ActorLimits::default(), ProtocolLimits::default());
        let (peer_handle, mut peer) =
            ConnectionActor::new(ActorLimits::default(), ProtocolLimits::default());
        let hosted = HostedCapability::new().expect("hosted identity");
        let descriptors = owner
            .capabilities
            .describe_all(&[
                OutgoingCapability::Hosted(hosted.clone()),
                OutgoingCapability::Hosted(hosted.clone()),
            ])
            .expect("descriptors");
        assert_eq!(owner.stats().active_exports, 1);
        assert_eq!(owner.stats().export_references, 2);
        peer.receive_cap_table(&descriptors).expect("imports");
        assert_eq!(peer.stats().active_imports, 1);
        assert_eq!(peer.stats().import_references, 2);

        peer.release_import(0, 2).expect("release scheduled");
        let Poll::Ready(Some(release)) = next(&mut peer) else {
            panic!("release wire")
        };
        send_to(release, &owner_handle);
        assert!(matches!(next(&mut owner), Poll::Pending));
        assert_eq!(owner.stats().active_exports, 0);
        assert_eq!(owner.stats().export_references, 0);

        let descriptor = owner
            .capabilities
            .describe_all(&[OutgoingCapability::Hosted(hosted.clone())])
            .expect("re-export");
        assert_eq!(descriptor, vec![CapDescriptor::SenderHosted(0)]);
        peer.receive_cap_table(&descriptor)
            .expect("callback import");
        let callback = HostedCapability::new().expect("callback identity");
        let mut call = peer_handle
            .call_imported(
                0,
                0xfeed,
                7,
                message(11),
                vec![OutgoingCapability::Hosted(callback)],
            )
            .expect("callback call queued");
        let Poll::Ready(Some(call_wire)) = next(&mut peer) else {
            panic!("callback call wire")
        };
        assert_eq!(peer.stats().active_exports, 1);
        send_to(call_wire, &owner_handle);
        let Poll::Ready(Some(ActorEffect::Dispatch {
            request:
                IncomingRequest::Call {
                    target: IncomingCallTarget::Hosted(actual),
                    ..
                },
            completion,
        })) = next(&mut owner)
        else {
            panic!("hosted callback dispatch")
        };
        assert_eq!(actual, hosted);
        assert_eq!(owner.stats().active_imports, 1);
        completion
            .complete(HandlerResult::Results(message(12)))
            .expect("callback completion");
        let Poll::Ready(Some(return_wire)) = next(&mut owner) else {
            panic!("callback return")
        };
        assert_eq!(owner.stats().active_imports, 0);
        send_to(return_wire, &peer_handle);
        let Poll::Ready(Some(finish_wire)) = next(&mut peer) else {
            panic!("callback finish")
        };
        assert!(matches!(
            poll_future(&mut call),
            Poll::Ready(Ok(ReturnPayload::Results(_)))
        ));
        assert_eq!(peer.stats().active_exports, 0);
        send_to(finish_wire, &owner_handle);
        assert!(matches!(next(&mut owner), Poll::Pending));

        peer.transition_terminal(ConnectionError::Disconnected, false);
        owner.transition_terminal(ConnectionError::Disconnected, false);
        assert_eq!(peer.stats().active_imports, 0);
        assert_eq!(owner.stats().active_exports, 0);
    }

    #[test]
    fn actor_sends_and_binds_attached_capability_resources() {
        let (client_handle, mut client) =
            ConnectionActor::new(ActorLimits::default(), ProtocolLimits::default());
        let (server_handle, mut server) =
            ConnectionActor::new(ActorLimits::default(), ProtocolLimits::default());
        let mut request = client_handle.bootstrap().expect("bootstrap");
        let Poll::Ready(Some(wire)) = next(&mut client) else {
            panic!("bootstrap wire")
        };
        send_to(wire, &server_handle);
        let Poll::Ready(Some(ActorEffect::Dispatch { completion, .. })) = next(&mut server) else {
            panic!("bootstrap dispatch")
        };

        let hosted = HostedCapability::new().expect("result capability");
        let attached = AttachedResource::new(73_u32, 4);
        completion
            .complete_with_capabilities(
                capability_result(0),
                vec![OutgoingCapability::Hosted(hosted).with_attachment(attached.clone())],
            )
            .expect("complete with attachment");
        let Poll::Ready(Some(effect @ ActorEffect::SendWithResources { .. })) = next(&mut server)
        else {
            panic!("resource-bearing return")
        };
        if let ActorEffect::SendWithResources { message, resources } = &effect {
            assert_eq!(resources.len(), 1);
            let ProtocolMessage::Return(returned) =
                read_protocol_message(Arc::clone(message)).expect("return reads")
            else {
                panic!("return")
            };
            let ReturnPayload::Results(payload) = returned.payload else {
                panic!("results")
            };
            assert_eq!(payload.cap_table[0].resource_index(), Some(0));
        }
        send_to(effect, &client_handle);
        let Poll::Ready(Some(ActorEffect::Send(_finish))) = next(&mut client) else {
            panic!("finish")
        };
        assert!(matches!(
            poll_future(&mut request),
            Poll::Ready(Ok(ReturnPayload::Results(_)))
        ));
        let imported = client
            .capabilities
            .import_attachment(0)
            .expect("import attachment");
        assert!(imported.same_identity(&attached));
        assert_eq!(imported.with::<u32, _>(|value| *value), Some(73));
    }

    #[test]
    fn promise_pipeline_chains_and_diamonds_dispatch_before_source_return() {
        let (client_handle, mut client) =
            ConnectionActor::new(ActorLimits::default(), ProtocolLimits::default());
        let (server_handle, mut server) =
            ConnectionActor::new(ActorLimits::default(), ProtocolLimits::default());

        let bootstrap = client_handle.bootstrap().expect("bootstrap");
        let root_pipeline = bootstrap.target().pointer_field(0);
        let Poll::Ready(Some(bootstrap_wire)) = next(&mut client) else {
            panic!("bootstrap wire")
        };
        send_to(bootstrap_wire, &server_handle);
        let Poll::Ready(Some(ActorEffect::Dispatch {
            request: IncomingRequest::Bootstrap,
            completion: root_completion,
        })) = next(&mut server)
        else {
            panic!("bootstrap dispatch")
        };

        let first = client_handle
            .call(&root_pipeline, 1, 1, message(1))
            .expect("first pipeline call");
        let first_pipeline = first.target().pointer_field(0);
        let Poll::Ready(Some(first_wire)) = next(&mut client) else {
            panic!("first pipeline wire")
        };
        let _second = client_handle
            .call(&root_pipeline, 1, 2, message(2))
            .expect("diamond pipeline call");
        let Poll::Ready(Some(second_wire)) = next(&mut client) else {
            panic!("second pipeline wire")
        };
        let _chain = client_handle
            .call(&first_pipeline, 1, 3, message(3))
            .expect("chain pipeline call");
        let Poll::Ready(Some(chain_wire)) = next(&mut client) else {
            panic!("chain pipeline wire")
        };

        send_to(first_wire, &server_handle);
        assert!(matches!(next(&mut server), Poll::Pending));
        send_to(second_wire, &server_handle);
        assert!(matches!(next(&mut server), Poll::Pending));
        send_to(chain_wire, &server_handle);
        assert!(matches!(next(&mut server), Poll::Pending));

        let first_target = HostedCapability::new().expect("first target");
        root_completion
            .complete_with_capabilities(
                capability_result(0),
                vec![OutgoingCapability::Hosted(first_target.clone())],
            )
            .expect("root completes");

        let Poll::Ready(Some(ActorEffect::Dispatch {
            request:
                IncomingRequest::Call {
                    target: IncomingCallTarget::Hosted(first_actual),
                    method_id: 1,
                    ..
                },
            completion: first_completion,
        })) = next(&mut server)
        else {
            panic!("first queued call dispatches")
        };
        assert_eq!(first_actual, first_target);
        let Poll::Ready(Some(ActorEffect::Dispatch {
            request:
                IncomingRequest::Call {
                    target: IncomingCallTarget::Hosted(second_actual),
                    method_id: 2,
                    ..
                },
            completion: second_completion,
        })) = next(&mut server)
        else {
            panic!("diamond queued call dispatches")
        };
        assert_eq!(second_actual, first_target);

        let Poll::Ready(Some(ActorEffect::Send(_root_return))) = next(&mut server) else {
            panic!("root return follows local pipeline dispatch")
        };
        let chain_target = HostedCapability::new().expect("chain target");
        first_completion
            .complete_with_capabilities(
                capability_result(0),
                vec![OutgoingCapability::Hosted(chain_target.clone())],
            )
            .expect("first completes");
        let Poll::Ready(Some(ActorEffect::Dispatch {
            request:
                IncomingRequest::Call {
                    target: IncomingCallTarget::Hosted(chain_actual),
                    method_id: 3,
                    ..
                },
            completion: chain_completion,
        })) = next(&mut server)
        else {
            panic!("chained call dispatches")
        };
        assert_eq!(chain_actual, chain_target);

        second_completion
            .complete(HandlerResult::Results(message(22)))
            .expect("second completes");
        chain_completion
            .complete(HandlerResult::Results(message(33)))
            .expect("chain completes");
    }

    #[test]
    fn level_one_tail_call_routes_results_without_a_proxy_return_payload() {
        let (alice_handle, mut alice) =
            ConnectionActor::new(ActorLimits::default(), ProtocolLimits::default());
        let (bob_handle, mut bob) =
            ConnectionActor::new(ActorLimits::default(), ProtocolLimits::default());

        let carol = HostedCapability::new().expect("Carol");
        let carol_descriptor = alice
            .capabilities
            .describe_all(&[OutgoingCapability::Hosted(carol.clone())])
            .expect("Alice exports Carol");
        bob.receive_cap_table(&carol_descriptor)
            .expect("Bob imports Carol");

        let mut original = alice_handle.bootstrap().expect("original call");
        let Poll::Ready(Some(original_wire)) = next(&mut alice) else {
            panic!("original wire")
        };
        send_to(original_wire, &bob_handle);
        let Poll::Ready(Some(ActorEffect::Dispatch {
            request: IncomingRequest::Bootstrap,
            completion: original_completion,
        })) = next(&mut bob)
        else {
            panic!("original dispatch")
        };
        original_completion
            .tail_call_imported(0, 7, 8, message(41), Vec::new())
            .expect("tail call queued");

        let Poll::Ready(Some(tail_call_wire)) = next(&mut bob) else {
            panic!("tail call precedes routing return")
        };
        let ProtocolMessage::Call(tail_call) =
            read_protocol_message(effect_message(&tail_call_wire)).expect("tail call reads")
        else {
            panic!("tail Call")
        };
        assert_eq!(tail_call.send_results_to, SendResultsTo::Yourself);
        let Poll::Ready(Some(routing_return_wire)) = next(&mut bob) else {
            panic!("routing return")
        };

        send_to(tail_call_wire, &alice_handle);
        let Poll::Ready(Some(ActorEffect::Dispatch {
            request:
                IncomingRequest::Call {
                    target: IncomingCallTarget::Hosted(actual),
                    ..
                },
            completion: tail_completion,
        })) = next(&mut alice)
        else {
            panic!("tail target dispatch")
        };
        assert_eq!(actual, carol);
        send_to(routing_return_wire, &alice_handle);
        assert!(matches!(next(&mut alice), Poll::Pending));
        assert!(matches!(poll_future(&mut original), Poll::Pending));
        assert_eq!(alice.stats().active_questions, 1);
        let local_result_capability = HostedCapability::new().expect("local result capability");
        tail_completion
            .complete_with_capabilities(
                message(99),
                vec![OutgoingCapability::Hosted(local_result_capability.clone())],
            )
            .expect("tail target completes");
        let Poll::Ready(Some(results_elsewhere_wire)) = next(&mut alice) else {
            panic!("resultsSentElsewhere")
        };
        let Poll::Ready(Some(original_finish)) = next(&mut alice) else {
            panic!("original finish")
        };
        let Poll::Ready(Ok(ReturnPayload::LocalResults { capabilities, .. })) =
            poll_future(&mut original)
        else {
            panic!("local tail result")
        };
        assert_eq!(
            capabilities,
            vec![OutgoingCapability::Hosted(local_result_capability)]
        );

        send_to(results_elsewhere_wire, &bob_handle);
        assert!(matches!(next(&mut bob), Poll::Pending));
        send_to(original_finish, &bob_handle);
        let Poll::Ready(Some(tail_finish)) = next(&mut bob) else {
            panic!("tail finish follows original finish")
        };
        send_to(tail_finish, &alice_handle);
        assert!(matches!(next(&mut bob), Poll::Pending));
        assert!(matches!(next(&mut alice), Poll::Pending));
    }

    #[test]
    fn finish_keeps_only_the_pipeline_work_that_still_depends_on_an_answer() {
        let (client_handle, mut client) =
            ConnectionActor::new(ActorLimits::default(), ProtocolLimits::default());
        let (server_handle, mut server) =
            ConnectionActor::new(ActorLimits::default(), ProtocolLimits::default());
        let bootstrap = client_handle.bootstrap().expect("bootstrap");
        let pipeline = bootstrap.target().pointer_field(0);
        let Poll::Ready(Some(bootstrap_wire)) = next(&mut client) else {
            panic!("bootstrap wire")
        };
        send_to(bootstrap_wire, &server_handle);
        let Poll::Ready(Some(ActorEffect::Dispatch {
            completion: root_completion,
            ..
        })) = next(&mut server)
        else {
            panic!("root dispatch")
        };
        let _call = client_handle
            .call(&pipeline, 1, 9, message(9))
            .expect("pipeline call");
        let Poll::Ready(Some(call_wire)) = next(&mut client) else {
            panic!("pipeline wire")
        };
        send_to(call_wire, &server_handle);
        assert!(matches!(next(&mut server), Poll::Pending));

        server_handle
            .receive(crate::encode_finish(0, ProtocolLimits::default()).expect("finish"))
            .expect("finish queued");
        assert!(matches!(next(&mut server), Poll::Pending));
        assert_eq!(server.stats().active_answers, 2);

        let target = HostedCapability::new().expect("pipeline target");
        root_completion
            .complete_with_capabilities(
                capability_result(0),
                vec![OutgoingCapability::Hosted(target.clone())],
            )
            .expect("root completion");
        let Poll::Ready(Some(ActorEffect::Dispatch {
            request:
                IncomingRequest::Call {
                    target: IncomingCallTarget::Hosted(actual),
                    ..
                },
            completion,
        })) = next(&mut server)
        else {
            panic!("dependent call survives finish")
        };
        assert_eq!(actual, target);
        assert_eq!(server.stats().active_answers, 1);
        assert_eq!(server.stats().active_exports, 0);
        completion
            .complete(HandlerResult::Results(message(10)))
            .expect("dependent completion");
        let Poll::Ready(Some(ActorEffect::Send(returned))) = next(&mut server) else {
            panic!("dependent return")
        };
        let ProtocolMessage::Return(returned) = read_protocol_message(returned).expect("return")
        else {
            panic!("Return")
        };
        assert_eq!(returned.answer_id, 1);
    }

    #[test]
    fn finished_queued_pipeline_call_is_not_dispatched_when_source_resolves() {
        let (client_handle, mut client) =
            ConnectionActor::new(ActorLimits::default(), ProtocolLimits::default());
        let (server_handle, mut server) =
            ConnectionActor::new(ActorLimits::default(), ProtocolLimits::default());
        let bootstrap = client_handle.bootstrap().expect("bootstrap");
        let pipeline = bootstrap.target().pointer_field(0);
        let Poll::Ready(Some(bootstrap_wire)) = next(&mut client) else {
            panic!("bootstrap wire")
        };
        send_to(bootstrap_wire, &server_handle);
        let Poll::Ready(Some(ActorEffect::Dispatch {
            completion: root_completion,
            ..
        })) = next(&mut server)
        else {
            panic!("root dispatch")
        };

        let _canceled = client_handle
            .call(&pipeline, 1, 9, message(9))
            .expect("pipeline call");
        let Poll::Ready(Some(call_wire)) = next(&mut client) else {
            panic!("pipeline wire")
        };
        send_to(call_wire, &server_handle);
        assert!(matches!(next(&mut server), Poll::Pending));
        server_handle
            .receive(crate::encode_finish(1, ProtocolLimits::default()).expect("finish"))
            .expect("finish queued");
        assert!(matches!(next(&mut server), Poll::Pending));
        assert_eq!(server.stats().active_answers, 1);

        let target = HostedCapability::new().expect("pipeline target");
        root_completion
            .complete_with_capabilities(
                capability_result(0),
                vec![OutgoingCapability::Hosted(target)],
            )
            .expect("root completion");
        let Poll::Ready(Some(ActorEffect::Send(returned))) = next(&mut server) else {
            panic!("root return")
        };
        let ProtocolMessage::Return(returned) = read_protocol_message(returned).expect("return")
        else {
            panic!("Return")
        };
        assert_eq!(returned.answer_id, 0);
        assert!(matches!(next(&mut server), Poll::Pending));
    }

    #[test]
    fn invalid_pipeline_transform_fails_only_the_dependent_call() {
        let (client_handle, mut client) =
            ConnectionActor::new(ActorLimits::default(), ProtocolLimits::default());
        let (server_handle, mut server) =
            ConnectionActor::new(ActorLimits::default(), ProtocolLimits::default());
        let bootstrap = client_handle.bootstrap().expect("bootstrap");
        let invalid_pipeline = bootstrap.target().pointer_field(0);
        let Poll::Ready(Some(bootstrap_wire)) = next(&mut client) else {
            panic!("bootstrap wire")
        };
        send_to(bootstrap_wire, &server_handle);
        let Poll::Ready(Some(ActorEffect::Dispatch {
            completion: root_completion,
            ..
        })) = next(&mut server)
        else {
            panic!("root dispatch")
        };
        let _dependent = client_handle
            .call(&invalid_pipeline, 1, 9, message(9))
            .expect("pipeline call");
        let Poll::Ready(Some(call_wire)) = next(&mut client) else {
            panic!("pipeline wire")
        };
        send_to(call_wire, &server_handle);
        assert!(matches!(next(&mut server), Poll::Pending));

        root_completion
            .complete(HandlerResult::Results(message(10)))
            .expect("root completion");
        let Poll::Ready(Some(ActorEffect::Send(failed))) = next(&mut server) else {
            panic!("dependent exception")
        };
        let ProtocolMessage::Return(failed) = read_protocol_message(failed).expect("return") else {
            panic!("Return")
        };
        assert_eq!(failed.answer_id, 1);
        assert!(matches!(failed.payload, ReturnPayload::Exception(_)));
        let Poll::Ready(Some(ActorEffect::Send(root))) = next(&mut server) else {
            panic!("root return")
        };
        let ProtocolMessage::Return(root) = read_protocol_message(root).expect("return") else {
            panic!("Return")
        };
        assert_eq!(root.answer_id, 0);
        assert!(matches!(next(&mut server), Poll::Pending));
    }

    fn effect_message(effect: &ActorEffect) -> Arc<OwnedMessage> {
        let ActorEffect::Send(message) = effect else {
            panic!("send effect")
        };
        Arc::clone(message)
    }

    #[test]
    fn finish_before_handler_completion_makes_the_generation_token_stale() {
        let (sender, mut sender_actor) =
            ConnectionActor::new(ActorLimits::default(), ProtocolLimits::default());
        let (receiver, mut receiver_actor) =
            ConnectionActor::new(ActorLimits::default(), ProtocolLimits::default());
        let request = sender.bootstrap().expect("request");
        let Poll::Ready(Some(wire)) = next(&mut sender_actor) else {
            panic!("wire")
        };
        let key = request.target().key().expect("key");
        send_to(wire, &receiver);
        let Poll::Ready(Some(ActorEffect::Dispatch { completion, .. })) = next(&mut receiver_actor)
        else {
            panic!("dispatch")
        };
        receiver
            .receive(crate::encode_finish(key.id, ProtocolLimits::default()).expect("finish"))
            .expect("finish queued");
        assert!(matches!(next(&mut receiver_actor), Poll::Pending));
        completion
            .complete(HandlerResult::Results(message(1)))
            .expect("stale completion still queues safely");
        assert!(matches!(next(&mut receiver_actor), Poll::Pending));
        assert_eq!(receiver_actor.stats().stale_handler_completions, 1);
    }

    #[test]
    fn duplicate_active_answer_aborts_and_closes_exactly_once() {
        let (handle, mut actor) =
            ConnectionActor::new(ActorLimits::default(), ProtocolLimits::default());
        let bootstrap = encode_bootstrap(44, ProtocolLimits::default()).expect("bootstrap");
        handle
            .receive(Arc::clone(&bootstrap))
            .expect("first bootstrap");
        handle.receive(bootstrap).expect("duplicate queued");
        let Poll::Ready(Some(ActorEffect::Dispatch { completion, .. })) = next(&mut actor) else {
            panic!("first dispatch")
        };
        let Poll::Ready(Some(ActorEffect::Send(abort))) = next(&mut actor) else {
            panic!("abort")
        };
        assert!(matches!(
            read_protocol_message(abort),
            Ok(ProtocolMessage::Abort(_))
        ));
        assert!(matches!(
            next(&mut actor),
            Poll::Ready(Some(ActorEffect::CloseTransport))
        ));
        assert!(matches!(next(&mut actor), Poll::Ready(None)));
        assert!(matches!(
            completion.complete(HandlerResult::Canceled),
            Err(ConnectionError::Disconnected)
        ));
    }

    #[test]
    fn incoming_call_byte_quota_rejects_before_dispatch() {
        let call = crate::encode_call(
            7,
            CallTarget::ImportedCap(0),
            1,
            2,
            &message(3),
            ProtocolLimits::default(),
        )
        .expect("call");
        let call_bytes = u64::try_from(message_bytes(&call).expect("message size")).expect("u64");
        let limits = ActorLimits {
            max_incoming_call_bytes: call_bytes - 1,
            ..ActorLimits::default()
        };
        let (handle, mut actor) = ConnectionActor::new(limits, ProtocolLimits::default());
        handle.receive(call).expect("call queued");
        let Poll::Ready(Some(ActorEffect::Send(abort))) = next(&mut actor) else {
            panic!("abort")
        };
        let ProtocolMessage::Abort(exception) =
            read_protocol_message(abort).expect("abort message")
        else {
            panic!("abort protocol message")
        };
        assert!(exception.reason.contains("IncomingCallByteLimit"));
        assert_eq!(actor.stats().active_answers, 0);
        assert_eq!(actor.stats().incoming_call_bytes, 0);
        assert!(matches!(
            next(&mut actor),
            Poll::Ready(Some(ActorEffect::CloseTransport))
        ));
    }

    #[test]
    fn answer_byte_quota_is_transactional_and_released_exactly() {
        let mut answers = AnswerTable::new(2, 8);
        let first = answers
            .insert(0, AnswerKind::Call, Vec::new(), false, 5)
            .expect("first answer");
        assert_eq!(answers.incoming_bytes(), 5);
        assert!(matches!(
            answers.insert(1, AnswerKind::Call, Vec::new(), false, 4),
            Err(ConnectionError::IncomingCallByteLimit {
                requested: 9,
                limit: 8
            })
        ));
        assert_eq!(answers.len(), 1);
        assert_eq!(answers.incoming_bytes(), 5);
        assert!(answers.remove_id(first.id()).is_some());
        assert_eq!(answers.incoming_bytes(), 0);
        answers
            .insert(1, AnswerKind::Call, Vec::new(), false, 8)
            .expect("quota reusable after release");
        assert_eq!(answers.incoming_bytes(), 8);
        answers.clear();
        assert_eq!(answers.incoming_bytes(), 0);
    }

    #[test]
    fn dropping_the_last_question_lease_sends_finish_once() {
        let (handle, mut actor) =
            ConnectionActor::new(ActorLimits::default(), ProtocolLimits::default());
        let response = handle.bootstrap().expect("bootstrap");
        let target = response.target();
        let Poll::Ready(Some(ActorEffect::Send(_bootstrap))) = next(&mut actor) else {
            panic!("bootstrap wire")
        };

        drop(response);
        assert!(matches!(next(&mut actor), Poll::Pending));
        drop(target);
        let Poll::Ready(Some(ActorEffect::Send(finish))) = next(&mut actor) else {
            panic!("finish after last lease")
        };
        assert!(matches!(
            read_protocol_message(finish).expect("finish"),
            ProtocolMessage::Finish(FinishMessage {
                question_id: 0,
                release_result_caps: true,
                require_early_cancellation_workaround: false,
            })
        ));
        assert!(matches!(next(&mut actor), Poll::Pending));
        assert_eq!(actor.stats().active_questions, 1);

        handle
            .receive(
                crate::encode_return(0, &HandlerResult::Canceled, ProtocolLimits::default())
                    .expect("late return"),
            )
            .expect("late return queued");
        assert!(matches!(next(&mut actor), Poll::Pending));
        assert_eq!(actor.stats().active_questions, 0);
    }

    #[test]
    fn finish_cancels_dispatched_work_and_late_completion_is_stale() {
        let (handle, mut actor) =
            ConnectionActor::new(ActorLimits::default(), ProtocolLimits::default());
        handle
            .receive(encode_bootstrap(7, ProtocolLimits::default()).expect("bootstrap"))
            .expect("bootstrap queued");
        let Poll::Ready(Some(ActorEffect::Dispatch { completion, .. })) = next(&mut actor) else {
            panic!("dispatch")
        };
        let cancellation = completion.cancellation();
        assert!(cancellation.is_allowed());

        handle
            .receive(
                crate::encode_finish_with_options(7, true, false, ProtocolLimits::default())
                    .expect("finish"),
            )
            .expect("finish queued");
        assert!(matches!(next(&mut actor), Poll::Pending));
        assert!(cancellation.is_canceled());
        assert!(!completion.disallow_cancellation());
        assert_eq!(actor.stats().active_answers, 0);

        completion
            .complete(HandlerResult::Results(message(1)))
            .expect("late completion queued");
        assert!(matches!(next(&mut actor), Poll::Pending));
        assert_eq!(actor.stats().stale_handler_completions, 1);
    }

    #[test]
    fn explicit_question_cancel_uses_the_same_idempotent_finish_path() {
        let (handle, mut actor) =
            ConnectionActor::new(ActorLimits::default(), ProtocolLimits::default());
        let response = handle.bootstrap().expect("bootstrap");
        assert!(matches!(
            next(&mut actor),
            Poll::Ready(Some(ActorEffect::Send(_)))
        ));
        response.cancel().expect("explicit cancel queued");
        let Poll::Ready(Some(ActorEffect::Send(finish))) = next(&mut actor) else {
            panic!("cancel finish")
        };
        assert!(matches!(
            read_protocol_message(finish).expect("finish"),
            ProtocolMessage::Finish(FinishMessage {
                question_id: 0,
                release_result_caps: true,
                require_early_cancellation_workaround: false,
            })
        ));
        assert!(matches!(next(&mut actor), Poll::Pending));
    }

    #[test]
    fn application_opt_out_runs_to_completion_without_returning_to_finished_caller() {
        let (handle, mut actor) =
            ConnectionActor::new(ActorLimits::default(), ProtocolLimits::default());
        handle
            .receive(encode_bootstrap(8, ProtocolLimits::default()).expect("bootstrap"))
            .expect("bootstrap queued");
        let Poll::Ready(Some(ActorEffect::Dispatch { completion, .. })) = next(&mut actor) else {
            panic!("dispatch")
        };
        let cancellation = completion.cancellation();
        assert!(completion.disallow_cancellation());

        handle
            .receive(
                crate::encode_finish_with_options(8, true, false, ProtocolLimits::default())
                    .expect("finish"),
            )
            .expect("finish queued");
        assert!(matches!(next(&mut actor), Poll::Pending));
        assert!(!cancellation.is_canceled());
        assert_eq!(actor.stats().active_answers, 1);

        completion
            .complete(HandlerResult::Results(message(2)))
            .expect("completion queued");
        assert!(matches!(next(&mut actor), Poll::Pending));
        assert_eq!(actor.stats().active_answers, 0);
        assert_eq!(actor.stats().completed_handlers, 1);
    }

    #[test]
    fn legacy_finish_yields_once_so_application_can_opt_out() {
        let (handle, mut actor) =
            ConnectionActor::new(ActorLimits::default(), ProtocolLimits::default());
        handle
            .receive(encode_bootstrap(9, ProtocolLimits::default()).expect("bootstrap"))
            .expect("bootstrap queued");
        let Poll::Ready(Some(ActorEffect::Dispatch { completion, .. })) = next(&mut actor) else {
            panic!("dispatch")
        };
        let cancellation = completion.cancellation();
        handle
            .receive(
                crate::encode_finish_with_options(9, true, true, ProtocolLimits::default())
                    .expect("legacy finish"),
            )
            .expect("finish queued");

        assert!(matches!(next(&mut actor), Poll::Pending));
        assert!(cancellation.is_allowed());
        assert!(completion.disallow_cancellation());
        assert!(matches!(next(&mut actor), Poll::Pending));
        assert!(!cancellation.is_canceled());
        assert_eq!(actor.stats().active_answers, 1);

        completion
            .complete(HandlerResult::Results(message(3)))
            .expect("completion queued");
        assert!(matches!(next(&mut actor), Poll::Pending));
        assert_eq!(actor.stats().active_answers, 0);
    }

    #[test]
    fn no_finish_needed_completes_without_emitting_finish() {
        let (handle, mut actor) =
            ConnectionActor::new(ActorLimits::default(), ProtocolLimits::default());
        let mut response = handle.bootstrap().expect("bootstrap");
        assert!(matches!(
            next(&mut actor),
            Poll::Ready(Some(ActorEffect::Send(_)))
        ));
        handle
            .receive(
                crate::encode_return_with_options(
                    0,
                    &HandlerResult::Results(message(4)),
                    true,
                    true,
                    ProtocolLimits::default(),
                )
                .expect("return"),
            )
            .expect("return queued");
        assert!(matches!(next(&mut actor), Poll::Pending));
        assert!(matches!(
            poll_future(&mut response),
            Poll::Ready(Ok(ReturnPayload::Results(_)))
        ));
        assert_eq!(actor.stats().active_questions, 0);
    }

    #[test]
    fn disconnect_completes_embargoed_question_and_cancels_dispatch() {
        let (handle, mut actor) =
            ConnectionActor::new(ActorLimits::default(), ProtocolLimits::default());
        let hosted = HostedCapability::new().expect("hosted");
        actor.promise_imports.insert(
            7,
            ImportPromiseState::Loopback {
                capability: hosted.clone(),
                embargo_id: 0,
            },
        );
        actor.embargoes.insert(
            0,
            EmbargoState {
                promise_id: 7,
                capability: hosted,
                queued_calls: VecDeque::new(),
            },
        );
        let mut embargoed = handle
            .call_imported(7, 1, 2, message(5), Vec::new())
            .expect("embargoed call");
        assert!(matches!(next(&mut actor), Poll::Pending));
        assert_eq!(actor.stats().queued_embargo_calls, 1);

        handle.shutdown().expect("shutdown queued");
        assert!(matches!(
            next(&mut actor),
            Poll::Ready(Some(ActorEffect::CloseTransport))
        ));
        assert!(matches!(
            poll_future(&mut embargoed),
            Poll::Ready(Err(ConnectionError::Disconnected))
        ));
        assert_eq!(actor.stats().queued_embargo_calls, 0);
    }

    #[test]
    fn disconnect_force_cancels_opted_out_application_work() {
        let (handle, mut actor) =
            ConnectionActor::new(ActorLimits::default(), ProtocolLimits::default());
        handle
            .receive(encode_bootstrap(10, ProtocolLimits::default()).expect("bootstrap"))
            .expect("bootstrap queued");
        let Poll::Ready(Some(ActorEffect::Dispatch { completion, .. })) = next(&mut actor) else {
            panic!("dispatch")
        };
        let cancellation = completion.cancellation();
        assert!(completion.disallow_cancellation());
        handle.shutdown().expect("shutdown queued");
        assert!(matches!(
            next(&mut actor),
            Poll::Ready(Some(ActorEffect::CloseTransport))
        ));
        assert!(cancellation.is_canceled());
        assert!(matches!(
            completion.complete(HandlerResult::Canceled),
            Err(ConnectionError::Disconnected)
        ));
    }
}
