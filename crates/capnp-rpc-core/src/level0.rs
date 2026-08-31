//! Pinned two-party Level-1 RPC message bindings through promise pipelining.
//!
//! M34 adds settled hosted descriptors and `Release`; M35 adds promised-answer
//! transforms, `receiverAnswer`, redirected results, and Level-1 tail routing.
//! M36 adds `senderPromise`, `Resolve`, and loopback `Disembargo`. M43 adds the
//! orthogonal `attachedFd` field and binds each received resource to at most
//! one descriptor. Third-party behavior remains owned by later milestones.

use std::sync::Arc;

use capnp_message::{ExclusiveArena, OwnedMessage};
use capnp_schema::{DynamicAnyPointer, DynamicInput, DynamicStruct, DynamicValue, OpaquePointer};

use crate::protocol::{
    MESSAGE_TYPE_ID, ProtocolError, ProtocolLimits, RpcException, opaque_root, owned,
    protocol_schema, read_exception, write_exception,
};
use crate::{AttachedResource, OwnedResource};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BootstrapMessage {
    pub question_id: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CallTarget {
    BootstrapAnswer(u32),
    ImportedCap(u32),
    PromisedAnswer(PromisedAnswer),
}

macro_rules! third_party_token {
    ($name:ident) => {
        #[derive(Clone, Debug, Eq, PartialEq)]
        pub struct $name(OpaquePointer);

        impl $name {
            pub fn from_opaque(value: OpaquePointer) -> Self {
                Self(value)
            }

            pub fn null() -> Self {
                Self(OpaquePointer::null())
            }

            pub fn as_opaque(&self) -> &OpaquePointer {
                &self.0
            }

            pub fn into_opaque(self) -> OpaquePointer {
                self.0
            }
        }
    };
}

third_party_token!(ThirdPartyToContact);
third_party_token!(ThirdPartyToAwait);
third_party_token!(ThirdPartyCompletion);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThirdPartyCapDescriptor {
    pub id: ThirdPartyToContact,
    pub vine_id: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PipelineOp {
    Noop,
    GetPointerField(u16),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PromisedAnswer {
    pub question_id: u32,
    pub transform: Vec<PipelineOp>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CapDescriptor {
    None,
    SenderHosted(u32),
    SenderPromise(u32),
    ReceiverHosted(u32),
    ReceiverAnswer(PromisedAnswer),
    ThirdPartyHosted(ThirdPartyCapDescriptor),
    /// A normal descriptor plus the orthogonal pinned `attachedFd` field.
    Attached {
        descriptor: Box<CapDescriptor>,
        resource_index: u8,
        resource: Option<AttachedResource>,
    },
}

impl CapDescriptor {
    pub fn with_attachment(self, resource_index: u8, resource: Option<AttachedResource>) -> Self {
        let descriptor = match self {
            Self::Attached { descriptor, .. } => descriptor,
            descriptor => Box::new(descriptor),
        };
        Self::Attached {
            descriptor,
            resource_index,
            resource,
        }
    }

    pub fn descriptor(&self) -> &CapDescriptor {
        match self {
            Self::Attached { descriptor, .. } => descriptor,
            descriptor => descriptor,
        }
    }

    pub fn resource_index(&self) -> Option<u8> {
        match self {
            Self::Attached { resource_index, .. } => Some(*resource_index),
            _ => None,
        }
    }

    pub fn attached_resource(&self) -> Option<&AttachedResource> {
        match self {
            Self::Attached { resource, .. } => resource.as_ref(),
            _ => None,
        }
    }

    pub(crate) fn set_attached_resource(&mut self, attached: Option<AttachedResource>) {
        if let Self::Attached { resource, .. } = self {
            *resource = attached;
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ResourceBindingStats {
    pub received: usize,
    pub attached: usize,
    pub discarded: usize,
}

/// Moves each transport resource into at most one descriptor slot. Duplicate
/// and out-of-range indices become resource-less, and every unused resource is
/// dropped before this function returns.
pub fn bind_attached_resources(
    descriptors: &mut [CapDescriptor],
    resources: Vec<OwnedResource>,
) -> ResourceBindingStats {
    let received = resources.len();
    let mut slots = resources
        .into_iter()
        .map(|resource| Some(resource.into_attached()))
        .collect::<Vec<_>>();
    let mut attached = 0usize;
    for descriptor in descriptors {
        let resource = descriptor
            .resource_index()
            .and_then(|index| slots.get_mut(usize::from(index)))
            .and_then(Option::take);
        if resource.is_some() {
            attached = attached.saturating_add(1);
        }
        descriptor.set_attached_resource(resource);
    }
    ResourceBindingStats {
        received,
        attached,
        discarded: received.saturating_sub(attached),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PromiseResolution {
    Cap(CapDescriptor),
    Exception(RpcException),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolveMessage {
    pub promise_id: u32,
    pub resolution: PromiseResolution,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DisembargoContext {
    SenderLoopback(u32),
    ReceiverLoopback(u32),
    Accept(Vec<u8>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DisembargoMessage {
    pub target: CallTarget,
    pub context: DisembargoContext,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SendResultsTo {
    Caller,
    Yourself,
    ThirdParty(ThirdPartyToContact),
}

#[derive(Clone, Debug)]
pub struct Payload {
    pub content: DynamicAnyPointer,
    pub cap_table: Vec<CapDescriptor>,
}

#[derive(Clone, Debug)]
pub struct CallMessage {
    pub question_id: u32,
    pub target: CallTarget,
    pub interface_id: u64,
    pub method_id: u16,
    pub params: Payload,
    pub send_results_to: SendResultsTo,
}

#[derive(Clone, Debug)]
pub enum ReturnPayload {
    Results(Payload),
    LocalResults {
        content: DynamicAnyPointer,
        capabilities: Vec<crate::OutgoingCapability>,
    },
    Exception(RpcException),
    Canceled,
    ResultsSentElsewhere,
    TakeFromOtherQuestion(u32),
    AwaitFromThirdParty(ThirdPartyToAwait),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProvideMessage {
    pub question_id: u32,
    pub target: CallTarget,
    pub recipient: ThirdPartyToAwait,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AcceptMessage {
    pub question_id: u32,
    pub provision: ThirdPartyCompletion,
    pub embargo: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThirdPartyAnswerMessage {
    pub completion: ThirdPartyCompletion,
    pub answer_id: u32,
}

#[derive(Clone, Debug)]
pub struct ReturnMessage {
    pub answer_id: u32,
    pub release_param_caps: bool,
    pub no_finish_needed: bool,
    pub payload: ReturnPayload,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FinishMessage {
    pub question_id: u32,
    pub release_result_caps: bool,
    pub require_early_cancellation_workaround: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReleaseMessage {
    pub id: u32,
    pub reference_count: u32,
}

/// A handler completion submitted back to the owning connection actor.
#[derive(Clone, Debug)]
pub enum HandlerResult {
    Results(Arc<OwnedMessage>),
    ResultsWithCapabilities {
        content: Arc<OwnedMessage>,
        cap_table: Vec<CapDescriptor>,
    },
    Exception(RpcException),
    Canceled,
    ResultsSentElsewhere,
    TakeFromOtherQuestion(u32),
}

pub fn encode_bootstrap(
    question_id: u32,
    limits: ProtocolLimits,
) -> Result<Arc<OwnedMessage>, ProtocolError> {
    let schema = protocol_schema()?;
    let mut arena = arena(limits)?;
    {
        let mut message =
            capnp_schema::DynamicStructBuilder::root(&schema, &mut arena, MESSAGE_TYPE_ID)?;
        message
            .init_struct("bootstrap")?
            .set("questionId", DynamicInput::UInt32(question_id))?;
    }
    owned(arena, limits)
}

pub fn encode_call(
    question_id: u32,
    target: CallTarget,
    interface_id: u64,
    method_id: u16,
    params: &Arc<OwnedMessage>,
    limits: ProtocolLimits,
) -> Result<Arc<OwnedMessage>, ProtocolError> {
    encode_call_with_capabilities(
        question_id,
        target,
        interface_id,
        method_id,
        params,
        &[],
        limits,
    )
}

pub fn encode_call_with_capabilities(
    question_id: u32,
    target: CallTarget,
    interface_id: u64,
    method_id: u16,
    params: &Arc<OwnedMessage>,
    cap_table: &[CapDescriptor],
    limits: ProtocolLimits,
) -> Result<Arc<OwnedMessage>, ProtocolError> {
    encode_call_with_options(
        question_id,
        target,
        interface_id,
        method_id,
        params,
        cap_table,
        SendResultsTo::Caller,
        limits,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn encode_call_with_options(
    question_id: u32,
    target: CallTarget,
    interface_id: u64,
    method_id: u16,
    params: &Arc<OwnedMessage>,
    cap_table: &[CapDescriptor],
    send_results_to: SendResultsTo,
    limits: ProtocolLimits,
) -> Result<Arc<OwnedMessage>, ProtocolError> {
    check_cap_table_len(cap_table.len(), limits)?;
    let content = opaque_root(params, limits)?;
    encode_call_with_opaque_options(
        question_id,
        target,
        interface_id,
        method_id,
        &content,
        cap_table,
        send_results_to,
        limits,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn encode_call_payload_with_options(
    question_id: u32,
    target: CallTarget,
    interface_id: u64,
    method_id: u16,
    params: &Payload,
    cap_table: &[CapDescriptor],
    send_results_to: SendResultsTo,
    limits: ProtocolLimits,
) -> Result<Arc<OwnedMessage>, ProtocolError> {
    check_cap_table_len(cap_table.len(), limits)?;
    let content = params
        .content
        .to_opaque(crate::protocol::reader_limits(limits))?;
    encode_call_with_opaque_options(
        question_id,
        target,
        interface_id,
        method_id,
        &content,
        cap_table,
        send_results_to,
        limits,
    )
}

#[allow(clippy::too_many_arguments)]
fn encode_call_with_opaque_options(
    question_id: u32,
    target: CallTarget,
    interface_id: u64,
    method_id: u16,
    content: &capnp_schema::OpaquePointer,
    cap_table: &[CapDescriptor],
    send_results_to: SendResultsTo,
    limits: ProtocolLimits,
) -> Result<Arc<OwnedMessage>, ProtocolError> {
    let schema = protocol_schema()?;
    let mut arena = arena(limits)?;
    {
        let mut message =
            capnp_schema::DynamicStructBuilder::root(&schema, &mut arena, MESSAGE_TYPE_ID)?;
        let mut call = message.init_struct("call")?;
        call.set("questionId", DynamicInput::UInt32(question_id))?;
        call.set("interfaceId", DynamicInput::UInt64(interface_id))?;
        call.set("methodId", DynamicInput::UInt16(method_id))?;
        {
            let mut output = call.group("sendResultsTo")?;
            match send_results_to {
                SendResultsTo::Caller => output.activate("caller")?,
                SendResultsTo::Yourself => output.activate("yourself")?,
                SendResultsTo::ThirdParty(contact) => {
                    output.set("thirdParty", DynamicInput::Pointer(contact.as_opaque()))?;
                }
            }
        }
        {
            let mut target_builder = call.init_struct("target")?;
            match target {
                CallTarget::BootstrapAnswer(answer_id) => {
                    let mut promised = target_builder.init_struct("promisedAnswer")?;
                    promised.set("questionId", DynamicInput::UInt32(answer_id))?;
                    promised.init_list("transform", 0)?;
                }
                CallTarget::ImportedCap(import_id) => {
                    target_builder.set("importedCap", DynamicInput::UInt32(import_id))?;
                }
                CallTarget::PromisedAnswer(promised_answer) => {
                    let mut promised = target_builder.init_struct("promisedAnswer")?;
                    write_promised_answer(&mut promised, &promised_answer, limits)?;
                }
            }
        }
        {
            let mut payload = call.init_struct("params")?;
            payload.set("content", DynamicInput::Pointer(content))?;
            write_cap_table(&mut payload, cap_table, limits)?;
        }
    }
    owned(arena, limits)
}

pub fn encode_return(
    answer_id: u32,
    result: &HandlerResult,
    limits: ProtocolLimits,
) -> Result<Arc<OwnedMessage>, ProtocolError> {
    encode_return_with_options(answer_id, result, true, false, limits)
}

/// Encodes a `Return` including the lifecycle flags used by implementations
/// that have already released parameter or answer state.
pub fn encode_return_with_options(
    answer_id: u32,
    result: &HandlerResult,
    release_param_caps: bool,
    no_finish_needed: bool,
    limits: ProtocolLimits,
) -> Result<Arc<OwnedMessage>, ProtocolError> {
    if no_finish_needed
        && matches!(
            result,
            HandlerResult::ResultsWithCapabilities { cap_table, .. } if !cap_table.is_empty()
        )
    {
        return Err(ProtocolError::FieldType("return.noFinishNeeded"));
    }
    let result_pointer = match result {
        HandlerResult::Results(message)
        | HandlerResult::ResultsWithCapabilities {
            content: message, ..
        } => Some(opaque_root(message, limits)?),
        HandlerResult::Exception(exception) => {
            check_exception_limits(exception, limits)?;
            None
        }
        HandlerResult::Canceled => None,
        HandlerResult::ResultsSentElsewhere | HandlerResult::TakeFromOtherQuestion(_) => None,
    };
    let schema = protocol_schema()?;
    let mut arena = arena(limits)?;
    {
        let mut message =
            capnp_schema::DynamicStructBuilder::root(&schema, &mut arena, MESSAGE_TYPE_ID)?;
        let mut returned = message.init_struct("return")?;
        returned.set("answerId", DynamicInput::UInt32(answer_id))?;
        returned.set("releaseParamCaps", DynamicInput::Bool(release_param_caps))?;
        returned.set("noFinishNeeded", DynamicInput::Bool(no_finish_needed))?;
        match (result, result_pointer.as_ref()) {
            (HandlerResult::Results(_), Some(pointer)) => {
                let mut payload = returned.init_struct("results")?;
                payload.set("content", DynamicInput::Pointer(pointer))?;
                payload.init_list("capTable", 0)?;
            }
            (HandlerResult::ResultsWithCapabilities { cap_table, .. }, Some(pointer)) => {
                check_cap_table_len(cap_table.len(), limits)?;
                let mut payload = returned.init_struct("results")?;
                payload.set("content", DynamicInput::Pointer(pointer))?;
                write_cap_table(&mut payload, cap_table, limits)?;
            }
            (HandlerResult::Exception(exception), None) => {
                let mut output = returned.init_struct("exception")?;
                write_exception(&mut output, exception)?;
            }
            (HandlerResult::Canceled, None) => returned.activate("canceled")?,
            (HandlerResult::ResultsSentElsewhere, None) => {
                returned.activate("resultsSentElsewhere")?
            }
            (HandlerResult::TakeFromOtherQuestion(question_id), None) => {
                returned.set("takeFromOtherQuestion", DynamicInput::UInt32(*question_id))?;
            }
            _ => return Err(ProtocolError::FieldType("return")),
        }
    }
    owned(arena, limits)
}

pub fn encode_release(
    id: u32,
    reference_count: u32,
    limits: ProtocolLimits,
) -> Result<Arc<OwnedMessage>, ProtocolError> {
    if reference_count == 0 {
        return Err(ProtocolError::InvalidReferenceCount);
    }
    let schema = protocol_schema()?;
    let mut arena = arena(limits)?;
    {
        let mut message =
            capnp_schema::DynamicStructBuilder::root(&schema, &mut arena, MESSAGE_TYPE_ID)?;
        let mut release = message.init_struct("release")?;
        release.set("id", DynamicInput::UInt32(id))?;
        release.set("referenceCount", DynamicInput::UInt32(reference_count))?;
    }
    owned(arena, limits)
}

pub fn encode_resolve(
    promise_id: u32,
    resolution: &PromiseResolution,
    limits: ProtocolLimits,
) -> Result<Arc<OwnedMessage>, ProtocolError> {
    if let PromiseResolution::Exception(exception) = resolution {
        check_exception_limits(exception, limits)?;
    }
    let schema = protocol_schema()?;
    let mut arena = arena(limits)?;
    {
        let mut message =
            capnp_schema::DynamicStructBuilder::root(&schema, &mut arena, MESSAGE_TYPE_ID)?;
        let mut resolve = message.init_struct("resolve")?;
        resolve.set("promiseId", DynamicInput::UInt32(promise_id))?;
        match resolution {
            PromiseResolution::Cap(descriptor) => {
                let mut cap = resolve.init_struct("cap")?;
                write_cap_descriptor(&mut cap, descriptor, limits)?;
            }
            PromiseResolution::Exception(exception) => {
                let mut output = resolve.init_struct("exception")?;
                write_exception(&mut output, exception)?;
            }
        }
    }
    owned(arena, limits)
}

pub fn encode_disembargo(
    target: &CallTarget,
    context: DisembargoContext,
    limits: ProtocolLimits,
) -> Result<Arc<OwnedMessage>, ProtocolError> {
    if let DisembargoContext::Accept(id) = &context {
        check_embargo_id(id, limits)?;
    }
    let schema = protocol_schema()?;
    let mut arena = arena(limits)?;
    {
        let mut message =
            capnp_schema::DynamicStructBuilder::root(&schema, &mut arena, MESSAGE_TYPE_ID)?;
        let mut disembargo = message.init_struct("disembargo")?;
        let mut target_builder = disembargo.init_struct("target")?;
        write_message_target(&mut target_builder, target, limits)?;
        let mut output = disembargo.group("context")?;
        match context {
            DisembargoContext::SenderLoopback(id) => {
                output.set("senderLoopback", DynamicInput::UInt32(id))?;
            }
            DisembargoContext::ReceiverLoopback(id) => {
                output.set("receiverLoopback", DynamicInput::UInt32(id))?;
            }
            DisembargoContext::Accept(id) => {
                output.set("accept", DynamicInput::Data(&id))?;
            }
        }
    }
    owned(arena, limits)
}

pub fn encode_provide(
    message: &ProvideMessage,
    limits: ProtocolLimits,
) -> Result<Arc<OwnedMessage>, ProtocolError> {
    let schema = protocol_schema()?;
    let mut arena = arena(limits)?;
    {
        let mut root =
            capnp_schema::DynamicStructBuilder::root(&schema, &mut arena, MESSAGE_TYPE_ID)?;
        let mut provide = root.init_struct("provide")?;
        provide.set("questionId", DynamicInput::UInt32(message.question_id))?;
        {
            let mut target = provide.init_struct("target")?;
            write_message_target(&mut target, &message.target, limits)?;
        }
        provide.set(
            "recipient",
            DynamicInput::Pointer(message.recipient.as_opaque()),
        )?;
    }
    owned(arena, limits)
}

pub fn encode_accept(
    message: &AcceptMessage,
    limits: ProtocolLimits,
) -> Result<Arc<OwnedMessage>, ProtocolError> {
    check_embargo_id(&message.embargo, limits)?;
    let schema = protocol_schema()?;
    let mut arena = arena(limits)?;
    {
        let mut root =
            capnp_schema::DynamicStructBuilder::root(&schema, &mut arena, MESSAGE_TYPE_ID)?;
        let mut accept = root.init_struct("accept")?;
        accept.set("questionId", DynamicInput::UInt32(message.question_id))?;
        accept.set(
            "provision",
            DynamicInput::Pointer(message.provision.as_opaque()),
        )?;
        if !message.embargo.is_empty() {
            accept.set("embargo", DynamicInput::Data(&message.embargo))?;
        }
    }
    owned(arena, limits)
}

pub fn encode_third_party_answer(
    message: &ThirdPartyAnswerMessage,
    limits: ProtocolLimits,
) -> Result<Arc<OwnedMessage>, ProtocolError> {
    if !(1_u32 << 30..1_u32 << 31).contains(&message.answer_id) {
        return Err(ProtocolError::InvalidThirdPartyAnswerId(message.answer_id));
    }
    let schema = protocol_schema()?;
    let mut arena = arena(limits)?;
    {
        let mut root =
            capnp_schema::DynamicStructBuilder::root(&schema, &mut arena, MESSAGE_TYPE_ID)?;
        let mut answer = root.init_struct("thirdPartyAnswer")?;
        answer.set(
            "completion",
            DynamicInput::Pointer(message.completion.as_opaque()),
        )?;
        answer.set("answerId", DynamicInput::UInt32(message.answer_id))?;
    }
    owned(arena, limits)
}

pub fn encode_return_await_from_third_party(
    answer_id: u32,
    party: &ThirdPartyToAwait,
    limits: ProtocolLimits,
) -> Result<Arc<OwnedMessage>, ProtocolError> {
    let schema = protocol_schema()?;
    let mut arena = arena(limits)?;
    {
        let mut root =
            capnp_schema::DynamicStructBuilder::root(&schema, &mut arena, MESSAGE_TYPE_ID)?;
        let mut returned = root.init_struct("return")?;
        returned.set("answerId", DynamicInput::UInt32(answer_id))?;
        returned.set("releaseParamCaps", DynamicInput::Bool(true))?;
        returned.set(
            "awaitFromThirdParty",
            DynamicInput::Pointer(party.as_opaque()),
        )?;
    }
    owned(arena, limits)
}

pub fn encode_finish(
    question_id: u32,
    limits: ProtocolLimits,
) -> Result<Arc<OwnedMessage>, ProtocolError> {
    encode_finish_with_release(question_id, true, limits)
}

pub fn encode_finish_with_release(
    question_id: u32,
    release_result_caps: bool,
    limits: ProtocolLimits,
) -> Result<Arc<OwnedMessage>, ProtocolError> {
    encode_finish_with_options(question_id, release_result_caps, false, limits)
}

pub fn encode_finish_with_options(
    question_id: u32,
    release_result_caps: bool,
    require_early_cancellation_workaround: bool,
    limits: ProtocolLimits,
) -> Result<Arc<OwnedMessage>, ProtocolError> {
    let schema = protocol_schema()?;
    let mut arena = arena(limits)?;
    {
        let mut message =
            capnp_schema::DynamicStructBuilder::root(&schema, &mut arena, MESSAGE_TYPE_ID)?;
        let mut finish = message.init_struct("finish")?;
        finish.set("questionId", DynamicInput::UInt32(question_id))?;
        finish.set("releaseResultCaps", DynamicInput::Bool(release_result_caps))?;
        finish.set(
            "requireEarlyCancellationWorkaround",
            DynamicInput::Bool(require_early_cancellation_workaround),
        )?;
    }
    owned(arena, limits)
}

pub(crate) fn read_bootstrap(root: &DynamicStruct) -> Result<BootstrapMessage, ProtocolError> {
    let bootstrap = structure(root.get("bootstrap")?, "bootstrap")?;
    Ok(BootstrapMessage {
        question_id: uint32(bootstrap.get("questionId")?, "questionId")?,
    })
}

pub(crate) fn read_call(
    root: &DynamicStruct,
    limits: ProtocolLimits,
) -> Result<CallMessage, ProtocolError> {
    let call = structure(root.get("call")?, "call")?;
    let target = structure(call.get("target")?, "target")?;
    let target = match target.union_discriminant()?.unwrap_or(u16::MAX) {
        0 => CallTarget::ImportedCap(uint32(target.get("importedCap")?, "importedCap")?),
        1 => {
            let promised = structure(target.get("promisedAnswer")?, "promisedAnswer")?;
            let promised = read_promised_answer(&promised, limits)?;
            if promised.transform.is_empty() {
                CallTarget::BootstrapAnswer(promised.question_id)
            } else {
                CallTarget::PromisedAnswer(promised)
            }
        }
        _ => return Err(ProtocolError::UnsupportedFeature("call target")),
    };
    let send_results_to = structure(call.get("sendResultsTo")?, "sendResultsTo")?;
    let send_results_to = match send_results_to.union_discriminant()?.unwrap_or(u16::MAX) {
        0 => SendResultsTo::Caller,
        1 => SendResultsTo::Yourself,
        2 => SendResultsTo::ThirdParty(ThirdPartyToContact::from_opaque(opaque_pointer(
            send_results_to.get("thirdParty")?,
            "sendResultsTo.thirdParty",
            limits,
        )?)),
        _ => return Err(ProtocolError::UnsupportedFeature("sendResultsTo")),
    };
    let params = structure(call.get("params")?, "params")?;
    Ok(CallMessage {
        question_id: uint32(call.get("questionId")?, "questionId")?,
        target,
        interface_id: uint64(call.get("interfaceId")?, "interfaceId")?,
        method_id: uint16(call.get("methodId")?, "methodId")?,
        params: read_payload(&params, "params.content", limits)?,
        send_results_to,
    })
}

pub(crate) fn read_return(
    root: &DynamicStruct,
    limits: ProtocolLimits,
) -> Result<ReturnMessage, ProtocolError> {
    let returned = structure(root.get("return")?, "return")?;
    let payload = match returned.union_discriminant()?.unwrap_or(u16::MAX) {
        0 => {
            let results = structure(returned.get("results")?, "results")?;
            ReturnPayload::Results(read_payload(&results, "results", limits)?)
        }
        1 => {
            let exception = structure(returned.get("exception")?, "exception")?;
            ReturnPayload::Exception(read_exception(&exception)?)
        }
        2 => ReturnPayload::Canceled,
        3 => ReturnPayload::ResultsSentElsewhere,
        4 => ReturnPayload::TakeFromOtherQuestion(uint32(
            returned.get("takeFromOtherQuestion")?,
            "takeFromOtherQuestion",
        )?),
        5 => ReturnPayload::AwaitFromThirdParty(ThirdPartyToAwait::from_opaque(opaque_pointer(
            returned.get("awaitFromThirdParty")?,
            "awaitFromThirdParty",
            limits,
        )?)),
        _ => return Err(ProtocolError::UnsupportedFeature("return payload")),
    };
    Ok(ReturnMessage {
        answer_id: uint32(returned.get("answerId")?, "answerId")?,
        release_param_caps: boolean(returned.get("releaseParamCaps")?, "releaseParamCaps")?,
        no_finish_needed: boolean(returned.get("noFinishNeeded")?, "noFinishNeeded")?,
        payload,
    })
}

pub(crate) fn read_release(root: &DynamicStruct) -> Result<ReleaseMessage, ProtocolError> {
    let release = structure(root.get("release")?, "release")?;
    let reference_count = uint32(release.get("referenceCount")?, "referenceCount")?;
    if reference_count == 0 {
        return Err(ProtocolError::InvalidReferenceCount);
    }
    Ok(ReleaseMessage {
        id: uint32(release.get("id")?, "id")?,
        reference_count,
    })
}

pub(crate) fn read_resolve(
    root: &DynamicStruct,
    limits: ProtocolLimits,
) -> Result<ResolveMessage, ProtocolError> {
    let resolve = structure(root.get("resolve")?, "resolve")?;
    let resolution = match resolve.union_discriminant()?.unwrap_or(u16::MAX) {
        0 => {
            let cap = structure(resolve.get("cap")?, "resolve.cap")?;
            PromiseResolution::Cap(read_cap_descriptor(&cap, limits)?)
        }
        1 => {
            let exception = structure(resolve.get("exception")?, "resolve.exception")?;
            PromiseResolution::Exception(read_exception(&exception)?)
        }
        _ => return Err(ProtocolError::UnsupportedFeature("resolve payload")),
    };
    Ok(ResolveMessage {
        promise_id: uint32(resolve.get("promiseId")?, "promiseId")?,
        resolution,
    })
}

pub(crate) fn read_disembargo(
    root: &DynamicStruct,
    limits: ProtocolLimits,
) -> Result<DisembargoMessage, ProtocolError> {
    let disembargo = structure(root.get("disembargo")?, "disembargo")?;
    let target = structure(disembargo.get("target")?, "disembargo.target")?;
    let context = structure(disembargo.get("context")?, "disembargo.context")?;
    let context = match context.union_discriminant()?.unwrap_or(u16::MAX) {
        0 => DisembargoContext::SenderLoopback(uint32(
            context.get("senderLoopback")?,
            "senderLoopback",
        )?),
        1 => DisembargoContext::ReceiverLoopback(uint32(
            context.get("receiverLoopback")?,
            "receiverLoopback",
        )?),
        2 => {
            let id = data(context.get("accept")?, "disembargo.context.accept")?;
            check_embargo_id(&id, limits)?;
            DisembargoContext::Accept(id)
        }
        _ => return Err(ProtocolError::UnsupportedFeature("disembargo context")),
    };
    Ok(DisembargoMessage {
        target: read_message_target(&target, limits)?,
        context,
    })
}

pub(crate) fn read_provide(
    root: &DynamicStruct,
    limits: ProtocolLimits,
) -> Result<ProvideMessage, ProtocolError> {
    let provide = structure(root.get("provide")?, "provide")?;
    let target = structure(provide.get("target")?, "provide.target")?;
    Ok(ProvideMessage {
        question_id: uint32(provide.get("questionId")?, "provide.questionId")?,
        target: read_message_target(&target, limits)?,
        recipient: ThirdPartyToAwait::from_opaque(opaque_pointer(
            provide.get("recipient")?,
            "provide.recipient",
            limits,
        )?),
    })
}

pub(crate) fn read_accept(
    root: &DynamicStruct,
    limits: ProtocolLimits,
) -> Result<AcceptMessage, ProtocolError> {
    let accept = structure(root.get("accept")?, "accept")?;
    let embargo = data(accept.get("embargo")?, "accept.embargo")?;
    check_embargo_id(&embargo, limits)?;
    Ok(AcceptMessage {
        question_id: uint32(accept.get("questionId")?, "accept.questionId")?,
        provision: ThirdPartyCompletion::from_opaque(opaque_pointer(
            accept.get("provision")?,
            "accept.provision",
            limits,
        )?),
        embargo,
    })
}

pub(crate) fn read_third_party_answer(
    root: &DynamicStruct,
    limits: ProtocolLimits,
) -> Result<ThirdPartyAnswerMessage, ProtocolError> {
    let answer = structure(root.get("thirdPartyAnswer")?, "thirdPartyAnswer")?;
    let answer_id = uint32(answer.get("answerId")?, "thirdPartyAnswer.answerId")?;
    if !(1_u32 << 30..1_u32 << 31).contains(&answer_id) {
        return Err(ProtocolError::InvalidThirdPartyAnswerId(answer_id));
    }
    Ok(ThirdPartyAnswerMessage {
        completion: ThirdPartyCompletion::from_opaque(opaque_pointer(
            answer.get("completion")?,
            "thirdPartyAnswer.completion",
            limits,
        )?),
        answer_id,
    })
}

pub(crate) fn read_finish(root: &DynamicStruct) -> Result<FinishMessage, ProtocolError> {
    let finish = structure(root.get("finish")?, "finish")?;
    Ok(FinishMessage {
        question_id: uint32(finish.get("questionId")?, "questionId")?,
        release_result_caps: boolean(finish.get("releaseResultCaps")?, "releaseResultCaps")?,
        require_early_cancellation_workaround: boolean(
            finish.get("requireEarlyCancellationWorkaround")?,
            "requireEarlyCancellationWorkaround",
        )?,
    })
}

fn arena(limits: ProtocolLimits) -> Result<ExclusiveArena, ProtocolError> {
    ExclusiveArena::new(8, limits.max_message_words)
        .map_err(|error| ProtocolError::Schema(error.to_string()))
}

fn check_exception_limits(
    exception: &RpcException,
    limits: ProtocolLimits,
) -> Result<(), ProtocolError> {
    if exception.reason.len() > limits.max_reason_bytes {
        return Err(ProtocolError::Limit {
            field: "reason",
            requested: exception.reason.len(),
            limit: limits.max_reason_bytes,
        });
    }
    if exception.trace.len() > limits.max_trace_bytes {
        return Err(ProtocolError::Limit {
            field: "trace",
            requested: exception.trace.len(),
            limit: limits.max_trace_bytes,
        });
    }
    Ok(())
}

fn structure(value: DynamicValue, field: &'static str) -> Result<DynamicStruct, ProtocolError> {
    match value {
        DynamicValue::Struct(Some(value)) => Ok(value),
        DynamicValue::Struct(None) => Err(ProtocolError::FieldType(field)),
        _ => Err(ProtocolError::FieldType(field)),
    }
}

fn list(
    value: DynamicValue,
    field: &'static str,
) -> Result<Option<capnp_schema::DynamicList>, ProtocolError> {
    match value {
        DynamicValue::List(value) => Ok(value),
        _ => Err(ProtocolError::FieldType(field)),
    }
}

fn check_cap_table_len(len: usize, limits: ProtocolLimits) -> Result<(), ProtocolError> {
    if len > limits.max_cap_table_entries {
        return Err(ProtocolError::Limit {
            field: "capTable",
            requested: len,
            limit: limits.max_cap_table_entries,
        });
    }
    Ok(())
}

fn write_cap_table(
    payload: &mut capnp_schema::DynamicStructBuilder<'_, '_>,
    cap_table: &[CapDescriptor],
    limits: ProtocolLimits,
) -> Result<(), ProtocolError> {
    let count = u32::try_from(cap_table.len()).map_err(|_| ProtocolError::MessageSizeOverflow)?;
    let mut output = payload.init_list("capTable", count)?;
    for (index, descriptor) in cap_table.iter().enumerate() {
        let index = u32::try_from(index).map_err(|_| ProtocolError::MessageSizeOverflow)?;
        let mut value = output.struct_element(index)?;
        write_cap_descriptor(&mut value, descriptor, limits)?;
    }
    Ok(())
}

fn write_cap_descriptor(
    output: &mut capnp_schema::DynamicStructBuilder<'_, '_>,
    descriptor: &CapDescriptor,
    limits: ProtocolLimits,
) -> Result<(), ProtocolError> {
    match descriptor.descriptor() {
        CapDescriptor::None => output.activate("none")?,
        CapDescriptor::SenderHosted(id) => {
            output.set("senderHosted", DynamicInput::UInt32(*id))?;
        }
        CapDescriptor::SenderPromise(id) => {
            output.set("senderPromise", DynamicInput::UInt32(*id))?;
        }
        CapDescriptor::ReceiverHosted(id) => {
            output.set("receiverHosted", DynamicInput::UInt32(*id))?;
        }
        CapDescriptor::ReceiverAnswer(promised_answer) => {
            let mut promised = output.init_struct("receiverAnswer")?;
            write_promised_answer(&mut promised, promised_answer, limits)?;
        }
        CapDescriptor::ThirdPartyHosted(third_party) => {
            let mut hosted = output.init_struct("thirdPartyHosted")?;
            hosted.set("id", DynamicInput::Pointer(third_party.id.as_opaque()))?;
            hosted.set("vineId", DynamicInput::UInt32(third_party.vine_id))?;
        }
        CapDescriptor::Attached { .. } => {
            return Err(ProtocolError::UnsupportedFeature(
                "nested attached capability descriptor",
            ));
        }
    }
    if let Some(index) = descriptor.resource_index() {
        if index == u8::MAX {
            return Err(ProtocolError::InvalidAttachedResourceIndex);
        }
        output.set("attachedFd", DynamicInput::UInt8(index))?;
    }
    Ok(())
}

fn write_message_target(
    output: &mut capnp_schema::DynamicStructBuilder<'_, '_>,
    target: &CallTarget,
    limits: ProtocolLimits,
) -> Result<(), ProtocolError> {
    match target {
        CallTarget::BootstrapAnswer(answer_id) => {
            let mut promised = output.init_struct("promisedAnswer")?;
            promised.set("questionId", DynamicInput::UInt32(*answer_id))?;
            promised.init_list("transform", 0)?;
        }
        CallTarget::ImportedCap(import_id) => {
            output.set("importedCap", DynamicInput::UInt32(*import_id))?;
        }
        CallTarget::PromisedAnswer(promised_answer) => {
            let mut promised = output.init_struct("promisedAnswer")?;
            write_promised_answer(&mut promised, promised_answer, limits)?;
        }
    }
    Ok(())
}

fn read_message_target(
    target: &DynamicStruct,
    limits: ProtocolLimits,
) -> Result<CallTarget, ProtocolError> {
    match target.union_discriminant()?.unwrap_or(u16::MAX) {
        0 => Ok(CallTarget::ImportedCap(uint32(
            target.get("importedCap")?,
            "importedCap",
        )?)),
        1 => {
            let promised = structure(target.get("promisedAnswer")?, "promisedAnswer")?;
            let promised = read_promised_answer(&promised, limits)?;
            if promised.transform.is_empty() {
                Ok(CallTarget::BootstrapAnswer(promised.question_id))
            } else {
                Ok(CallTarget::PromisedAnswer(promised))
            }
        }
        _ => Err(ProtocolError::UnsupportedFeature("message target")),
    }
}

fn read_cap_descriptor(
    value: &DynamicStruct,
    limits: ProtocolLimits,
) -> Result<CapDescriptor, ProtocolError> {
    let descriptor = match value.union_discriminant()?.unwrap_or(u16::MAX) {
        0 => Ok(CapDescriptor::None),
        1 => Ok(CapDescriptor::SenderHosted(uint32(
            value.get("senderHosted")?,
            "senderHosted",
        )?)),
        2 => Ok(CapDescriptor::SenderPromise(uint32(
            value.get("senderPromise")?,
            "senderPromise",
        )?)),
        3 => Ok(CapDescriptor::ReceiverHosted(uint32(
            value.get("receiverHosted")?,
            "receiverHosted",
        )?)),
        4 => {
            let promised = structure(value.get("receiverAnswer")?, "receiverAnswer")?;
            Ok(CapDescriptor::ReceiverAnswer(read_promised_answer(
                &promised, limits,
            )?))
        }
        5 => {
            let hosted = structure(value.get("thirdPartyHosted")?, "thirdPartyHosted")?;
            Ok(CapDescriptor::ThirdPartyHosted(ThirdPartyCapDescriptor {
                id: ThirdPartyToContact::from_opaque(opaque_pointer(
                    hosted.get("id")?,
                    "thirdPartyHosted.id",
                    limits,
                )?),
                vine_id: uint32(hosted.get("vineId")?, "thirdPartyHosted.vineId")?,
            }))
        }
        _ => Err(ProtocolError::UnsupportedFeature("capability descriptor")),
    }?;
    let attached = uint8(value.get("attachedFd")?, "attachedFd")?;
    Ok(if attached == u8::MAX {
        descriptor
    } else {
        descriptor.with_attachment(attached, None)
    })
}

fn write_promised_answer(
    promised: &mut capnp_schema::DynamicStructBuilder<'_, '_>,
    value: &PromisedAnswer,
    limits: ProtocolLimits,
) -> Result<(), ProtocolError> {
    check_transform_len(value.transform.len(), limits)?;
    promised.set("questionId", DynamicInput::UInt32(value.question_id))?;
    let count =
        u32::try_from(value.transform.len()).map_err(|_| ProtocolError::MessageSizeOverflow)?;
    let mut transform = promised.init_list("transform", count)?;
    for (index, operation) in value.transform.iter().enumerate() {
        let index = u32::try_from(index).map_err(|_| ProtocolError::MessageSizeOverflow)?;
        let mut output = transform.struct_element(index)?;
        match operation {
            PipelineOp::Noop => output.activate("noop")?,
            PipelineOp::GetPointerField(field) => {
                output.set("getPointerField", DynamicInput::UInt16(*field))?;
            }
        }
    }
    Ok(())
}

fn read_promised_answer(
    promised: &DynamicStruct,
    limits: ProtocolLimits,
) -> Result<PromisedAnswer, ProtocolError> {
    let mut output = Vec::new();
    if let Some(transform) = list(promised.get("transform")?, "transform")? {
        let len =
            usize::try_from(transform.len()?).map_err(|_| ProtocolError::MessageSizeOverflow)?;
        check_transform_len(len, limits)?;
        output.reserve(len);
        for index in 0..transform.len()? {
            let operation = structure(transform.get(index)?, "transform operation")?;
            output.push(match operation.union_discriminant()?.unwrap_or(u16::MAX) {
                0 => PipelineOp::Noop,
                1 => PipelineOp::GetPointerField(uint16(
                    operation.get("getPointerField")?,
                    "getPointerField",
                )?),
                _ => return Err(ProtocolError::InvalidPipelineTransform),
            });
        }
    }
    Ok(PromisedAnswer {
        question_id: uint32(promised.get("questionId")?, "promisedAnswer.questionId")?,
        transform: output,
    })
}

fn check_transform_len(len: usize, limits: ProtocolLimits) -> Result<(), ProtocolError> {
    if len > limits.max_pipeline_ops {
        return Err(ProtocolError::Limit {
            field: "promisedAnswer.transform",
            requested: len,
            limit: limits.max_pipeline_ops,
        });
    }
    Ok(())
}

fn read_payload(
    payload: &DynamicStruct,
    field: &'static str,
    limits: ProtocolLimits,
) -> Result<Payload, ProtocolError> {
    let cap_table = match list(payload.get("capTable")?, "capTable")? {
        None => Vec::new(),
        Some(values) => {
            let len =
                usize::try_from(values.len()?).map_err(|_| ProtocolError::MessageSizeOverflow)?;
            check_cap_table_len(len, limits)?;
            let mut output = Vec::with_capacity(len);
            for index in 0..values.len()? {
                let value = structure(values.get(index)?, "capTable element")?;
                output.push(read_cap_descriptor(&value, limits)?);
            }
            output
        }
    };
    Ok(Payload {
        content: any_pointer(payload.get("content")?, field)?,
        cap_table,
    })
}

fn uint16(value: DynamicValue, field: &'static str) -> Result<u16, ProtocolError> {
    match value {
        DynamicValue::UInt16(value) => Ok(value),
        _ => Err(ProtocolError::FieldType(field)),
    }
}

fn uint8(value: DynamicValue, field: &'static str) -> Result<u8, ProtocolError> {
    match value {
        DynamicValue::UInt8(value) => Ok(value),
        _ => Err(ProtocolError::FieldType(field)),
    }
}

fn uint32(value: DynamicValue, field: &'static str) -> Result<u32, ProtocolError> {
    match value {
        DynamicValue::UInt32(value) => Ok(value),
        _ => Err(ProtocolError::FieldType(field)),
    }
}

fn uint64(value: DynamicValue, field: &'static str) -> Result<u64, ProtocolError> {
    match value {
        DynamicValue::UInt64(value) => Ok(value),
        _ => Err(ProtocolError::FieldType(field)),
    }
}

fn boolean(value: DynamicValue, field: &'static str) -> Result<bool, ProtocolError> {
    match value {
        DynamicValue::Bool(value) => Ok(value),
        _ => Err(ProtocolError::FieldType(field)),
    }
}

fn any_pointer(
    value: DynamicValue,
    field: &'static str,
) -> Result<DynamicAnyPointer, ProtocolError> {
    match value {
        DynamicValue::AnyPointer(value) => Ok(value),
        _ => Err(ProtocolError::FieldType(field)),
    }
}

fn opaque_pointer(
    value: DynamicValue,
    field: &'static str,
    limits: ProtocolLimits,
) -> Result<OpaquePointer, ProtocolError> {
    any_pointer(value, field)?
        .to_opaque(crate::protocol::reader_limits(limits))
        .map_err(Into::into)
}

fn data(value: DynamicValue, field: &'static str) -> Result<Vec<u8>, ProtocolError> {
    match value {
        DynamicValue::Data(value) => Ok(value),
        _ => Err(ProtocolError::FieldType(field)),
    }
}

fn check_embargo_id(id: &[u8], limits: ProtocolLimits) -> Result<(), ProtocolError> {
    if id.len() > limits.max_embargo_id_bytes {
        return Err(ProtocolError::Limit {
            field: "embargoId",
            requested: id.len(),
            limit: limits.max_embargo_id_bytes,
        });
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;
    use crate::{ProtocolMessage, read_protocol_message};
    use capnp_message::ReaderLimits;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn data_message(value: u64) -> Arc<OwnedMessage> {
        let mut arena = ExclusiveArena::new(2, 16).expect("arena");
        arena
            .init_root_struct(1, 0)
            .expect("root")
            .set_u64(0, value, 0)
            .expect("value");
        OwnedMessage::new(arena.into_segments(), ReaderLimits::default()).expect("owned")
    }

    fn opaque_token(value: u64) -> OpaquePointer {
        opaque_root(&data_message(value), ProtocolLimits::default()).expect("opaque token")
    }

    fn token_value(pointer: &OpaquePointer) -> u64 {
        let capnp_message::OwnedPointerRef::Struct(value) =
            pointer.open(ReaderLimits::default()).expect("token opens")
        else {
            panic!("token struct")
        };
        value
            .with_reader(|reader| reader.data_section().expect("token data").read_u64(0, 0))
            .expect("token reader")
            .expect("token value")
    }

    #[test]
    fn rpc_messages_through_m36_round_trip() {
        let limits = ProtocolLimits::default();
        assert!(matches!(
            read_protocol_message(encode_bootstrap(7, limits).expect("bootstrap")),
            Ok(ProtocolMessage::Bootstrap(BootstrapMessage {
                question_id: 7
            }))
        ));

        let ProtocolMessage::Call(call) = read_protocol_message(
            encode_call(
                9,
                CallTarget::BootstrapAnswer(7),
                0xfeed,
                3,
                &data_message(42),
                limits,
            )
            .expect("call"),
        )
        .expect("call reads") else {
            panic!("call")
        };
        assert_eq!(call.question_id, 9);
        assert_eq!(call.target, CallTarget::BootstrapAnswer(7));
        assert_eq!(call.interface_id, 0xfeed);
        assert_eq!(call.method_id, 3);
        assert!(matches!(call.params.content, DynamicAnyPointer::Struct(_)));

        let descriptors = vec![
            CapDescriptor::SenderHosted(4),
            CapDescriptor::SenderHosted(4),
            CapDescriptor::SenderPromise(5),
            CapDescriptor::ReceiverHosted(9),
            CapDescriptor::None,
        ];
        let ProtocolMessage::Call(call) = read_protocol_message(
            encode_call_with_capabilities(
                10,
                CallTarget::ImportedCap(12),
                0xbeef,
                5,
                &data_message(43),
                &descriptors,
                limits,
            )
            .expect("capability call"),
        )
        .expect("capability call reads") else {
            panic!("capability call")
        };
        assert_eq!(call.target, CallTarget::ImportedCap(12));
        assert_eq!(call.params.cap_table, descriptors);

        let promised = PromisedAnswer {
            question_id: 10,
            transform: vec![PipelineOp::Noop, PipelineOp::GetPointerField(3)],
        };
        let ProtocolMessage::Call(call) = read_protocol_message(
            encode_call_with_options(
                11,
                CallTarget::PromisedAnswer(promised.clone()),
                0xcafe,
                6,
                &data_message(44),
                &[CapDescriptor::ReceiverAnswer(promised.clone())],
                SendResultsTo::Yourself,
                limits,
            )
            .expect("pipeline call"),
        )
        .expect("pipeline call reads") else {
            panic!("pipeline call")
        };
        assert_eq!(call.target, CallTarget::PromisedAnswer(promised.clone()));
        assert_eq!(call.send_results_to, SendResultsTo::Yourself);
        assert_eq!(
            call.params.cap_table,
            vec![CapDescriptor::ReceiverAnswer(promised)]
        );

        for result in [
            HandlerResult::Results(data_message(88)),
            HandlerResult::Exception(RpcException::new(
                "failed",
                crate::ExceptionType::Overloaded,
            )),
            HandlerResult::Canceled,
            HandlerResult::ResultsSentElsewhere,
            HandlerResult::TakeFromOtherQuestion(71),
        ] {
            let ProtocolMessage::Return(returned) =
                read_protocol_message(encode_return(9, &result, limits).expect("return"))
                    .expect("return reads")
            else {
                panic!("return")
            };
            assert_eq!(returned.answer_id, 9);
            assert!(returned.release_param_caps);
            match (&result, returned.payload) {
                (HandlerResult::Results(_), ReturnPayload::Results(_))
                | (HandlerResult::Exception(_), ReturnPayload::Exception(_))
                | (HandlerResult::Canceled, ReturnPayload::Canceled) => {}
                (HandlerResult::ResultsSentElsewhere, ReturnPayload::ResultsSentElsewhere)
                | (
                    HandlerResult::TakeFromOtherQuestion(71),
                    ReturnPayload::TakeFromOtherQuestion(71),
                ) => {}
                _ => panic!("return variant mismatch"),
            }
        }

        assert!(matches!(
            read_protocol_message(encode_finish(9, limits).expect("finish")),
            Ok(ProtocolMessage::Finish(FinishMessage {
                question_id: 9,
                release_result_caps: true,
                require_early_cancellation_workaround: false,
            }))
        ));

        assert!(matches!(
            read_protocol_message(encode_release(4, 2, limits).expect("release")),
            Ok(ProtocolMessage::Release(ReleaseMessage {
                id: 4,
                reference_count: 2
            }))
        ));
        assert!(matches!(
            encode_release(4, 0, limits),
            Err(ProtocolError::InvalidReferenceCount)
        ));

        for resolution in [
            PromiseResolution::Cap(CapDescriptor::ReceiverHosted(9)),
            PromiseResolution::Exception(RpcException::new(
                "broken promise",
                crate::ExceptionType::Failed,
            )),
        ] {
            let ProtocolMessage::Resolve(actual) =
                read_protocol_message(encode_resolve(5, &resolution, limits).expect("resolve"))
                    .expect("resolve reads")
            else {
                panic!("Resolve")
            };
            assert_eq!(actual.promise_id, 5);
            assert_eq!(actual.resolution, resolution);
        }

        for context in [
            DisembargoContext::SenderLoopback(17),
            DisembargoContext::ReceiverLoopback(17),
        ] {
            let target = CallTarget::ImportedCap(9);
            let ProtocolMessage::Disembargo(actual) = read_protocol_message(
                encode_disembargo(&target, context.clone(), limits).expect("disembargo"),
            )
            .expect("disembargo reads") else {
                panic!("Disembargo")
            };
            assert_eq!(actual.target, target);
            assert_eq!(actual.context, context);
        }
    }

    #[test]
    fn level_three_messages_and_descriptors_round_trip_losslessly() {
        let limits = ProtocolLimits::default();
        let contact = ThirdPartyToContact::from_opaque(opaque_token(41));
        let await_token = ThirdPartyToAwait::from_opaque(opaque_token(42));
        let completion = ThirdPartyCompletion::from_opaque(opaque_token(43));

        let ProtocolMessage::Call(call) = read_protocol_message(
            encode_call_with_options(
                7,
                CallTarget::ImportedCap(3),
                0xfeed,
                9,
                &data_message(44),
                &[CapDescriptor::ThirdPartyHosted(ThirdPartyCapDescriptor {
                    id: contact.clone(),
                    vine_id: 23,
                })],
                SendResultsTo::ThirdParty(contact.clone()),
                limits,
            )
            .expect("third-party call"),
        )
        .expect("third-party call reads") else {
            panic!("Call")
        };
        let SendResultsTo::ThirdParty(actual_contact) = call.send_results_to else {
            panic!("sendResultsTo.thirdParty")
        };
        assert_eq!(token_value(actual_contact.as_opaque()), 41);
        let [CapDescriptor::ThirdPartyHosted(actual_descriptor)] = call.params.cap_table.as_slice()
        else {
            panic!("thirdPartyHosted")
        };
        assert_eq!(actual_descriptor.vine_id, 23);
        assert_eq!(token_value(actual_descriptor.id.as_opaque()), 41);

        let provide = ProvideMessage {
            question_id: 11,
            target: CallTarget::ImportedCap(5),
            recipient: await_token.clone(),
        };
        let ProtocolMessage::Provide(actual) =
            read_protocol_message(encode_provide(&provide, limits).expect("provide"))
                .expect("provide reads")
        else {
            panic!("Provide")
        };
        assert_eq!(actual.question_id, provide.question_id);
        assert_eq!(actual.target, provide.target);
        assert_eq!(token_value(actual.recipient.as_opaque()), 42);

        let accept = AcceptMessage {
            question_id: 12,
            provision: completion.clone(),
            embargo: vec![1, 3, 3, 7],
        };
        let ProtocolMessage::Accept(actual) =
            read_protocol_message(encode_accept(&accept, limits).expect("accept"))
                .expect("accept reads")
        else {
            panic!("Accept")
        };
        assert_eq!(actual.question_id, accept.question_id);
        assert_eq!(actual.embargo, accept.embargo);
        assert_eq!(token_value(actual.provision.as_opaque()), 43);

        for answer_id in [1_u32 << 30, (1_u32 << 31) - 1] {
            let answer = ThirdPartyAnswerMessage {
                completion: completion.clone(),
                answer_id,
            };
            let ProtocolMessage::ThirdPartyAnswer(actual) = read_protocol_message(
                encode_third_party_answer(&answer, limits).expect("third-party answer"),
            )
            .expect("third-party answer reads") else {
                panic!("ThirdPartyAnswer")
            };
            assert_eq!(actual.answer_id, answer.answer_id);
            assert_eq!(token_value(actual.completion.as_opaque()), 43);
        }

        let ProtocolMessage::Return(returned) = read_protocol_message(
            encode_return_await_from_third_party(13, &await_token, limits)
                .expect("awaitFromThirdParty"),
        )
        .expect("awaitFromThirdParty reads") else {
            panic!("Return")
        };
        assert_eq!(returned.answer_id, 13);
        assert!(returned.release_param_caps);
        assert!(matches!(
            returned.payload,
            ReturnPayload::AwaitFromThirdParty(actual)
                if token_value(actual.as_opaque()) == 42
        ));

        let target = CallTarget::ImportedCap(5);
        let context = DisembargoContext::Accept(vec![8, 5, 3]);
        let ProtocolMessage::Disembargo(actual) = read_protocol_message(
            encode_disembargo(&target, context.clone(), limits).expect("accept disembargo"),
        )
        .expect("accept disembargo reads") else {
            panic!("Disembargo")
        };
        assert_eq!(actual, DisembargoMessage { target, context });
    }

    #[test]
    fn level_three_bounds_reject_reserved_answers_and_large_embargoes() {
        let completion = ThirdPartyCompletion::null();
        for answer_id in [(1_u32 << 30) - 1, 1_u32 << 31, u32::MAX] {
            assert!(matches!(
                encode_third_party_answer(
                    &ThirdPartyAnswerMessage {
                        completion: completion.clone(),
                        answer_id,
                    },
                    ProtocolLimits::default(),
                ),
                Err(ProtocolError::InvalidThirdPartyAnswerId(actual)) if actual == answer_id
            ));
        }

        let limits = ProtocolLimits {
            max_embargo_id_bytes: 3,
            ..ProtocolLimits::default()
        };
        assert!(matches!(
            encode_accept(
                &AcceptMessage {
                    question_id: 1,
                    provision: completion,
                    embargo: vec![0; 4],
                },
                limits,
            ),
            Err(ProtocolError::Limit {
                field: "embargoId",
                requested: 4,
                limit: 3,
            })
        ));
        assert!(matches!(
            encode_disembargo(
                &CallTarget::ImportedCap(0),
                DisembargoContext::Accept(vec![0; 4]),
                limits,
            ),
            Err(ProtocolError::Limit {
                field: "embargoId",
                requested: 4,
                limit: 3,
            })
        ));
    }

    #[test]
    fn capability_table_limit_is_checked_before_allocation() {
        let limits = ProtocolLimits {
            max_cap_table_entries: 1,
            ..ProtocolLimits::default()
        };
        assert!(matches!(
            encode_call_with_capabilities(
                1,
                CallTarget::ImportedCap(0),
                0,
                0,
                &data_message(1),
                &[CapDescriptor::None, CapDescriptor::None],
                limits,
            ),
            Err(ProtocolError::Limit {
                field: "capTable",
                requested: 2,
                limit: 1
            })
        ));
    }

    #[test]
    fn promise_transform_limit_applies_to_targets_and_capability_descriptors() {
        let limits = ProtocolLimits {
            max_pipeline_ops: 1,
            ..ProtocolLimits::default()
        };
        let promised = PromisedAnswer {
            question_id: 3,
            transform: vec![PipelineOp::Noop, PipelineOp::GetPointerField(0)],
        };
        for result in [
            encode_call(
                4,
                CallTarget::PromisedAnswer(promised.clone()),
                0,
                0,
                &data_message(1),
                limits,
            ),
            encode_call_with_capabilities(
                4,
                CallTarget::ImportedCap(0),
                0,
                0,
                &data_message(1),
                &[CapDescriptor::ReceiverAnswer(promised.clone())],
                limits,
            ),
        ] {
            assert!(matches!(
                result,
                Err(ProtocolError::Limit {
                    field: "promisedAnswer.transform",
                    requested: 2,
                    limit: 1,
                })
            ));
        }
    }

    #[test]
    fn attached_descriptor_round_trips_and_reserved_index_is_rejected() {
        let limits = ProtocolLimits::default();
        let descriptors = [CapDescriptor::SenderHosted(4).with_attachment(7, None)];
        let encoded = encode_call_with_capabilities(
            1,
            CallTarget::ImportedCap(0),
            0,
            0,
            &data_message(1),
            &descriptors,
            limits,
        )
        .expect("attached descriptor");
        let ProtocolMessage::Call(call) =
            read_protocol_message(encoded).expect("attached descriptor reads")
        else {
            panic!("call")
        };
        assert_eq!(call.params.cap_table, descriptors);
        assert_eq!(call.params.cap_table[0].resource_index(), Some(7));
        assert!(call.params.cap_table[0].attached_resource().is_none());

        assert!(matches!(
            encode_call_with_capabilities(
                1,
                CallTarget::ImportedCap(0),
                0,
                0,
                &data_message(1),
                &[CapDescriptor::None.with_attachment(u8::MAX, None)],
                limits,
            ),
            Err(ProtocolError::InvalidAttachedResourceIndex)
        ));
    }

    #[test]
    fn resource_binding_has_one_owner_and_first_descriptor_wins_duplicates() {
        struct CountDrop(Arc<AtomicUsize>);
        impl Drop for CountDrop {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }

        let drops = Arc::new(AtomicUsize::new(0));
        let mut descriptors = vec![
            CapDescriptor::SenderHosted(1).with_attachment(0, None),
            CapDescriptor::SenderHosted(2).with_attachment(0, None),
            CapDescriptor::SenderHosted(3).with_attachment(9, None),
            CapDescriptor::SenderHosted(4).with_attachment(1, None),
        ];
        let resources = (0..3)
            .map(|_| OwnedResource::new(CountDrop(Arc::clone(&drops)), 0))
            .collect();
        assert_eq!(
            bind_attached_resources(&mut descriptors, resources),
            ResourceBindingStats {
                received: 3,
                attached: 2,
                discarded: 1,
            }
        );
        assert!(descriptors[0].attached_resource().is_some());
        assert!(descriptors[1].attached_resource().is_none());
        assert!(descriptors[2].attached_resource().is_none());
        assert!(descriptors[3].attached_resource().is_some());
        assert_eq!(
            drops.load(Ordering::SeqCst),
            1,
            "unused resource closes now"
        );
        drop(descriptors);
        assert_eq!(drops.load(Ordering::SeqCst), 3, "each owner closes once");
    }

    #[test]
    fn missing_transport_resource_keeps_the_rpc_capability_as_fallback() {
        let mut descriptors = [CapDescriptor::SenderHosted(8).with_attachment(0, None)];
        assert_eq!(
            bind_attached_resources(&mut descriptors, Vec::new()),
            ResourceBindingStats {
                received: 0,
                attached: 0,
                discarded: 0,
            }
        );
        assert_eq!(descriptors[0].descriptor(), &CapDescriptor::SenderHosted(8));
        assert!(descriptors[0].attached_resource().is_none());
    }
}
