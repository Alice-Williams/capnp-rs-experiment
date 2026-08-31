//! Thread-safe local RPC surfaces used by generated interfaces.
//!
//! M21 deliberately provides an in-memory transport boundary, typed response
//! futures, streaming backpressure futures, and exact capability pipeline
//! transforms. M33 adds the executor-neutral two-party driver around the
//! single-owner Level-0 actor from `capnp-rpc-core`. M37 makes streaming sends
//! eager: calling a generated streaming method invokes dispatch synchronously
//! to preserve E-order, while acknowledgement-driven flow-control futures only
//! govern when the caller should submit another message. M38 adds cooperative
//! dispatch cancellation, transport-complete shutdown, and generation-safe
//! capability recreation after disconnect.
//! M39 adds explicit concurrent, serial, keyed, and dedicated-local scheduling
//! policies. M40 freezes these executor-neutral APIs as the two-party Level-1
//! release candidate. M41 and M42 add mature local capabilities and membranes;
//! M43 adds generic attachments and Unix `SCM_RIGHTS`. Higher-level routing,
//! join/equality, and persistence remain explicit non-goals.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use capnp_message::OwnedMessage;
use capnp_schema::{CompiledSchema, DynamicError};

mod driver;
mod dynamic;
mod flow;
mod local;
mod membrane;
mod reconnect;
mod scheduler;
#[cfg(unix)]
mod unix_transport;

pub use capnp_rpc_core::{
    ActorLimits, AnswerKey, AttachedResource, CancellationSignal, CapDescriptor, CapabilityError,
    CapabilityStats, CapabilityTables, CompletionToken, ConnectionError, ConnectionHandle,
    ConnectionStats, DuplexTransport, EnvelopeLimits, ExceptionType, HandlerResult,
    HostedCapability, IncomingCallTarget, IncomingRequest, LocalCompletionToken,
    OutgoingCapability, OwnedResource, Payload, PromiseCapability, PromiseResolver, ProtocolLimits,
    QuestionFuture, QuestionKey, QuestionTarget, ReceivedCapability, ReturnPayload, RpcException,
    TransportEnvelope,
};
pub use driver::{ConnectionDriver, DriverCompletion, DriverDispatch, DriverError, DriverShutdown};
pub use dynamic::{
    DynamicCapability, DynamicMethod, DynamicPendingCall, DynamicPipeline, DynamicResponse,
    DynamicServer, DynamicServerCall,
};
pub use flow::{
    AllAcked, FlowAck, FlowController, FlowError, FlowLimits, FlowMode, FlowReady, FlowSend,
    FlowStats,
};
pub use local::{
    CapabilityFailure, CapabilityList, CapabilityPipeline, CapabilityServerSet, FromLocalClient,
    LocalCall, LocalClient, LocalRequest, LocalResponse, PendingCall, PipelineBinding,
    PipelineBuilder, PipelineSource, PipelineTransform, PromiseClientResolver, StreamingCall,
    TypedPipeline, UntypedPendingCall, UntypedPipeline, direct_tail_call, flatten_pending,
    tail_call,
};
pub use membrane::{Membrane, MembraneDecision, MembraneLimits, MembranePolicy, RevocableServer};
pub use reconnect::{
    CapabilityReconnector, ReconnectLease, RetryDisposition, classify_connection_error,
    classify_exception,
};
pub use scheduler::{
    Concurrent, ExecutorService, GenericExecutor, Keyed, LocalServer, SchedulerError, Serial,
    TaskExecutor, ThreadPoolExecutor, TokioExecutor,
};
#[cfg(unix)]
pub use unix_transport::{UnixScmRightsTransport, UnixTransportError};

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;
pub type MessageFuture = BoxFuture<Result<Arc<OwnedMessage>, RpcError>>;
pub type LocalResponseFuture = BoxFuture<Result<LocalResponse, RpcError>>;

#[derive(Debug)]
pub enum RpcError {
    Unimplemented {
        interface_id: u64,
        method_id: u16,
    },
    UnknownInterface(u64),
    UnknownMethod {
        interface_id: u64,
        method_id: u16,
    },
    MissingResponse,
    LocalCapability(CapabilityFailure),
    Shared(Arc<RpcError>),
    CapabilityLimit {
        requested: usize,
        limit: usize,
    },
    CapabilityIndex {
        index: usize,
        length: usize,
    },
    PipelineLimit {
        requested: usize,
        limit: usize,
    },
    DuplicatePipelinePath,
    PipelineAlreadySet,
    PromiseAlreadyResolved,
    PromiseCycle,
    MissingCapability(u32),
    UnboundPipeline,
    DynamicNotInterface(u64),
    DynamicMethodName {
        interface_id: u64,
        name: String,
    },
    DynamicField {
        type_id: u64,
        name: String,
    },
    DynamicPipelineType {
        type_id: u64,
        name: String,
    },
    DynamicUntypedCapability,
    MembraneAlreadyRevoked,
    MembraneLimit {
        resource: &'static str,
        requested: usize,
        limit: usize,
    },
    Scheduler(SchedulerError),
    PipelineExpectedStruct,
    PipelineExpectedCapability,
    Connection(ConnectionError),
    Dynamic(DynamicError),
    Message(capnp_message::OwnedReadError),
}

impl fmt::Display for RpcError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for RpcError {}

impl From<DynamicError> for RpcError {
    fn from(value: DynamicError) -> Self {
        Self::Dynamic(value)
    }
}

impl From<capnp_message::OwnedReadError> for RpcError {
    fn from(value: capnp_message::OwnedReadError) -> Self {
        Self::Message(value)
    }
}

impl From<ConnectionError> for RpcError {
    fn from(value: ConnectionError) -> Self {
        Self::Connection(value)
    }
}

pub trait TypedReader: Sized + Send + 'static {
    fn from_message(
        schema: Arc<CompiledSchema>,
        message: Arc<OwnedMessage>,
    ) -> Result<Self, RpcError>;
}

pub trait LocalService: Send + Sync + 'static {
    fn dispatch(
        self: Arc<Self>,
        interface_id: u64,
        method_id: u16,
        params: Arc<OwnedMessage>,
    ) -> MessageFuture;
    fn dispatch_call(
        self: Arc<Self>,
        interface_id: u64,
        method_id: u16,
        params: Arc<OwnedMessage>,
    ) -> LocalCall {
        LocalCall::from_message_future(self.dispatch(interface_id, method_id, params))
    }

    fn dispatch_request(
        self: Arc<Self>,
        interface_id: u64,
        method_id: u16,
        request: LocalRequest,
    ) -> LocalCall {
        self.dispatch_call(interface_id, method_id, request.into_message())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use capnp_message::{ExclusiveArena, ReaderLimits};

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn public_local_rpc_handles_are_thread_safe() {
        assert_send_sync::<LocalClient>();
        assert_send_sync::<PipelineTransform>();
        assert_send_sync::<CapabilityPipeline>();
        assert_send_sync::<FlowController>();
        assert_send_sync::<FlowAck>();
        assert_send_sync::<FlowReady>();
        assert_send_sync::<AllAcked>();
        assert_send_sync::<CancellationSignal>();
        assert_send_sync::<CapabilityReconnector<usize, fn() -> Result<usize, ConnectionError>>>();
    }

    #[test]
    fn pipeline_transform_resolves_exact_nested_capability_path() {
        let mut arena = ExclusiveArena::new(8, 64).expect("arena");
        arena
            .init_root_struct(0, 1)
            .expect("root")
            .init_struct(0, 0, 1)
            .expect("child")
            .set_capability(0, 17)
            .expect("capability");
        let message =
            OwnedMessage::new(arena.into_segments(), ReaderLimits::default()).expect("message");
        let transform = PipelineTransform::root().pointer_field(0).pointer_field(0);
        assert_eq!(transform.pointer_fields(), &[0, 0]);
        assert_eq!(transform.capability(&message).expect("resolves"), Some(17));
    }
}
