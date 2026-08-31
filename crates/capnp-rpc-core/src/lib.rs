#![doc = "Deterministic Cap'n Proto RPC wire types, transport, and protocol state."]
//!
//! The wire schema is the exact `rpc.capnp` and `rpc-twoparty.capnp` pair from
//! pinned Cap'n Proto commit `e7c9cd96f1505b5ae486db7821006c2f5dce5b5b`.
//! M32 binds the revision-tolerant `Message` union and transport envelope;
//! M33 adds the connection actor; M34 adds settled hosted-capability payloads
//! and actor-owned import/export lifetime accounting; M35 adds bounded promise
//! pipelining and two-party tail routing; M36 adds `Resolve`, frozen promise
//! routes, and loopback disembargo/E-order. Unknown union discriminants remain
//! inspectable instead of being rejected. Streaming, cancellation, reconnect,
//! and network-resource behavior remain later milestones.

mod actor;
mod capability;
mod level0;
mod protocol;
mod transport;

pub use actor::{
    ActorEffect, ActorLimits, AnswerKey, CompletionToken, ConnectionActor, ConnectionError,
    ConnectionHandle, ConnectionStats, IncomingCallTarget, IncomingRequest, LocalCompletionToken,
    PromiseResolver, QuestionFuture, QuestionKey, QuestionTarget,
};
pub use capability::{
    CapabilityError, CapabilityStats, CapabilityTables, HostedCapability, OutgoingCapability,
    PromiseCapability, ReceivedCapability,
};
pub use level0::{
    BootstrapMessage, CallMessage, CallTarget, CapDescriptor, DisembargoContext, DisembargoMessage,
    FinishMessage, HandlerResult, Payload, PipelineOp, PromiseResolution, PromisedAnswer,
    ReleaseMessage, ResolveMessage, ReturnMessage, ReturnPayload, SendResultsTo, encode_bootstrap,
    encode_call, encode_call_with_capabilities, encode_call_with_options, encode_disembargo,
    encode_finish, encode_finish_with_release, encode_release, encode_resolve, encode_return,
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
