//! Pinned Level-0 RPC message bindings.
//!
//! Level 0 deliberately has no import/export capability descriptors. Calls may
//! target only the raw capability returned by a prior `Bootstrap`, and payload
//! capability tables must be empty.

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CallTarget {
    BootstrapAnswer(u32),
}

#[derive(Clone, Debug)]
pub struct CallMessage {
    pub question_id: u32,
    pub target: CallTarget,
    pub interface_id: u64,
    pub method_id: u16,
    pub params: DynamicAnyPointer,
}

#[derive(Clone, Debug)]
pub enum ReturnPayload {
    Results(DynamicAnyPointer),
    Exception(RpcException),
    Canceled,
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

/// A handler completion submitted back to the owning connection actor.
#[derive(Clone, Debug)]
pub enum HandlerResult {
    Results(Arc<OwnedMessage>),
    Exception(RpcException),
    Canceled,
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
            let mut send_results_to = call.group("sendResultsTo")?;
            send_results_to.activate("caller")?;
        }
        {
            let mut target_builder = call.init_struct("target")?;
            match target {
                CallTarget::BootstrapAnswer(answer_id) => {
                    let mut promised = target_builder.init_struct("promisedAnswer")?;
                    promised.set("questionId", DynamicInput::UInt32(answer_id))?;
                    promised.init_list("transform", 0)?;
                }
            }
        }
        {
            let mut payload = call.init_struct("params")?;
            payload.set("content", DynamicInput::Pointer(&content))?;
            payload.init_list("capTable", 0)?;
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
        HandlerResult::Results(message) => Some(opaque_root(message, limits)?),
        HandlerResult::Exception(exception) => {
            check_exception_limits(exception, limits)?;
            None
        }
        HandlerResult::Canceled => None,
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
            (HandlerResult::Exception(exception), None) => {
                let mut output = returned.init_struct("exception")?;
                write_exception(&mut output, exception)?;
            }
            (HandlerResult::Canceled, None) => returned.activate("canceled")?,
            _ => return Err(ProtocolError::FieldType("return")),
        }
    }
    owned(arena, limits)
}

pub fn encode_finish(
    question_id: u32,
    limits: ProtocolLimits,
) -> Result<Arc<OwnedMessage>, ProtocolError> {
    let schema = protocol_schema()?;
    let mut arena = arena(limits)?;
    {
        let mut message =
            capnp_schema::DynamicStructBuilder::root(&schema, &mut arena, MESSAGE_TYPE_ID)?;
        let mut finish = message.init_struct("finish")?;
        finish.set("questionId", DynamicInput::UInt32(question_id))?;
        finish.set("releaseResultCaps", DynamicInput::Bool(true))?;
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

pub(crate) fn read_call(root: &DynamicStruct) -> Result<CallMessage, ProtocolError> {
    let call = structure(root.get("call")?, "call")?;
    let target = structure(call.get("target")?, "target")?;
    if target.union_discriminant()? != Some(1) {
        return Err(ProtocolError::UnsupportedLevel0("call target"));
    }
    let promised = structure(target.get("promisedAnswer")?, "promisedAnswer")?;
    let transform = list(promised.get("transform")?, "transform")?;
    if let Some(transform) = transform {
        if transform.len()? != 0 {
            return Err(ProtocolError::UnsupportedLevel0(
                "non-empty promised-answer transform",
            ));
        }
    }
    let send_results_to = structure(call.get("sendResultsTo")?, "sendResultsTo")?;
    if send_results_to.union_discriminant()? != Some(0) {
        return Err(ProtocolError::UnsupportedLevel0("sendResultsTo"));
    }
    let params = structure(call.get("params")?, "params")?;
    require_empty_cap_table(&params)?;
    Ok(CallMessage {
        question_id: uint32(call.get("questionId")?, "questionId")?,
        target: CallTarget::BootstrapAnswer(uint32(
            promised.get("questionId")?,
            "promisedAnswer.questionId",
        )?),
        interface_id: uint64(call.get("interfaceId")?, "interfaceId")?,
        method_id: uint16(call.get("methodId")?, "methodId")?,
        params: any_pointer(params.get("content")?, "params.content")?,
    })
}

pub(crate) fn read_return(root: &DynamicStruct) -> Result<ReturnMessage, ProtocolError> {
    let returned = structure(root.get("return")?, "return")?;
    let payload = match returned.union_discriminant()?.unwrap_or(u16::MAX) {
        0 => {
            let results = structure(returned.get("results")?, "results")?;
            require_empty_cap_table(&results)?;
            ReturnPayload::Results(any_pointer(results.get("content")?, "results.content")?)
        }
        1 => {
            let exception = structure(returned.get("exception")?, "exception")?;
            ReturnPayload::Exception(read_exception(&exception)?)
        }
        2 => ReturnPayload::Canceled,
        _ => return Err(ProtocolError::UnsupportedLevel0("return payload")),
    };
    Ok(ReturnMessage {
        answer_id: uint32(returned.get("answerId")?, "answerId")?,
        release_param_caps: boolean(returned.get("releaseParamCaps")?, "releaseParamCaps")?,
        no_finish_needed: boolean(returned.get("noFinishNeeded")?, "noFinishNeeded")?,
        payload,
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

fn require_empty_cap_table(payload: &DynamicStruct) -> Result<(), ProtocolError> {
    if let Some(cap_table) = list(payload.get("capTable")?, "capTable")? {
        if cap_table.len()? != 0 {
            return Err(ProtocolError::UnsupportedLevel0("payload capabilities"));
        }
    }
    Ok(())
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
    fn every_level_zero_message_round_trips() {
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
        assert!(matches!(call.params, DynamicAnyPointer::Struct(_)));

        for result in [
            HandlerResult::Results(data_message(88)),
            HandlerResult::Exception(RpcException::new(
                "failed",
                crate::ExceptionType::Overloaded,
            )),
            HandlerResult::Canceled,
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
    }
}
