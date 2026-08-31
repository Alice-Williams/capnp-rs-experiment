//! Bounded bindings for the pinned Cap'n Proto RPC schema.
//!
//! The implemented union members cover the complete two-party Level-1 boundary:
//! bootstrap, call, return, finish, resolve, release, and disembargo plus abort
//! and unimplemented control traffic. Later-level and future union members are
//! retained as raw discriminants, so receiving them is revision-tolerant but
//! does not claim semantics outside M40.

use std::fmt;
use std::sync::{Arc, OnceLock};

use capnp_message::{ExclusiveArena, OwnedMessage, OwnedReadError, ReaderLimits, ValidationError};
use capnp_schema::{
    CompiledSchema, DynamicError, DynamicInput, DynamicStruct, DynamicStructBuilder, DynamicValue,
    LoadLimits, OpaquePointer,
};

use crate::level0::{
    AcceptMessage, BootstrapMessage, CallMessage, DisembargoMessage, FinishMessage, ProvideMessage,
    ReleaseMessage, ResolveMessage, ReturnMessage, ThirdPartyAnswerMessage,
    bind_attached_resources, read_accept, read_bootstrap, read_call, read_disembargo, read_finish,
    read_provide, read_release, read_resolve, read_return, read_third_party_answer,
};
use crate::{OwnedResource, ResourceBindingStats, ReturnPayload};

pub const RPC_SCHEMA_SHA256: &str =
    "2ecc3049d4f7f2d48a3a368dbb9ef4b97b31c1365996d615bd19c267983a1931";
pub const RPC_TWOPARTY_SCHEMA_SHA256: &str =
    "22680f70c56e3c44dc73b52bf8dfd2838a5ea44249be01609be2d362d308b518";
pub const MESSAGE_TYPE_ID: u64 = 0x91b7_9f1f_808d_b032;
pub const EXCEPTION_TYPE_ID: u64 = 0xd625_b706_3acf_691a;

const REQUEST: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/schema/rpc-protocol-request.bin"
));

static SCHEMA: OnceLock<Arc<CompiledSchema>> = OnceLock::new();

/// Bounds construction and human-readable exception material before allocation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProtocolLimits {
    pub max_message_words: u32,
    pub max_reason_bytes: usize,
    pub max_trace_bytes: usize,
    pub max_cap_table_entries: usize,
    pub max_pipeline_ops: usize,
    pub max_embargo_id_bytes: usize,
}

impl Default for ProtocolLimits {
    fn default() -> Self {
        Self {
            max_message_words: 8 * 1024 * 1024,
            max_reason_bytes: 64 * 1024,
            max_trace_bytes: 1024 * 1024,
            max_cap_table_entries: 4096,
            max_pipeline_ops: 64,
            max_embargo_id_bytes: 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExceptionType {
    Failed,
    Overloaded,
    Disconnected,
    Unimplemented,
    Unrecognized(u16),
}

impl ExceptionType {
    const fn ordinal(self) -> u16 {
        match self {
            Self::Failed => 0,
            Self::Overloaded => 1,
            Self::Disconnected => 2,
            Self::Unimplemented => 3,
            Self::Unrecognized(value) => value,
        }
    }

    const fn from_ordinal(value: u16) -> Self {
        match value {
            0 => Self::Failed,
            1 => Self::Overloaded,
            2 => Self::Disconnected,
            3 => Self::Unimplemented,
            value => Self::Unrecognized(value),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RpcException {
    pub reason: String,
    pub kind: ExceptionType,
    pub trace: String,
}

impl RpcException {
    pub fn new(reason: impl Into<String>, kind: ExceptionType) -> Self {
        Self {
            reason: reason.into(),
            kind,
            trace: String::new(),
        }
    }

    pub fn with_trace(mut self, trace: impl Into<String>) -> Self {
        self.trace = trace.into();
        self
    }
}

#[derive(Clone, Debug)]
pub enum ProtocolMessage {
    /// The nested value is absent only for a selected-but-null pointer, which
    /// old and revised peers must tolerate as the schema default.
    Unimplemented(Option<DynamicStruct>),
    Abort(RpcException),
    Bootstrap(BootstrapMessage),
    Call(CallMessage),
    Return(ReturnMessage),
    Finish(FinishMessage),
    Resolve(ResolveMessage),
    Release(ReleaseMessage),
    Disembargo(DisembargoMessage),
    Provide(ProvideMessage),
    Accept(AcceptMessage),
    ThirdPartyAnswer(ThirdPartyAnswerMessage),
    /// Includes both later-level messages and discriminants added by a newer
    /// schema revision. The M40 Level-1 boundary assigns neither semantics.
    Unsupported {
        discriminant: u16,
    },
}

impl ProtocolMessage {
    pub fn bind_resources(&mut self, resources: Vec<OwnedResource>) -> ResourceBindingStats {
        match self {
            Self::Call(message) => {
                bind_attached_resources(&mut message.params.cap_table, resources)
            }
            Self::Return(message) => match &mut message.payload {
                ReturnPayload::Results(payload) => {
                    bind_attached_resources(&mut payload.cap_table, resources)
                }
                _ => discard_resources(resources),
            },
            Self::Resolve(message) => match &mut message.resolution {
                crate::PromiseResolution::Cap(descriptor) => {
                    bind_attached_resources(core::slice::from_mut(descriptor), resources)
                }
                crate::PromiseResolution::Exception(_) => discard_resources(resources),
            },
            _ => discard_resources(resources),
        }
    }
}

fn discard_resources(resources: Vec<OwnedResource>) -> ResourceBindingStats {
    let received = resources.len();
    drop(resources);
    ResourceBindingStats {
        received,
        attached: 0,
        discarded: received,
    }
}

#[derive(Debug)]
pub enum ProtocolError {
    Schema(String),
    Dynamic(DynamicError),
    Message(OwnedReadError),
    Validation(ValidationError),
    FieldType(&'static str),
    UnsupportedFeature(&'static str),
    InvalidAttachedResourceIndex,
    InvalidThirdPartyAnswerId(u32),
    InvalidReferenceCount,
    InvalidPipelineTransform,
    Limit {
        field: &'static str,
        requested: usize,
        limit: usize,
    },
    MessageSizeOverflow,
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Schema(error) => write!(formatter, "pinned RPC schema: {error}"),
            Self::Dynamic(error) => error.fmt(formatter),
            Self::Message(error) => error.fmt(formatter),
            Self::Validation(error) => error.fmt(formatter),
            Self::FieldType(field) => write!(formatter, "RPC field `{field}` has the wrong type"),
            Self::UnsupportedFeature(feature) => {
                write!(
                    formatter,
                    "RPC feature `{feature}` is outside the implemented subset"
                )
            }
            Self::InvalidAttachedResourceIndex => {
                formatter.write_str("RPC attached resource index 255 is reserved for no resource")
            }
            Self::InvalidThirdPartyAnswerId(id) => write!(
                formatter,
                "RPC third-party answer ID {id} is outside the callee range [2^30, 2^31)"
            ),
            Self::InvalidReferenceCount => {
                formatter.write_str("RPC release reference count must be non-zero")
            }
            Self::InvalidPipelineTransform => {
                formatter.write_str("RPC promised-answer transform contains an unknown operation")
            }
            Self::Limit {
                field,
                requested,
                limit,
            } => write!(
                formatter,
                "RPC field `{field}` requires {requested} bytes; limit is {limit}"
            ),
            Self::MessageSizeOverflow => formatter.write_str("RPC message size overflow"),
        }
    }
}

impl std::error::Error for ProtocolError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Dynamic(error) => Some(error),
            Self::Message(error) => Some(error),
            Self::Validation(error) => Some(error),
            Self::Schema(_)
            | Self::FieldType(_)
            | Self::UnsupportedFeature(_)
            | Self::InvalidAttachedResourceIndex
            | Self::InvalidThirdPartyAnswerId(_)
            | Self::InvalidReferenceCount
            | Self::InvalidPipelineTransform
            | Self::Limit { .. }
            | Self::MessageSizeOverflow => None,
        }
    }
}

impl From<DynamicError> for ProtocolError {
    fn from(value: DynamicError) -> Self {
        Self::Dynamic(value)
    }
}

impl From<OwnedReadError> for ProtocolError {
    fn from(value: OwnedReadError) -> Self {
        Self::Message(value)
    }
}

impl From<ValidationError> for ProtocolError {
    fn from(value: ValidationError) -> Self {
        Self::Validation(value)
    }
}

/// Loads the deterministic compiler request generated from both pinned RPC
/// schemas. Successful loads are shared for the life of the process.
pub fn protocol_schema() -> Result<Arc<CompiledSchema>, ProtocolError> {
    if let Some(schema) = SCHEMA.get() {
        return Ok(Arc::clone(schema));
    }
    let schema = Arc::new(
        CompiledSchema::from_code_generator_request(REQUEST, LoadLimits::default())
            .map_err(|error| ProtocolError::Schema(error.to_string()))?,
    );
    let _ = SCHEMA.set(Arc::clone(&schema));
    Ok(SCHEMA.get().map_or(schema, Arc::clone))
}

/// Encodes an `abort` union member using only revision-stable fields.
pub fn encode_abort(
    exception: &RpcException,
    limits: ProtocolLimits,
) -> Result<Arc<OwnedMessage>, ProtocolError> {
    check_text("reason", &exception.reason, limits.max_reason_bytes)?;
    check_text("trace", &exception.trace, limits.max_trace_bytes)?;
    let schema = protocol_schema()?;
    let mut arena = ExclusiveArena::new(8, limits.max_message_words)
        .map_err(|error| ProtocolError::Schema(error.to_string()))?;
    {
        let mut message = DynamicStructBuilder::root(&schema, &mut arena, MESSAGE_TYPE_ID)?;
        let mut abort = message.init_struct("abort")?;
        abort.set("reason", DynamicInput::Text(&exception.reason))?;
        abort.set("type", DynamicInput::Enum(exception.kind.ordinal()))?;
        if !exception.trace.is_empty() {
            abort.set("trace", DynamicInput::Text(&exception.trace))?;
        }
    }
    owned(arena, limits)
}

/// Wraps a complete RPC message in the protocol's `unimplemented` member.
/// The echoed graph is copied and validated; capability pointers are rejected
/// by the schema-independent graph copier rather than silently renumbered.
pub fn encode_unimplemented(
    echoed: &Arc<OwnedMessage>,
    limits: ProtocolLimits,
) -> Result<Arc<OwnedMessage>, ProtocolError> {
    let bytes = message_bytes(echoed)?;
    let byte_limit = usize::try_from(limits.max_message_words)
        .ok()
        .and_then(|words| words.checked_mul(8))
        .ok_or(ProtocolError::MessageSizeOverflow)?;
    if bytes > byte_limit {
        return Err(ProtocolError::Limit {
            field: "unimplemented",
            requested: bytes,
            limit: byte_limit,
        });
    }
    let copied = (0..echoed.segment_count())
        .map(|index| {
            echoed
                .segment(u32::try_from(index).map_err(|_| ProtocolError::MessageSizeOverflow)?)
                .map(|segment| segment.to_vec().into_boxed_slice())
                .ok_or(ProtocolError::MessageSizeOverflow)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let pointer = OpaquePointer::from_root_segments(copied, reader_limits(limits))?;
    let schema = protocol_schema()?;
    let mut arena = ExclusiveArena::new(8, limits.max_message_words)
        .map_err(|error| ProtocolError::Schema(error.to_string()))?;
    {
        let mut message = DynamicStructBuilder::root(&schema, &mut arena, MESSAGE_TYPE_ID)?;
        message.set("unimplemented", DynamicInput::Pointer(&pointer))?;
    }
    owned(arena, limits)
}

/// Decodes the M32 control subset while retaining revision-unknown union tags.
pub fn read_protocol_message(message: Arc<OwnedMessage>) -> Result<ProtocolMessage, ProtocolError> {
    read_protocol_message_with_limits(message, ProtocolLimits::default())
}

pub fn read_protocol_message_with_limits(
    message: Arc<OwnedMessage>,
    limits: ProtocolLimits,
) -> Result<ProtocolMessage, ProtocolError> {
    let schema = protocol_schema()?;
    let root = DynamicStruct::root(schema, message, MESSAGE_TYPE_ID)?;
    read_protocol_struct_with_limits(root, limits)
}

pub(crate) fn read_protocol_struct(root: DynamicStruct) -> Result<ProtocolMessage, ProtocolError> {
    read_protocol_struct_with_limits(root, ProtocolLimits::default())
}

fn read_protocol_struct_with_limits(
    root: DynamicStruct,
    limits: ProtocolLimits,
) -> Result<ProtocolMessage, ProtocolError> {
    let discriminant = root.union_discriminant()?.unwrap_or(u16::MAX);
    match discriminant {
        0 => match root.get("unimplemented")? {
            DynamicValue::Struct(value) => Ok(ProtocolMessage::Unimplemented(value)),
            _ => Err(ProtocolError::FieldType("unimplemented")),
        },
        1 => match root.get("abort")? {
            DynamicValue::Struct(Some(value)) => {
                Ok(ProtocolMessage::Abort(read_exception(&value)?))
            }
            DynamicValue::Struct(None) => Ok(ProtocolMessage::Abort(RpcException::new(
                "",
                ExceptionType::Failed,
            ))),
            _ => Err(ProtocolError::FieldType("abort")),
        },
        8 => Ok(ProtocolMessage::Bootstrap(read_bootstrap(&root)?)),
        2 => Ok(ProtocolMessage::Call(read_call(&root, limits)?)),
        3 => Ok(ProtocolMessage::Return(read_return(&root, limits)?)),
        4 => Ok(ProtocolMessage::Finish(read_finish(&root)?)),
        5 => Ok(ProtocolMessage::Resolve(read_resolve(&root, limits)?)),
        6 => Ok(ProtocolMessage::Release(read_release(&root)?)),
        13 => Ok(ProtocolMessage::Disembargo(read_disembargo(&root, limits)?)),
        10 => Ok(ProtocolMessage::Provide(read_provide(&root, limits)?)),
        11 => Ok(ProtocolMessage::Accept(read_accept(&root, limits)?)),
        14 => Ok(ProtocolMessage::ThirdPartyAnswer(read_third_party_answer(
            &root, limits,
        )?)),
        discriminant => Ok(ProtocolMessage::Unsupported { discriminant }),
    }
}

pub(crate) fn read_exception(value: &DynamicStruct) -> Result<RpcException, ProtocolError> {
    let reason = match value.get("reason")? {
        DynamicValue::Text(value) => value,
        _ => return Err(ProtocolError::FieldType("reason")),
    };
    let kind = match value.get("type")? {
        DynamicValue::Enum(value) => ExceptionType::from_ordinal(value.ordinal),
        _ => return Err(ProtocolError::FieldType("type")),
    };
    let trace = match value.get("trace")? {
        DynamicValue::Text(value) => value,
        _ => return Err(ProtocolError::FieldType("trace")),
    };
    Ok(RpcException {
        reason,
        kind,
        trace,
    })
}

fn check_text(field: &'static str, value: &str, limit: usize) -> Result<(), ProtocolError> {
    if value.len() > limit {
        return Err(ProtocolError::Limit {
            field,
            requested: value.len(),
            limit,
        });
    }
    Ok(())
}

pub(crate) fn message_bytes(message: &OwnedMessage) -> Result<usize, ProtocolError> {
    let mut total = 0usize;
    for index in 0..message.segment_count() {
        let id = u32::try_from(index).map_err(|_| ProtocolError::MessageSizeOverflow)?;
        total = total
            .checked_add(
                message
                    .segment(id)
                    .ok_or(ProtocolError::MessageSizeOverflow)?
                    .len(),
            )
            .ok_or(ProtocolError::MessageSizeOverflow)?;
    }
    Ok(total)
}

pub(crate) fn reader_limits(limits: ProtocolLimits) -> ReaderLimits {
    ReaderLimits {
        traversal_words: u64::from(limits.max_message_words),
        nesting_levels: 64,
    }
}

pub(crate) fn owned(
    arena: ExclusiveArena,
    limits: ProtocolLimits,
) -> Result<Arc<OwnedMessage>, ProtocolError> {
    OwnedMessage::new(arena.into_segments(), reader_limits(limits)).map_err(Into::into)
}

pub(crate) fn opaque_root(
    message: &Arc<OwnedMessage>,
    limits: ProtocolLimits,
) -> Result<OpaquePointer, ProtocolError> {
    let copied = (0..message.segment_count())
        .map(|index| {
            message
                .segment(u32::try_from(index).map_err(|_| ProtocolError::MessageSizeOverflow)?)
                .map(|segment| segment.to_vec().into_boxed_slice())
                .ok_or(ProtocolError::MessageSizeOverflow)
        })
        .collect::<Result<Vec<_>, _>>()?;
    OpaquePointer::from_root_segments(copied, reader_limits(limits)).map_err(Into::into)
}

pub(crate) fn write_exception(
    output: &mut DynamicStructBuilder<'_, '_>,
    exception: &RpcException,
) -> Result<(), ProtocolError> {
    output.set("reason", DynamicInput::Text(&exception.reason))?;
    output.set("type", DynamicInput::Enum(exception.kind.ordinal()))?;
    if !exception.trace.is_empty() {
        output.set("trace", DynamicInput::Text(&exception.trace))?;
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;
    use std::pin::Pin;
    use std::task::{Context, Poll, Waker};

    use capnp_schema::NodeKind;

    use crate::{DuplexTransport, EnvelopeLimits, TransportEnvelope, memory_transport_pair};

    const RPC_FILE_ID: u64 = 0xb312_981b_2552_a250;
    const TWOPARTY_FILE_ID: u64 = 0xa184_c788_5cda_f2a1;

    #[test]
    fn embedded_request_binds_both_exact_pinned_files() {
        let schema = protocol_schema().expect("schema loads");
        assert!(matches!(
            schema.node(RPC_FILE_ID).map(|node| &node.kind),
            Some(NodeKind::File)
        ));
        assert!(matches!(
            schema.node(TWOPARTY_FILE_ID).map(|node| &node.kind),
            Some(NodeKind::File)
        ));
        assert!(schema.node(MESSAGE_TYPE_ID).is_some());
        assert!(schema.node(EXCEPTION_TYPE_ID).is_some());
    }

    #[test]
    fn abort_round_trips_revision_stable_fields() {
        let expected = RpcException::new("bounded failure", ExceptionType::Overloaded)
            .with_trace("remote trace");
        let message = encode_abort(&expected, ProtocolLimits::default()).expect("encodes");
        let ProtocolMessage::Abort(actual) = read_protocol_message(message).expect("decodes")
        else {
            panic!("expected abort")
        };
        assert_eq!(actual, expected);
    }

    #[test]
    fn unimplemented_preserves_the_echoed_message() {
        let abort = encode_abort(
            &RpcException::new("echo me", ExceptionType::Unimplemented),
            ProtocolLimits::default(),
        )
        .expect("abort");
        let wrapped = encode_unimplemented(&abort, ProtocolLimits::default()).expect("wraps");
        let ProtocolMessage::Unimplemented(Some(echoed)) =
            read_protocol_message(wrapped).expect("decodes")
        else {
            panic!("expected nested message")
        };
        assert_eq!(echoed.union_discriminant().expect("union"), Some(1));
        let DynamicValue::Struct(Some(exception)) = echoed.get("abort").expect("abort field")
        else {
            panic!("expected nested abort")
        };
        assert_eq!(
            read_exception(&exception).expect("exception").reason,
            "echo me"
        );
    }

    #[test]
    fn unknown_union_and_enum_values_are_retained() {
        let schema = protocol_schema().expect("schema");
        let structure = schema.node(MESSAGE_TYPE_ID).expect("message node");
        let NodeKind::Struct(structure) = &structure.kind else {
            panic!("message is a struct")
        };
        let mut arena = ExclusiveArena::new(1, 8).expect("arena");
        arena
            .init_root_struct(structure.data_word_count, structure.pointer_count)
            .expect("root")
            .set_u16(structure.discriminant_offset, 0xfffe, 0)
            .expect("unknown discriminant");
        let message = owned(arena, ProtocolLimits::default()).expect("owned");
        assert!(matches!(
            read_protocol_message(message),
            Ok(ProtocolMessage::Unsupported {
                discriminant: 0xfffe
            })
        ));

        let exception = RpcException::new("future", ExceptionType::Unrecognized(19));
        let message = encode_abort(&exception, ProtocolLimits::default()).expect("encodes");
        let ProtocolMessage::Abort(actual) = read_protocol_message(message).expect("reads") else {
            panic!("abort")
        };
        assert_eq!(actual.kind, ExceptionType::Unrecognized(19));
    }

    #[test]
    fn exception_limits_fail_before_message_construction() {
        let limits = ProtocolLimits {
            max_reason_bytes: 3,
            ..ProtocolLimits::default()
        };
        assert!(matches!(
            encode_abort(&RpcException::new("four", ExceptionType::Failed), limits),
            Err(ProtocolError::Limit {
                field: "reason",
                requested: 4,
                limit: 3
            })
        ));
    }

    #[test]
    fn in_memory_peers_exchange_abort_and_unimplemented() {
        let protocol_limits = ProtocolLimits::default();
        let envelope_limits = EnvelopeLimits::default();
        let (mut alice, mut bob) = memory_transport_pair(envelope_limits);
        let abort = encode_abort(
            &RpcException::new("not here", ExceptionType::Unimplemented),
            protocol_limits,
        )
        .expect("abort");
        let mut outbound = Some(
            TransportEnvelope::new(Arc::clone(&abort), Vec::new(), envelope_limits)
                .expect("envelope"),
        );
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(
            Pin::new(&mut alice).poll_send(&mut context, &mut outbound),
            Poll::Ready(Ok(()))
        ));
        let Poll::Ready(Ok(Some(received))) = Pin::new(&mut bob).poll_receive(&mut context) else {
            panic!("bob receives abort")
        };
        assert!(matches!(
            read_protocol_message(Arc::clone(received.message())),
            Ok(ProtocolMessage::Abort(RpcException {
                kind: ExceptionType::Unimplemented,
                ..
            }))
        ));

        let echoed = encode_unimplemented(received.message(), protocol_limits).expect("echo");
        let mut response =
            Some(TransportEnvelope::new(echoed, Vec::new(), envelope_limits).expect("response"));
        assert!(matches!(
            Pin::new(&mut bob).poll_send(&mut context, &mut response),
            Poll::Ready(Ok(()))
        ));
        let Poll::Ready(Ok(Some(received))) = Pin::new(&mut alice).poll_receive(&mut context)
        else {
            panic!("alice receives unimplemented")
        };
        let ProtocolMessage::Unimplemented(Some(echoed)) =
            read_protocol_message(Arc::clone(received.message())).expect("control message")
        else {
            panic!("unimplemented")
        };
        assert_eq!(
            echoed.union_discriminant().expect("echo discriminant"),
            Some(1)
        );
    }
}
