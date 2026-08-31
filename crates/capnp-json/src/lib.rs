//! Bounded, reflection-driven Cap'n Proto JSON conversion.
//!
//! The compatibility oracle is the C++ `capnp/compat/json.{h,c++}` at commit
//! `e7c9cd96f1505b5ae486db7821006c2f5dce5b5b`. In particular, signed and
//! unsigned 64-bit integers use JSON strings, non-finite floats use the strings
//! `Infinity`, `-Infinity`, and `NaN`, Data uses byte arrays unless annotated,
//! primitive fields are present by default, and null pointer fields are absent.
//! Capabilities and AnyPointers have no implicit JSON representation.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Write as _};
use std::sync::Arc;

use capnp_message::{OwnedMessage, ReaderLimits};
use capnp_schema::{
    Annotation, CompiledSchema, DynamicAnyPointer, DynamicList, DynamicStruct, DynamicValue, Field,
    FieldKind, NodeId, NodeKind, OpaquePointerKind, Type, Value,
};
use capnp_text::{EncodedMessage, TextLimits, encode_structs};

/// Upstream annotation IDs from `capnp/compat/json.capnp`.
pub const NAME_ANNOTATION_ID: NodeId = 0xfa5b_1fd6_1c2e_7c3d;
pub const FLATTEN_ANNOTATION_ID: NodeId = 0x82d3_e852_af03_36bf;
pub const DISCRIMINATOR_ANNOTATION_ID: NodeId = 0xcfa7_94e8_d19a_0162;
pub const BASE64_ANNOTATION_ID: NodeId = 0xd7d8_7945_0a25_3e4b;
pub const HEX_ANNOTATION_ID: NodeId = 0xf061_e22f_0ae5_c7b5;

/// Independent bounds for untrusted JSON and produced messages.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JsonLimits {
    pub max_input_bytes: usize,
    pub max_values: usize,
    pub max_nesting: u16,
    pub max_message_words: u32,
}

impl Default for JsonLimits {
    fn default() -> Self {
        Self {
            max_input_bytes: 16 * 1024 * 1024,
            max_values: 1_000_000,
            max_nesting: 64,
            max_message_words: 8 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FormatStyle {
    Compact,
    Pretty,
}

/// Determines whether scalar fields equal to their schema default are emitted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HasMode {
    NonNull,
    NonDefault,
}

/// A lossless-enough JSON syntax tree. Numbers retain their source spelling.
#[derive(Clone, Debug, PartialEq)]
pub enum JsonValue {
    Null,
    Bool(bool),
    Number(String),
    String(String),
    Array(Vec<JsonValue>),
    Object(Vec<(String, JsonValue)>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JsonError {
    pub byte: usize,
    pub line: usize,
    pub column: usize,
    pub message: String,
}

impl fmt::Display for JsonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}:{} (byte {}): {}",
            self.line, self.column, self.byte, self.message
        )
    }
}

impl std::error::Error for JsonError {}

/// Selects all occurrences of a reflected type for an extension handler.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum TypeSelector {
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
    List,
    Enum(NodeId),
    Struct(NodeId),
    Interface(NodeId),
    AnyPointer,
}

/// A deterministic, thread-safe override for a type or an individual field.
///
/// Decode returns the canonical JSON representation expected by the built-in
/// decoder. This keeps extensions independent from arena lifetimes.
pub trait JsonHandler: Send + Sync {
    fn encode(&self, input: &DynamicValue) -> Result<JsonValue, JsonError>;
    fn decode(&self, input: &JsonValue) -> Result<JsonValue, JsonError>;
}

/// Configurable reflection codec. Clones share immutable handler objects.
#[derive(Clone)]
pub struct JsonCodec {
    limits: JsonLimits,
    style: FormatStyle,
    has_mode: HasMode,
    reject_unknown_fields: bool,
    annotations: bool,
    type_handlers: BTreeMap<TypeSelector, Arc<dyn JsonHandler>>,
    field_handlers: BTreeMap<(NodeId, String), Arc<dyn JsonHandler>>,
}

impl fmt::Debug for JsonCodec {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("JsonCodec")
            .field("limits", &self.limits)
            .field("style", &self.style)
            .field("has_mode", &self.has_mode)
            .field("reject_unknown_fields", &self.reject_unknown_fields)
            .field("annotations", &self.annotations)
            .field("type_handler_count", &self.type_handlers.len())
            .field("field_handler_count", &self.field_handlers.len())
            .finish()
    }
}

impl Default for JsonCodec {
    fn default() -> Self {
        Self {
            limits: JsonLimits::default(),
            style: FormatStyle::Compact,
            has_mode: HasMode::NonNull,
            reject_unknown_fields: false,
            annotations: false,
            type_handlers: BTreeMap::new(),
            field_handlers: BTreeMap::new(),
        }
    }
}

impl JsonCodec {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_limits(&mut self, limits: JsonLimits) -> &mut Self {
        self.limits = limits;
        self
    }

    pub fn set_style(&mut self, style: FormatStyle) -> &mut Self {
        self.style = style;
        self
    }

    pub fn set_has_mode(&mut self, mode: HasMode) -> &mut Self {
        self.has_mode = mode;
        self
    }

    pub fn set_reject_unknown_fields(&mut self, reject: bool) -> &mut Self {
        self.reject_unknown_fields = reject;
        self
    }

    /// Enables the upstream name, flatten, discriminator, base64, and hex annotations.
    pub fn handle_by_annotation(&mut self, enabled: bool) -> &mut Self {
        self.annotations = enabled;
        self
    }

    pub fn add_type_handler(
        &mut self,
        selector: TypeSelector,
        handler: Arc<dyn JsonHandler>,
    ) -> Option<Arc<dyn JsonHandler>> {
        self.type_handlers.insert(selector, handler)
    }

    pub fn add_field_handler(
        &mut self,
        struct_id: NodeId,
        field: impl Into<String>,
        handler: Arc<dyn JsonHandler>,
    ) -> Option<Arc<dyn JsonHandler>> {
        self.field_handlers
            .insert((struct_id, field.into()), handler)
    }

    pub fn parse(&self, source: &str) -> Result<JsonValue, JsonError> {
        Parser::new(source, self.limits)?.parse()
    }

    pub fn format(&self, value: &JsonValue) -> Result<String, JsonError> {
        let mut output = String::new();
        write_json(value, self.style, 0, &mut output)?;
        Ok(output)
    }

    pub fn encode_struct(&self, input: &DynamicStruct) -> Result<String, JsonError> {
        self.format(&self.encode_struct_value(input, 0)?)
    }

    pub fn encode_message(
        &self,
        schema: Arc<CompiledSchema>,
        type_id: NodeId,
        segments: impl IntoIterator<Item = Arc<[u8]>>,
        reader_limits: ReaderLimits,
    ) -> Result<String, JsonError> {
        let message = OwnedMessage::new(segments, reader_limits)
            .map_err(|error| plain(format!("message validation failed: {error}")))?;
        let root = DynamicStruct::root(schema, message, type_id)
            .map_err(|error| plain(format!("root type mismatch: {error}")))?;
        self.encode_struct(&root)
    }

    pub fn decode_structs(
        &self,
        schema: &CompiledSchema,
        type_id: NodeId,
        source: &str,
    ) -> Result<Vec<EncodedMessage>, JsonError> {
        let mut value = self.parse(source)?;
        if let Some(handler) = self.type_handlers.get(&TypeSelector::Struct(type_id)) {
            value = handler.decode(&value)?;
        }
        let text = self.json_struct_to_text(schema, type_id, &value, 0)?;
        encode_structs(
            schema,
            type_id,
            &text,
            TextLimits {
                max_input_bytes: self.limits.max_input_bytes.saturating_mul(4),
                max_values: self.limits.max_values,
                max_nesting: self.limits.max_nesting,
                max_message_words: self.limits.max_message_words,
            },
        )
        .map_err(|error| plain(format!("decoded Cap'n Proto value: {error}")))
    }

    fn encode_struct_value(
        &self,
        input: &DynamicStruct,
        depth: u16,
    ) -> Result<JsonValue, JsonError> {
        self.check_depth(depth)?;
        if let Some(handler) = self
            .type_handlers
            .get(&TypeSelector::Struct(input.type_id()))
        {
            return handler.encode(&DynamicValue::Struct(Some(input.clone())));
        }
        let node = input
            .schema()
            .node(input.type_id())
            .ok_or_else(|| plain("dynamic struct has no schema"))?;
        let NodeKind::Struct(structure) = &node.kind else {
            return Err(plain("dynamic struct type is not a struct schema"));
        };
        let discriminator = self.discriminator_options(input.schema(), &node.annotations)?;
        let active = input
            .active_union_field()
            .map_err(|error| plain(error.to_string()))?;
        let mut fields = structure.fields.iter().collect::<Vec<_>>();
        fields.sort_by_key(|field| field.code_order);
        let mut object = Vec::new();
        for field in fields {
            if field.discriminant_value.is_some()
                && active.is_none_or(|active| active.name != field.name)
            {
                continue;
            }
            if !input
                .is_field_present(&field.name)
                .map_err(|error| plain(error.to_string()))?
            {
                continue;
            }
            let value = input
                .get(&field.name)
                .map_err(|error| plain(error.to_string()))?;
            if self.has_mode == HasMode::NonDefault && is_default_value(field, &value) {
                continue;
            }
            let json_name = self.field_name(field);
            if field.discriminant_value.is_some() {
                if let Some(options) = &discriminator {
                    object.push((options.name.clone(), JsonValue::String(json_name.clone())));
                    if matches!(value, DynamicValue::Void) {
                        continue;
                    }
                    if !options.value_name.is_empty() {
                        object.push((
                            options.value_name.clone(),
                            self.encode_field(input.type_id(), field, &value, depth + 1)?,
                        ));
                        continue;
                    }
                }
            }
            if let Some(prefix) = self.flatten_prefix(input.schema(), &field.annotations)? {
                let DynamicValue::Struct(Some(child)) = value else {
                    return Err(plain(format!(
                        "flatten annotation on non-struct field `{}`",
                        field.name
                    )));
                };
                let JsonValue::Object(children) = self.encode_struct_value(&child, depth + 1)?
                else {
                    return Err(plain(
                        "struct handler returned non-object for flattened field",
                    ));
                };
                for (name, value) in children {
                    object.push((format!("{prefix}{name}"), value));
                }
            } else {
                object.push((
                    json_name,
                    self.encode_field(input.type_id(), field, &value, depth + 1)?,
                ));
            }
        }
        Ok(JsonValue::Object(object))
    }

    fn encode_field(
        &self,
        struct_id: NodeId,
        field: &Field,
        value: &DynamicValue,
        depth: u16,
    ) -> Result<JsonValue, JsonError> {
        if let Some(handler) = self.field_handlers.get(&(struct_id, field.name.clone())) {
            return handler.encode(value);
        }
        if self.annotations && has_annotation(&field.annotations, BASE64_ANNOTATION_ID) {
            let DynamicValue::Data(bytes) = value else {
                return Err(plain("base64 annotation requires Data"));
            };
            return Ok(JsonValue::String(base64_encode(bytes)));
        }
        if self.annotations && has_annotation(&field.annotations, HEX_ANNOTATION_ID) {
            let DynamicValue::Data(bytes) = value else {
                return Err(plain("hex annotation requires Data"));
            };
            let mut text = String::with_capacity(bytes.len() * 2);
            for byte in bytes {
                write!(&mut text, "{byte:02x}").map_err(|_| plain("format failure"))?;
            }
            return Ok(JsonValue::String(text));
        }
        let ty = field_type(field);
        self.encode_dynamic(value, ty, depth)
    }

    fn encode_dynamic(
        &self,
        value: &DynamicValue,
        ty: Option<&Type>,
        depth: u16,
    ) -> Result<JsonValue, JsonError> {
        self.check_depth(depth)?;
        if let Some(ty) = ty {
            if let Some(handler) = self.type_handlers.get(&selector(ty)) {
                return handler.encode(value);
            }
        }
        Ok(match value {
            DynamicValue::Void => JsonValue::Null,
            DynamicValue::Bool(value) => JsonValue::Bool(*value),
            DynamicValue::Int8(value) => number(*value),
            DynamicValue::Int16(value) => number(*value),
            DynamicValue::Int32(value) => number(*value),
            DynamicValue::Int64(value) => JsonValue::String(value.to_string()),
            DynamicValue::UInt8(value) => number(*value),
            DynamicValue::UInt16(value) => number(*value),
            DynamicValue::UInt32(value) => number(*value),
            DynamicValue::UInt64(value) => JsonValue::String(value.to_string()),
            DynamicValue::Float32(value) => json_float(f64::from(*value)),
            DynamicValue::Float64(value) => json_float(*value),
            DynamicValue::Text(value) => JsonValue::String(value.clone()),
            DynamicValue::Data(value) => {
                JsonValue::Array(value.iter().map(|byte| number(*byte)).collect())
            }
            DynamicValue::List(Some(value)) => self.encode_list(value, depth + 1)?,
            DynamicValue::List(None) => JsonValue::Array(Vec::new()),
            DynamicValue::Enum(value) => match value.name() {
                Some(name) => {
                    let name = if self.annotations {
                        value
                            .enumerant()
                            .map(|enumerant| annotation_name(&enumerant.annotations, name))
                            .unwrap_or_else(|| name.to_owned())
                    } else {
                        name.to_owned()
                    };
                    JsonValue::String(name)
                }
                None => number(value.ordinal),
            },
            DynamicValue::Struct(Some(value)) => self.encode_struct_value(value, depth + 1)?,
            DynamicValue::Struct(None) => JsonValue::Object(Vec::new()),
            DynamicValue::Capability(_) => {
                return Err(plain("capability JSON conversion requires a handler"));
            }
            DynamicValue::AnyPointer(DynamicAnyPointer::Null) => {
                return Err(plain("AnyPointer JSON conversion requires a handler"));
            }
            DynamicValue::AnyPointer(_) => {
                return Err(plain("AnyPointer JSON conversion requires a handler"));
            }
        })
    }

    fn encode_list(&self, input: &DynamicList, depth: u16) -> Result<JsonValue, JsonError> {
        self.check_depth(depth)?;
        let mut array = Vec::with_capacity(
            usize::try_from(input.len().map_err(|error| plain(error.to_string()))?)
                .map_err(|_| plain("list length does not fit usize"))?,
        );
        for index in 0..input.len().map_err(|error| plain(error.to_string()))? {
            let value = input.get(index).map_err(|error| plain(error.to_string()))?;
            array.push(self.encode_dynamic(&value, Some(input.element_type()), depth + 1)?);
        }
        Ok(JsonValue::Array(array))
    }

    fn json_struct_to_text(
        &self,
        schema: &CompiledSchema,
        type_id: NodeId,
        input: &JsonValue,
        depth: u16,
    ) -> Result<String, JsonError> {
        self.check_depth(depth)?;
        let JsonValue::Object(entries) = input else {
            return Err(plain("expected JSON object for Cap'n Proto struct"));
        };
        let node = schema
            .node(type_id)
            .ok_or_else(|| plain(format!("unknown struct schema 0x{type_id:016x}")))?;
        let NodeKind::Struct(structure) = &node.kind else {
            return Err(plain(format!("0x{type_id:016x} is not a struct schema")));
        };
        let mut names = BTreeMap::new();
        for (index, (name, _)) in entries.iter().enumerate() {
            if names.insert(name.as_str(), index).is_some() {
                return Err(plain(format!("duplicate JSON field `{name}`")));
            }
        }
        let discriminator = self.discriminator_options(schema, &node.annotations)?;
        let selected_union = if let Some(options) = &discriminator {
            match names
                .get(options.name.as_str())
                .map(|index| &entries[*index].1)
            {
                Some(JsonValue::String(name)) => structure.fields.iter().find(|field| {
                    field.discriminant_value.is_some() && self.field_name(field) == *name
                }),
                Some(_) => return Err(plain("union discriminator must be a JSON string")),
                None => None,
            }
        } else {
            None
        };
        let mut consumed = BTreeSet::new();
        if let Some(options) = &discriminator {
            if names.contains_key(options.name.as_str()) {
                consumed.insert(options.name.as_str());
            }
        }
        let mut fields = Vec::new();
        for field in &structure.fields {
            if field.discriminant_value.is_some()
                && selected_union.is_some_and(|selected| selected.name != field.name)
            {
                continue;
            }
            if let Some(prefix) = self.flatten_prefix(schema, &field.annotations)? {
                let child_id = struct_field_id(field).ok_or_else(|| {
                    plain(format!(
                        "flatten annotation on non-struct field `{}`",
                        field.name
                    ))
                })?;
                let child = schema
                    .node(child_id)
                    .and_then(|node| match &node.kind {
                        NodeKind::Struct(value) => Some(value),
                        _ => None,
                    })
                    .ok_or_else(|| plain("flattened child schema is unavailable"))?;
                let mut child_entries = Vec::new();
                for child_field in &child.fields {
                    let child_name = format!("{prefix}{}", self.field_name(child_field));
                    if let Some(index) = names.get(child_name.as_str()) {
                        child_entries
                            .push((self.field_name(child_field), entries[*index].1.clone()));
                        consumed.insert(entries[*index].0.as_str());
                    }
                }
                if !child_entries.is_empty() || selected_union.is_some_and(|f| f.name == field.name)
                {
                    let text = self.json_struct_to_text(
                        schema,
                        child_id,
                        &JsonValue::Object(child_entries),
                        depth + 1,
                    )?;
                    fields.push((field.name.clone(), text));
                }
                continue;
            }
            let json_name = if selected_union.is_some_and(|selected| selected.name == field.name) {
                discriminator
                    .as_ref()
                    .filter(|options| !options.value_name.is_empty())
                    .map_or_else(
                        || self.field_name(field),
                        |options| options.value_name.clone(),
                    )
            } else {
                self.field_name(field)
            };
            let Some(index) = names.get(json_name.as_str()) else {
                if selected_union.is_some_and(|selected| selected.name == field.name)
                    && matches!(field_type(field), Some(Type::Void))
                {
                    fields.push((field.name.clone(), "void".to_owned()));
                }
                continue;
            };
            consumed.insert(entries[*index].0.as_str());
            let mut value = entries[*index].1.clone();
            let field_handler = self.field_handlers.get(&(type_id, field.name.clone()));
            if let Some(handler) = field_handler {
                value = handler.decode(&value)?;
            }
            fields.push((
                field.name.clone(),
                self.json_value_to_text(schema, field, &value, depth + 1, field_handler.is_none())?,
            ));
        }
        if self.reject_unknown_fields {
            if let Some((name, _)) = entries
                .iter()
                .find(|(name, _)| !consumed.contains(name.as_str()))
            {
                return Err(plain(format!("unknown JSON field `{name}`")));
            }
        }
        let mut output = String::from("(");
        for (index, (name, value)) in fields.iter().enumerate() {
            if index != 0 {
                output.push_str(", ");
            }
            output.push_str(name);
            output.push_str(" = ");
            output.push_str(value);
        }
        output.push(')');
        Ok(output)
    }

    fn json_value_to_text(
        &self,
        schema: &CompiledSchema,
        field: &Field,
        input: &JsonValue,
        depth: u16,
        apply_type_handler: bool,
    ) -> Result<String, JsonError> {
        let Some(ty) = field_type(field) else {
            let FieldKind::Group { type_id } = field.kind else {
                return Err(plain("field has no type"));
            };
            return self.json_struct_to_text(schema, type_id, input, depth);
        };
        if self.annotations && has_annotation(&field.annotations, BASE64_ANNOTATION_ID) {
            let JsonValue::String(value) = input else {
                return Err(plain("base64 Data field requires a JSON string"));
            };
            return Ok(hex_text(&base64_decode(value)?));
        }
        if self.annotations && has_annotation(&field.annotations, HEX_ANNOTATION_ID) {
            let JsonValue::String(value) = input else {
                return Err(plain("hex Data field requires a JSON string"));
            };
            return Ok(hex_text(&hex_decode(value)?));
        }
        if apply_type_handler {
            self.json_typed_to_text(schema, ty, input, depth)
        } else {
            self.json_typed_to_text_builtin(schema, ty, input, depth)
        }
    }

    fn json_typed_to_text(
        &self,
        schema: &CompiledSchema,
        ty: &Type,
        input: &JsonValue,
        depth: u16,
    ) -> Result<String, JsonError> {
        if let Some(handler) = self.type_handlers.get(&selector(ty)) {
            let transformed = handler.decode(input)?;
            self.json_typed_to_text_builtin(schema, ty, &transformed, depth)
        } else {
            self.json_typed_to_text_builtin(schema, ty, input, depth)
        }
    }

    fn json_typed_to_text_builtin(
        &self,
        schema: &CompiledSchema,
        ty: &Type,
        input: &JsonValue,
        depth: u16,
    ) -> Result<String, JsonError> {
        self.check_depth(depth)?;
        match ty {
            Type::Void => match input {
                JsonValue::Null => Ok("void".to_owned()),
                _ => Err(plain("Void requires JSON null")),
            },
            Type::Bool => match input {
                JsonValue::Bool(value) => Ok(value.to_string()),
                _ => Err(plain("Bool requires a JSON boolean")),
            },
            Type::Int8 | Type::Int16 | Type::Int32 | Type::UInt8 | Type::UInt16 | Type::UInt32 => {
                match input {
                    JsonValue::Number(value) => integer_number_text(value),
                    JsonValue::String(value) => Ok(value.clone()),
                    _ => Err(plain("integer requires a JSON number or decimal string")),
                }
            }
            Type::Int64 | Type::UInt64 => match input {
                JsonValue::Number(value) => integer_number_text(value),
                JsonValue::String(value) => Ok(value.clone()),
                _ => Err(plain(
                    "64-bit integer requires a JSON number or decimal string",
                )),
            },
            Type::Float32 | Type::Float64 => match input {
                JsonValue::Null => Ok("nan".to_owned()),
                JsonValue::Number(value) => Ok(value.clone()),
                JsonValue::String(value) => match value.as_str() {
                    "Infinity" => Ok("inf".to_owned()),
                    "-Infinity" => Ok("-inf".to_owned()),
                    "NaN" => Ok("nan".to_owned()),
                    _ => Ok(value.clone()),
                },
                _ => Err(plain("float requires a JSON number, string, or null")),
            },
            Type::Text => match input {
                JsonValue::Null => Ok("null".to_owned()),
                JsonValue::String(value) => quoted_text(value.as_bytes()),
                _ => Err(plain("Text requires a JSON string")),
            },
            Type::Data => match input {
                JsonValue::Null => Ok("null".to_owned()),
                JsonValue::Array(values) => {
                    let mut bytes = Vec::with_capacity(values.len());
                    for value in values {
                        let JsonValue::Number(number) = value else {
                            return Err(plain("Data array items must be JSON numbers"));
                        };
                        let number = number
                            .parse::<f64>()
                            .map_err(|_| plain("Data array item must be an integer in [0, 255]"))?;
                        if !number.is_finite()
                            || number.fract() != 0.0
                            || !(0.0..=255.0).contains(&number)
                        {
                            return Err(plain("Data array item must be an integer in [0, 255]"));
                        }
                        bytes.push(number as u8);
                    }
                    Ok(hex_text(&bytes))
                }
                _ => Err(plain("Data requires a JSON byte array")),
            },
            Type::Enum { type_id, .. } => match input {
                JsonValue::String(name) => {
                    let enumeration = schema
                        .node(*type_id)
                        .and_then(|node| match &node.kind {
                            NodeKind::Enum(value) => Some(value),
                            _ => None,
                        })
                        .ok_or_else(|| plain("enum schema is unavailable"))?;
                    let enumerant = enumeration.enumerants.iter().find(|enumerant| {
                        if self.annotations {
                            annotation_name(&enumerant.annotations, &enumerant.name) == *name
                        } else {
                            enumerant.name == *name
                        }
                    });
                    enumerant
                        .map(|enumerant| enumerant.name.clone())
                        .ok_or_else(|| plain(format!("unknown enum name `{name}`")))
                }
                JsonValue::Number(value) if self.annotations => Ok(value.clone()),
                _ => Err(plain("enum requires a known JSON string")),
            },
            Type::Struct { type_id, .. } => match input {
                JsonValue::Null => Ok("null".to_owned()),
                _ => self.json_struct_to_text(schema, *type_id, input, depth + 1),
            },
            Type::List(element) => match input {
                JsonValue::Null => Ok("null".to_owned()),
                JsonValue::Array(values) => {
                    let mut output = String::from("[");
                    for (index, value) in values.iter().enumerate() {
                        if index != 0 {
                            output.push_str(", ");
                        }
                        output.push_str(&self.json_typed_to_text(
                            schema,
                            element,
                            value,
                            depth + 1,
                        )?);
                    }
                    output.push(']');
                    Ok(output)
                }
                _ => Err(plain("List requires a JSON array")),
            },
            Type::Interface { .. } => Err(plain("capability JSON conversion requires a handler")),
            Type::AnyPointer(_) => Err(plain("AnyPointer JSON conversion requires a handler")),
        }
    }

    fn field_name(&self, field: &Field) -> String {
        if self.annotations {
            annotation_name(&field.annotations, &field.name)
        } else {
            field.name.clone()
        }
    }

    fn flatten_prefix(
        &self,
        schema: &CompiledSchema,
        annotations: &[Annotation],
    ) -> Result<Option<String>, JsonError> {
        if !self.annotations {
            return Ok(None);
        }
        let Some(annotation) = annotations
            .iter()
            .find(|value| value.id == FLATTEN_ANNOTATION_ID)
        else {
            return Ok(None);
        };
        Ok(Some(
            annotation_struct_text(schema, annotation, "prefix")?.unwrap_or_default(),
        ))
    }

    fn discriminator_options(
        &self,
        schema: &CompiledSchema,
        annotations: &[Annotation],
    ) -> Result<Option<DiscriminatorOptions>, JsonError> {
        if !self.annotations {
            return Ok(None);
        }
        let Some(annotation) = annotations
            .iter()
            .find(|value| value.id == DISCRIMINATOR_ANNOTATION_ID)
        else {
            return Ok(None);
        };
        Ok(Some(DiscriminatorOptions {
            name: annotation_struct_text(schema, annotation, "name")?.unwrap_or_default(),
            value_name: annotation_struct_text(schema, annotation, "valueName")?
                .unwrap_or_default(),
        }))
    }

    fn check_depth(&self, depth: u16) -> Result<(), JsonError> {
        if depth > self.limits.max_nesting {
            Err(plain(format!(
                "JSON nesting exceeds {}",
                self.limits.max_nesting
            )))
        } else {
            Ok(())
        }
    }
}

#[derive(Debug)]
struct DiscriminatorOptions {
    name: String,
    value_name: String,
}

fn annotation_struct_text(
    schema: &CompiledSchema,
    annotation: &Annotation,
    field: &str,
) -> Result<Option<String>, JsonError> {
    let type_id = schema
        .node(annotation.id)
        .and_then(|node| match &node.kind {
            NodeKind::Annotation(value) => match &value.ty {
                Type::Struct { type_id, .. } => Some(*type_id),
                _ => None,
            },
            _ => None,
        })
        .ok_or_else(|| plain("JSON annotation declaration is unavailable"))?;
    let Some(value) = DynamicStruct::from_branded_value(
        Arc::new(schema.clone()),
        type_id,
        annotation.brand.clone(),
        &annotation.value,
        ReaderLimits::default(),
    )
    .map_err(|error| plain(format!("JSON annotation value: {error}")))?
    else {
        return Ok(None);
    };
    match value.get(field).map_err(|error| plain(error.to_string()))? {
        DynamicValue::Text(value) if value.is_empty() => Ok(None),
        DynamicValue::Text(value) => Ok(Some(value)),
        _ => Err(plain(format!("JSON annotation `{field}` is not Text"))),
    }
}

fn has_annotation(values: &[Annotation], id: NodeId) -> bool {
    values.iter().any(|value| value.id == id)
}

fn annotation_name(values: &[Annotation], fallback: &str) -> String {
    values
        .iter()
        .find_map(|annotation| match (&annotation.id, &annotation.value) {
            (&NAME_ANNOTATION_ID, Value::Text(value)) => Some(value.clone()),
            _ => None,
        })
        .unwrap_or_else(|| fallback.to_owned())
}

fn field_type(field: &Field) -> Option<&Type> {
    match &field.kind {
        FieldKind::Slot { ty, .. } => Some(ty),
        FieldKind::Group { .. } => None,
    }
}

fn struct_field_id(field: &Field) -> Option<NodeId> {
    match &field.kind {
        FieldKind::Group { type_id } => Some(*type_id),
        FieldKind::Slot {
            ty: Type::Struct { type_id, .. },
            ..
        } => Some(*type_id),
        _ => None,
    }
}

fn selector(ty: &Type) -> TypeSelector {
    match ty {
        Type::Void => TypeSelector::Void,
        Type::Bool => TypeSelector::Bool,
        Type::Int8 => TypeSelector::Int8,
        Type::Int16 => TypeSelector::Int16,
        Type::Int32 => TypeSelector::Int32,
        Type::Int64 => TypeSelector::Int64,
        Type::UInt8 => TypeSelector::UInt8,
        Type::UInt16 => TypeSelector::UInt16,
        Type::UInt32 => TypeSelector::UInt32,
        Type::UInt64 => TypeSelector::UInt64,
        Type::Float32 => TypeSelector::Float32,
        Type::Float64 => TypeSelector::Float64,
        Type::Text => TypeSelector::Text,
        Type::Data => TypeSelector::Data,
        Type::List(_) => TypeSelector::List,
        Type::Enum { type_id, .. } => TypeSelector::Enum(*type_id),
        Type::Struct { type_id, .. } => TypeSelector::Struct(*type_id),
        Type::Interface { type_id, .. } => TypeSelector::Interface(*type_id),
        Type::AnyPointer(_) => TypeSelector::AnyPointer,
    }
}

fn is_default_value(field: &Field, value: &DynamicValue) -> bool {
    let FieldKind::Slot { default_value, .. } = &field.kind else {
        return false;
    };
    match (default_value, value) {
        (Value::Void, DynamicValue::Void) => true,
        (Value::Bool(a), DynamicValue::Bool(b)) => a == b,
        (Value::Int8(a), DynamicValue::Int8(b)) => a == b,
        (Value::Int16(a), DynamicValue::Int16(b)) => a == b,
        (Value::Int32(a), DynamicValue::Int32(b)) => a == b,
        (Value::Int64(a), DynamicValue::Int64(b)) => a == b,
        (Value::UInt8(a), DynamicValue::UInt8(b)) => a == b,
        (Value::UInt16(a), DynamicValue::UInt16(b)) => a == b,
        (Value::UInt32(a), DynamicValue::UInt32(b)) => a == b,
        (Value::UInt64(a), DynamicValue::UInt64(b)) => a == b,
        (Value::Float32(a), DynamicValue::Float32(b)) => a.to_bits() == b.to_bits(),
        (Value::Float64(a), DynamicValue::Float64(b)) => a.to_bits() == b.to_bits(),
        (Value::Enum(a), DynamicValue::Enum(b)) => *a == b.ordinal,
        (Value::Text(a), DynamicValue::Text(b)) => a == b,
        (Value::Data(a), DynamicValue::Data(b)) => a == b,
        (Value::List(a), DynamicValue::List(None))
        | (Value::Struct(a), DynamicValue::Struct(None)) => a.kind == OpaquePointerKind::Null,
        _ => false,
    }
}

fn number(value: impl fmt::Display) -> JsonValue {
    JsonValue::Number(value.to_string())
}

fn integer_number_text(value: &str) -> Result<String, JsonError> {
    if value.bytes().any(|byte| matches!(byte, b'.' | b'e' | b'E')) {
        let number = value
            .parse::<f64>()
            .map_err(|_| plain("invalid JSON integer"))?;
        if !number.is_finite() || number.fract() != 0.0 {
            return Err(plain("JSON number is not an integer"));
        }
        Ok(format!("{number:.0}"))
    } else {
        Ok(value.to_owned())
    }
}

fn json_float(value: f64) -> JsonValue {
    if value.is_nan() {
        JsonValue::String("NaN".to_owned())
    } else if value == f64::INFINITY {
        JsonValue::String("Infinity".to_owned())
    } else if value == f64::NEG_INFINITY {
        JsonValue::String("-Infinity".to_owned())
    } else if value == 0.0 && value.is_sign_negative() {
        JsonValue::Number("-0".to_owned())
    } else {
        let magnitude = value.abs();
        JsonValue::Number(if magnitude != 0.0 && !(1e-6..1e21).contains(&magnitude) {
            format!("{value:e}")
        } else {
            value.to_string()
        })
    }
}

fn write_json(
    value: &JsonValue,
    style: FormatStyle,
    depth: usize,
    output: &mut String,
) -> Result<(), JsonError> {
    match value {
        JsonValue::Null => output.push_str("null"),
        JsonValue::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
        JsonValue::Number(value) => output.push_str(value),
        JsonValue::String(value) => write_json_string(value, output)?,
        JsonValue::Array(values) => {
            output.push('[');
            for (index, value) in values.iter().enumerate() {
                if index != 0 {
                    output.push(',');
                }
                if style == FormatStyle::Pretty {
                    output.push('\n');
                    indent(depth + 1, output);
                }
                write_json(value, style, depth + 1, output)?;
            }
            if style == FormatStyle::Pretty && !values.is_empty() {
                output.push('\n');
                indent(depth, output);
            }
            output.push(']');
        }
        JsonValue::Object(values) => {
            output.push('{');
            for (index, (name, value)) in values.iter().enumerate() {
                if index != 0 {
                    output.push(',');
                }
                if style == FormatStyle::Pretty {
                    output.push('\n');
                    indent(depth + 1, output);
                }
                write_json_string(name, output)?;
                output.push(':');
                if style == FormatStyle::Pretty {
                    output.push(' ');
                }
                write_json(value, style, depth + 1, output)?;
            }
            if style == FormatStyle::Pretty && !values.is_empty() {
                output.push('\n');
                indent(depth, output);
            }
            output.push('}');
        }
    }
    Ok(())
}

fn indent(depth: usize, output: &mut String) {
    for _ in 0..depth {
        output.push_str("  ");
    }
}

fn write_json_string(value: &str, output: &mut String) -> Result<(), JsonError> {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{08}' => output.push_str("\\b"),
            '\u{0c}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            value if value <= '\u{1f}' => {
                write!(output, "\\u{:04x}", value as u32).map_err(|_| plain("format failure"))?;
            }
            value => output.push(value),
        }
    }
    output.push('"');
    Ok(())
}

fn quoted_text(bytes: &[u8]) -> Result<String, JsonError> {
    let value = std::str::from_utf8(bytes).map_err(|_| plain("Text is not UTF-8"))?;
    let mut output = String::new();
    write_json_string(value, &mut output)?;
    Ok(output)
}

fn hex_text(bytes: &[u8]) -> String {
    let mut output = String::from("0x\"");
    for byte in bytes {
        let _ = write!(&mut output, "{byte:02x}");
    }
    output.push('"');
    output
}

fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let bits = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        output.push(char::from(TABLE[((bits >> 18) & 63) as usize]));
        output.push(char::from(TABLE[((bits >> 12) & 63) as usize]));
        output.push(if chunk.len() > 1 {
            char::from(TABLE[((bits >> 6) & 63) as usize])
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            char::from(TABLE[(bits & 63) as usize])
        } else {
            '='
        });
    }
    output
}

fn base64_decode(input: &str) -> Result<Vec<u8>, JsonError> {
    if input.len() % 4 != 0 {
        return Err(plain("base64 length must be a multiple of four"));
    }
    let mut output = Vec::with_capacity(input.len() / 4 * 3);
    for (chunk_index, chunk) in input.as_bytes().chunks(4).enumerate() {
        let last = chunk_index + 1 == input.len() / 4;
        let a = base64_digit(chunk[0])?;
        let b = base64_digit(chunk[1])?;
        let c = if chunk[2] == b'=' {
            0
        } else {
            base64_digit(chunk[2])?
        };
        let d = if chunk[3] == b'=' {
            0
        } else {
            base64_digit(chunk[3])?
        };
        if (!last && (chunk[2] == b'=' || chunk[3] == b'='))
            || (chunk[2] == b'=' && chunk[3] != b'=')
        {
            return Err(plain("invalid base64 padding"));
        }
        let bits = (u32::from(a) << 18) | (u32::from(b) << 12) | (u32::from(c) << 6) | u32::from(d);
        output.push((bits >> 16) as u8);
        if chunk[2] != b'=' {
            output.push((bits >> 8) as u8);
        }
        if chunk[3] != b'=' {
            output.push(bits as u8);
        }
    }
    Ok(output)
}

fn base64_digit(value: u8) -> Result<u8, JsonError> {
    match value {
        b'A'..=b'Z' => Ok(value - b'A'),
        b'a'..=b'z' => Ok(value - b'a' + 26),
        b'0'..=b'9' => Ok(value - b'0' + 52),
        b'+' => Ok(62),
        b'/' => Ok(63),
        _ => Err(plain("invalid base64 character")),
    }
}

fn hex_decode(input: &str) -> Result<Vec<u8>, JsonError> {
    if input.len() % 2 != 0 {
        return Err(plain("hex Data requires pairs of digits"));
    }
    input
        .as_bytes()
        .chunks(2)
        .map(|pair| {
            let high = hex_digit(pair[0])?;
            let low = hex_digit(pair[1])?;
            Ok((high << 4) | low)
        })
        .collect()
}

fn hex_digit(value: u8) -> Result<u8, JsonError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err(plain("invalid hexadecimal digit")),
    }
}

struct Parser<'a> {
    source: &'a str,
    offset: usize,
    values: usize,
    limits: JsonLimits,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str, limits: JsonLimits) -> Result<Self, JsonError> {
        if source.len() > limits.max_input_bytes {
            return Err(error_at(
                source,
                limits.max_input_bytes,
                "JSON input exceeds byte limit",
            ));
        }
        Ok(Self {
            source,
            offset: 0,
            values: 0,
            limits,
        })
    }

    fn parse(mut self) -> Result<JsonValue, JsonError> {
        self.whitespace();
        let value = self.value(0)?;
        self.whitespace();
        if self.offset != self.source.len() {
            return Err(self.error("trailing characters after JSON value"));
        }
        Ok(value)
    }

    fn value(&mut self, depth: u16) -> Result<JsonValue, JsonError> {
        if depth > self.limits.max_nesting {
            return Err(self.error("JSON nesting limit exceeded"));
        }
        self.values = self
            .values
            .checked_add(1)
            .ok_or_else(|| self.error("JSON value count overflow"))?;
        if self.values > self.limits.max_values {
            return Err(self.error("JSON value count limit exceeded"));
        }
        self.whitespace();
        match self.peek() {
            Some(b'n') => self.keyword("null", JsonValue::Null),
            Some(b't') => self.keyword("true", JsonValue::Bool(true)),
            Some(b'f') => self.keyword("false", JsonValue::Bool(false)),
            Some(b'"') => self.string().map(JsonValue::String),
            Some(b'[') => self.array(depth + 1),
            Some(b'{') => self.object(depth + 1),
            Some(b'-' | b'0'..=b'9') => self.number().map(JsonValue::Number),
            Some(_) => Err(self.error("expected a JSON value")),
            None => Err(self.error("JSON input ends prematurely")),
        }
    }

    fn keyword(&mut self, keyword: &str, value: JsonValue) -> Result<JsonValue, JsonError> {
        if self.source[self.offset..].starts_with(keyword) {
            self.offset += keyword.len();
            Ok(value)
        } else {
            Err(self.error(format!("expected `{keyword}`")))
        }
    }

    fn array(&mut self, depth: u16) -> Result<JsonValue, JsonError> {
        self.offset += 1;
        self.whitespace();
        let mut values = Vec::new();
        if self.take(b']') {
            return Ok(JsonValue::Array(values));
        }
        loop {
            values.push(self.value(depth)?);
            self.whitespace();
            if self.take(b']') {
                break;
            }
            if !self.take(b',') {
                return Err(self.error("expected `,` or `]`"));
            }
        }
        Ok(JsonValue::Array(values))
    }

    fn object(&mut self, depth: u16) -> Result<JsonValue, JsonError> {
        self.offset += 1;
        self.whitespace();
        let mut values = Vec::new();
        if self.take(b'}') {
            return Ok(JsonValue::Object(values));
        }
        loop {
            self.whitespace();
            if self.peek() != Some(b'"') {
                return Err(self.error("expected a quoted object field name"));
            }
            let name = self.string()?;
            self.whitespace();
            if !self.take(b':') {
                return Err(self.error("expected `:` after object field name"));
            }
            values.push((name, self.value(depth)?));
            self.whitespace();
            if self.take(b'}') {
                break;
            }
            if !self.take(b',') {
                return Err(self.error("expected `,` or `}`"));
            }
        }
        Ok(JsonValue::Object(values))
    }

    fn string(&mut self) -> Result<String, JsonError> {
        let start = self.offset;
        self.offset += 1;
        let mut output = String::new();
        while let Some(byte) = self.peek() {
            match byte {
                b'"' => {
                    self.offset += 1;
                    return Ok(output);
                }
                b'\\' => {
                    self.offset += 1;
                    let escape = self
                        .peek()
                        .ok_or_else(|| self.error("incomplete JSON escape"))?;
                    self.offset += 1;
                    match escape {
                        b'"' => output.push('"'),
                        b'\\' => output.push('\\'),
                        b'/' => output.push('/'),
                        b'b' => output.push('\u{08}'),
                        b'f' => output.push('\u{0c}'),
                        b'n' => output.push('\n'),
                        b'r' => output.push('\r'),
                        b't' => output.push('\t'),
                        b'u' => self.unicode_escape(&mut output)?,
                        _ => return Err(self.error("invalid JSON escape")),
                    }
                }
                0..=31 => return Err(self.error("unescaped control character in JSON string")),
                _ => {
                    let value = self.source[self.offset..]
                        .chars()
                        .next()
                        .ok_or_else(|| self.error("invalid UTF-8 boundary"))?;
                    output.push(value);
                    self.offset += value.len_utf8();
                }
            }
        }
        Err(error_at(self.source, start, "unterminated JSON string"))
    }

    fn unicode_escape(&mut self, output: &mut String) -> Result<(), JsonError> {
        let start = self.offset.saturating_sub(2);
        let first = self.hex_quad()?;
        let scalar = if (0xd800..=0xdbff).contains(&first) {
            if self.source.as_bytes().get(self.offset..self.offset + 2) != Some(b"\\u") {
                return Err(error_at(
                    self.source,
                    start,
                    "high surrogate requires a low surrogate",
                ));
            }
            self.offset += 2;
            let second = self.hex_quad()?;
            if !(0xdc00..=0xdfff).contains(&second) {
                return Err(error_at(self.source, start, "invalid low surrogate"));
            }
            0x10000 + ((u32::from(first) - 0xd800) << 10) + (u32::from(second) - 0xdc00)
        } else if (0xdc00..=0xdfff).contains(&first) {
            return Err(error_at(self.source, start, "unexpected low surrogate"));
        } else {
            u32::from(first)
        };
        output.push(char::from_u32(scalar).ok_or_else(|| self.error("invalid Unicode scalar"))?);
        Ok(())
    }

    fn hex_quad(&mut self) -> Result<u16, JsonError> {
        let start = self.offset;
        let mut value = 0u16;
        for _ in 0..4 {
            let byte = self
                .peek()
                .ok_or_else(|| error_at(self.source, start, "short Unicode escape"))?;
            self.offset += 1;
            value = (value << 4)
                | u16::from(
                    hex_digit(byte)
                        .map_err(|_| error_at(self.source, start, "invalid Unicode escape"))?,
                );
        }
        Ok(value)
    }

    fn number(&mut self) -> Result<String, JsonError> {
        let start = self.offset;
        self.take(b'-');
        match self.peek() {
            Some(b'0') => {
                self.offset += 1;
                if self.peek().is_some_and(|value| value.is_ascii_digit()) {
                    return Err(self.error("leading zero in JSON number"));
                }
            }
            Some(b'1'..=b'9') => {
                while self.peek().is_some_and(|value| value.is_ascii_digit()) {
                    self.offset += 1;
                }
            }
            _ => return Err(self.error("invalid JSON number")),
        }
        if self.take(b'.') {
            let fraction = self.offset;
            while self.peek().is_some_and(|value| value.is_ascii_digit()) {
                self.offset += 1;
            }
            if self.offset == fraction {
                return Err(self.error("JSON fraction requires digits"));
            }
        }
        if self
            .peek()
            .is_some_and(|value| matches!(value, b'e' | b'E'))
        {
            self.offset += 1;
            if self
                .peek()
                .is_some_and(|value| matches!(value, b'+' | b'-'))
            {
                self.offset += 1;
            }
            let exponent = self.offset;
            while self.peek().is_some_and(|value| value.is_ascii_digit()) {
                self.offset += 1;
            }
            if self.offset == exponent {
                return Err(self.error("JSON exponent requires digits"));
            }
        }
        Ok(self.source[start..self.offset].to_owned())
    }

    fn whitespace(&mut self) {
        while self
            .peek()
            .is_some_and(|value| matches!(value, b' ' | b'\t' | b'\r' | b'\n'))
        {
            self.offset += 1;
        }
    }

    fn take(&mut self, expected: u8) -> bool {
        self.whitespace();
        if self.peek() == Some(expected) {
            self.offset += 1;
            true
        } else {
            false
        }
    }

    fn peek(&self) -> Option<u8> {
        self.source.as_bytes().get(self.offset).copied()
    }

    fn error(&self, message: impl Into<String>) -> JsonError {
        error_at(self.source, self.offset, message)
    }
}

fn error_at(source: &str, byte: usize, message: impl Into<String>) -> JsonError {
    let mut byte = byte.min(source.len());
    while !source.is_char_boundary(byte) {
        byte = byte.saturating_sub(1);
    }
    let prefix = &source[..byte];
    let line = prefix.bytes().filter(|value| *value == b'\n').count() + 1;
    let column = prefix
        .rsplit_once('\n')
        .map_or(prefix.len(), |(_, tail)| tail.len())
        + 1;
    JsonError {
        byte,
        line,
        column,
        message: message.into(),
    }
}

fn plain(message: impl Into<String>) -> JsonError {
    JsonError {
        byte: 0,
        line: 1,
        column: 1,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REQUEST: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../conformance/fixtures/cpp/e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/",
        "compiler-request-wire-fixture.bin"
    ));
    const FRAME: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../conformance/fixtures/cpp/e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/",
        "wire-unpacked.bin"
    ));
    const JSON: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../conformance/fixtures/json/wire-short.json"
    ));

    fn schema() -> Arc<CompiledSchema> {
        Arc::new(
            CompiledSchema::from_code_generator_request(REQUEST, Default::default())
                .expect("schema loads"),
        )
    }

    fn root_id(schema: &CompiledSchema) -> NodeId {
        let file = schema.requested_files().first().expect("requested file");
        schema.nested(file.id, "WireFixture").expect("root type").id
    }

    #[test]
    fn pinned_cpp_json_is_byte_exact_and_round_trips() {
        let schema = schema();
        let type_id = root_id(&schema);
        let parsed = capnp_io::parse_frame(FRAME, Default::default()).expect("frame");
        assert!(matches!(parsed, capnp_io::FrameRead::Message { .. }));
        let capnp_io::FrameRead::Message { frame, remaining } = parsed else {
            return;
        };
        assert!(remaining.is_empty());
        let segments = frame
            .segments()
            .iter()
            .map(|segment| Arc::<[u8]>::from(segment.bytes()))
            .collect::<Vec<_>>();
        let codec = JsonCodec::new();
        assert_eq!(
            codec
                .encode_message(
                    Arc::clone(&schema),
                    type_id,
                    segments,
                    ReaderLimits::default()
                )
                .expect("encode"),
            JSON.trim_end()
        );
        let messages = codec
            .decode_structs(&schema, type_id, JSON)
            .expect("decode");
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn parser_is_bounded_located_and_round_trips_unicode() {
        let mut codec = JsonCodec::new();
        let parsed = codec
            .parse(r#"{"emoji":"\ud83d\ude80","array":[null,true,-1.5e2]}"#)
            .expect("valid JSON");
        assert_eq!(
            codec.parse(&codec.format(&parsed).expect("format")),
            Ok(parsed)
        );
        codec.set_limits(JsonLimits {
            max_nesting: 2,
            ..JsonLimits::default()
        });
        assert!(codec.parse("[[[0]]]").is_err());
        let error = codec.parse("{\n  \"x\": 01\n}").expect_err("leading zero");
        assert_eq!(error.line, 2);
        assert!(error.column > 1);
        codec.set_limits(JsonLimits {
            max_input_bytes: 1,
            ..JsonLimits::default()
        });
        assert_eq!(codec.parse("λ").expect_err("byte bound").byte, 0);
    }

    #[test]
    fn arbitrary_input_never_panics() {
        let codec = JsonCodec::new();
        let mut state = 0x1234_5678u32;
        for len in 0..512 {
            let mut bytes = Vec::with_capacity(len);
            for _ in 0..len {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                bytes.push((state >> 24) as u8);
            }
            if let Ok(text) = String::from_utf8(bytes) {
                let _ = codec.parse(&text);
            }
        }
    }

    #[test]
    fn base64_and_hex_are_strict() {
        let bytes = [0, 1, 2, 0xfe, 0xff];
        assert_eq!(base64_decode(&base64_encode(&bytes)), Ok(bytes.to_vec()));
        assert_eq!(hex_decode("000102feFF"), Ok(bytes.to_vec()));
        assert!(base64_decode("A==A").is_err());
        assert!(hex_decode("abc").is_err());
        assert_eq!(integer_number_text("1.0"), Ok("1".to_owned()));
        assert!(integer_number_text("1.5").is_err());
    }

    fn frame_segments() -> Vec<Arc<[u8]>> {
        let parsed = capnp_io::parse_frame(FRAME, Default::default()).expect("frame");
        assert!(matches!(parsed, capnp_io::FrameRead::Message { .. }));
        let capnp_io::FrameRead::Message { frame, remaining } = parsed else {
            return Vec::new();
        };
        assert!(remaining.is_empty());
        frame
            .segments()
            .iter()
            .map(|segment| Arc::<[u8]>::from(segment.bytes()))
            .collect()
    }

    #[test]
    fn unknown_fields_default_to_evolution_tolerant_but_strict_is_available() {
        let schema = schema();
        let type_id = root_id(&schema);
        let codec = JsonCodec::new();
        assert!(
            codec
                .decode_structs(&schema, type_id, "{\"future\":1}")
                .is_ok()
        );
        let mut strict = JsonCodec::new();
        strict.set_reject_unknown_fields(true);
        let error = strict
            .decode_structs(&schema, type_id, "{\"future\":1}")
            .expect_err("strict mode rejects unknown fields");
        assert!(error.message.contains("unknown JSON field `future`"));
    }

    #[test]
    fn non_default_mode_omits_scalar_defaults() {
        use capnp_message::ExclusiveArena;
        use capnp_schema::DynamicStructBuilder;

        let schema = schema();
        let type_id = root_id(&schema);
        let mut arena = ExclusiveArena::new(32, 4096).expect("arena");
        DynamicStructBuilder::root(&schema, &mut arena, type_id).expect("root builder");
        let message = OwnedMessage::new(arena.into_segments(), ReaderLimits::default())
            .expect("zero message");
        let root = DynamicStruct::root(Arc::clone(&schema), message, type_id).expect("root");
        let normal = JsonCodec::new().encode_struct(&root).expect("normal JSON");
        assert!(normal.contains("\"boolValue\":false"));
        let mut sparse = JsonCodec::new();
        sparse.set_has_mode(HasMode::NonDefault);
        let sparse = sparse.encode_struct(&root).expect("sparse JSON");
        assert!(!sparse.contains("boolValue"));
        assert!(!sparse.contains("defaulted"));
    }

    struct ReplaceText;

    impl JsonHandler for ReplaceText {
        fn encode(&self, input: &DynamicValue) -> Result<JsonValue, JsonError> {
            match input {
                DynamicValue::Text(_) => Ok(JsonValue::String("handled".to_owned())),
                _ => Err(plain("handler expected Text")),
            }
        }

        fn decode(&self, input: &JsonValue) -> Result<JsonValue, JsonError> {
            match input {
                JsonValue::String(_) => Ok(JsonValue::String("decoded".to_owned())),
                _ => Err(plain("handler expected a JSON string")),
            }
        }
    }

    #[test]
    fn field_handlers_override_encode_and_decode_without_arena_access() {
        let schema = schema();
        let type_id = root_id(&schema);
        let mut codec = JsonCodec::new();
        codec.add_field_handler(type_id, "text", Arc::new(ReplaceText));
        let json = codec
            .encode_message(
                Arc::clone(&schema),
                type_id,
                frame_segments(),
                ReaderLimits::default(),
            )
            .expect("handler encode");
        assert!(json.contains("\"text\":\"handled\""));
        let messages = codec
            .decode_structs(&schema, type_id, "{\"text\":\"ignored\"}")
            .expect("handler decode");
        let output = JsonCodec::new()
            .encode_message(
                Arc::clone(&schema),
                type_id,
                messages[0]
                    .segments
                    .iter()
                    .map(|segment| Arc::<[u8]>::from(segment.as_ref()))
                    .collect::<Vec<_>>(),
                ReaderLimits::default(),
            )
            .expect("decoded message JSON");
        assert!(output.contains("\"text\":\"decoded\""));

        let mut type_codec = JsonCodec::new();
        type_codec.add_type_handler(TypeSelector::Text, Arc::new(ReplaceText));
        let messages = type_codec
            .decode_structs(&schema, type_id, "{\"text\":\"ignored\"}")
            .expect("type handler decode");
        let output = JsonCodec::new()
            .encode_message(
                Arc::clone(&schema),
                type_id,
                messages[0]
                    .segments
                    .iter()
                    .map(|segment| Arc::<[u8]>::from(segment.as_ref()))
                    .collect::<Vec<_>>(),
                ReaderLimits::default(),
            )
            .expect("type-decoded message JSON");
        assert!(output.contains("\"text\":\"decoded\""));
    }
}
