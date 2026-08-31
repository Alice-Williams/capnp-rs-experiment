//! Single-owner two-party Level-0 connection actor.
//!
//! The actor alone mutates protocol tables. Thread-safe handles only append to
//! a bounded mailbox. Application work leaves the actor as a `Dispatch` effect
//! and returns through a generation-bearing completion token, so handlers may
//! run concurrently and finish out of order without sharing table state.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use capnp_message::OwnedMessage;
use capnp_schema::DynamicAnyPointer;

use crate::level0::{
    CallTarget, HandlerResult, ReturnPayload, encode_bootstrap, encode_call, encode_finish,
    encode_return,
};
use crate::protocol::{
    ExceptionType, ProtocolLimits, ProtocolMessage, RpcException, encode_abort,
    encode_unimplemented, read_protocol_message, read_protocol_struct,
};

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
}

impl Default for ActorLimits {
    fn default() -> Self {
        Self {
            mailbox_capacity: 256,
            max_questions: 4096,
            max_answers: 4096,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ConnectionStats {
    pub active_questions: usize,
    pub active_answers: usize,
    pub allocated_questions: u64,
    pub reused_question_ids: u64,
    pub dispatched_handlers: u64,
    pub completed_handlers: u64,
    pub stale_handler_completions: u64,
}

#[derive(Clone, Debug)]
pub enum ConnectionError {
    Overloaded { capacity: usize },
    QuestionLimit { limit: usize },
    AnswerLimit { limit: usize },
    DuplicateAnswer(u32),
    UnknownQuestion(u32),
    StaleTarget(QuestionKey),
    StaleAnswer(AnswerKey),
    GenerationExhausted,
    Unimplemented,
    Disconnected,
    RemoteAbort(RpcException),
    Protocol(String),
    Wire(String),
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
    pub fn bootstrap(&self) -> Result<QuestionFuture, ConnectionError> {
        let cell = Arc::new(QuestionCell::new());
        self.submit(ActorCommand::StartBootstrap {
            cell: Arc::clone(&cell),
        })?;
        Ok(QuestionFuture { cell })
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
            target: target.clone(),
            interface_id,
            method_id,
            params,
            cell: Arc::clone(&cell),
        })?;
        Ok(QuestionFuture { cell })
    }

    pub fn receive(&self, message: Arc<OwnedMessage>) -> Result<(), ConnectionError> {
        self.submit(ActorCommand::Incoming(message))
    }

    pub fn shutdown(&self) -> Result<(), ConnectionError> {
        self.submit(ActorCommand::Shutdown)
    }

    fn submit(&self, command: ActorCommand) -> Result<(), ConnectionError> {
        self.mailbox.submit(command)
    }
}

#[derive(Clone)]
pub struct QuestionTarget {
    cell: Arc<QuestionCell>,
}

impl fmt::Debug for QuestionTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("QuestionTarget")
            .field("key", &self.key())
            .finish()
    }
}

impl QuestionTarget {
    pub fn key(&self) -> Option<QuestionKey> {
        self.cell.active_key().ok().flatten()
    }
}

pub struct QuestionFuture {
    cell: Arc<QuestionCell>,
}

impl fmt::Debug for QuestionFuture {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("QuestionFuture")
            .finish_non_exhaustive()
    }
}

impl QuestionFuture {
    pub fn target(&self) -> QuestionTarget {
        QuestionTarget {
            cell: Arc::clone(&self.cell),
        }
    }
}

impl Future for QuestionFuture {
    type Output = Result<ReturnPayload, ConnectionError>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        self.cell.poll(context)
    }
}

#[derive(Clone, Debug)]
pub enum IncomingRequest {
    Bootstrap,
    Call {
        target: AnswerKey,
        interface_id: u64,
        method_id: u16,
        params: DynamicAnyPointer,
    },
}

pub struct CompletionToken {
    handle: ConnectionHandle,
    answer: AnswerKey,
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

    pub fn complete(self, result: HandlerResult) -> Result<(), ConnectionError> {
        self.handle.submit(ActorCommand::HandlerComplete {
            answer: self.answer,
            result,
        })
    }
}

#[derive(Debug)]
pub enum ActorEffect {
    Send(Arc<OwnedMessage>),
    Dispatch {
        request: IncomingRequest,
        completion: CompletionToken,
    },
    CloseTransport,
}

/// The only owner of a connection's ordered protocol state.
pub struct ConnectionActor {
    mailbox: Arc<SharedMailbox>,
    handle: ConnectionHandle,
    protocol_limits: ProtocolLimits,
    questions: QuestionTable,
    answers: AnswerTable,
    effects: VecDeque<ActorEffect>,
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
                answers: AnswerTable::new(limits.max_answers),
                effects: VecDeque::new(),
                stats: ConnectionStats::default(),
                terminal: false,
            },
        )
    }

    pub fn stats(&self) -> ConnectionStats {
        ConnectionStats {
            active_questions: self.questions.len(),
            active_answers: self.answers.len(),
            ..self.stats
        }
    }

    /// Processes ordered commands until one externally-visible effect is ready.
    pub fn poll_next_effect(&mut self, context: &mut Context<'_>) -> Poll<Option<ActorEffect>> {
        loop {
            if let Some(effect) = self.effects.pop_front() {
                return Poll::Ready(Some(effect));
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
                cell,
            } => self.start_call(target, interface_id, method_id, params, cell),
            ActorCommand::Incoming(message) => self.incoming(message),
            ActorCommand::HandlerComplete { answer, result } => {
                self.handler_complete(answer, result)
            }
            ActorCommand::Shutdown => {
                self.transition_terminal(ConnectionError::Disconnected, false)
            }
        }
    }

    fn start_bootstrap(&mut self, cell: Arc<QuestionCell>) {
        let key = match self.questions.allocate(QuestionState {
            cell: Arc::clone(&cell),
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
        target: QuestionTarget,
        interface_id: u64,
        method_id: u16,
        params: Arc<OwnedMessage>,
        cell: Arc<QuestionCell>,
    ) {
        let target_key = match target.cell.active_key() {
            Ok(Some(key)) if self.questions.contains(key) => key,
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
        };
        let key = match self.questions.allocate(QuestionState {
            cell: Arc::clone(&cell),
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
        match encode_call(
            key.id,
            CallTarget::BootstrapAnswer(target_key.id),
            interface_id,
            method_id,
            &params,
            self.protocol_limits,
        ) {
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

    fn incoming(&mut self, raw: Arc<OwnedMessage>) {
        let message = match read_protocol_message(Arc::clone(&raw)) {
            Ok(message) => message,
            Err(error) => {
                self.protocol_failure(ConnectionError::Protocol(error.to_string()));
                return;
            }
        };
        match message {
            ProtocolMessage::Bootstrap(message) => {
                let answer = match self
                    .answers
                    .insert(message.question_id, AnswerKind::Bootstrap)
                {
                    Ok(answer) => answer,
                    Err(error) => {
                        self.protocol_failure(error);
                        return;
                    }
                };
                self.dispatch(IncomingRequest::Bootstrap, answer);
            }
            ProtocolMessage::Call(message) => {
                let CallTarget::BootstrapAnswer(target_id) = message.target;
                let Some(target) = self.answers.key_for_id(target_id) else {
                    self.protocol_failure(ConnectionError::Protocol(format!(
                        "call targets unknown bootstrap answer {target_id}"
                    )));
                    return;
                };
                if !self.answers.is_bootstrap(target) {
                    self.protocol_failure(ConnectionError::Protocol(format!(
                        "call target {target_id} is not a bootstrap answer"
                    )));
                    return;
                }
                let answer = match self.answers.insert(message.question_id, AnswerKind::Call) {
                    Ok(answer) => answer,
                    Err(error) => {
                        self.protocol_failure(error);
                        return;
                    }
                };
                self.dispatch(
                    IncomingRequest::Call {
                        target,
                        interface_id: message.interface_id,
                        method_id: message.method_id,
                        params: message.params,
                    },
                    answer,
                );
            }
            ProtocolMessage::Return(message) => {
                let Some((key, question)) = self.questions.remove_id(message.answer_id) else {
                    self.protocol_failure(ConnectionError::UnknownQuestion(message.answer_id));
                    return;
                };
                question.cell.complete(Ok(message.payload));
                match encode_finish(key.id, self.protocol_limits) {
                    Ok(finish) => self.effects.push_back(ActorEffect::Send(finish)),
                    Err(error) => self.protocol_failure(ConnectionError::Wire(error.to_string())),
                }
            }
            ProtocolMessage::Finish(message) => {
                let _ = self.answers.remove_id(message.question_id);
            }
            ProtocolMessage::Abort(exception) => {
                self.transition_terminal(ConnectionError::RemoteAbort(exception), false);
            }
            ProtocolMessage::Unimplemented(Some(nested)) => {
                self.handle_unimplemented(nested);
            }
            ProtocolMessage::Unimplemented(None) => {}
            ProtocolMessage::Unsupported { .. } => {
                match encode_unimplemented(&raw, self.protocol_limits) {
                    Ok(message) => self.effects.push_back(ActorEffect::Send(message)),
                    Err(error) => {
                        self.protocol_failure(ConnectionError::Wire(error.to_string()));
                    }
                }
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
        self.stats.dispatched_handlers = self.stats.dispatched_handlers.saturating_add(1);
        self.effects.push_back(ActorEffect::Dispatch {
            request,
            completion: CompletionToken {
                handle: self.handle.clone(),
                answer,
            },
        });
    }

    fn handler_complete(&mut self, answer: AnswerKey, result: HandlerResult) {
        if !self.answers.mark_returned(answer) {
            self.stats.stale_handler_completions =
                self.stats.stale_handler_completions.saturating_add(1);
            return;
        }
        match encode_return(answer.id, &result, self.protocol_limits) {
            Ok(message) => {
                self.stats.completed_handlers = self.stats.completed_handlers.saturating_add(1);
                self.effects.push_back(ActorEffect::Send(message));
            }
            Err(error) => self.protocol_failure(ConnectionError::Wire(error.to_string())),
        }
    }

    fn record_question_allocation(&mut self, key: QuestionKey) {
        self.stats.allocated_questions = self.stats.allocated_questions.saturating_add(1);
        if key.generation != 0 {
            self.stats.reused_question_ids = self.stats.reused_question_ids.saturating_add(1);
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
        self.answers.clear();
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
        target: QuestionTarget,
        interface_id: u64,
        method_id: u16,
        params: Arc<OwnedMessage>,
        cell: Arc<QuestionCell>,
    },
    Incoming(Arc<OwnedMessage>),
    HandlerComplete {
        answer: AnswerKey,
        result: HandlerResult,
    },
    Shutdown,
}

fn complete_rejected(command: ActorCommand, error: ConnectionError) {
    match command {
        ActorCommand::StartBootstrap { cell } | ActorCommand::StartCall { cell, .. } => {
            cell.complete(Err(error));
        }
        ActorCommand::Incoming(_)
        | ActorCommand::HandlerComplete { .. }
        | ActorCommand::Shutdown => {}
    }
}

struct SharedMailbox {
    capacity: usize,
    closed: AtomicBool,
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
}

struct QuestionSlot {
    generation: u64,
    value: Option<QuestionState>,
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
            if slot.value.is_none() {
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

    fn drain(&mut self) -> Vec<QuestionState> {
        let mut output = Vec::with_capacity(self.len);
        for slot in &mut self.slots {
            if let Some(value) = slot.value.take() {
                output.push(value);
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

struct AnswerState {
    kind: AnswerKind,
    phase: AnswerPhase,
}

struct AnswerTable {
    values: BTreeMap<u32, (u64, AnswerState)>,
    next_generation: u64,
    max: usize,
}

impl AnswerTable {
    fn new(max: usize) -> Self {
        Self {
            values: BTreeMap::new(),
            next_generation: 0,
            max,
        }
    }

    fn len(&self) -> usize {
        self.values.len()
    }

    fn insert(&mut self, id: u32, kind: AnswerKind) -> Result<AnswerKey, ConnectionError> {
        if self.values.contains_key(&id) {
            return Err(ConnectionError::DuplicateAnswer(id));
        }
        if self.values.len() >= self.max {
            return Err(ConnectionError::AnswerLimit { limit: self.max });
        }
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
                },
            ),
        );
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

    fn remove_id(&mut self, id: u32) -> Option<AnswerState> {
        self.values.remove(&id).map(|(_, value)| value)
    }

    fn clear(&mut self) {
        self.values.clear();
    }
}

fn _assert_public_send_traits() {
    fn assert_send<T: Send>() {}
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ConnectionHandle>();
    assert_send::<ConnectionActor>();
    assert_send::<QuestionFuture>();
    assert_send::<CompletionToken>();
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;
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

    fn next(actor: &mut ConnectionActor) -> Poll<Option<ActorEffect>> {
        let mut context = Context::from_waker(Waker::noop());
        actor.poll_next_effect(&mut context)
    }

    fn send_to(effect: ActorEffect, peer: &ConnectionHandle) {
        let ActorEffect::Send(message) = effect else {
            panic!("send effect")
        };
        peer.receive(message).expect("peer accepts wire message");
    }

    fn poll_future(future: &mut QuestionFuture) -> Poll<Result<ReturnPayload, ConnectionError>> {
        let mut context = Context::from_waker(Waker::noop());
        Pin::new(future).poll(&mut context)
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
            .receive(encode_finish(key.id, ProtocolLimits::default()).expect("finish"))
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
}
