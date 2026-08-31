//! Pinned two-party Level-1 RPC message bindings through promise pipelining.
//!
//! M34 adds settled hosted descriptors and `Release`; M35 adds promised-answer
//! transforms, `receiverAnswer`, redirected results, and Level-1 tail routing.
//! `Resolve`, embargo, third-party, and attached-resource behavior remains
//! owned by later milestones.

use std::sync::Arc;

use capnp_message::{ExclusiveArena, OwnedMessage};
use capnp_schema::{DynamicAnyPointer, DynamicInput, DynamicStruct, DynamicValue};

use crate::protocol::{
    MESSAGE_TYPE_ID, ProtocolError, ProtocolLimits, RpcException, opaque_root, owned,
    protocol_schema, read_exception, write_exception,
};

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
    ReceiverHosted(u32),
    ReceiverAnswer(PromisedAnswer),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SendResultsTo {
    Caller,
    Yourself,
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
            payload.set("content", DynamicInput::Pointer(&content))?;
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
        returned.set("releaseParamCaps", DynamicInput::Bool(true))?;
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
            DynamicInput::Bool(false),
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
        match descriptor {
            CapDescriptor::None => value.activate("none")?,
            CapDescriptor::SenderHosted(id) => {
                value.set("senderHosted", DynamicInput::UInt32(*id))?;
            }
            CapDescriptor::ReceiverHosted(id) => {
                value.set("receiverHosted", DynamicInput::UInt32(*id))?;
            }
            CapDescriptor::ReceiverAnswer(promised_answer) => {
                let mut promised = value.init_struct("receiverAnswer")?;
                write_promised_answer(&mut promised, promised_answer, limits)?;
            }
        }
    }
    Ok(())
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
                let descriptor = match value.union_discriminant()?.unwrap_or(u16::MAX) {
                    0 => CapDescriptor::None,
                    1 => CapDescriptor::SenderHosted(uint32(
                        value.get("senderHosted")?,
                        "senderHosted",
                    )?),
                    3 => CapDescriptor::ReceiverHosted(uint32(
                        value.get("receiverHosted")?,
                        "receiverHosted",
                    )?),
                    2 => return Err(ProtocolError::UnsupportedFeature("senderPromise")),
                    4 => {
                        let promised = structure(value.get("receiverAnswer")?, "receiverAnswer")?;
                        CapDescriptor::ReceiverAnswer(read_promised_answer(&promised, limits)?)
                    }
                    5 => return Err(ProtocolError::UnsupportedFeature("thirdPartyHosted")),
                    _ => return Err(ProtocolError::UnsupportedFeature("capability descriptor")),
                };
                output.push(descriptor);
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

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;
    use crate::{ProtocolMessage, read_protocol_message};
    use capnp_message::ReaderLimits;

    fn data_message(value: u64) -> Arc<OwnedMessage> {
        let mut arena = ExclusiveArena::new(2, 16).expect("arena");
        arena
            .init_root_struct(1, 0)
            .expect("root")
            .set_u64(0, value, 0)
            .expect("value");
        OwnedMessage::new(arena.into_segments(), ReaderLimits::default()).expect("owned")
    }

    #[test]
    fn rpc_messages_through_m35_round_trip() {
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
}
