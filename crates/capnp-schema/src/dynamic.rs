//! Reflection-driven views over owned messages.
//!
//! The schema and message are both held by `Arc`; no registry, leaked schema,
//! or self-referential borrow is needed. A struct-list element is retained as
//! its list coordinate, while pointer targets retain checked object handles.
//! Reads use the same evolution, traversal, and nesting rules as generated
//! readers. Code generation and text/JSON codecs remain later milestones.

use std::fmt::{self, Write};
use std::sync::Arc;

use capnp_message::{
    ArenaError, DataListBuilder, DataSection, ExclusiveArena, GraphError, ListObject,
    ListReadError, ObjectRef, OwnedMessage, OwnedPointerRef, OwnedReadError, PointerListBuilder,
    PreparedStructRef, PrimitiveError, ReaderLimits, SharedTraversalBudget, StructBuilder,
    StructElementReader, StructListBuilder, StructObject, StructReadError, StructReader,
};

use crate::{
    AnyPointerKind, AnyPointerType, Brand, BrandBinding, CompiledSchema, Field, FieldKind, NodeId,
    NodeKind, OpaquePointer, OpaquePointerKind, ScopeBinding, StructSchema, Type, Value,
};

#[derive(Clone, Debug, PartialEq)]
pub enum DynamicError {
    Message(OwnedReadError),
    Struct(StructReadError),
    List(ListReadError),
    Primitive(PrimitiveError),
    Arena(ArenaError),
    Graph(GraphError),
    UnknownSchema(NodeId),
    ExpectedStructSchema(NodeId),
    UnknownField {
        type_id: NodeId,
        name: String,
    },
    FieldIndexOutOfBounds {
        type_id: NodeId,
        index: usize,
    },
    InactiveUnion {
        field: String,
        expected: u16,
        actual: u16,
    },
    TypeMismatch {
        expected: &'static str,
    },
    IndexOutOfBounds {
        index: u32,
        len: u32,
    },
    Downcast {
        expected: NodeId,
        actual: NodeId,
    },
    InvalidUtf8,
    Format,
}

impl fmt::Display for DynamicError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for DynamicError {}

impl From<OwnedReadError> for DynamicError {
    fn from(value: OwnedReadError) -> Self {
        Self::Message(value)
    }
}
impl From<StructReadError> for DynamicError {
    fn from(value: StructReadError) -> Self {
        Self::Struct(value)
    }
}
impl From<ListReadError> for DynamicError {
    fn from(value: ListReadError) -> Self {
        Self::List(value)
    }
}
impl From<PrimitiveError> for DynamicError {
    fn from(value: PrimitiveError) -> Self {
        Self::Primitive(value)
    }
}
impl From<ArenaError> for DynamicError {
    fn from(value: ArenaError) -> Self {
        Self::Arena(value)
    }
}
impl From<GraphError> for DynamicError {
    fn from(value: GraphError) -> Self {
        Self::Graph(value)
    }
}

#[derive(Clone, Debug)]
enum StructBacking {
    Pointer(ObjectRef<StructObject>),
    Element(capnp_message::StructElementRef),
}

#[derive(Clone, Debug)]
pub struct DynamicStruct {
    schema: Arc<CompiledSchema>,
    type_id: NodeId,
    brand: Brand,
    backing: StructBacking,
}

pub trait FromDynamicStruct: Sized {
    const TYPE_ID: NodeId;
    fn from_dynamic(value: DynamicStruct) -> Result<Self, DynamicError>;
}

/// Constant-layout generated access over a reflection-capable struct value.
///
/// Root and retained struct pointers are resolved and traversal-charged once.
/// Scalar generated accessors then borrow their validated data section directly;
/// unsupported element-backed shapes continue through the dynamic fallback.
#[derive(Clone, Debug)]
pub struct GeneratedStructReader {
    dynamic: DynamicStruct,
    prepared: Option<PreparedStructRef>,
}

impl GeneratedStructReader {
    #[doc(hidden)]
    pub fn new(dynamic: DynamicStruct) -> Self {
        let prepared = dynamic.prepared_reader().ok().flatten();
        Self { dynamic, prepared }
    }

    pub fn dynamic(&self) -> &DynamicStruct {
        &self.dynamic
    }

    /// Copies a fixed generated data prefix from immutable retained storage.
    /// Short evolution views return `None` and continue through checked reads.
    #[doc(hidden)]
    #[inline(always)]
    pub fn copy_data_prefix<const N: usize>(&self) -> Option<[u8; N]> {
        let data = self.data_section().ok()??;
        data.as_bytes().get(..N)?.try_into().ok()
    }

    #[doc(hidden)]
    pub fn get(&self, name: &str) -> Result<DynamicValue, DynamicError> {
        self.dynamic.get(name)
    }

    #[doc(hidden)]
    pub fn union_discriminant(&self) -> Result<Option<u16>, DynamicError> {
        self.dynamic.union_discriminant()
    }

    fn data_section(&self) -> Result<Option<DataSection<'_>>, DynamicError> {
        self.prepared
            .as_ref()
            .map(PreparedStructRef::data_section)
            .transpose()
            .map_err(Into::into)
    }

    #[doc(hidden)]
    #[inline(always)]
    pub fn read_bool_slot(&self, offset: u32, default: bool) -> Result<bool, DynamicError> {
        match self.data_section()? {
            Some(data) => Ok(data.read_bool(offset, default)?),
            None => self.dynamic.read_bool_slot(offset, default),
        }
    }

    #[doc(hidden)]
    #[inline(always)]
    pub fn read_i8_slot(&self, offset: u32, default: i8) -> Result<i8, DynamicError> {
        match self.data_section()? {
            Some(data) => Ok(data.read_i8(offset, default)?),
            None => self.dynamic.read_i8_slot(offset, default),
        }
    }

    #[doc(hidden)]
    #[inline(always)]
    pub fn read_i16_slot(&self, offset: u32, default: i16) -> Result<i16, DynamicError> {
        match self.data_section()? {
            Some(data) => Ok(data.read_i16(offset, default)?),
            None => self.dynamic.read_i16_slot(offset, default),
        }
    }

    #[doc(hidden)]
    #[inline(always)]
    pub fn read_i32_slot(&self, offset: u32, default: i32) -> Result<i32, DynamicError> {
        match self.data_section()? {
            Some(data) => Ok(data.read_i32(offset, default)?),
            None => self.dynamic.read_i32_slot(offset, default),
        }
    }

    #[doc(hidden)]
    #[inline(always)]
    pub fn read_i64_slot(&self, offset: u32, default: i64) -> Result<i64, DynamicError> {
        match self.data_section()? {
            Some(data) => Ok(data.read_i64(offset, default)?),
            None => self.dynamic.read_i64_slot(offset, default),
        }
    }

    #[doc(hidden)]
    #[inline(always)]
    pub fn read_u8_slot(&self, offset: u32, default: u8) -> Result<u8, DynamicError> {
        match self.data_section()? {
            Some(data) => Ok(data.read_u8(offset, default)?),
            None => self.dynamic.read_u8_slot(offset, default),
        }
    }

    #[doc(hidden)]
    #[inline(always)]
    pub fn read_u16_slot(&self, offset: u32, default: u16) -> Result<u16, DynamicError> {
        match self.data_section()? {
            Some(data) => Ok(data.read_u16(offset, default)?),
            None => self.dynamic.read_u16_slot(offset, default),
        }
    }

    #[doc(hidden)]
    #[inline(always)]
    pub fn read_u32_slot(&self, offset: u32, default: u32) -> Result<u32, DynamicError> {
        match self.data_section()? {
            Some(data) => Ok(data.read_u32(offset, default)?),
            None => self.dynamic.read_u32_slot(offset, default),
        }
    }

    #[doc(hidden)]
    #[inline(always)]
    pub fn read_u64_slot(&self, offset: u32, default: u64) -> Result<u64, DynamicError> {
        match self.data_section()? {
            Some(data) => Ok(data.read_u64(offset, default)?),
            None => self.dynamic.read_u64_slot(offset, default),
        }
    }

    #[doc(hidden)]
    #[inline(always)]
    pub fn read_f32_slot(&self, offset: u32, default: f32) -> Result<f32, DynamicError> {
        match self.data_section()? {
            Some(data) => Ok(data.read_f32(offset, default)?),
            None => self.dynamic.read_f32_slot(offset, default),
        }
    }

    #[doc(hidden)]
    #[inline(always)]
    pub fn read_f64_slot(&self, offset: u32, default: f64) -> Result<f64, DynamicError> {
        match self.data_section()? {
            Some(data) => Ok(data.read_f64(offset, default)?),
            None => self.dynamic.read_f64_slot(offset, default),
        }
    }
}

impl DynamicStruct {
    fn prepared_reader(&self) -> Result<Option<PreparedStructRef>, DynamicError> {
        match &self.backing {
            StructBacking::Pointer(value) => Ok(Some(value.prepare_reader()?)),
            StructBacking::Element(_) => Ok(None),
        }
    }

    pub fn root(
        schema: Arc<CompiledSchema>,
        message: Arc<OwnedMessage>,
        type_id: NodeId,
    ) -> Result<Self, DynamicError> {
        Self::root_branded(schema, message, type_id, Brand::default())
    }

    pub fn root_branded(
        schema: Arc<CompiledSchema>,
        message: Arc<OwnedMessage>,
        type_id: NodeId,
        brand: Brand,
    ) -> Result<Self, DynamicError> {
        require_struct(&schema, type_id)?;
        Ok(Self {
            schema,
            type_id,
            brand,
            backing: StructBacking::Pointer(message.root_struct()?.into_root()),
        })
    }

    fn from_pointer(
        schema: Arc<CompiledSchema>,
        type_id: NodeId,
        brand: Brand,
        pointer: ObjectRef<StructObject>,
    ) -> Result<Self, DynamicError> {
        require_struct(&schema, type_id)?;
        Ok(Self {
            schema,
            type_id,
            brand,
            backing: StructBacking::Pointer(pointer),
        })
    }

    pub fn from_value(
        schema: Arc<CompiledSchema>,
        type_id: NodeId,
        value: &Value,
        limits: ReaderLimits,
    ) -> Result<Option<Self>, DynamicError> {
        Self::from_branded_value(schema, type_id, Brand::default(), value, limits)
    }

    pub fn from_branded_value(
        schema: Arc<CompiledSchema>,
        type_id: NodeId,
        brand: Brand,
        value: &Value,
        limits: ReaderLimits,
    ) -> Result<Option<Self>, DynamicError> {
        let Value::Struct(pointer) = value else {
            return Err(type_mismatch("struct schema value"));
        };
        match pointer.open(limits)? {
            OwnedPointerRef::Null => Ok(None),
            OwnedPointerRef::Struct(value) => {
                Ok(Some(Self::from_pointer(schema, type_id, brand, value)?))
            }
            _ => Err(type_mismatch("struct schema value")),
        }
    }

    fn from_element(
        schema: Arc<CompiledSchema>,
        type_id: NodeId,
        brand: Brand,
        element: capnp_message::StructElementRef,
    ) -> Result<Self, DynamicError> {
        require_struct(&schema, type_id)?;
        Ok(Self {
            schema,
            type_id,
            brand,
            backing: StructBacking::Element(element),
        })
    }

    pub const fn type_id(&self) -> NodeId {
        self.type_id
    }

    pub fn schema(&self) -> &Arc<CompiledSchema> {
        &self.schema
    }

    pub fn downcast<T: FromDynamicStruct>(self) -> Result<T, DynamicError> {
        if self.type_id != T::TYPE_ID {
            return Err(DynamicError::Downcast {
                expected: T::TYPE_ID,
                actual: self.type_id,
            });
        }
        T::from_dynamic(self)
    }

    pub fn get(&self, name: &str) -> Result<DynamicValue, DynamicError> {
        let structure = self.struct_schema()?;
        let field = structure
            .field(name)
            .ok_or_else(|| DynamicError::UnknownField {
                type_id: self.type_id,
                name: name.to_owned(),
            })?;
        self.get_field(field, structure)
    }

    pub fn get_by_index(&self, index: usize) -> Result<DynamicValue, DynamicError> {
        let structure = self.struct_schema()?;
        let field = structure
            .field_by_index(index)
            .ok_or(DynamicError::FieldIndexOutOfBounds {
                type_id: self.type_id,
                index,
            })?;
        self.get_field(field, structure)
    }

    /// Reads a generated scalar slot without performing reflection lookup.
    #[doc(hidden)]
    #[inline(always)]
    pub fn read_bool_slot(&self, offset: u32, default: bool) -> Result<bool, DynamicError> {
        self.with_reader(|reader| Ok(reader.data()?.read_bool(offset, default)?))
    }

    /// Reads a generated scalar slot without performing reflection lookup.
    #[doc(hidden)]
    #[inline(always)]
    pub fn read_i8_slot(&self, offset: u32, default: i8) -> Result<i8, DynamicError> {
        self.with_reader(|reader| Ok(reader.data()?.read_i8(offset, default)?))
    }

    /// Reads a generated scalar slot without performing reflection lookup.
    #[doc(hidden)]
    #[inline(always)]
    pub fn read_i16_slot(&self, offset: u32, default: i16) -> Result<i16, DynamicError> {
        self.with_reader(|reader| Ok(reader.data()?.read_i16(offset, default)?))
    }

    /// Reads a generated scalar slot without performing reflection lookup.
    #[doc(hidden)]
    #[inline(always)]
    pub fn read_i32_slot(&self, offset: u32, default: i32) -> Result<i32, DynamicError> {
        self.with_reader(|reader| Ok(reader.data()?.read_i32(offset, default)?))
    }

    /// Reads a generated scalar slot without performing reflection lookup.
    #[doc(hidden)]
    #[inline(always)]
    pub fn read_i64_slot(&self, offset: u32, default: i64) -> Result<i64, DynamicError> {
        self.with_reader(|reader| Ok(reader.data()?.read_i64(offset, default)?))
    }

    /// Reads a generated scalar slot without performing reflection lookup.
    #[doc(hidden)]
    #[inline(always)]
    pub fn read_u8_slot(&self, offset: u32, default: u8) -> Result<u8, DynamicError> {
        self.with_reader(|reader| Ok(reader.data()?.read_u8(offset, default)?))
    }

    /// Reads a generated scalar slot without performing reflection lookup.
    #[doc(hidden)]
    #[inline(always)]
    pub fn read_u16_slot(&self, offset: u32, default: u16) -> Result<u16, DynamicError> {
        self.with_reader(|reader| Ok(reader.data()?.read_u16(offset, default)?))
    }

    /// Reads a generated scalar slot without performing reflection lookup.
    #[doc(hidden)]
    #[inline(always)]
    pub fn read_u32_slot(&self, offset: u32, default: u32) -> Result<u32, DynamicError> {
        self.with_reader(|reader| Ok(reader.data()?.read_u32(offset, default)?))
    }

    /// Reads a generated scalar slot without performing reflection lookup.
    #[doc(hidden)]
    #[inline(always)]
    pub fn read_u64_slot(&self, offset: u32, default: u64) -> Result<u64, DynamicError> {
        self.with_reader(|reader| Ok(reader.data()?.read_u64(offset, default)?))
    }

    /// Reads a generated scalar slot without performing reflection lookup.
    #[doc(hidden)]
    #[inline(always)]
    pub fn read_f32_slot(&self, offset: u32, default: f32) -> Result<f32, DynamicError> {
        self.with_reader(|reader| Ok(reader.data()?.read_f32(offset, default)?))
    }

    /// Reads a generated scalar slot without performing reflection lookup.
    #[doc(hidden)]
    #[inline(always)]
    pub fn read_f64_slot(&self, offset: u32, default: f64) -> Result<f64, DynamicError> {
        self.with_reader(|reader| Ok(reader.data()?.read_f64(offset, default)?))
    }

    /// Resolves a field's runtime type through this value's current brand.
    pub fn field_type(&self, name: &str) -> Result<Type, DynamicError> {
        let structure = self.struct_schema()?;
        let field = structure
            .field(name)
            .ok_or_else(|| DynamicError::UnknownField {
                type_id: self.type_id,
                name: name.to_owned(),
            })?;
        match &field.kind {
            FieldKind::Slot { ty, .. } => Ok(self.resolve_type(ty)),
            FieldKind::Group { type_id } => Ok(Type::Struct {
                type_id: *type_id,
                brand: self.brand.clone(),
            }),
        }
    }

    /// Reports whether a field has an on-wire value worth printing.
    ///
    /// Data fields and groups are always present. Pointer slots are present
    /// only when their wire pointer is non-null, matching the reference text
    /// printer's omission of absent pointer fields even when they have a
    /// non-null schema default.
    pub fn is_field_present(&self, name: &str) -> Result<bool, DynamicError> {
        let structure = self.struct_schema()?;
        let field = structure
            .field(name)
            .ok_or_else(|| DynamicError::UnknownField {
                type_id: self.type_id,
                name: name.to_owned(),
            })?;
        let active_union = if let Some(expected) = field.discriminant_value {
            let actual = self.with_reader(|reader| {
                Ok(reader.data()?.read_u16(structure.discriminant_offset, 0)?)
            })?;
            if actual != expected {
                return Ok(false);
            }
            true
        } else {
            false
        };
        if active_union {
            return Ok(true);
        }
        match &field.kind {
            FieldKind::Group { .. } => Ok(true),
            FieldKind::Slot { offset, ty, .. } => match self.resolve_type(ty) {
                Type::Text
                | Type::Data
                | Type::List(_)
                | Type::Struct { .. }
                | Type::Interface { .. }
                | Type::AnyPointer(_) => Ok(!matches!(
                    self.pointer(u16_offset(*offset)?)?,
                    OwnedPointerRef::Null
                )),
                _ => Ok(true),
            },
        }
    }

    pub fn active_union_field(&self) -> Result<Option<&Field>, DynamicError> {
        let structure = self.struct_schema()?;
        if structure.discriminant_count == 0 {
            return Ok(None);
        }
        let Some(actual) = self.union_discriminant()? else {
            return Ok(None);
        };
        Ok(structure
            .fields
            .iter()
            .find(|field| field.discriminant_value == Some(actual)))
    }

    /// Returns the raw union discriminant, preserving values unknown to this schema.
    pub fn union_discriminant(&self) -> Result<Option<u16>, DynamicError> {
        let structure = self.struct_schema()?;
        if structure.discriminant_count == 0 {
            return Ok(None);
        }
        self.with_reader(|reader| {
            Ok(Some(
                reader.data()?.read_u16(structure.discriminant_offset, 0)?,
            ))
        })
    }

    pub fn stringify(&self) -> Result<String, DynamicError> {
        let mut output = String::new();
        self.write_string(&mut output)?;
        Ok(output)
    }

    fn write_string(&self, output: &mut String) -> Result<(), DynamicError> {
        output.push('(');
        let structure = self.struct_schema()?;
        let mut first = true;
        for (index, field) in structure.fields.iter().enumerate() {
            match self.get_by_index(index) {
                Ok(value) => {
                    if !first {
                        output.push_str(", ");
                    }
                    first = false;
                    output.push_str(&field.name);
                    output.push_str(" = ");
                    value.write_string(output)?;
                }
                Err(DynamicError::InactiveUnion { .. }) => {}
                Err(error) => return Err(error),
            }
        }
        output.push(')');
        Ok(())
    }

    fn struct_schema(&self) -> Result<&StructSchema, DynamicError> {
        require_struct(&self.schema, self.type_id)
    }

    fn get_field(
        &self,
        field: &Field,
        structure: &StructSchema,
    ) -> Result<DynamicValue, DynamicError> {
        if let Some(expected) = field.discriminant_value {
            let actual = self.with_reader(|reader| {
                Ok(reader.data()?.read_u16(structure.discriminant_offset, 0)?)
            })?;
            if actual != expected {
                return Err(DynamicError::InactiveUnion {
                    field: field.name.clone(),
                    expected,
                    actual,
                });
            }
        }
        match &field.kind {
            FieldKind::Group { type_id } => Ok(DynamicValue::Struct(Some(Self {
                schema: Arc::clone(&self.schema),
                type_id: *type_id,
                brand: self.brand.clone(),
                backing: self.backing.clone(),
            }))),
            FieldKind::Slot {
                offset,
                ty,
                default_value,
                ..
            } => self.read_slot(*offset, ty, default_value),
        }
    }

    fn read_slot(
        &self,
        offset: u32,
        ty: &Type,
        default: &Value,
    ) -> Result<DynamicValue, DynamicError> {
        let resolved_type = self.resolve_type(ty);
        match &resolved_type {
            Type::Void => Ok(DynamicValue::Void),
            Type::Bool => self.scalar(|data| {
                Ok(DynamicValue::Bool(
                    data.read_bool(offset, bool_default(default)?)?,
                ))
            }),
            Type::Int8 => self.scalar(|data| {
                Ok(DynamicValue::Int8(
                    data.read_i8(offset, i8_default(default)?)?,
                ))
            }),
            Type::Int16 => self.scalar(|data| {
                Ok(DynamicValue::Int16(
                    data.read_i16(offset, i16_default(default)?)?,
                ))
            }),
            Type::Int32 => self.scalar(|data| {
                Ok(DynamicValue::Int32(
                    data.read_i32(offset, i32_default(default)?)?,
                ))
            }),
            Type::Int64 => self.scalar(|data| {
                Ok(DynamicValue::Int64(
                    data.read_i64(offset, i64_default(default)?)?,
                ))
            }),
            Type::UInt8 => self.scalar(|data| {
                Ok(DynamicValue::UInt8(
                    data.read_u8(offset, u8_default(default)?)?,
                ))
            }),
            Type::UInt16 => self.scalar(|data| {
                Ok(DynamicValue::UInt16(
                    data.read_u16(offset, u16_default(default)?)?,
                ))
            }),
            Type::UInt32 => self.scalar(|data| {
                Ok(DynamicValue::UInt32(
                    data.read_u32(offset, u32_default(default)?)?,
                ))
            }),
            Type::UInt64 => self.scalar(|data| {
                Ok(DynamicValue::UInt64(
                    data.read_u64(offset, u64_default(default)?)?,
                ))
            }),
            Type::Float32 => self.scalar(|data| {
                Ok(DynamicValue::Float32(
                    data.read_f32(offset, f32_default(default)?)?,
                ))
            }),
            Type::Float64 => self.scalar(|data| {
                Ok(DynamicValue::Float64(
                    data.read_f64(offset, f64_default(default)?)?,
                ))
            }),
            Type::Enum { type_id, .. } => self.scalar(|data| {
                Ok(DynamicValue::Enum(DynamicEnum {
                    schema: Arc::clone(&self.schema),
                    type_id: *type_id,
                    ordinal: data.read_u16(offset, enum_default(default)?)?,
                }))
            }),
            Type::Text => self.read_text(offset, default),
            Type::Data => self.read_data(offset, default),
            Type::Struct { type_id, brand } => match self.pointer(u16_offset(offset)?)? {
                OwnedPointerRef::Null => self.default_struct(*type_id, brand, default),
                OwnedPointerRef::Struct(pointer) => Ok(DynamicValue::Struct(Some(
                    Self::from_pointer(Arc::clone(&self.schema), *type_id, brand.clone(), pointer)?,
                ))),
                _ => Err(type_mismatch("struct pointer")),
            },
            Type::List(element) => match self.pointer(u16_offset(offset)?)? {
                OwnedPointerRef::Null => self.default_list(element, default),
                OwnedPointerRef::List(pointer) => Ok(DynamicValue::List(Some(DynamicList {
                    schema: Arc::clone(&self.schema),
                    element_type: (**element).clone(),
                    pointer,
                }))),
                _ => Err(type_mismatch("list pointer")),
            },
            Type::Interface { .. } => match self.pointer(u16_offset(offset)?)? {
                OwnedPointerRef::Null => Ok(DynamicValue::Capability(None)),
                OwnedPointerRef::Capability(index) => Ok(DynamicValue::Capability(Some(index))),
                _ => Err(type_mismatch("capability pointer")),
            },
            Type::AnyPointer(kind) => self.read_any_pointer(offset, kind, default),
        }
    }

    fn scalar(
        &self,
        read: impl FnOnce(DataSection<'_>) -> Result<DynamicValue, DynamicError>,
    ) -> Result<DynamicValue, DynamicError> {
        self.with_reader(|reader| read(reader.data()?))
    }

    fn read_text(&self, offset: u32, default: &Value) -> Result<DynamicValue, DynamicError> {
        match self.pointer(u16_offset(offset)?)? {
            OwnedPointerRef::Null => match default {
                Value::Text(value) => Ok(DynamicValue::Text(value.clone())),
                Value::AnyPointer(value) if value.kind == OpaquePointerKind::Null => {
                    Ok(DynamicValue::Text(String::new()))
                }
                _ => Err(type_mismatch("Text default")),
            },
            OwnedPointerRef::List(pointer) => {
                let value = pointer
                    .with_text(|text| text.to_str().map(str::to_owned))?
                    .map_err(|_| DynamicError::InvalidUtf8)?;
                Ok(DynamicValue::Text(value))
            }
            _ => Err(type_mismatch("Text pointer")),
        }
    }

    fn read_data(&self, offset: u32, default: &Value) -> Result<DynamicValue, DynamicError> {
        match self.pointer(u16_offset(offset)?)? {
            OwnedPointerRef::Null => match default {
                Value::Data(value) => Ok(DynamicValue::Data(value.clone())),
                Value::AnyPointer(value) if value.kind == OpaquePointerKind::Null => {
                    Ok(DynamicValue::Data(Vec::new()))
                }
                _ => Err(type_mismatch("Data default")),
            },
            OwnedPointerRef::List(pointer) => Ok(DynamicValue::Data(
                pointer.with_data(|data| data.as_bytes().to_vec())?,
            )),
            _ => Err(type_mismatch("Data pointer")),
        }
    }

    fn default_struct(
        &self,
        type_id: NodeId,
        brand: &Brand,
        default: &Value,
    ) -> Result<DynamicValue, DynamicError> {
        match default {
            Value::Struct(pointer) => match pointer.open(ReaderLimits::default())? {
                OwnedPointerRef::Null => Ok(DynamicValue::Struct(None)),
                OwnedPointerRef::Struct(value) => Ok(DynamicValue::Struct(Some(
                    Self::from_pointer(Arc::clone(&self.schema), type_id, brand.clone(), value)?,
                ))),
                _ => Err(type_mismatch("struct default")),
            },
            Value::AnyPointer(pointer) if pointer.kind == OpaquePointerKind::Null => {
                Ok(DynamicValue::Struct(None))
            }
            _ => Err(type_mismatch("struct default")),
        }
    }

    fn default_list(&self, element: &Type, default: &Value) -> Result<DynamicValue, DynamicError> {
        match default {
            Value::List(pointer) => match pointer.open(ReaderLimits::default())? {
                OwnedPointerRef::Null => Ok(DynamicValue::List(None)),
                OwnedPointerRef::List(value) => Ok(DynamicValue::List(Some(DynamicList {
                    schema: Arc::clone(&self.schema),
                    element_type: element.clone(),
                    pointer: value,
                }))),
                _ => Err(type_mismatch("list default")),
            },
            Value::AnyPointer(pointer) if pointer.kind == OpaquePointerKind::Null => {
                Ok(DynamicValue::List(None))
            }
            _ => Err(type_mismatch("list default")),
        }
    }

    fn read_any_pointer(
        &self,
        offset: u32,
        kind: &AnyPointerType,
        default: &Value,
    ) -> Result<DynamicValue, DynamicError> {
        let mut pointer = self.pointer(u16_offset(offset)?)?;
        if matches!(pointer, OwnedPointerRef::Null) {
            let Value::AnyPointer(default) = default else {
                return Err(type_mismatch("AnyPointer default"));
            };
            pointer = default.open(ReaderLimits::default())?;
        }
        enforce_any_kind(kind, &pointer)?;
        Ok(DynamicValue::AnyPointer(DynamicAnyPointer::from_owned(
            pointer,
        )))
    }

    fn pointer(&self, index: u16) -> Result<OwnedPointerRef, DynamicError> {
        Ok(match &self.backing {
            StructBacking::Pointer(value) => value.child_pointer(index)?,
            StructBacking::Element(value) => value.child_pointer(index)?,
        })
    }

    fn resolve_type(&self, ty: &Type) -> Type {
        resolve_type_with_brand(ty, &self.brand)
    }

    fn with_reader<R>(
        &self,
        read: impl for<'reader> FnOnce(DynamicStructReader<'reader>) -> Result<R, DynamicError>,
    ) -> Result<R, DynamicError> {
        let result = match &self.backing {
            StructBacking::Pointer(value) => {
                value.with_reader(|reader| read(DynamicStructReader::Struct(reader)))??
            }
            StructBacking::Element(value) => {
                value.with_reader(|reader| read(DynamicStructReader::Element(reader)))??
            }
        };
        Ok(result)
    }
}

enum DynamicStructReader<'reader> {
    Struct(StructReader<'reader, 'reader, SharedTraversalBudget>),
    Element(StructElementReader<'reader, 'reader, SharedTraversalBudget>),
}

impl DynamicStructReader<'_> {
    fn data(&self) -> Result<DataSection<'_>, DynamicError> {
        Ok(match self {
            Self::Struct(value) => (*value).data_section()?,
            Self::Element(value) => (*value).data_section()?,
        })
    }
}

fn require_struct(schema: &CompiledSchema, type_id: NodeId) -> Result<&StructSchema, DynamicError> {
    let node = schema
        .node(type_id)
        .ok_or(DynamicError::UnknownSchema(type_id))?;
    match &node.kind {
        NodeKind::Struct(value) => Ok(value),
        _ => Err(DynamicError::ExpectedStructSchema(type_id)),
    }
}

#[derive(Clone, Debug)]
pub struct DynamicList {
    schema: Arc<CompiledSchema>,
    element_type: Type,
    pointer: ObjectRef<ListObject>,
}

impl DynamicList {
    pub fn from_value(
        schema: Arc<CompiledSchema>,
        element_type: Type,
        value: &Value,
        limits: ReaderLimits,
    ) -> Result<Option<Self>, DynamicError> {
        let Value::List(pointer) = value else {
            return Err(type_mismatch("list schema value"));
        };
        match pointer.open(limits)? {
            OwnedPointerRef::Null => Ok(None),
            OwnedPointerRef::List(pointer) => Ok(Some(Self {
                schema,
                element_type,
                pointer,
            })),
            _ => Err(type_mismatch("list schema value")),
        }
    }

    pub fn len(&self) -> Result<u32, DynamicError> {
        Ok(self.pointer.with_reader(|reader| reader.len())?)
    }

    pub fn is_empty(&self) -> Result<bool, DynamicError> {
        Ok(self.len()? == 0)
    }

    pub fn element_type(&self) -> &Type {
        &self.element_type
    }

    pub fn get(&self, index: u32) -> Result<DynamicValue, DynamicError> {
        let len = self.len()?;
        if index >= len {
            return Err(DynamicError::IndexOutOfBounds { index, len });
        }
        match &self.element_type {
            Type::Void => Ok(DynamicValue::Void),
            Type::Bool => self.primitive(index, |list| {
                Ok(DynamicValue::Bool(list.as_primitive::<bool>()?.get(index)?))
            }),
            Type::Int8 => self.primitive(index, |list| {
                Ok(DynamicValue::Int8(list.as_primitive::<i8>()?.get(index)?))
            }),
            Type::Int16 => self.primitive(index, |list| {
                Ok(DynamicValue::Int16(list.as_primitive::<i16>()?.get(index)?))
            }),
            Type::Int32 => self.primitive(index, |list| {
                Ok(DynamicValue::Int32(list.as_primitive::<i32>()?.get(index)?))
            }),
            Type::Int64 => self.primitive(index, |list| {
                Ok(DynamicValue::Int64(list.as_primitive::<i64>()?.get(index)?))
            }),
            Type::UInt8 => self.primitive(index, |list| {
                Ok(DynamicValue::UInt8(list.as_primitive::<u8>()?.get(index)?))
            }),
            Type::UInt16 => self.primitive(index, |list| {
                Ok(DynamicValue::UInt16(
                    list.as_primitive::<u16>()?.get(index)?,
                ))
            }),
            Type::UInt32 => self.primitive(index, |list| {
                Ok(DynamicValue::UInt32(
                    list.as_primitive::<u32>()?.get(index)?,
                ))
            }),
            Type::UInt64 => self.primitive(index, |list| {
                Ok(DynamicValue::UInt64(
                    list.as_primitive::<u64>()?.get(index)?,
                ))
            }),
            Type::Float32 => self.primitive(index, |list| {
                Ok(DynamicValue::Float32(
                    list.as_primitive::<f32>()?.get(index)?,
                ))
            }),
            Type::Float64 => self.primitive(index, |list| {
                Ok(DynamicValue::Float64(
                    list.as_primitive::<f64>()?.get(index)?,
                ))
            }),
            Type::Enum { type_id, .. } => {
                let ordinal = self
                    .pointer
                    .with_reader(|reader| reader.as_primitive::<u16>()?.get(index))??;
                Ok(DynamicValue::Enum(DynamicEnum {
                    schema: Arc::clone(&self.schema),
                    type_id: *type_id,
                    ordinal,
                }))
            }
            Type::Text => match self.pointer.pointer_element(index)? {
                OwnedPointerRef::Null => Ok(DynamicValue::Text(String::new())),
                OwnedPointerRef::List(value) => {
                    let text = value
                        .with_text(|text| text.to_str().map(str::to_owned))?
                        .map_err(|_| DynamicError::InvalidUtf8)?;
                    Ok(DynamicValue::Text(text))
                }
                _ => Err(type_mismatch("Text list element")),
            },
            Type::Data => match self.pointer.pointer_element(index)? {
                OwnedPointerRef::Null => Ok(DynamicValue::Data(Vec::new())),
                OwnedPointerRef::List(value) => Ok(DynamicValue::Data(
                    value.with_data(|data| data.as_bytes().to_vec())?,
                )),
                _ => Err(type_mismatch("Data list element")),
            },
            Type::List(element) => match self.pointer.pointer_element(index)? {
                OwnedPointerRef::Null => Ok(DynamicValue::List(None)),
                OwnedPointerRef::List(value) => Ok(DynamicValue::List(Some(Self {
                    schema: Arc::clone(&self.schema),
                    element_type: (**element).clone(),
                    pointer: value,
                }))),
                _ => Err(type_mismatch("nested list element")),
            },
            Type::Struct { type_id, brand } => {
                Ok(DynamicValue::Struct(Some(DynamicStruct::from_element(
                    Arc::clone(&self.schema),
                    *type_id,
                    brand.clone(),
                    self.pointer.struct_element(index)?,
                )?)))
            }
            Type::Interface { .. } => match self.pointer.pointer_element(index)? {
                OwnedPointerRef::Null => Ok(DynamicValue::Capability(None)),
                OwnedPointerRef::Capability(value) => Ok(DynamicValue::Capability(Some(value))),
                _ => Err(type_mismatch("capability list element")),
            },
            Type::AnyPointer(kind) => {
                let pointer = self.pointer.pointer_element(index)?;
                enforce_any_kind(kind, &pointer)?;
                Ok(DynamicValue::AnyPointer(DynamicAnyPointer::from_owned(
                    pointer,
                )))
            }
        }
    }

    fn primitive(
        &self,
        _index: u32,
        read: impl for<'reader> FnOnce(
            capnp_message::ListReader<'reader, 'reader, SharedTraversalBudget>,
        ) -> Result<DynamicValue, ListReadError>,
    ) -> Result<DynamicValue, DynamicError> {
        Ok(self.pointer.with_reader(read)??)
    }

    pub fn stringify(&self) -> Result<String, DynamicError> {
        let mut output = String::new();
        output.push('[');
        for index in 0..self.len()? {
            if index != 0 {
                output.push_str(", ");
            }
            self.get(index)?.write_string(&mut output)?;
        }
        output.push(']');
        Ok(output)
    }
}

#[derive(Clone, Debug)]
pub struct DynamicEnum {
    schema: Arc<CompiledSchema>,
    pub type_id: NodeId,
    pub ordinal: u16,
}

impl DynamicEnum {
    pub fn enumerant(&self) -> Option<&crate::Enumerant> {
        let node = self.schema.node(self.type_id)?;
        let NodeKind::Enum(schema) = &node.kind else {
            return None;
        };
        schema.enumerants.get(usize::from(self.ordinal))
    }

    pub fn name(&self) -> Option<&str> {
        self.enumerant().map(|value| value.name.as_str())
    }
}

#[derive(Clone, Debug)]
pub enum DynamicAnyPointer {
    Null,
    Struct(ObjectRef<StructObject>),
    List(ObjectRef<ListObject>),
    Capability(u32),
}

impl DynamicAnyPointer {
    fn from_owned(value: OwnedPointerRef) -> Self {
        match value {
            OwnedPointerRef::Null => Self::Null,
            OwnedPointerRef::Struct(value) => Self::Struct(value),
            OwnedPointerRef::List(value) => Self::List(value),
            OwnedPointerRef::Capability(value) => Self::Capability(value),
        }
    }

    /// Retains this dynamic pointer as an opaque schema value suitable for
    /// lossless forwarding without decoding its application schema.
    pub fn to_opaque(&self, limits: ReaderLimits) -> Result<OpaquePointer, DynamicError> {
        let (kind, value) = match self {
            Self::Null => return Ok(OpaquePointer::null()),
            Self::Struct(value) => (OpaquePointerKind::Struct, value),
            Self::List(value) => {
                let message = value.message();
                let backing = (0..message.segment_count())
                    .filter_map(|index| message.segment(u32::try_from(index).ok()?).map(Arc::from))
                    .collect::<Vec<Arc<[u8]>>>();
                if backing.len() != message.segment_count() {
                    return Err(DynamicError::TypeMismatch {
                        expected: "retained list pointer",
                    });
                }
                return Ok(OpaquePointer {
                    kind: OpaquePointerKind::List,
                    backing: Arc::from(backing),
                    location: value.location(),
                    nesting: capnp_message::NestingLimit::new(limits.nesting_levels),
                });
            }
            Self::Capability(_) => {
                return Err(DynamicError::TypeMismatch {
                    expected: "non-capability any-pointer",
                });
            }
        };
        let message = value.message();
        let backing = (0..message.segment_count())
            .filter_map(|index| message.segment(u32::try_from(index).ok()?).map(Arc::from))
            .collect::<Vec<Arc<[u8]>>>();
        if backing.len() != message.segment_count() {
            return Err(DynamicError::TypeMismatch {
                expected: "retained struct pointer",
            });
        }
        Ok(OpaquePointer {
            kind,
            backing: Arc::from(backing),
            location: value.location(),
            nesting: capnp_message::NestingLimit::new(limits.nesting_levels),
        })
    }
}

#[derive(Clone, Debug)]
pub enum DynamicValue {
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
    List(Option<DynamicList>),
    Enum(DynamicEnum),
    Struct(Option<DynamicStruct>),
    Capability(Option<u32>),
    AnyPointer(DynamicAnyPointer),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DynamicInput<'a> {
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
    Text(&'a str),
    Data(&'a [u8]),
    Enum(u16),
    Capability(u32),
    /// An already validated pointer graph to copy into the destination.
    Pointer(&'a OpaquePointer),
}

/// An exclusive reflection-driven struct builder.
///
/// Child builders reborrow the parent, so two mutable descendants cannot
/// coexist even when field names are selected at runtime.
///
/// ```compile_fail
/// use capnp_schema::DynamicStructBuilder;
/// fn alias(builder: &mut DynamicStructBuilder<'_, '_>) {
///     let first = builder.init_struct("node").unwrap();
///     let second = builder.init_struct("node").unwrap();
///     drop((first, second));
/// }
/// ```
pub struct DynamicStructBuilder<'schema, 'arena> {
    schema: &'schema CompiledSchema,
    type_id: NodeId,
    brand: Brand,
    builder: StructBuilder<'arena>,
}

macro_rules! generated_slot_setters {
    ($(($method:ident, $setter:ident, $ty:ty)),+ $(,)?) => {
        $(
            #[doc(hidden)]
            #[inline(always)]
            pub fn $method(
                &mut self,
                offset: u32,
                value: $ty,
                default: $ty,
            ) -> Result<(), DynamicError> {
                Ok(self.builder.$setter(offset, value, default)?)
            }
        )+
    };
}

impl<'schema, 'arena> DynamicStructBuilder<'schema, 'arena> {
    pub fn root(
        schema: &'schema CompiledSchema,
        arena: &'arena mut ExclusiveArena,
        type_id: NodeId,
    ) -> Result<Self, DynamicError> {
        Self::root_branded(schema, arena, type_id, Brand::default())
    }

    pub fn root_branded(
        schema: &'schema CompiledSchema,
        arena: &'arena mut ExclusiveArena,
        type_id: NodeId,
        brand: Brand,
    ) -> Result<Self, DynamicError> {
        let structure = require_struct(schema, type_id)?;
        let builder = arena.init_root_struct(structure.data_word_count, structure.pointer_count)?;
        Ok(Self {
            schema,
            type_id,
            brand,
            builder,
        })
    }

    pub const fn type_id(&self) -> NodeId {
        self.type_id
    }

    generated_slot_setters!(
        (set_bool_slot, set_bool, bool),
        (set_i8_slot, set_i8, i8),
        (set_i16_slot, set_i16, i16),
        (set_i32_slot, set_i32, i32),
        (set_i64_slot, set_i64, i64),
        (set_u8_slot, set_u8, u8),
        (set_u16_slot, set_u16, u16),
        (set_u32_slot, set_u32, u32),
        (set_u64_slot, set_u64, u64),
        (set_f32_slot, set_f32, f32),
        (set_f64_slot, set_f64, f64),
    );

    /// Resolves a field's runtime type through this builder's current brand.
    pub fn field_type(&self, name: &str) -> Result<Type, DynamicError> {
        let (field, _) = self.field_owned(name)?;
        let FieldKind::Slot { ty, .. } = field.kind else {
            return Err(type_mismatch("slot field"));
        };
        Ok(resolve_type_with_brand(&ty, &self.brand))
    }

    pub fn set(&mut self, name: &str, value: DynamicInput<'_>) -> Result<(), DynamicError> {
        let (field, discriminant_offset) = self.field_owned(name)?;
        self.activate_field(&field, discriminant_offset)?;
        let FieldKind::Slot {
            offset,
            ty,
            default_value,
            ..
        } = &field.kind
        else {
            return Err(type_mismatch("slot field"));
        };
        let resolved_type = resolve_type_with_brand(ty, &self.brand);
        match (&resolved_type, value) {
            (Type::Void, DynamicInput::Void) => {}
            (Type::Bool, DynamicInput::Bool(value)) => {
                self.builder
                    .set_bool(*offset, value, bool_default(default_value)?)?
            }
            (Type::Int8, DynamicInput::Int8(value)) => {
                self.builder
                    .set_i8(*offset, value, i8_default(default_value)?)?;
            }
            (Type::Int16, DynamicInput::Int16(value)) => {
                self.builder
                    .set_i16(*offset, value, i16_default(default_value)?)?;
            }
            (Type::Int32, DynamicInput::Int32(value)) => {
                self.builder
                    .set_i32(*offset, value, i32_default(default_value)?)?;
            }
            (Type::Int64, DynamicInput::Int64(value)) => {
                self.builder
                    .set_i64(*offset, value, i64_default(default_value)?)?;
            }
            (Type::UInt8, DynamicInput::UInt8(value)) => {
                self.builder
                    .set_u8(*offset, value, u8_default(default_value)?)?;
            }
            (Type::UInt16, DynamicInput::UInt16(value)) => {
                self.builder
                    .set_u16(*offset, value, u16_default(default_value)?)?;
            }
            (Type::UInt32, DynamicInput::UInt32(value)) => {
                self.builder
                    .set_u32(*offset, value, u32_default(default_value)?)?;
            }
            (Type::UInt64, DynamicInput::UInt64(value)) => {
                self.builder
                    .set_u64(*offset, value, u64_default(default_value)?)?;
            }
            (Type::Float32, DynamicInput::Float32(value)) => {
                self.builder
                    .set_f32(*offset, value, f32_default(default_value)?)?;
            }
            (Type::Float64, DynamicInput::Float64(value)) => {
                self.builder
                    .set_f64(*offset, value, f64_default(default_value)?)?;
            }
            (Type::Enum { .. }, DynamicInput::Enum(value)) => {
                self.builder
                    .set_u16(*offset, value, enum_default(default_value)?)?;
            }
            (Type::Text, DynamicInput::Text(value)) => {
                self.builder.set_text(u16_offset(*offset)?, value)?;
            }
            (Type::Data, DynamicInput::Data(value)) => {
                self.builder.set_data(u16_offset(*offset)?, value)?;
            }
            (Type::Interface { .. }, DynamicInput::Capability(value))
            | (Type::AnyPointer(_), DynamicInput::Capability(value)) => {
                self.builder.set_capability(u16_offset(*offset)?, value)?;
            }
            (
                Type::Text
                | Type::Data
                | Type::List(_)
                | Type::Struct { .. }
                | Type::Interface { .. }
                | Type::AnyPointer(_),
                DynamicInput::Pointer(value),
            ) => value
                .open(ReaderLimits::default())?
                .copy_to_struct(&mut self.builder, u16_offset(*offset)?)?,
            _ => return Err(type_mismatch("input matching field type")),
        }
        Ok(())
    }

    /// Selects a union field while leaving its zero/default payload intact.
    ///
    /// This is useful for null pointer-valued schema unions, where selecting
    /// the discriminant is semantically meaningful even though no pointer is
    /// emitted.
    pub fn activate(&mut self, name: &str) -> Result<(), DynamicError> {
        let (field, discriminant_offset) = self.field_owned(name)?;
        self.activate_field(&field, discriminant_offset)
    }

    pub fn group(&mut self, name: &str) -> Result<DynamicStructBuilder<'schema, '_>, DynamicError> {
        let (field, discriminant_offset) = self.field_owned(name)?;
        self.activate_field(&field, discriminant_offset)?;
        let FieldKind::Group { type_id } = field.kind else {
            return Err(type_mismatch("group field"));
        };
        Ok(DynamicStructBuilder {
            schema: self.schema,
            type_id,
            brand: self.brand.clone(),
            builder: self.builder.group(),
        })
    }

    pub fn init_struct(
        &mut self,
        name: &str,
    ) -> Result<DynamicStructBuilder<'schema, '_>, DynamicError> {
        let (field, discriminant_offset) = self.field_owned(name)?;
        self.activate_field(&field, discriminant_offset)?;
        let FieldKind::Slot { offset, ty, .. } = field.kind else {
            return Err(type_mismatch("struct field"));
        };
        let Type::Struct { type_id, brand } = resolve_type_with_brand(&ty, &self.brand) else {
            return Err(type_mismatch("struct field"));
        };
        let structure = require_struct(self.schema, type_id)?;
        Ok(DynamicStructBuilder {
            schema: self.schema,
            type_id,
            brand,
            builder: self.builder.init_struct(
                u16_offset(offset)?,
                structure.data_word_count,
                structure.pointer_count,
            )?,
        })
    }

    pub fn init_list(
        &mut self,
        name: &str,
        element_count: u32,
    ) -> Result<DynamicListBuilder<'schema, '_>, DynamicError> {
        let (field, discriminant_offset) = self.field_owned(name)?;
        self.activate_field(&field, discriminant_offset)?;
        let FieldKind::Slot { offset, ty, .. } = field.kind else {
            return Err(type_mismatch("list field"));
        };
        let Type::List(element) = resolve_type_with_brand(&ty, &self.brand) else {
            return Err(type_mismatch("list field"));
        };
        DynamicListBuilder::from_struct_field(
            self.schema,
            &mut self.builder,
            u16_offset(offset)?,
            *element,
            element_count,
        )
    }

    fn field_owned(&self, name: &str) -> Result<(Field, u32), DynamicError> {
        let structure = require_struct(self.schema, self.type_id)?;
        let field = structure
            .field(name)
            .ok_or_else(|| DynamicError::UnknownField {
                type_id: self.type_id,
                name: name.to_owned(),
            })?
            .clone();
        Ok((field, structure.discriminant_offset))
    }

    fn activate_field(
        &mut self,
        field: &Field,
        discriminant_offset: u32,
    ) -> Result<(), DynamicError> {
        if let Some(value) = field.discriminant_value {
            self.builder.set_u16(discriminant_offset, value, 0)?;
        }
        Ok(())
    }
}

pub struct DynamicListBuilder<'schema, 'arena> {
    storage: DynamicListStorage<'schema, 'arena>,
}

enum DynamicListStorage<'schema, 'arena> {
    Void(DataListBuilder<'arena, ()>),
    Bool(DataListBuilder<'arena, bool>),
    Int8(DataListBuilder<'arena, i8>),
    Int16(DataListBuilder<'arena, i16>),
    Int32(DataListBuilder<'arena, i32>),
    Int64(DataListBuilder<'arena, i64>),
    UInt8(DataListBuilder<'arena, u8>),
    UInt16(DataListBuilder<'arena, u16>),
    UInt32(DataListBuilder<'arena, u32>),
    UInt64(DataListBuilder<'arena, u64>),
    Float32(DataListBuilder<'arena, f32>),
    Float64(DataListBuilder<'arena, f64>),
    Enum(DataListBuilder<'arena, u16>),
    Pointer {
        schema: &'schema CompiledSchema,
        element_type: Type,
        builder: PointerListBuilder<'arena>,
    },
    Struct {
        schema: &'schema CompiledSchema,
        type_id: NodeId,
        brand: Brand,
        builder: StructListBuilder<'arena>,
    },
}

impl<'schema, 'arena> DynamicListBuilder<'schema, 'arena> {
    /// Initializes a root list with its element type selected at runtime.
    pub fn root(
        schema: &'schema CompiledSchema,
        arena: &'arena mut ExclusiveArena,
        element_type: Type,
        count: u32,
    ) -> Result<Self, DynamicError> {
        let storage = match element_type {
            Type::Void => DynamicListStorage::Void(arena.init_root_list::<()>(count)?),
            Type::Bool => DynamicListStorage::Bool(arena.init_root_list::<bool>(count)?),
            Type::Int8 => DynamicListStorage::Int8(arena.init_root_list::<i8>(count)?),
            Type::Int16 => DynamicListStorage::Int16(arena.init_root_list::<i16>(count)?),
            Type::Int32 => DynamicListStorage::Int32(arena.init_root_list::<i32>(count)?),
            Type::Int64 => DynamicListStorage::Int64(arena.init_root_list::<i64>(count)?),
            Type::UInt8 => DynamicListStorage::UInt8(arena.init_root_list::<u8>(count)?),
            Type::UInt16 => DynamicListStorage::UInt16(arena.init_root_list::<u16>(count)?),
            Type::UInt32 => DynamicListStorage::UInt32(arena.init_root_list::<u32>(count)?),
            Type::UInt64 => DynamicListStorage::UInt64(arena.init_root_list::<u64>(count)?),
            Type::Float32 => DynamicListStorage::Float32(arena.init_root_list::<f32>(count)?),
            Type::Float64 => DynamicListStorage::Float64(arena.init_root_list::<f64>(count)?),
            Type::Enum { .. } => DynamicListStorage::Enum(arena.init_root_list::<u16>(count)?),
            Type::Struct { type_id, brand } => {
                let structure = require_struct(schema, type_id)?;
                DynamicListStorage::Struct {
                    schema,
                    type_id,
                    brand,
                    builder: arena.init_root_struct_list(
                        count,
                        structure.data_word_count,
                        structure.pointer_count,
                    )?,
                }
            }
            element_type => DynamicListStorage::Pointer {
                schema,
                element_type,
                builder: arena.init_root_pointer_list(count)?,
            },
        };
        Ok(Self { storage })
    }

    fn from_struct_field(
        schema: &'schema CompiledSchema,
        builder: &'arena mut StructBuilder<'_>,
        pointer_index: u16,
        element_type: Type,
        count: u32,
    ) -> Result<Self, DynamicError> {
        let storage = match element_type {
            Type::Void => DynamicListStorage::Void(builder.init_list::<()>(pointer_index, count)?),
            Type::Bool => {
                DynamicListStorage::Bool(builder.init_list::<bool>(pointer_index, count)?)
            }
            Type::Int8 => DynamicListStorage::Int8(builder.init_list::<i8>(pointer_index, count)?),
            Type::Int16 => {
                DynamicListStorage::Int16(builder.init_list::<i16>(pointer_index, count)?)
            }
            Type::Int32 => {
                DynamicListStorage::Int32(builder.init_list::<i32>(pointer_index, count)?)
            }
            Type::Int64 => {
                DynamicListStorage::Int64(builder.init_list::<i64>(pointer_index, count)?)
            }
            Type::UInt8 => {
                DynamicListStorage::UInt8(builder.init_list::<u8>(pointer_index, count)?)
            }
            Type::UInt16 => {
                DynamicListStorage::UInt16(builder.init_list::<u16>(pointer_index, count)?)
            }
            Type::UInt32 => {
                DynamicListStorage::UInt32(builder.init_list::<u32>(pointer_index, count)?)
            }
            Type::UInt64 => {
                DynamicListStorage::UInt64(builder.init_list::<u64>(pointer_index, count)?)
            }
            Type::Float32 => {
                DynamicListStorage::Float32(builder.init_list::<f32>(pointer_index, count)?)
            }
            Type::Float64 => {
                DynamicListStorage::Float64(builder.init_list::<f64>(pointer_index, count)?)
            }
            Type::Enum { .. } => {
                DynamicListStorage::Enum(builder.init_list::<u16>(pointer_index, count)?)
            }
            Type::Struct { type_id, brand } => {
                let structure = require_struct(schema, type_id)?;
                DynamicListStorage::Struct {
                    schema,
                    type_id,
                    brand,
                    builder: builder.init_struct_list(
                        pointer_index,
                        count,
                        structure.data_word_count,
                        structure.pointer_count,
                    )?,
                }
            }
            element_type => DynamicListStorage::Pointer {
                schema,
                element_type,
                builder: builder.init_pointer_list(pointer_index, count)?,
            },
        };
        Ok(Self { storage })
    }

    pub fn len(&self) -> u32 {
        match &self.storage {
            DynamicListStorage::Void(value) => value.len(),
            DynamicListStorage::Bool(value) => value.len(),
            DynamicListStorage::Int8(value) => value.len(),
            DynamicListStorage::Int16(value) => value.len(),
            DynamicListStorage::Int32(value) => value.len(),
            DynamicListStorage::Int64(value) => value.len(),
            DynamicListStorage::UInt8(value) => value.len(),
            DynamicListStorage::UInt16(value) => value.len(),
            DynamicListStorage::UInt32(value) => value.len(),
            DynamicListStorage::UInt64(value) => value.len(),
            DynamicListStorage::Float32(value) => value.len(),
            DynamicListStorage::Float64(value) => value.len(),
            DynamicListStorage::Enum(value) => value.len(),
            DynamicListStorage::Pointer { builder, .. } => builder.len(),
            DynamicListStorage::Struct { builder, .. } => builder.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn set(&mut self, index: u32, value: DynamicInput<'_>) -> Result<(), DynamicError> {
        match (&mut self.storage, value) {
            (DynamicListStorage::Void(values), DynamicInput::Void) => values.set(index, ())?,
            (DynamicListStorage::Bool(values), DynamicInput::Bool(value)) => {
                values.set(index, value)?;
            }
            (DynamicListStorage::Int8(values), DynamicInput::Int8(value)) => {
                values.set(index, value)?;
            }
            (DynamicListStorage::Int16(values), DynamicInput::Int16(value)) => {
                values.set(index, value)?;
            }
            (DynamicListStorage::Int32(values), DynamicInput::Int32(value)) => {
                values.set(index, value)?;
            }
            (DynamicListStorage::Int64(values), DynamicInput::Int64(value)) => {
                values.set(index, value)?;
            }
            (DynamicListStorage::UInt8(values), DynamicInput::UInt8(value)) => {
                values.set(index, value)?;
            }
            (DynamicListStorage::UInt16(values), DynamicInput::UInt16(value)) => {
                values.set(index, value)?;
            }
            (DynamicListStorage::UInt32(values), DynamicInput::UInt32(value)) => {
                values.set(index, value)?;
            }
            (DynamicListStorage::UInt64(values), DynamicInput::UInt64(value)) => {
                values.set(index, value)?;
            }
            (DynamicListStorage::Float32(values), DynamicInput::Float32(value)) => {
                values.set(index, value)?;
            }
            (DynamicListStorage::Float64(values), DynamicInput::Float64(value)) => {
                values.set(index, value)?;
            }
            (DynamicListStorage::Enum(values), DynamicInput::Enum(value)) => {
                values.set(index, value)?;
            }
            (
                DynamicListStorage::Pointer {
                    builder,
                    element_type: Type::Text,
                    ..
                },
                DynamicInput::Text(value),
            ) => builder.set_text(index, value)?,
            (
                DynamicListStorage::Pointer {
                    builder,
                    element_type: Type::Data,
                    ..
                },
                DynamicInput::Data(value),
            ) => builder.set_data(index, value)?,
            (
                DynamicListStorage::Pointer {
                    builder,
                    element_type: Type::Interface { .. } | Type::AnyPointer(_),
                    ..
                },
                DynamicInput::Capability(value),
            ) => builder.set_capability(index, value)?,
            _ => return Err(type_mismatch("input matching list element type")),
        }
        Ok(())
    }

    pub fn struct_element(
        &mut self,
        index: u32,
    ) -> Result<DynamicStructBuilder<'schema, '_>, DynamicError> {
        let DynamicListStorage::Struct {
            schema,
            type_id,
            brand,
            builder,
        } = &mut self.storage
        else {
            return Err(type_mismatch("struct list"));
        };
        Ok(DynamicStructBuilder {
            schema,
            type_id: *type_id,
            brand: brand.clone(),
            builder: builder.get(index)?,
        })
    }

    pub fn init_list(
        &mut self,
        index: u32,
        element_count: u32,
    ) -> Result<DynamicListBuilder<'schema, '_>, DynamicError> {
        let DynamicListStorage::Pointer {
            schema,
            element_type: Type::List(element),
            builder,
        } = &mut self.storage
        else {
            return Err(type_mismatch("nested list"));
        };
        Self::from_pointer_element(schema, builder, index, (**element).clone(), element_count)
    }

    fn from_pointer_element<'builder>(
        schema: &'schema CompiledSchema,
        builder: &'builder mut PointerListBuilder<'arena>,
        index: u32,
        element_type: Type,
        count: u32,
    ) -> Result<DynamicListBuilder<'schema, 'builder>, DynamicError>
    where
        'arena: 'builder,
    {
        let storage = match element_type {
            Type::Void => DynamicListStorage::Void(builder.init_list::<()>(index, count)?),
            Type::Bool => DynamicListStorage::Bool(builder.init_list::<bool>(index, count)?),
            Type::Int8 => DynamicListStorage::Int8(builder.init_list::<i8>(index, count)?),
            Type::Int16 => DynamicListStorage::Int16(builder.init_list::<i16>(index, count)?),
            Type::Int32 => DynamicListStorage::Int32(builder.init_list::<i32>(index, count)?),
            Type::Int64 => DynamicListStorage::Int64(builder.init_list::<i64>(index, count)?),
            Type::UInt8 => DynamicListStorage::UInt8(builder.init_list::<u8>(index, count)?),
            Type::UInt16 => DynamicListStorage::UInt16(builder.init_list::<u16>(index, count)?),
            Type::UInt32 => DynamicListStorage::UInt32(builder.init_list::<u32>(index, count)?),
            Type::UInt64 => DynamicListStorage::UInt64(builder.init_list::<u64>(index, count)?),
            Type::Float32 => DynamicListStorage::Float32(builder.init_list::<f32>(index, count)?),
            Type::Float64 => DynamicListStorage::Float64(builder.init_list::<f64>(index, count)?),
            Type::Enum { .. } => DynamicListStorage::Enum(builder.init_list::<u16>(index, count)?),
            Type::Struct { type_id, brand } => {
                let structure = require_struct(schema, type_id)?;
                DynamicListStorage::Struct {
                    schema,
                    type_id,
                    brand,
                    builder: builder.init_struct_list(
                        index,
                        count,
                        structure.data_word_count,
                        structure.pointer_count,
                    )?,
                }
            }
            element_type => DynamicListStorage::Pointer {
                schema,
                element_type,
                builder: builder.init_pointer_list(index, count)?,
            },
        };
        Ok(DynamicListBuilder { storage })
    }
}

impl DynamicValue {
    pub fn stringify(&self) -> Result<String, DynamicError> {
        let mut output = String::new();
        self.write_string(&mut output)?;
        Ok(output)
    }

    fn write_string(&self, output: &mut String) -> Result<(), DynamicError> {
        match self {
            Self::Void => output.push_str("void"),
            Self::Bool(value) => write!(output, "{value}").map_err(|_| DynamicError::Format)?,
            Self::Int8(value) => write!(output, "{value}").map_err(|_| DynamicError::Format)?,
            Self::Int16(value) => write!(output, "{value}").map_err(|_| DynamicError::Format)?,
            Self::Int32(value) => write!(output, "{value}").map_err(|_| DynamicError::Format)?,
            Self::Int64(value) => write!(output, "{value}").map_err(|_| DynamicError::Format)?,
            Self::UInt8(value) => write!(output, "{value}").map_err(|_| DynamicError::Format)?,
            Self::UInt16(value) => write!(output, "{value}").map_err(|_| DynamicError::Format)?,
            Self::UInt32(value) => write!(output, "{value}").map_err(|_| DynamicError::Format)?,
            Self::UInt64(value) => write!(output, "{value}").map_err(|_| DynamicError::Format)?,
            Self::Float32(value) => write!(output, "{value}").map_err(|_| DynamicError::Format)?,
            Self::Float64(value) => write!(output, "{value}").map_err(|_| DynamicError::Format)?,
            Self::Text(value) => write!(output, "{value:?}").map_err(|_| DynamicError::Format)?,
            Self::Data(value) => {
                output.push_str("0x\"");
                for byte in value {
                    write!(output, "{byte:02x}").map_err(|_| DynamicError::Format)?;
                }
                output.push('"');
            }
            Self::List(Some(value)) => output.push_str(&value.stringify()?),
            Self::List(None) | Self::Struct(None) | Self::Capability(None) => {
                output.push_str("null")
            }
            Self::Enum(value) => match value.name() {
                Some(name) => output.push_str(name),
                None => write!(output, "{}", value.ordinal).map_err(|_| DynamicError::Format)?,
            },
            Self::Struct(Some(value)) => value.write_string(output)?,
            Self::Capability(Some(value)) => {
                write!(output, "capability({value})").map_err(|_| DynamicError::Format)?
            }
            Self::AnyPointer(value) => match value {
                DynamicAnyPointer::Null => output.push_str("null"),
                DynamicAnyPointer::Struct(_) => output.push_str("anyPointer(struct)"),
                DynamicAnyPointer::List(_) => output.push_str("anyPointer(list)"),
                DynamicAnyPointer::Capability(index) => {
                    write!(output, "anyPointer(capability {index})")
                        .map_err(|_| DynamicError::Format)?
                }
            },
        }
        Ok(())
    }
}

fn u16_offset(value: u32) -> Result<u16, DynamicError> {
    u16::try_from(value).map_err(|_| type_mismatch("pointer offset fitting u16"))
}

fn enforce_any_kind(kind: &AnyPointerType, pointer: &OwnedPointerRef) -> Result<(), DynamicError> {
    let allowed = match kind {
        AnyPointerType::Unconstrained(AnyPointerKind::Any) => true,
        AnyPointerType::Unconstrained(AnyPointerKind::Struct) => {
            matches!(pointer, OwnedPointerRef::Null | OwnedPointerRef::Struct(_))
        }
        AnyPointerType::Unconstrained(AnyPointerKind::List) => {
            matches!(pointer, OwnedPointerRef::Null | OwnedPointerRef::List(_))
        }
        AnyPointerType::Unconstrained(AnyPointerKind::Capability) => matches!(
            pointer,
            OwnedPointerRef::Null | OwnedPointerRef::Capability(_)
        ),
        AnyPointerType::Parameter { .. } | AnyPointerType::ImplicitMethodParameter { .. } => true,
    };
    if allowed {
        Ok(())
    } else {
        Err(type_mismatch("constrained AnyPointer"))
    }
}

const fn type_mismatch(expected: &'static str) -> DynamicError {
    DynamicError::TypeMismatch { expected }
}

macro_rules! default_value {
    ($name:ident, $variant:ident, $ty:ty) => {
        fn $name(value: &Value) -> Result<$ty, DynamicError> {
            match value {
                Value::$variant(value) => Ok(*value),
                _ => Err(type_mismatch(stringify!($variant))),
            }
        }
    };
}

default_value!(bool_default, Bool, bool);
default_value!(i8_default, Int8, i8);
default_value!(i16_default, Int16, i16);
default_value!(i32_default, Int32, i32);
default_value!(i64_default, Int64, i64);
default_value!(u8_default, UInt8, u8);
default_value!(u16_default, UInt16, u16);
default_value!(u32_default, UInt32, u32);
default_value!(u64_default, UInt64, u64);
default_value!(f32_default, Float32, f32);
default_value!(f64_default, Float64, f64);
default_value!(enum_default, Enum, u16);

fn resolve_type_with_brand(ty: &Type, environment: &Brand) -> Type {
    match ty {
        Type::List(element) => Type::List(Box::new(resolve_type_with_brand(element, environment))),
        Type::Enum { type_id, brand } => Type::Enum {
            type_id: *type_id,
            brand: resolve_brand_with_brand(brand, environment),
        },
        Type::Struct { type_id, brand } => Type::Struct {
            type_id: *type_id,
            brand: resolve_brand_with_brand(brand, environment),
        },
        Type::Interface { type_id, brand } => Type::Interface {
            type_id: *type_id,
            brand: resolve_brand_with_brand(brand, environment),
        },
        Type::AnyPointer(AnyPointerType::Parameter { scope_id, index }) => environment
            .scopes
            .iter()
            .find(|scope| scope.scope_id == *scope_id)
            .and_then(|scope| match &scope.binding {
                ScopeBinding::Bind(bindings) => bindings.get(usize::from(*index)),
                ScopeBinding::Inherit => None,
            })
            .and_then(|binding| match binding {
                BrandBinding::Type(value) => Some(resolve_type_with_brand(value, environment)),
                BrandBinding::Unbound => None,
            })
            .unwrap_or_else(|| ty.clone()),
        _ => ty.clone(),
    }
}

fn resolve_brand_with_brand(brand: &Brand, environment: &Brand) -> Brand {
    Brand {
        scopes: brand
            .scopes
            .iter()
            .map(|scope| crate::BrandScope {
                scope_id: scope.scope_id,
                binding: match &scope.binding {
                    ScopeBinding::Inherit => ScopeBinding::Inherit,
                    ScopeBinding::Bind(bindings) => ScopeBinding::Bind(
                        bindings
                            .iter()
                            .map(|binding| match binding {
                                BrandBinding::Unbound => BrandBinding::Unbound,
                                BrandBinding::Type(value) => {
                                    BrandBinding::Type(resolve_type_with_brand(value, environment))
                                }
                            })
                            .collect(),
                    ),
                },
            })
            .collect(),
    }
}
