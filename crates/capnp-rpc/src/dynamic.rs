//! Schema-checked dynamic local capabilities.

use std::collections::BTreeSet;
use std::sync::Arc;

use capnp_message::OwnedMessage;
use capnp_schema::{
    AnyPointerKind, AnyPointerType, CompiledSchema, DynamicStruct, FieldKind, Method, NodeKind,
    Type,
};

use crate::{
    BoxFuture, CapabilityList, LocalCall, LocalClient, LocalService, MessageFuture,
    PipelineTransform, RpcError, UntypedPendingCall, UntypedPipeline,
};

/// A method resolved against a concrete interface schema, including inherited
/// declarations and method-level implicit parameters.
#[derive(Clone, Debug)]
pub struct DynamicMethod {
    interface_id: u64,
    method: Method,
}

impl DynamicMethod {
    pub fn interface_id(&self) -> u64 {
        self.interface_id
    }

    pub fn name(&self) -> &str {
        &self.method.name
    }

    pub fn method_id(&self) -> u16 {
        self.method.code_order
    }

    pub fn param_type_id(&self) -> u64 {
        self.method.param_struct_type
    }

    pub fn result_type_id(&self) -> u64 {
        self.method.result_struct_type
    }

    pub fn implicit_parameter_count(&self) -> usize {
        self.method.implicit_parameters.len()
    }
}

/// A capability whose callable interface is selected from a compiled schema.
#[derive(Clone, Debug)]
pub struct DynamicCapability {
    schema: Arc<CompiledSchema>,
    client: LocalClient,
    interface_id: Option<u64>,
}

impl DynamicCapability {
    pub fn new(
        schema: Arc<CompiledSchema>,
        client: LocalClient,
        interface_id: u64,
    ) -> Result<Self, RpcError> {
        require_interface(&schema, interface_id)?;
        Ok(Self {
            schema,
            client,
            interface_id: Some(interface_id),
        })
    }

    fn untyped(schema: Arc<CompiledSchema>, client: LocalClient) -> Self {
        Self {
            schema,
            client,
            interface_id: None,
        }
    }

    pub fn from_server<S>(
        schema: Arc<CompiledSchema>,
        interface_id: u64,
        server: Arc<S>,
    ) -> Result<Self, RpcError>
    where
        S: DynamicServer,
    {
        require_interface(&schema, interface_id)?;
        let local = LocalClient::new(
            Arc::clone(&schema),
            Arc::new(DynamicServerAdapter {
                schema: Arc::clone(&schema),
                root_interface_id: interface_id,
                server,
            }),
        );
        Self::new(schema, local, interface_id)
    }

    pub fn interface_id(&self) -> Option<u64> {
        self.interface_id
    }

    pub fn local(&self) -> &LocalClient {
        &self.client
    }

    /// Reinterprets this capability as a schema-known interface. Dispatch still
    /// reaches the same capability identity and may fail if the server does not
    /// implement the selected interface.
    pub fn cast(&self, interface_id: u64) -> Result<Self, RpcError> {
        require_interface(&self.schema, interface_id)?;
        Ok(Self {
            schema: Arc::clone(&self.schema),
            client: self.client.clone(),
            interface_id: Some(interface_id),
        })
    }

    /// Upcasts through the declared interface inheritance graph.
    pub fn upcast(&self, interface_id: u64) -> Result<Self, RpcError> {
        let current = self
            .interface_id
            .ok_or(RpcError::DynamicUntypedCapability)?;
        require_interface(&self.schema, interface_id)?;
        if !inherits(&self.schema, current, interface_id, &mut BTreeSet::new()) {
            return Err(RpcError::UnknownInterface(interface_id));
        }
        self.cast(interface_id)
    }

    pub fn method(&self, name: &str) -> Result<DynamicMethod, RpcError> {
        let interface_id = self
            .interface_id
            .ok_or(RpcError::DynamicUntypedCapability)?;
        find_method(&self.schema, interface_id, name, &mut BTreeSet::new()).ok_or_else(|| {
            RpcError::DynamicMethodName {
                interface_id,
                name: name.to_owned(),
            }
        })
    }

    pub fn call(
        &self,
        name: &str,
        params: Arc<OwnedMessage>,
    ) -> Result<DynamicPendingCall, RpcError> {
        let method = self.method(name)?;
        DynamicStruct::root(
            Arc::clone(&self.schema),
            Arc::clone(&params),
            method.param_type_id(),
        )?;
        let raw = self
            .client
            .call_untyped(method.interface_id(), method.method_id(), params);
        let pipeline = DynamicPipeline {
            schema: Arc::clone(&self.schema),
            result_type_id: method.result_type_id(),
            raw: raw.pipeline.clone(),
        };
        Ok(DynamicPendingCall {
            schema: Arc::clone(&self.schema),
            method,
            raw,
            pub_pipeline: pipeline,
        })
    }
}

/// Reflected request passed to a schema-driven local server.
#[derive(Clone, Debug)]
pub struct DynamicServerCall {
    method: DynamicMethod,
    params: DynamicStruct,
}

impl DynamicServerCall {
    pub fn method(&self) -> &DynamicMethod {
        &self.method
    }

    pub fn params(&self) -> &DynamicStruct {
        &self.params
    }
}

/// A schema-driven server. Returning `LocalCall` permits provisional pipelines
/// and direct tail responses without moving protocol behavior into generated
/// code.
pub trait DynamicServer: Send + Sync + 'static {
    fn dispatch(&self, call: DynamicServerCall) -> LocalCall;
}

impl<F> DynamicServer for F
where
    F: Fn(DynamicServerCall) -> LocalCall + Send + Sync + 'static,
{
    fn dispatch(&self, call: DynamicServerCall) -> LocalCall {
        self(call)
    }
}

struct DynamicServerAdapter<S> {
    schema: Arc<CompiledSchema>,
    root_interface_id: u64,
    server: Arc<S>,
}

impl<S: DynamicServer> DynamicServerAdapter<S> {
    fn start(&self, interface_id: u64, method_id: u16, params: Arc<OwnedMessage>) -> LocalCall {
        let result = (|| {
            if !inherits(
                &self.schema,
                self.root_interface_id,
                interface_id,
                &mut BTreeSet::new(),
            ) {
                return Err(RpcError::UnknownInterface(interface_id));
            }
            let method = find_method_id(&self.schema, interface_id, method_id)?;
            let params =
                DynamicStruct::root(Arc::clone(&self.schema), params, method.param_type_id())?;
            Ok(self.server.dispatch(DynamicServerCall { method, params }))
        })();
        match result {
            Ok(call) => call,
            Err(error) => LocalCall::new(Box::pin(async move { Err(error) })),
        }
    }
}

impl<S: DynamicServer> LocalService for DynamicServerAdapter<S> {
    fn dispatch(
        self: Arc<Self>,
        interface_id: u64,
        method_id: u16,
        params: Arc<OwnedMessage>,
    ) -> MessageFuture {
        let call = self.start(interface_id, method_id, params);
        Box::pin(async move {
            let response = call.into_response().await?;
            Ok(Arc::clone(response.message()))
        })
    }

    fn dispatch_call(
        self: Arc<Self>,
        interface_id: u64,
        method_id: u16,
        params: Arc<OwnedMessage>,
    ) -> LocalCall {
        self.start(interface_id, method_id, params)
    }
}

/// A dynamic response preserving both the reflected result and its capability
/// table.
#[derive(Clone, Debug)]
pub struct DynamicResponse {
    result: DynamicStruct,
    capabilities: CapabilityList,
}

impl DynamicResponse {
    pub fn result(&self) -> &DynamicStruct {
        &self.result
    }

    pub fn capabilities(&self) -> &CapabilityList {
        &self.capabilities
    }
}

/// A response pipeline that resolves field names through the result schema.
#[derive(Clone, Debug)]
pub struct DynamicPipeline {
    schema: Arc<CompiledSchema>,
    result_type_id: u64,
    raw: UntypedPipeline,
}

impl DynamicPipeline {
    pub fn capability(&self, fields: &[&str]) -> Result<DynamicCapability, RpcError> {
        let (transform, interface_id) = pipeline_path(&self.schema, self.result_type_id, fields)?;
        let client = self.raw.client(transform);
        Ok(match interface_id {
            Some(interface_id) => {
                DynamicCapability::new(Arc::clone(&self.schema), client, interface_id)?
            }
            None => DynamicCapability::untyped(Arc::clone(&self.schema), client),
        })
    }
}

/// A dynamic response future whose pipeline remains usable if the response is
/// dropped, matching generated `RemotePromise` ownership.
pub struct DynamicPendingCall {
    schema: Arc<CompiledSchema>,
    method: DynamicMethod,
    raw: UntypedPendingCall,
    pub_pipeline: DynamicPipeline,
}

impl DynamicPendingCall {
    pub fn method(&self) -> &DynamicMethod {
        &self.method
    }

    pub fn pipeline(&self) -> &DynamicPipeline {
        &self.pub_pipeline
    }

    pub fn into_pipeline(self) -> DynamicPipeline {
        self.pub_pipeline
    }

    pub fn response(self) -> BoxFuture<Result<DynamicResponse, RpcError>> {
        let schema = self.schema;
        let result_type_id = self.method.result_type_id();
        Box::pin(async move {
            let response = self.raw.response().await?;
            let result = DynamicStruct::root(
                Arc::clone(&schema),
                Arc::clone(response.message()),
                result_type_id,
            )?;
            Ok(DynamicResponse {
                result,
                capabilities: response.capabilities().clone(),
            })
        })
    }
}

fn require_interface(schema: &CompiledSchema, interface_id: u64) -> Result<(), RpcError> {
    match schema.node(interface_id).map(|node| &node.kind) {
        Some(NodeKind::Interface(_)) => Ok(()),
        Some(_) => Err(RpcError::DynamicNotInterface(interface_id)),
        None => Err(RpcError::UnknownInterface(interface_id)),
    }
}

fn find_method(
    schema: &CompiledSchema,
    interface_id: u64,
    name: &str,
    visited: &mut BTreeSet<u64>,
) -> Option<DynamicMethod> {
    if !visited.insert(interface_id) {
        return None;
    }
    let NodeKind::Interface(interface) = &schema.node(interface_id)?.kind else {
        return None;
    };
    if let Some(method) = interface.method(name) {
        return Some(DynamicMethod {
            interface_id,
            method: method.clone(),
        });
    }
    interface
        .superclasses
        .iter()
        .find_map(|superclass| find_method(schema, superclass.id, name, visited))
}

fn find_method_id(
    schema: &CompiledSchema,
    interface_id: u64,
    method_id: u16,
) -> Result<DynamicMethod, RpcError> {
    let node = schema
        .node(interface_id)
        .ok_or(RpcError::UnknownInterface(interface_id))?;
    let NodeKind::Interface(interface) = &node.kind else {
        return Err(RpcError::DynamicNotInterface(interface_id));
    };
    let method = interface
        .methods
        .iter()
        .find(|method| method.code_order == method_id)
        .cloned()
        .ok_or(RpcError::UnknownMethod {
            interface_id,
            method_id,
        })?;
    Ok(DynamicMethod {
        interface_id,
        method,
    })
}

fn inherits(
    schema: &CompiledSchema,
    interface_id: u64,
    expected: u64,
    visited: &mut BTreeSet<u64>,
) -> bool {
    if interface_id == expected {
        return true;
    }
    if !visited.insert(interface_id) {
        return false;
    }
    let Some(node) = schema.node(interface_id) else {
        return false;
    };
    let NodeKind::Interface(interface) = &node.kind else {
        return false;
    };
    interface
        .superclasses
        .iter()
        .any(|superclass| inherits(schema, superclass.id, expected, visited))
}

fn pipeline_path(
    schema: &CompiledSchema,
    mut type_id: u64,
    fields: &[&str],
) -> Result<(PipelineTransform, Option<u64>), RpcError> {
    let mut transform = PipelineTransform::root();
    for (position, name) in fields.iter().enumerate() {
        let Some(node) = schema.node(type_id) else {
            return Err(RpcError::DynamicField {
                type_id,
                name: (*name).to_owned(),
            });
        };
        let NodeKind::Struct(structure) = &node.kind else {
            return Err(RpcError::DynamicField {
                type_id,
                name: (*name).to_owned(),
            });
        };
        let field = structure
            .field(name)
            .ok_or_else(|| RpcError::DynamicField {
                type_id,
                name: (*name).to_owned(),
            })?;
        let last = position + 1 == fields.len();
        match &field.kind {
            FieldKind::Group { type_id: group_id } => {
                type_id = *group_id;
                if last {
                    return Err(RpcError::DynamicPipelineType {
                        type_id,
                        name: (*name).to_owned(),
                    });
                }
            }
            FieldKind::Slot { offset, ty, .. } => {
                let offset = u16::try_from(*offset).map_err(|_| RpcError::DynamicPipelineType {
                    type_id,
                    name: (*name).to_owned(),
                })?;
                transform = transform.pointer_field(offset);
                match ty {
                    Type::Struct {
                        type_id: child_id, ..
                    } if !last => type_id = *child_id,
                    Type::Interface {
                        type_id: interface_id,
                        ..
                    } if last => return Ok((transform, Some(*interface_id))),
                    Type::AnyPointer(AnyPointerType::Unconstrained(AnyPointerKind::Capability))
                        if last =>
                    {
                        return Ok((transform, None));
                    }
                    Type::AnyPointer(AnyPointerType::Parameter { .. })
                    | Type::AnyPointer(AnyPointerType::ImplicitMethodParameter { .. })
                        if last =>
                    {
                        return Ok((transform, None));
                    }
                    _ => {
                        return Err(RpcError::DynamicPipelineType {
                            type_id,
                            name: (*name).to_owned(),
                        });
                    }
                }
            }
        }
    }
    Err(RpcError::PipelineExpectedCapability)
}
