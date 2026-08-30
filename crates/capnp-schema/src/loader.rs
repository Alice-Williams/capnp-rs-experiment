use std::fmt;
use std::sync::Arc;

use capnp_io::{FrameError, FrameLimits, FrameRead, parse_frame};
use capnp_message::{
    DataSection, ListReadError, ListReader, LocalTraversalBudget, MessageSegments, NestingLimit,
    PrimitiveError, ResolvedPointer, StructElementReader, StructReadError, StructReader,
    TraversalBudget, ValidationError, WireLocation,
};

use crate::model::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoadLimits {
    pub max_segments: u32,
    pub max_total_words: u64,
    pub max_traversal_words: u64,
    pub max_nesting: u32,
    pub max_metadata_items: usize,
}

impl Default for LoadLimits {
    fn default() -> Self {
        Self {
            max_segments: 512,
            max_total_words: 8 * 1024 * 1024,
            max_traversal_words: 8 * 1024 * 1024,
            max_nesting: 64,
            max_metadata_items: 1_000_000,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LoadError {
    Frame(FrameError),
    Validation(ValidationError),
    Struct(StructReadError),
    List(ListReadError),
    Primitive(PrimitiveError),
    EmptyRequest,
    TrailingData(usize),
    InvalidUtf8,
    UnknownDiscriminant {
        context: &'static str,
        value: u16,
    },
    MetadataLimit {
        limit: usize,
    },
    DuplicateNodeId(NodeId),
    DuplicateSourceInfo(NodeId),
    DuplicateRequestedFile(NodeId),
    UnknownNodeReference {
        context: &'static str,
        id: NodeId,
    },
    RequestedNodeIsNotFile(NodeId),
    DisplayNamePrefix {
        id: NodeId,
        prefix: u32,
        bytes: usize,
    },
    SourceMemberCount {
        id: NodeId,
        expected: usize,
        actual: usize,
    },
    PointerKind {
        context: &'static str,
        expected: OpaquePointerKind,
        actual: OpaquePointerKind,
    },
}

impl fmt::Display for LoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for LoadError {}

impl From<FrameError> for LoadError {
    fn from(value: FrameError) -> Self {
        Self::Frame(value)
    }
}
impl From<ValidationError> for LoadError {
    fn from(value: ValidationError) -> Self {
        Self::Validation(value)
    }
}
impl From<StructReadError> for LoadError {
    fn from(value: StructReadError) -> Self {
        Self::Struct(value)
    }
}
impl From<ListReadError> for LoadError {
    fn from(value: ListReadError) -> Self {
        Self::List(value)
    }
}
impl From<PrimitiveError> for LoadError {
    fn from(value: PrimitiveError) -> Self {
        Self::Primitive(value)
    }
}

#[derive(Clone, Copy)]
enum RawStruct<'context, 'data> {
    Struct(StructReader<'context, 'data, LocalTraversalBudget>),
    Element(StructElementReader<'context, 'data, LocalTraversalBudget>),
}

impl<'context, 'data> RawStruct<'context, 'data> {
    fn data(&self) -> Result<DataSection<'data>, LoadError> {
        match self {
            Self::Struct(reader) => Ok((*reader).data_section()?),
            Self::Element(reader) => Ok((*reader).data_section()?),
        }
    }

    fn text(&self, index: u16) -> Result<String, LoadError> {
        let text = match self {
            Self::Struct(reader) => reader.read_text(index, None)?.to_str(),
            Self::Element(reader) => reader.read_text(index, None)?.to_str(),
        }
        .map_err(|_| LoadError::InvalidUtf8)?;
        Ok(text.to_owned())
    }

    fn bytes(&self, index: u16) -> Result<Vec<u8>, LoadError> {
        Ok(match self {
            Self::Struct(reader) => reader.read_data(index, None)?.as_bytes().to_vec(),
            Self::Element(reader) => reader.read_data(index, None)?.as_bytes().to_vec(),
        })
    }

    fn child<'reader>(&'reader self, index: u16) -> Result<RawStruct<'reader, 'data>, LoadError>
    where
        'context: 'reader,
    {
        Ok(match self {
            Self::Struct(reader) => RawStruct::Struct(reader.read_struct(index, None)?),
            Self::Element(reader) => RawStruct::Struct(reader.read_struct(index, None)?),
        })
    }

    fn list<'reader>(
        &'reader self,
        index: u16,
    ) -> Result<ListReader<'reader, 'data, LocalTraversalBudget>, LoadError>
    where
        'context: 'reader,
    {
        Ok(match self {
            Self::Struct(reader) => reader.read_list(index, None)?,
            Self::Element(reader) => reader.read_list(index, None)?,
        })
    }

    fn pointer_location(&self, index: u16) -> Result<Option<WireLocation>, LoadError> {
        Ok(match self {
            Self::Struct(reader) => reader.pointer_location(index)?,
            Self::Element(reader) => reader.pointer_location(index)?,
        })
    }

    fn nesting(&self) -> NestingLimit {
        match self {
            Self::Struct(reader) => reader.nesting_limit(),
            Self::Element(reader) => reader.nesting_limit(),
        }
    }

    fn opaque(&self, index: u16, backing: Arc<[Arc<[u8]>]>) -> Result<OpaquePointer, LoadError> {
        let resolved = match self {
            Self::Struct(reader) => reader.resolve_pointer(index, None)?.value.pointer,
            Self::Element(reader) => reader.resolve_pointer(index, None)?.value.pointer,
        };
        let kind = match resolved {
            ResolvedPointer::Null => OpaquePointerKind::Null,
            ResolvedPointer::Struct(_) => OpaquePointerKind::Struct,
            ResolvedPointer::List(_) => OpaquePointerKind::List,
            ResolvedPointer::Capability(_) => OpaquePointerKind::Capability,
        };
        Ok(OpaquePointer {
            kind,
            backing,
            location: self.pointer_location(index)?.unwrap_or(WireLocation {
                segment_id: 0,
                word_offset: 0,
            }),
            nesting: self.nesting(),
        })
    }
}

struct Loader {
    remaining_items: usize,
    backing: Arc<[Arc<[u8]>]>,
}

impl Loader {
    fn charge(&mut self, count: u32, limit: usize) -> Result<(), LoadError> {
        let count = usize::try_from(count).map_err(|_| LoadError::MetadataLimit { limit })?;
        self.remaining_items = self
            .remaining_items
            .checked_sub(count)
            .ok_or(LoadError::MetadataLimit { limit })?;
        Ok(())
    }

    fn structs<'context, 'data, T>(
        &mut self,
        raw: RawStruct<'context, 'data>,
        index: u16,
        limit: usize,
        mut parse: impl FnMut(&mut Self, RawStruct<'_, 'data>) -> Result<T, LoadError>,
    ) -> Result<Vec<T>, LoadError> {
        let values = raw.list(index)?.as_structs()?;
        self.charge(values.len(), limit)?;
        let mut result = Vec::with_capacity(values.len() as usize);
        for value in values.iter() {
            result.push(parse(self, RawStruct::Element(value?))?);
        }
        Ok(result)
    }

    fn node(&mut self, raw: RawStruct<'_, '_>, limit: usize) -> Result<Node, LoadError> {
        let data = raw.data()?;
        let id = data.read_u64(0, 0)?;
        let parameters = self.structs(raw, 5, limit, |_, value| {
            Ok(Parameter {
                name: value.text(0)?,
            })
        })?;
        let nested_nodes = self.structs(raw, 1, limit, |_, value| {
            Ok(NestedNode {
                name: value.text(0)?,
                id: value.data()?.read_u64(0, 0)?,
            })
        })?;
        let annotations = self.annotations(raw, 2, limit)?;
        let kind = match data.read_u16(6, 0)? {
            0 => NodeKind::File,
            1 => NodeKind::Struct(StructSchema {
                data_word_count: data.read_u16(7, 0)?,
                pointer_count: data.read_u16(12, 0)?,
                preferred_list_encoding: parse_element_size(data.read_u16(13, 0)?)?,
                is_group: data.read_bool(224, false)?,
                discriminant_count: data.read_u16(15, 0)?,
                discriminant_offset: data.read_u32(8, 0)?,
                fields: self.structs(raw, 3, limit, |loader, value| loader.field(value, limit))?,
            }),
            2 => NodeKind::Enum(EnumSchema {
                enumerants: self.structs(raw, 3, limit, |loader, value| {
                    loader.enumerant(value, limit)
                })?,
            }),
            3 => NodeKind::Interface(InterfaceSchema {
                methods: self
                    .structs(raw, 3, limit, |loader, value| loader.method(value, limit))?,
                superclasses: self.structs(raw, 4, limit, |loader, value| {
                    Ok(Superclass {
                        id: value.data()?.read_u64(0, 0)?,
                        brand: loader.brand(value.child(0)?, limit)?,
                    })
                })?,
            }),
            4 => NodeKind::Const(ConstSchema {
                ty: self.ty(raw.child(3)?, limit)?,
                value: self.value(raw.child(4)?)?,
            }),
            5 => NodeKind::Annotation(AnnotationSchema {
                ty: self.ty(raw.child(3)?, limit)?,
                targets: AnnotationTargets {
                    file: data.read_bool(112, false)?,
                    constant: data.read_bool(113, false)?,
                    enumeration: data.read_bool(114, false)?,
                    enumerant: data.read_bool(115, false)?,
                    structure: data.read_bool(116, false)?,
                    field: data.read_bool(117, false)?,
                    union: data.read_bool(118, false)?,
                    group: data.read_bool(119, false)?,
                    interface: data.read_bool(120, false)?,
                    method: data.read_bool(121, false)?,
                    parameter: data.read_bool(122, false)?,
                    annotation: data.read_bool(123, false)?,
                },
            }),
            value => return Err(unknown("Node", value)),
        };
        Ok(Node {
            id,
            display_name: raw.text(0)?,
            display_name_prefix_length: data.read_u32(2, 0)?,
            scope_id: data.read_u64(2, 0)?,
            parameters,
            is_generic: data.read_bool(288, false)?,
            nested_nodes,
            annotations,
            kind,
            start_byte: data.read_u32(10, 0)?,
            end_byte: data.read_u32(11, 0)?,
        })
    }

    fn annotations(
        &mut self,
        raw: RawStruct<'_, '_>,
        index: u16,
        limit: usize,
    ) -> Result<Vec<Annotation>, LoadError> {
        self.structs(raw, index, limit, |loader, value| {
            Ok(Annotation {
                id: value.data()?.read_u64(0, 0)?,
                value: loader.value(value.child(0)?)?,
                brand: loader.brand(value.child(1)?, limit)?,
            })
        })
    }

    fn field(&mut self, raw: RawStruct<'_, '_>, limit: usize) -> Result<Field, LoadError> {
        let data = raw.data()?;
        let discriminant = data.read_u16(1, u16::MAX)?;
        let kind = match data.read_u16(4, 0)? {
            0 => FieldKind::Slot {
                offset: data.read_u32(1, 0)?,
                ty: self.ty(raw.child(2)?, limit)?,
                default_value: self.value(raw.child(3)?)?,
                had_explicit_default: data.read_bool(128, false)?,
            },
            1 => FieldKind::Group {
                type_id: data.read_u64(2, 0)?,
            },
            value => return Err(unknown("Field", value)),
        };
        let ordinal = match data.read_u16(5, 0)? {
            0 => Ordinal::Implicit,
            1 => Ordinal::Explicit(data.read_u16(6, 0)?),
            value => return Err(unknown("Field.ordinal", value)),
        };
        Ok(Field {
            name: raw.text(0)?,
            code_order: data.read_u16(0, 0)?,
            annotations: self.annotations(raw, 1, limit)?,
            discriminant_value: (discriminant != u16::MAX).then_some(discriminant),
            kind,
            ordinal,
        })
    }

    fn enumerant(&mut self, raw: RawStruct<'_, '_>, limit: usize) -> Result<Enumerant, LoadError> {
        Ok(Enumerant {
            name: raw.text(0)?,
            code_order: raw.data()?.read_u16(0, 0)?,
            annotations: self.annotations(raw, 1, limit)?,
        })
    }

    fn method(&mut self, raw: RawStruct<'_, '_>, limit: usize) -> Result<Method, LoadError> {
        let data = raw.data()?;
        Ok(Method {
            name: raw.text(0)?,
            code_order: data.read_u16(0, 0)?,
            implicit_parameters: self.structs(raw, 4, limit, |_, value| {
                Ok(Parameter {
                    name: value.text(0)?,
                })
            })?,
            param_struct_type: data.read_u64(1, 0)?,
            param_brand: self.brand(raw.child(2)?, limit)?,
            result_struct_type: data.read_u64(2, 0)?,
            result_brand: self.brand(raw.child(3)?, limit)?,
            annotations: self.annotations(raw, 1, limit)?,
        })
    }

    fn ty(&mut self, raw: RawStruct<'_, '_>, limit: usize) -> Result<Type, LoadError> {
        let data = raw.data()?;
        Ok(match data.read_u16(0, 0)? {
            0 => Type::Void,
            1 => Type::Bool,
            2 => Type::Int8,
            3 => Type::Int16,
            4 => Type::Int32,
            5 => Type::Int64,
            6 => Type::UInt8,
            7 => Type::UInt16,
            8 => Type::UInt32,
            9 => Type::UInt64,
            10 => Type::Float32,
            11 => Type::Float64,
            12 => Type::Text,
            13 => Type::Data,
            14 => Type::List(Box::new(self.ty(raw.child(0)?, limit)?)),
            15 => Type::Enum {
                type_id: data.read_u64(1, 0)?,
                brand: self.brand(raw.child(0)?, limit)?,
            },
            16 => Type::Struct {
                type_id: data.read_u64(1, 0)?,
                brand: self.brand(raw.child(0)?, limit)?,
            },
            17 => Type::Interface {
                type_id: data.read_u64(1, 0)?,
                brand: self.brand(raw.child(0)?, limit)?,
            },
            18 => Type::AnyPointer(match data.read_u16(4, 0)? {
                0 => AnyPointerType::Unconstrained(match data.read_u16(5, 0)? {
                    0 => AnyPointerKind::Any,
                    1 => AnyPointerKind::Struct,
                    2 => AnyPointerKind::List,
                    3 => AnyPointerKind::Capability,
                    value => return Err(unknown("Type.anyPointer.unconstrained", value)),
                }),
                1 => AnyPointerType::Parameter {
                    scope_id: data.read_u64(2, 0)?,
                    index: data.read_u16(5, 0)?,
                },
                2 => AnyPointerType::ImplicitMethodParameter {
                    index: data.read_u16(5, 0)?,
                },
                value => return Err(unknown("Type.anyPointer", value)),
            }),
            value => return Err(unknown("Type", value)),
        })
    }

    fn brand(&mut self, raw: RawStruct<'_, '_>, limit: usize) -> Result<Brand, LoadError> {
        Ok(Brand {
            scopes: self.structs(raw, 0, limit, |loader, value| {
                let data = value.data()?;
                let binding = match data.read_u16(4, 0)? {
                    0 => {
                        ScopeBinding::Bind(loader.structs(value, 0, limit, |loader, binding| {
                            Ok(match binding.data()?.read_u16(0, 0)? {
                                0 => BrandBinding::Unbound,
                                1 => BrandBinding::Type(loader.ty(binding.child(0)?, limit)?),
                                tag => return Err(unknown("Brand.Binding", tag)),
                            })
                        })?)
                    }
                    1 => ScopeBinding::Inherit,
                    tag => return Err(unknown("Brand.Scope", tag)),
                };
                Ok(BrandScope {
                    scope_id: data.read_u64(0, 0)?,
                    binding,
                })
            })?,
        })
    }

    fn value(&mut self, raw: RawStruct<'_, '_>) -> Result<Value, LoadError> {
        let data = raw.data()?;
        Ok(match data.read_u16(0, 0)? {
            0 => Value::Void,
            1 => Value::Bool(data.read_bool(16, false)?),
            2 => Value::Int8(data.read_i8(2, 0)?),
            3 => Value::Int16(data.read_i16(1, 0)?),
            4 => Value::Int32(data.read_i32(1, 0)?),
            5 => Value::Int64(data.read_i64(1, 0)?),
            6 => Value::UInt8(data.read_u8(2, 0)?),
            7 => Value::UInt16(data.read_u16(1, 0)?),
            8 => Value::UInt32(data.read_u32(1, 0)?),
            9 => Value::UInt64(data.read_u64(1, 0)?),
            10 => Value::Float32(data.read_f32(1, 0.0)?),
            11 => Value::Float64(data.read_f64(1, 0.0)?),
            12 => Value::Text(raw.text(0)?),
            13 => Value::Data(raw.bytes(0)?),
            14 => Value::List(expect_pointer_kind(
                "Value.list",
                raw.opaque(0, Arc::clone(&self.backing))?,
                OpaquePointerKind::List,
            )?),
            15 => Value::Enum(data.read_u16(1, 0)?),
            16 => Value::Struct(expect_pointer_kind(
                "Value.struct",
                raw.opaque(0, Arc::clone(&self.backing))?,
                OpaquePointerKind::Struct,
            )?),
            17 => Value::Interface,
            18 => Value::AnyPointer(raw.opaque(0, Arc::clone(&self.backing))?),
            value => return Err(unknown("Value", value)),
        })
    }

    fn source_info(
        &mut self,
        raw: RawStruct<'_, '_>,
        limit: usize,
    ) -> Result<SourceInfo, LoadError> {
        let data = raw.data()?;
        Ok(SourceInfo {
            id: data.read_u64(0, 0)?,
            doc_comment: raw.text(0)?,
            members: self.structs(raw, 1, limit, |_, value| {
                let data = value.data()?;
                Ok(MemberSourceInfo {
                    doc_comment: value.text(0)?,
                    start_byte: data.read_u32(0, 0)?,
                    end_byte: data.read_u32(1, 0)?,
                })
            })?,
            start_byte: data.read_u32(2, 0)?,
            end_byte: data.read_u32(3, 0)?,
        })
    }

    fn requested_file(
        &mut self,
        raw: RawStruct<'_, '_>,
        limit: usize,
    ) -> Result<RequestedFile, LoadError> {
        let data = raw.data()?;
        let file_source = raw.child(2)?;
        Ok(RequestedFile {
            id: data.read_u64(0, 0)?,
            filename: raw.text(0)?,
            imports: self.structs(raw, 1, limit, |_, value| {
                Ok(Import {
                    id: value.data()?.read_u64(0, 0)?,
                    name: value.text(0)?,
                })
            })?,
            identifiers: self.structs(file_source, 0, limit, |_, value| {
                let data = value.data()?;
                let target = match data.read_u16(8, 0)? {
                    0 => IdentifierTarget::Type(data.read_u64(1, 0)?),
                    1 => IdentifierTarget::Member {
                        parent_type_id: data.read_u64(1, 0)?,
                        ordinal: data.read_u16(9, 0)?,
                    },
                    tag => return Err(unknown("Identifier", tag)),
                };
                Ok(Identifier {
                    start_byte: data.read_u32(0, 0)?,
                    end_byte: data.read_u32(1, 0)?,
                    target,
                })
            })?,
        })
    }
}

pub(crate) fn load(bytes: &[u8], limits: LoadLimits) -> Result<CompiledSchema, LoadError> {
    let frame = match parse_frame(
        bytes,
        FrameLimits {
            max_segments: limits.max_segments,
            max_total_words: limits.max_total_words,
        },
    )? {
        FrameRead::EndOfInput => return Err(LoadError::EmptyRequest),
        FrameRead::Message { frame, remaining } => {
            if !remaining.is_empty() {
                return Err(LoadError::TrailingData(remaining.len()));
            }
            frame
        }
    };
    let backing: Arc<[Arc<[u8]>]> = frame
        .segments()
        .iter()
        .map(|segment| Arc::<[u8]>::from(segment.bytes()))
        .collect::<Vec<_>>()
        .into();
    let segment_bytes = backing.iter().map(AsRef::as_ref).collect::<Vec<_>>();
    let message = MessageSegments::new(&segment_bytes)?;
    let budget = LocalTraversalBudget::new(limits.max_traversal_words);
    let root = message.read_struct(
        WireLocation {
            segment_id: 0,
            word_offset: 0,
        },
        &budget,
        NestingLimit::new(limits.max_nesting),
    )?;
    let root = RawStruct::Struct(root);
    let mut loader = Loader {
        remaining_items: limits.max_metadata_items,
        backing: Arc::clone(&backing),
    };
    let version_data = root.child(2)?.data()?;
    let version = CapnpVersion {
        major: version_data.read_u16(0, 0)?,
        minor: version_data.read_u8(2, 0)?,
        micro: version_data.read_u8(3, 0)?,
    };
    let nodes = loader.structs(root, 0, limits.max_metadata_items, |loader, value| {
        loader.node(value, limits.max_metadata_items)
    })?;
    let requested_files = loader.structs(root, 1, limits.max_metadata_items, |loader, value| {
        loader.requested_file(value, limits.max_metadata_items)
    })?;
    let source_info = loader.structs(root, 3, limits.max_metadata_items, |loader, value| {
        loader.source_info(value, limits.max_metadata_items)
    })?;
    let _remaining_words = budget.remaining_words();
    CompiledSchema::indexed(version, nodes, source_info, requested_files)
}

fn parse_element_size(value: u16) -> Result<ElementSize, LoadError> {
    Ok(match value {
        0 => ElementSize::Empty,
        1 => ElementSize::Bit,
        2 => ElementSize::Byte,
        3 => ElementSize::TwoBytes,
        4 => ElementSize::FourBytes,
        5 => ElementSize::EightBytes,
        6 => ElementSize::Pointer,
        7 => ElementSize::InlineComposite,
        value => return Err(unknown("ElementSize", value)),
    })
}

const fn unknown(context: &'static str, value: u16) -> LoadError {
    LoadError::UnknownDiscriminant { context, value }
}

fn expect_pointer_kind(
    context: &'static str,
    pointer: OpaquePointer,
    expected: OpaquePointerKind,
) -> Result<OpaquePointer, LoadError> {
    if pointer.kind == OpaquePointerKind::Null || pointer.kind == expected {
        Ok(pointer)
    } else {
        Err(LoadError::PointerKind {
            context,
            expected,
            actual: pointer.kind,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_opaque_values_reject_the_wrong_pointer_kind() {
        let pointer = OpaquePointer {
            kind: OpaquePointerKind::Struct,
            backing: Arc::from([]),
            location: WireLocation {
                segment_id: 0,
                word_offset: 0,
            },
            nesting: NestingLimit::new(0),
        };
        assert_eq!(
            expect_pointer_kind("Value.list", pointer, OpaquePointerKind::List),
            Err(LoadError::PointerKind {
                context: "Value.list",
                expected: OpaquePointerKind::List,
                actual: OpaquePointerKind::Struct,
            })
        );
        assert_eq!(
            expect_pointer_kind(
                "Value.struct",
                OpaquePointer {
                    kind: OpaquePointerKind::Null,
                    backing: Arc::from([]),
                    location: WireLocation {
                        segment_id: 0,
                        word_offset: 0,
                    },
                    nesting: NestingLimit::new(0),
                },
                OpaquePointerKind::Struct,
            ),
            Ok(OpaquePointer {
                kind: OpaquePointerKind::Null,
                backing: Arc::from([]),
                location: WireLocation {
                    segment_id: 0,
                    word_offset: 0,
                },
                nesting: NestingLimit::new(0),
            })
        );
    }
}
