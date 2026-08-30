//! Thread-safe local RPC surfaces used by generated interfaces.
//!
//! M21 deliberately provides an in-memory transport boundary, typed response
//! futures, streaming backpressure futures, and exact capability pipeline
//! transforms. Network protocol tables and connection actors remain M32+.

use std::fmt;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;

use capnp_message::{OwnedMessage, OwnedPointerRef};
use capnp_schema::{CompiledSchema, DynamicError};

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;
pub type MessageFuture = BoxFuture<Result<Arc<OwnedMessage>, RpcError>>;

#[derive(Debug)]
pub enum RpcError {
    Unimplemented { interface_id: u64, method_id: u16 },
    UnknownInterface(u64),
    UnknownMethod { interface_id: u64, method_id: u16 },
    MissingResponse,
    PipelineExpectedStruct,
    PipelineExpectedCapability,
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
}

#[derive(Clone)]
pub struct LocalClient {
    schema: Arc<CompiledSchema>,
    service: Arc<dyn LocalService>,
}

impl fmt::Debug for LocalClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalClient")
            .finish_non_exhaustive()
    }
}

impl LocalClient {
    pub fn new(schema: Arc<CompiledSchema>, service: Arc<dyn LocalService>) -> Self {
        Self { schema, service }
    }

    pub fn call<R, P>(
        &self,
        interface_id: u64,
        method_id: u16,
        params: Arc<OwnedMessage>,
        pipeline: P,
    ) -> PendingCall<R, P>
    where
        R: TypedReader,
    {
        let schema = Arc::clone(&self.schema);
        let service = Arc::clone(&self.service);
        let response = Box::pin(async move {
            let message = service.dispatch(interface_id, method_id, params).await?;
            R::from_message(schema, message)
        });
        PendingCall { response, pipeline }
    }

    pub fn call_streaming(
        &self,
        interface_id: u64,
        method_id: u16,
        params: Arc<OwnedMessage>,
    ) -> StreamingCall {
        let service = Arc::clone(&self.service);
        StreamingCall {
            completion: Box::pin(async move {
                service.dispatch(interface_id, method_id, params).await?;
                Ok(())
            }),
        }
    }
}

pub struct PendingCall<R, P> {
    response: BoxFuture<Result<R, RpcError>>,
    pub pipeline: P,
}

impl<R, P> PendingCall<R, P> {
    pub fn response(self) -> BoxFuture<Result<R, RpcError>> {
        self.response
    }
}

pub struct StreamingCall {
    completion: BoxFuture<Result<(), RpcError>>,
}

impl StreamingCall {
    pub fn completion(self) -> BoxFuture<Result<(), RpcError>> {
        self.completion
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PipelineTransform {
    pointer_fields: Vec<u16>,
}

impl PipelineTransform {
    pub fn root() -> Self {
        Self::default()
    }

    pub fn pointer_field(&self, index: u16) -> Self {
        let mut pointer_fields = self.pointer_fields.clone();
        pointer_fields.push(index);
        Self { pointer_fields }
    }

    pub fn pointer_fields(&self) -> &[u16] {
        &self.pointer_fields
    }

    pub fn capability(&self, message: &Arc<OwnedMessage>) -> Result<Option<u32>, RpcError> {
        let Some((&last, parents)) = self.pointer_fields.split_last() else {
            return Err(RpcError::PipelineExpectedCapability);
        };
        let mut structure = message.root_struct()?.into_root();
        for index in parents {
            structure = structure
                .child_struct(*index)?
                .ok_or(RpcError::PipelineExpectedStruct)?;
        }
        match structure.child_pointer(last)? {
            OwnedPointerRef::Null => Ok(None),
            OwnedPointerRef::Capability(value) => Ok(Some(value)),
            OwnedPointerRef::Struct(_) | OwnedPointerRef::List(_) => {
                Err(RpcError::PipelineExpectedCapability)
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct CapabilityPipeline {
    transform: PipelineTransform,
}

impl CapabilityPipeline {
    pub fn new(transform: PipelineTransform) -> Self {
        Self { transform }
    }

    pub fn transform(&self) -> &PipelineTransform {
        &self.transform
    }

    pub fn resolve(&self, message: &Arc<OwnedMessage>) -> Result<Option<u32>, RpcError> {
        self.transform.capability(message)
    }
}

#[derive(Clone, Debug)]
pub struct TypedPipeline<T> {
    transform: PipelineTransform,
    marker: PhantomData<fn() -> T>,
}

impl<T> TypedPipeline<T> {
    pub fn new(transform: PipelineTransform) -> Self {
        Self {
            transform,
            marker: PhantomData,
        }
    }

    pub fn transform(&self) -> &PipelineTransform {
        &self.transform
    }

    pub fn resolve(&self, message: &Arc<OwnedMessage>) -> Result<Option<u32>, RpcError> {
        self.transform.capability(message)
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
