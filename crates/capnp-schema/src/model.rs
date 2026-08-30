use std::collections::BTreeMap;

use crate::{LoadError, LoadLimits};

pub type NodeId = u64;

#[derive(Clone, Debug, PartialEq)]
pub struct CompiledSchema {
    pub version: CapnpVersion,
    nodes: Vec<Node>,
    source_info: Vec<SourceInfo>,
    requested_files: Vec<RequestedFile>,
    node_index: BTreeMap<NodeId, usize>,
    source_index: BTreeMap<NodeId, usize>,
    requested_file_index: BTreeMap<NodeId, usize>,
}

impl CompiledSchema {
    pub fn from_code_generator_request(
        bytes: &[u8],
        limits: LoadLimits,
    ) -> Result<Self, LoadError> {
        crate::loader::load(bytes, limits)
    }

    pub(crate) fn indexed(
        version: CapnpVersion,
        nodes: Vec<Node>,
        source_info: Vec<SourceInfo>,
        requested_files: Vec<RequestedFile>,
    ) -> Result<Self, LoadError> {
        let mut node_index = BTreeMap::new();
        for (index, node) in nodes.iter().enumerate() {
            if node_index.insert(node.id, index).is_some() {
                return Err(LoadError::DuplicateNodeId(node.id));
            }
        }
        let mut source_index = BTreeMap::new();
        for (index, source) in source_info.iter().enumerate() {
            if source_index.insert(source.id, index).is_some() {
                return Err(LoadError::DuplicateSourceInfo(source.id));
            }
            if !node_index.contains_key(&source.id) {
                return Err(LoadError::UnknownNodeReference {
                    context: "source info",
                    id: source.id,
                });
            }
            let expected = nodes[node_index[&source.id]].member_count();
            if !source.members.is_empty() && source.members.len() != expected {
                return Err(LoadError::SourceMemberCount {
                    id: source.id,
                    expected,
                    actual: source.members.len(),
                });
            }
        }
        let mut requested_file_index = BTreeMap::new();
        for (index, file) in requested_files.iter().enumerate() {
            if requested_file_index.insert(file.id, index).is_some() {
                return Err(LoadError::DuplicateRequestedFile(file.id));
            }
            let Some(node_index_value) = node_index.get(&file.id) else {
                return Err(LoadError::UnknownNodeReference {
                    context: "requested file",
                    id: file.id,
                });
            };
            if !matches!(nodes[*node_index_value].kind, NodeKind::File) {
                return Err(LoadError::RequestedNodeIsNotFile(file.id));
            }
        }
        for node in &nodes {
            if usize::try_from(node.display_name_prefix_length).map_or(true, |prefix| {
                prefix > node.display_name.len() || !node.display_name.is_char_boundary(prefix)
            }) {
                return Err(LoadError::DisplayNamePrefix {
                    id: node.id,
                    prefix: node.display_name_prefix_length,
                    bytes: node.display_name.len(),
                });
            }
            if let NodeKind::Struct(schema) = &node.kind {
                for field in &schema.fields {
                    if let FieldKind::Group { type_id } = field.kind {
                        if !node_index.contains_key(&type_id) {
                            return Err(LoadError::UnknownNodeReference {
                                context: "group field",
                                id: type_id,
                            });
                        }
                    }
                }
            }
        }
        Ok(Self {
            version,
            nodes,
            source_info,
            requested_files,
            node_index,
            source_index,
            requested_file_index,
        })
    }

    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.node_index.get(&id).map(|index| &self.nodes[*index])
    }

    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    pub fn source_infos(&self) -> &[SourceInfo] {
        &self.source_info
    }

    pub fn requested_files(&self) -> &[RequestedFile] {
        &self.requested_files
    }

    pub fn source_info(&self, id: NodeId) -> Option<&SourceInfo> {
        self.source_index
            .get(&id)
            .map(|index| &self.source_info[*index])
    }

    pub fn requested_file(&self, id: NodeId) -> Option<&RequestedFile> {
        self.requested_file_index
            .get(&id)
            .map(|index| &self.requested_files[*index])
    }

    pub fn nested(&self, parent: NodeId, name: &str) -> Option<&Node> {
        let id = self
            .node(parent)?
            .nested_nodes
            .iter()
            .find(|nested| nested.name == name)?
            .id;
        self.node(id)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CapnpVersion {
    pub major: u16,
    pub minor: u8,
    pub micro: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub id: NodeId,
    pub display_name: String,
    pub display_name_prefix_length: u32,
    pub scope_id: NodeId,
    pub parameters: Vec<Parameter>,
    pub is_generic: bool,
    pub nested_nodes: Vec<NestedNode>,
    pub annotations: Vec<Annotation>,
    pub kind: NodeKind,
    pub start_byte: u32,
    pub end_byte: u32,
}

impl Node {
    pub fn short_name(&self) -> Option<&str> {
        usize::try_from(self.display_name_prefix_length)
            .ok()
            .and_then(|prefix| self.display_name.get(prefix..))
    }

    pub fn member_count(&self) -> usize {
        match &self.kind {
            NodeKind::Struct(value) => value.fields.len(),
            NodeKind::Enum(value) => value.enumerants.len(),
            NodeKind::Interface(value) => value.methods.len(),
            NodeKind::File | NodeKind::Const(_) | NodeKind::Annotation(_) => 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum NodeKind {
    File,
    Struct(StructSchema),
    Enum(EnumSchema),
    Interface(InterfaceSchema),
    Const(ConstSchema),
    Annotation(AnnotationSchema),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Parameter {
    pub name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NestedNode {
    pub name: String,
    pub id: NodeId,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StructSchema {
    pub data_word_count: u16,
    pub pointer_count: u16,
    pub preferred_list_encoding: ElementSize,
    pub is_group: bool,
    pub discriminant_count: u16,
    pub discriminant_offset: u32,
    pub fields: Vec<Field>,
}

impl StructSchema {
    pub fn field(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|field| field.name == name)
    }

    pub fn field_by_index(&self, index: usize) -> Option<&Field> {
        self.fields.get(index)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ElementSize {
    Empty,
    Bit,
    Byte,
    TwoBytes,
    FourBytes,
    EightBytes,
    Pointer,
    InlineComposite,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub name: String,
    pub code_order: u16,
    pub annotations: Vec<Annotation>,
    pub discriminant_value: Option<u16>,
    pub kind: FieldKind,
    pub ordinal: Ordinal,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FieldKind {
    Slot {
        offset: u32,
        ty: Type,
        default_value: Value,
        had_explicit_default: bool,
    },
    Group {
        type_id: NodeId,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ordinal {
    Implicit,
    Explicit(u16),
}

#[derive(Clone, Debug, PartialEq)]
pub struct EnumSchema {
    pub enumerants: Vec<Enumerant>,
}

impl EnumSchema {
    pub fn enumerant(&self, name: &str) -> Option<&Enumerant> {
        self.enumerants.iter().find(|value| value.name == name)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Enumerant {
    pub name: String,
    pub code_order: u16,
    pub annotations: Vec<Annotation>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct InterfaceSchema {
    pub methods: Vec<Method>,
    pub superclasses: Vec<Superclass>,
}

impl InterfaceSchema {
    pub fn method(&self, name: &str) -> Option<&Method> {
        self.methods.iter().find(|value| value.name == name)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Superclass {
    pub id: NodeId,
    pub brand: Brand,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Method {
    pub name: String,
    pub code_order: u16,
    pub implicit_parameters: Vec<Parameter>,
    pub param_struct_type: NodeId,
    pub param_brand: Brand,
    pub result_struct_type: NodeId,
    pub result_brand: Brand,
    pub annotations: Vec<Annotation>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConstSchema {
    pub ty: Type,
    pub value: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AnnotationSchema {
    pub ty: Type,
    pub targets: AnnotationTargets,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AnnotationTargets {
    pub file: bool,
    pub constant: bool,
    pub enumeration: bool,
    pub enumerant: bool,
    pub structure: bool,
    pub field: bool,
    pub union: bool,
    pub group: bool,
    pub interface: bool,
    pub method: bool,
    pub parameter: bool,
    pub annotation: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Type {
    Void,
    Bool,
    Int8,
    Int16,
    Int32,
    Int64,
    UInt8,
    UInt16,
    UInt32,
    UInt64,
    Float32,
    Float64,
    Text,
    Data,
    List(Box<Type>),
    Enum { type_id: NodeId, brand: Brand },
    Struct { type_id: NodeId, brand: Brand },
    Interface { type_id: NodeId, brand: Brand },
    AnyPointer(AnyPointerType),
}

#[derive(Clone, Debug, PartialEq)]
pub enum AnyPointerType {
    Unconstrained(AnyPointerKind),
    Parameter { scope_id: NodeId, index: u16 },
    ImplicitMethodParameter { index: u16 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AnyPointerKind {
    Any,
    Struct,
    List,
    Capability,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Brand {
    pub scopes: Vec<BrandScope>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BrandScope {
    pub scope_id: NodeId,
    pub binding: ScopeBinding,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ScopeBinding {
    Bind(Vec<BrandBinding>),
    Inherit,
}

#[derive(Clone, Debug, PartialEq)]
pub enum BrandBinding {
    Unbound,
    Type(Type),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Void,
    Bool(bool),
    Int8(i8),
    Int16(i16),
    Int32(i32),
    Int64(i64),
    UInt8(u8),
    UInt16(u16),
    UInt32(u32),
    UInt64(u64),
    Float32(f32),
    Float64(f64),
    Text(String),
    Data(Vec<u8>),
    List(OpaquePointer),
    Enum(u16),
    Struct(OpaquePointer),
    Interface,
    AnyPointer(OpaquePointer),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OpaquePointer {
    pub kind: OpaquePointerKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpaquePointerKind {
    Null,
    Struct,
    List,
    Capability,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Annotation {
    pub id: NodeId,
    pub brand: Brand,
    pub value: Value,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceInfo {
    pub id: NodeId,
    pub doc_comment: String,
    pub members: Vec<MemberSourceInfo>,
    pub start_byte: u32,
    pub end_byte: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemberSourceInfo {
    pub doc_comment: String,
    pub start_byte: u32,
    pub end_byte: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestedFile {
    pub id: NodeId,
    pub filename: String,
    pub imports: Vec<Import>,
    pub identifiers: Vec<Identifier>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Import {
    pub id: NodeId,
    pub name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Identifier {
    pub start_byte: u32,
    pub end_byte: u32,
    pub target: IdentifierTarget,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdentifierTarget {
    Type(NodeId),
    Member {
        parent_type_id: NodeId,
        ordinal: u16,
    },
}
