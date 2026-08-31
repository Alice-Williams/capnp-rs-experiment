#![doc = "Deterministic Cap'n Proto RPC wire types, transport, and protocol state."]
//!
//! The wire schema is the exact `rpc.capnp` and `rpc-twoparty.capnp` pair from
//! pinned Cap'n Proto commit `e7c9cd96f1505b5ae486db7821006c2f5dce5b5b`.
//! M32 binds the revision-tolerant `Message` union and transport envelope;
//! M33 adds the connection actor; M34 adds settled hosted-capability payloads
//! and actor-owned import/export lifetime accounting; M35 adds bounded promise
//! pipelining and two-party tail routing; M36 adds `Resolve`, frozen promise
//! routes, and loopback disembargo/E-order; M37 adds bounded incoming call
//! bytes; M38 adds cooperative cancellation and complete disconnect cleanup.
//! Unknown union discriminants remain inspectable instead of being rejected.
//! M40 freezes this as the two-party Level-1 release-candidate boundary. The
//! pinned C++ implementation is the primary product oracle, the pinned schemas
//! and normative protocol are authoritative for wire evolution, and
//! `capnproto-rust` is a secondary regression oracle. Three-party handoff,
//! join/equality, persistence, membranes, and attached resources are not
//! implemented by this crate.

mod actor;
mod capability;
mod level0;
mod protocol;
mod transport;

pub use actor::{
    ActorEffect, ActorLimits, AnswerKey, CancellationSignal, CompletionToken, ConnectionActor,
    ConnectionError, ConnectionHandle, ConnectionStats, IncomingCallTarget, IncomingRequest,
    LocalCompletionToken, PromiseResolver, QuestionFuture, QuestionKey, QuestionTarget,
};
pub use capability::{
    CapabilityError, CapabilityStats, CapabilityTables, HostedCapability, OutgoingCapability,
    PromiseCapability, ReceivedCapability,
};
pub use level0::{
    AcceptMessage, BootstrapMessage, CallMessage, CallTarget, CapDescriptor, DisembargoContext,
    DisembargoMessage, FinishMessage, HandlerResult, Payload, PipelineOp, PromiseResolution,
    PromisedAnswer, ProvideMessage, ReleaseMessage, ResolveMessage, ResourceBindingStats,
    ReturnMessage, ReturnPayload, SendResultsTo, ThirdPartyAnswerMessage, ThirdPartyCapDescriptor,
    ThirdPartyCompletion, ThirdPartyToAwait, ThirdPartyToContact, bind_attached_resources,
    encode_accept, encode_bootstrap, encode_call, encode_call_with_capabilities,
    encode_call_with_options, encode_disembargo, encode_finish, encode_finish_with_options,
    encode_finish_with_release, encode_provide, encode_release, encode_resolve, encode_return,
    encode_return_await_from_third_party, encode_return_with_options, encode_third_party_answer,
};
pub use protocol::{
    EXCEPTION_TYPE_ID, ExceptionType, MESSAGE_TYPE_ID, ProtocolError, ProtocolLimits,
    ProtocolMessage, RPC_SCHEMA_SHA256, RPC_TWOPARTY_SCHEMA_SHA256, RpcException, encode_abort,
    encode_unimplemented, protocol_schema, read_protocol_message,
    read_protocol_message_with_limits,
};
pub use transport::{
    AttachedResource, DuplexTransport, EnvelopeLimits, MemoryTransport, OwnedResource,
    TransportEnvelope, TransportError, memory_transport_pair,
};
