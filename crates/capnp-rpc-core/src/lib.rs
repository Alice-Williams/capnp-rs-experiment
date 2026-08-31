#![doc = "Deterministic Cap'n Proto RPC wire types, transport, and protocol state."]
//!
//! The wire schema is the exact `rpc.capnp` and `rpc-twoparty.capnp` pair from
//! pinned Cap'n Proto commit `e7c9cd96f1505b5ae486db7821006c2f5dce5b5b`.
//! M32 binds the revision-tolerant `Message` union and `Exception` fields and
//! defines an executor-neutral, owned duplex transport envelope. Unknown union
//! discriminants remain inspectable instead of being rejected.
//!
//! Connection actors, question/answer/import/export tables, capability
//! semantics, cancellation, reconnect, and network-specific resource handling
//! deliberately remain later milestones.

mod protocol;
mod transport;

pub use protocol::{
    EXCEPTION_TYPE_ID, ExceptionType, MESSAGE_TYPE_ID, ProtocolError, ProtocolLimits,
    ProtocolMessage, RPC_SCHEMA_SHA256, RPC_TWOPARTY_SCHEMA_SHA256, RpcException, encode_abort,
    encode_unimplemented, protocol_schema, read_protocol_message,
};
pub use transport::{
    DuplexTransport, EnvelopeLimits, MemoryTransport, OwnedResource, TransportEnvelope,
    TransportError, memory_transport_pair,
};
