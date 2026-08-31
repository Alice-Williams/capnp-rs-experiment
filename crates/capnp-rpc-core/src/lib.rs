#![doc = "Deterministic Cap'n Proto RPC wire types, transport, and protocol state."]
//!
//! The wire schema is the exact `rpc.capnp` and `rpc-twoparty.capnp` pair from
//! pinned Cap'n Proto commit `e7c9cd96f1505b5ae486db7821006c2f5dce5b5b`.
//! M32 binds the revision-tolerant `Message` union and transport envelope;
//! M33 adds the connection actor; M34 adds settled hosted-capability payloads
//! and actor-owned import/export lifetime accounting; M35 adds bounded promise
//! pipelining and two-party tail routing. Unknown union discriminants remain
//! inspectable instead of being rejected. Promise resolution, cancellation,
//! reconnect, and network-resource behavior remain later milestones.

mod actor;
mod capability;
mod level0;
mod protocol;
mod transport;

pub use actor::{
    ActorEffect, ActorLimits, AnswerKey, CompletionToken, ConnectionActor, ConnectionError,
    ConnectionHandle, ConnectionStats, IncomingCallTarget, IncomingRequest, QuestionFuture,
    QuestionKey, QuestionTarget,
};
pub use capability::{
    CapabilityError, CapabilityStats, CapabilityTables, HostedCapability, OutgoingCapability,
    ReceivedCapability,
};
pub use level0::{
    BootstrapMessage, CallMessage, CallTarget, CapDescriptor, FinishMessage, HandlerResult,
    Payload, PipelineOp, PromisedAnswer, ReleaseMessage, ReturnMessage, ReturnPayload,
    SendResultsTo, encode_bootstrap, encode_call, encode_call_with_capabilities,
    encode_call_with_options, encode_finish, encode_finish_with_release, encode_release,
    encode_return,
};
pub use protocol::{
    EXCEPTION_TYPE_ID, ExceptionType, MESSAGE_TYPE_ID, ProtocolError, ProtocolLimits,
    ProtocolMessage, RPC_SCHEMA_SHA256, RPC_TWOPARTY_SCHEMA_SHA256, RpcException, encode_abort,
    encode_unimplemented, protocol_schema, read_protocol_message,
    read_protocol_message_with_limits,
};
pub use transport::{
    DuplexTransport, EnvelopeLimits, MemoryTransport, OwnedResource, TransportEnvelope,
    TransportError, memory_transport_pair,
};
