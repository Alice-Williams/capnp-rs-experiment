//! Schema-aware Cap'n Proto text values.
//!
//! Compatibility follows the pinned C++ `serialize-text` behavior: structs
//! use `(field = value)`, lists use brackets, fields print in schema code
//! order, absent pointer fields are omitted, and enum names are resolved from
//! the compiled schema. Parsing is bounded before message construction and
//! diagnostics retain byte, line, and column locations. JSON syntax belongs
//! to M28 and is deliberately not accepted here.

use std::collections::BTreeSet;
use std::fmt::{self, Write as _};
use std::sync::Arc;

use capnp_message::{ExclusiveArena, OwnedMessage, ReaderLimits};
use capnp_schema::{
    Brand, CompiledSchema, DynamicAnyPointer, DynamicInput, DynamicList, DynamicListBuilder,
    DynamicStruct, DynamicStructBuilder, DynamicValue, FieldKind, NodeId, NodeKind,
    OpaquePointerKind, Type, Value,
};

/// Independent bounds for textual work and produced messages.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextLimits {
    pub max_input_bytes: usize,
    pub max_values: usize,
    pub max_nesting: u16,
    pub max_message_words: u32,
}

impl Default for TextLimits {
    fn default() -> Self {
        Self {
            max_input_bytes: 16 * 1024 * 1024,
            max_values: 1_000_000,
            max_nesting: 128,
            max_message_words: 8 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FormatStyle {
    Pretty,
    Short,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextError {
    pub byte: usize,
    pub line: usize,
    pub column: usize,
    pub message: String,
}

impl fmt::Display for TextError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}:{} (byte {}): {}",
            self.line, self.column, self.byte, self.message
        )
    }
}

impl std::error::Error for TextError {}

/// One encoded message, before standard framing or packing.
#[derive(Debug)]
pub struct EncodedMessage {
    pub segments: Vec<Box<[u8]>>,
}

#[derive(Clone, Debug, PartialEq)]
struct SpannedValue {
    start: usize,
    value: ParsedValue,
}

#[derive(Clone, Debug, PartialEq)]
enum ParsedValue {
    Void,
    Bool(bool),
    Null,
    Number(String),
    Identifier(String),
    Bytes(Vec<u8>),
    List(Vec<SpannedValue>),
    Struct(Vec<ParsedField>),
}

#[derive(Clone, Debug, PartialEq)]
struct ParsedField {
    start: usize,
    name: String,
    value: SpannedValue,
}

/// Parses one or more struct literals and builds independent messages.
pub fn encode_structs(
    schema: &CompiledSchema,
    type_id: NodeId,
    source: &str,
    limits: TextLimits,
) -> Result<Vec<EncodedMessage>, TextError> {
    encode_structs_branded(schema, type_id, Brand::default(), source, limits)
}

fn encode_structs_branded(
    schema: &CompiledSchema,
    type_id: NodeId,
    brand: Brand,
    source: &str,
    limits: TextLimits,
) -> Result<Vec<EncodedMessage>, TextError> {
    let values = Parser::new(source, limits)?.parse_all()?;
    if values.is_empty() {
        return Err(error_at(source, 0, "expected at least one struct value"));
    }
    let mut messages = Vec::with_capacity(values.len());
    for value in values {
        let ParsedValue::Struct(fields) = &value.value else {
            return Err(error_at(
                source,
                value.start,
                "root value must be a struct literal",
            ));
        };
        let mut arena = ExclusiveArena::new(1024, limits.max_message_words)
            .map_err(|error| error_at(source, value.start, format!("message arena: {error}")))?;
        {
            let mut root =
                DynamicStructBuilder::root_branded(schema, &mut arena, type_id, brand.clone())
                    .map_err(|error| error_at(source, value.start, error.to_string()))?;
            fill_struct(schema, source, type_id, fields, &mut root)?;
        }
        messages.push(EncodedMessage {
            segments: arena.into_segments(),
        });
    }
    Ok(messages)
}

/// Formats a reflected struct with deterministic reference-compatible syntax.
pub fn format_struct(value: &DynamicStruct, style: FormatStyle) -> Result<String, TextError> {
    let mut output = String::new();
    write_struct(value, style, 0, &mut output)?;
    Ok(output)
}

/// Formats a reflected value with deterministic reference-compatible syntax.
pub fn format_value(value: &DynamicValue, style: FormatStyle) -> Result<String, TextError> {
    let mut output = String::new();
    write_dynamic(value, style, 0, &mut output)?;
    Ok(output)
}

/// Opens an owned message and formats its root as `type_id`.
pub fn format_message(
    schema: Arc<CompiledSchema>,
    type_id: NodeId,
    segments: impl IntoIterator<Item = Arc<[u8]>>,
    style: FormatStyle,
    reader_limits: ReaderLimits,
) -> Result<String, TextError> {
    let message = OwnedMessage::new(segments, reader_limits)
        .map_err(|error| plain_error(format!("message validation failed: {error}")))?;
    let root = DynamicStruct::root(schema, message, type_id)
        .map_err(|error| plain_error(format!("root type mismatch: {error}")))?;
    format_struct(&root, style)
}

/// Resolves and formats a constant, field default, or nested member/list item.
///
/// `scope_id` is normally the requested file node. Callers resolving a source
/// import alias may instead pass the imported file node and omit that alias
/// from `expression`.
pub fn evaluate(
    schema: Arc<CompiledSchema>,
    scope_id: NodeId,
    expression: &str,
    style: FormatStyle,
) -> Result<String, TextError> {
    resolve_evaluated(&schema, scope_id, expression)?.format(schema, style)
}

/// Evaluates a struct-valued expression and rebuilds it as one message.
pub fn evaluate_struct_message(
    schema: Arc<CompiledSchema>,
    scope_id: NodeId,
    expression: &str,
    limits: TextLimits,
) -> Result<EncodedMessage, TextError> {
    let value = resolve_evaluated(&schema, scope_id, expression)?;
    let Type::Struct { type_id, brand } = &value.ty else {
        return Err(error_at(
            expression,
            expression.len(),
            "binary eval output requires a struct value",
        ));
    };
    let type_id = *type_id;
    let brand = brand.clone();
    let text = value.format(Arc::clone(&schema), FormatStyle::Short)?;
    let mut messages = encode_structs_branded(&schema, type_id, brand, &text, limits)?;
    messages
        .pop()
        .ok_or_else(|| plain_error("evaluated struct did not produce a message"))
}

fn resolve_evaluated(
    schema: &Arc<CompiledSchema>,
    scope_id: NodeId,
    expression: &str,
) -> Result<EvaluatedValue, TextError> {
    let steps = parse_eval_path(expression)?;
    let mut cursor = 0;
    let mut node = schema
        .node(scope_id)
        .ok_or_else(|| error_at(expression, 0, "evaluation scope is not in the schema"))?;
    while let Some(EvalStep::Member { name, .. }) = steps.get(cursor) {
        let Some(next) = schema.nested(node.id, name) else {
            break;
        };
        node = next;
        cursor += 1;
    }
    let mut value = match &node.kind {
        NodeKind::Const(constant) => EvaluatedValue {
            ty: constant.ty.clone(),
            value: EvaluatedStorage::Schema(constant.value.clone()),
        },
        NodeKind::Struct(structure) => {
            let Some(EvalStep::Member { name, start }) = steps.get(cursor) else {
                return Err(error_at(
                    expression,
                    expression.len(),
                    "name does not resolve to a constant or field default",
                ));
            };
            let field = structure
                .field(name)
                .ok_or_else(|| error_at(expression, *start, format!("unknown field `{name}`")))?;
            let FieldKind::Slot {
                ty, default_value, ..
            } = &field.kind
            else {
                return Err(error_at(
                    expression,
                    *start,
                    "group defaults cannot be evaluated directly",
                ));
            };
            cursor += 1;
            EvaluatedValue {
                ty: ty.clone(),
                value: EvaluatedStorage::Schema(default_value.clone()),
            }
        }
        _ => {
            return Err(error_at(
                expression,
                expression.len(),
                "name does not resolve to a constant or field default",
            ));
        }
    };
    while let Some(step) = steps.get(cursor) {
        value = value.select(Arc::clone(schema), expression, step)?;
        cursor += 1;
    }
    Ok(value)
}

#[derive(Clone, Debug)]
enum EvalStep {
    Member { name: String, start: usize },
    Index { index: u32, start: usize },
}

fn parse_eval_path(source: &str) -> Result<Vec<EvalStep>, TextError> {
    let bytes = source.as_bytes();
    let mut offset = 0;
    let mut output = Vec::new();
    let parse_member = |offset: &mut usize| -> Result<EvalStep, TextError> {
        let start = *offset;
        if !bytes.get(*offset).copied().is_some_and(is_identifier_start) {
            return Err(error_at(source, start, "expected a name"));
        }
        *offset += 1;
        while bytes
            .get(*offset)
            .copied()
            .is_some_and(is_identifier_continue)
        {
            *offset += 1;
        }
        Ok(EvalStep::Member {
            name: source[start..*offset].to_owned(),
            start,
        })
    };
    output.push(parse_member(&mut offset)?);
    while offset < bytes.len() {
        match bytes[offset] {
            b'.' => {
                offset += 1;
                output.push(parse_member(&mut offset)?);
            }
            b'[' => {
                let start = offset;
                offset += 1;
                let digits = offset;
                while bytes.get(offset).is_some_and(u8::is_ascii_digit) {
                    offset += 1;
                }
                if digits == offset || bytes.get(offset) != Some(&b']') {
                    return Err(error_at(
                        source,
                        start,
                        "expected a decimal list index and `]`",
                    ));
                }
                let index = source[digits..offset]
                    .parse()
                    .map_err(|_| error_at(source, digits, "list index is too large"))?;
                offset += 1;
                output.push(EvalStep::Index { index, start });
            }
            _ => return Err(error_at(source, offset, "expected `.` or `[`")),
        }
    }
    Ok(output)
}

#[derive(Clone, Debug)]
struct EvaluatedValue {
    ty: Type,
    value: EvaluatedStorage,
}

#[derive(Clone, Debug)]
enum EvaluatedStorage {
    Schema(Value),
    Dynamic(DynamicValue),
}

impl EvaluatedValue {
    fn select(
        self,
        schema: Arc<CompiledSchema>,
        source: &str,
        step: &EvalStep,
    ) -> Result<Self, TextError> {
        match step {
            EvalStep::Member { name, start } => {
                let Type::Struct { type_id, brand } = &self.ty else {
                    return Err(error_at(source, *start, "member access requires a struct"));
                };
                let structure = match self.value {
                    EvaluatedStorage::Dynamic(DynamicValue::Struct(Some(value))) => value,
                    EvaluatedStorage::Schema(value) => DynamicStruct::from_branded_value(
                        Arc::clone(&schema),
                        *type_id,
                        brand.clone(),
                        &value,
                        ReaderLimits::default(),
                    )
                    .map_err(|error| error_at(source, *start, error.to_string()))?
                    .ok_or_else(|| error_at(source, *start, "cannot select a member of null"))?,
                    _ => return Err(error_at(source, *start, "member access requires a struct")),
                };
                let ty = structure
                    .field_type(name)
                    .map_err(|error| error_at(source, *start, error.to_string()))?;
                let value = structure
                    .get(name)
                    .map_err(|error| error_at(source, *start, error.to_string()))?;
                Ok(Self {
                    ty,
                    value: EvaluatedStorage::Dynamic(value),
                })
            }
            EvalStep::Index { index, start } => {
                let Type::List(element) = &self.ty else {
                    return Err(error_at(source, *start, "indexing requires a list"));
                };
                let list = match self.value {
                    EvaluatedStorage::Dynamic(DynamicValue::List(Some(value))) => value,
                    EvaluatedStorage::Schema(value) => DynamicList::from_value(
                        Arc::clone(&schema),
                        (**element).clone(),
                        &value,
                        ReaderLimits::default(),
                    )
                    .map_err(|error| error_at(source, *start, error.to_string()))?
                    .ok_or_else(|| error_at(source, *start, "cannot index a null list"))?,
                    _ => return Err(error_at(source, *start, "indexing requires a list")),
                };
                let value = list
                    .get(*index)
                    .map_err(|error| error_at(source, *start, error.to_string()))?;
                Ok(Self {
                    ty: (**element).clone(),
                    value: EvaluatedStorage::Dynamic(value),
                })
            }
        }
    }

    fn format(self, schema: Arc<CompiledSchema>, style: FormatStyle) -> Result<String, TextError> {
        match self.value {
            EvaluatedStorage::Dynamic(value) => format_value(&value, style),
            EvaluatedStorage::Schema(value) => format_schema_value(schema, &self.ty, &value, style),
        }
    }
}

fn format_schema_value(
    schema: Arc<CompiledSchema>,
    ty: &Type,
    value: &Value,
    style: FormatStyle,
) -> Result<String, TextError> {
    let mut output = String::new();
    match (ty, value) {
        (Type::Void, Value::Void) => output.push_str("void"),
        (Type::Bool, Value::Bool(value)) => output.push_str(if *value { "true" } else { "false" }),
        (Type::Int8, Value::Int8(value)) => {
            write!(output, "{value}").map_err(|_| plain_error("format failure"))?
        }
        (Type::Int16, Value::Int16(value)) => {
            write!(output, "{value}").map_err(|_| plain_error("format failure"))?
        }
        (Type::Int32, Value::Int32(value)) => {
            write!(output, "{value}").map_err(|_| plain_error("format failure"))?
        }
        (Type::Int64, Value::Int64(value)) => {
            write!(output, "{value}").map_err(|_| plain_error("format failure"))?
        }
        (Type::UInt8, Value::UInt8(value)) => {
            write!(output, "{value}").map_err(|_| plain_error("format failure"))?
        }
        (Type::UInt16, Value::UInt16(value)) => {
            write!(output, "{value}").map_err(|_| plain_error("format failure"))?
        }
        (Type::UInt32, Value::UInt32(value)) => {
            write!(output, "{value}").map_err(|_| plain_error("format failure"))?
        }
        (Type::UInt64, Value::UInt64(value)) => {
            write!(output, "{value}").map_err(|_| plain_error("format failure"))?
        }
        (Type::Float32, Value::Float32(value)) => write_float32(*value, &mut output),
        (Type::Float64, Value::Float64(value)) => write_float(*value, &mut output),
        (Type::Text, Value::Text(value)) => write_quoted(value.as_bytes(), true, &mut output),
        (Type::Data, Value::Data(value)) => write_quoted(value, false, &mut output),
        (Type::Enum { type_id, .. }, Value::Enum(ordinal)) => {
            if let Some(name) = schema.node(*type_id).and_then(|node| match &node.kind {
                NodeKind::Enum(value) => value
                    .enumerants
                    .get(usize::from(*ordinal))
                    .map(|value| value.name.as_str()),
                _ => None,
            }) {
                output.push_str(name);
            } else {
                write!(output, "{ordinal}").map_err(|_| plain_error("format failure"))?;
            }
        }
        (Type::Struct { type_id, brand }, Value::Struct(_)) => {
            let value = DynamicStruct::from_branded_value(
                schema,
                *type_id,
                brand.clone(),
                value,
                ReaderLimits::default(),
            )
            .map_err(|error| plain_error(error.to_string()))?;
            match value {
                Some(value) => return format_struct(&value, style),
                None => output.push_str("()"),
            }
        }
        (Type::List(element), Value::List(_)) => {
            let value = DynamicList::from_value(
                schema,
                (**element).clone(),
                value,
                ReaderLimits::default(),
            )
            .map_err(|error| plain_error(error.to_string()))?;
            match value {
                Some(value) => write_list(&value, style, 0, &mut output)?,
                None => output.push_str("[]"),
            }
        }
        (Type::Text, Value::AnyPointer(value)) if value.kind == OpaquePointerKind::Null => {
            output.push_str("\"\"")
        }
        (Type::Data, Value::AnyPointer(value)) if value.kind == OpaquePointerKind::Null => {
            output.push_str("\"\"")
        }
        (Type::Struct { .. }, Value::AnyPointer(value))
            if value.kind == OpaquePointerKind::Null =>
        {
            output.push_str("()")
        }
        (Type::List(_), Value::AnyPointer(value)) if value.kind == OpaquePointerKind::Null => {
            output.push_str("[]")
        }
        (Type::Interface { .. } | Type::AnyPointer(_), Value::AnyPointer(value))
            if value.kind == OpaquePointerKind::Null =>
        {
            output.push_str("null")
        }
        (Type::Interface { .. }, Value::Interface) => output.push_str("null"),
        _ => return Err(plain_error("schema value does not match its declared type")),
    }
    Ok(output)
}

fn fill_struct(
    schema: &CompiledSchema,
    source: &str,
    type_id: NodeId,
    fields: &[ParsedField],
    builder: &mut DynamicStructBuilder<'_, '_>,
) -> Result<(), TextError> {
    let structure = schema
        .node(type_id)
        .and_then(|node| match &node.kind {
            NodeKind::Struct(value) => Some(value),
            _ => None,
        })
        .ok_or_else(|| plain_error(format!("0x{type_id:016x} is not a struct schema")))?;
    let mut seen = BTreeSet::new();
    for parsed in fields {
        if !seen.insert(parsed.name.as_str()) {
            return Err(error_at(
                source,
                parsed.start,
                format!("duplicate field `{}`", parsed.name),
            ));
        }
        let field = structure.field(&parsed.name).ok_or_else(|| {
            error_at(
                source,
                parsed.start,
                format!("unknown field `{}`", parsed.name),
            )
        })?;
        match &field.kind {
            FieldKind::Group { type_id } => {
                let ParsedValue::Struct(children) = &parsed.value.value else {
                    return Err(error_at(
                        source,
                        parsed.value.start,
                        format!("group `{}` requires a struct literal", parsed.name),
                    ));
                };
                let mut group = builder
                    .group(&parsed.name)
                    .map_err(|error| error_at(source, parsed.start, error.to_string()))?;
                fill_struct(schema, source, *type_id, children, &mut group)?;
            }
            FieldKind::Slot { .. } => {
                let ty = builder
                    .field_type(&parsed.name)
                    .map_err(|error| error_at(source, parsed.start, error.to_string()))?;
                fill_struct_slot(schema, source, &parsed.name, &ty, &parsed.value, builder)?;
            }
        }
    }
    Ok(())
}

fn fill_struct_slot(
    schema: &CompiledSchema,
    source: &str,
    name: &str,
    ty: &Type,
    value: &SpannedValue,
    builder: &mut DynamicStructBuilder<'_, '_>,
) -> Result<(), TextError> {
    match ty {
        Type::Struct { type_id, .. } => match &value.value {
            ParsedValue::Null => builder
                .activate(name)
                .map_err(|error| error_at(source, value.start, error.to_string())),
            ParsedValue::Struct(fields) => {
                let mut child = builder
                    .init_struct(name)
                    .map_err(|error| error_at(source, value.start, error.to_string()))?;
                fill_struct(schema, source, *type_id, fields, &mut child)
            }
            _ => Err(error_at(source, value.start, "expected a struct literal")),
        },
        Type::List(element) => match &value.value {
            ParsedValue::Null => builder
                .activate(name)
                .map_err(|error| error_at(source, value.start, error.to_string())),
            ParsedValue::List(values) => {
                let count = u32::try_from(values.len())
                    .map_err(|_| error_at(source, value.start, "list is too large"))?;
                let mut list = builder
                    .init_list(name, count)
                    .map_err(|error| error_at(source, value.start, error.to_string()))?;
                fill_list(schema, source, element, values, &mut list)
            }
            _ => Err(error_at(source, value.start, "expected a list literal")),
        },
        Type::AnyPointer(_) | Type::Interface { .. }
            if matches!(value.value, ParsedValue::Null) =>
        {
            builder
                .activate(name)
                .map_err(|error| error_at(source, value.start, error.to_string()))
        }
        _ => {
            let input = scalar_input(schema, source, ty, value)?;
            builder
                .set(name, input)
                .map_err(|error| error_at(source, value.start, error.to_string()))
        }
    }
}

fn fill_list(
    schema: &CompiledSchema,
    source: &str,
    element: &Type,
    values: &[SpannedValue],
    builder: &mut DynamicListBuilder<'_, '_>,
) -> Result<(), TextError> {
    for (index, value) in values.iter().enumerate() {
        let index = u32::try_from(index)
            .map_err(|_| error_at(source, value.start, "list index overflow"))?;
        match element {
            Type::Struct { type_id, .. } => match &value.value {
                ParsedValue::Struct(fields) => {
                    let mut child = builder
                        .struct_element(index)
                        .map_err(|error| error_at(source, value.start, error.to_string()))?;
                    fill_struct(schema, source, *type_id, fields, &mut child)?;
                }
                _ => return Err(error_at(source, value.start, "expected a struct literal")),
            },
            Type::List(nested) => match &value.value {
                ParsedValue::Null => {}
                ParsedValue::List(children) => {
                    let count = u32::try_from(children.len())
                        .map_err(|_| error_at(source, value.start, "list is too large"))?;
                    let mut child = builder
                        .init_list(index, count)
                        .map_err(|error| error_at(source, value.start, error.to_string()))?;
                    fill_list(schema, source, nested, children, &mut child)?;
                }
                _ => return Err(error_at(source, value.start, "expected a list literal")),
            },
            Type::AnyPointer(_) | Type::Interface { .. }
                if matches!(value.value, ParsedValue::Null) => {}
            _ => {
                let input = scalar_input(schema, source, element, value)?;
                builder
                    .set(index, input)
                    .map_err(|error| error_at(source, value.start, error.to_string()))?;
            }
        }
    }
    Ok(())
}

fn scalar_input<'a>(
    schema: &CompiledSchema,
    source: &str,
    ty: &Type,
    value: &'a SpannedValue,
) -> Result<DynamicInput<'a>, TextError> {
    let mismatch = |expected: &str| error_at(source, value.start, format!("expected {expected}"));
    match ty {
        Type::Void if matches!(value.value, ParsedValue::Void) => Ok(DynamicInput::Void),
        Type::Bool => match value.value {
            ParsedValue::Bool(value) => Ok(DynamicInput::Bool(value)),
            _ => Err(mismatch("true or false")),
        },
        Type::Int8 => signed_number(value, source).and_then(|number| {
            i8::try_from(number)
                .map(DynamicInput::Int8)
                .map_err(|_| mismatch("Int8"))
        }),
        Type::Int16 => signed_number(value, source).and_then(|number| {
            i16::try_from(number)
                .map(DynamicInput::Int16)
                .map_err(|_| mismatch("Int16"))
        }),
        Type::Int32 => signed_number(value, source).and_then(|number| {
            i32::try_from(number)
                .map(DynamicInput::Int32)
                .map_err(|_| mismatch("Int32"))
        }),
        Type::Int64 => signed_number(value, source).and_then(|number| {
            i64::try_from(number)
                .map(DynamicInput::Int64)
                .map_err(|_| mismatch("Int64"))
        }),
        Type::UInt8 => unsigned_number(value, source).and_then(|number| {
            u8::try_from(number)
                .map(DynamicInput::UInt8)
                .map_err(|_| mismatch("UInt8"))
        }),
        Type::UInt16 => unsigned_number(value, source).and_then(|number| {
            u16::try_from(number)
                .map(DynamicInput::UInt16)
                .map_err(|_| mismatch("UInt16"))
        }),
        Type::UInt32 => unsigned_number(value, source).and_then(|number| {
            u32::try_from(number)
                .map(DynamicInput::UInt32)
                .map_err(|_| mismatch("UInt32"))
        }),
        Type::UInt64 => unsigned_number(value, source)
            .map(DynamicInput::UInt64)
            .map_err(|_| mismatch("UInt64")),
        Type::Float32 => {
            float_number(value, source).map(|value| DynamicInput::Float32(value as f32))
        }
        Type::Float64 => float_number(value, source).map(DynamicInput::Float64),
        Type::Text => match &value.value {
            ParsedValue::Bytes(bytes) => std::str::from_utf8(bytes)
                .map(DynamicInput::Text)
                .map_err(|_| mismatch("UTF-8 Text string")),
            _ => Err(mismatch("quoted Text string")),
        },
        Type::Data => match &value.value {
            ParsedValue::Bytes(bytes) => Ok(DynamicInput::Data(bytes)),
            _ => Err(mismatch("quoted Data string")),
        },
        Type::Enum { type_id, .. } => {
            let ordinal = match &value.value {
                ParsedValue::Identifier(name) => schema
                    .node(*type_id)
                    .and_then(|node| match &node.kind {
                        NodeKind::Enum(value) => value
                            .enumerants
                            .iter()
                            .position(|enumerant| enumerant.name == *name),
                        _ => None,
                    })
                    .and_then(|index| u16::try_from(index).ok())
                    .ok_or_else(|| mismatch("known enum name or ordinal"))?,
                ParsedValue::Number(_) => u16::try_from(unsigned_number(value, source)?)
                    .map_err(|_| mismatch("enum ordinal"))?,
                _ => return Err(mismatch("enum name or ordinal")),
            };
            Ok(DynamicInput::Enum(ordinal))
        }
        Type::Void => Err(mismatch("void")),
        Type::Struct { .. } | Type::List(_) | Type::Interface { .. } | Type::AnyPointer(_) => {
            Err(mismatch("supported pointer literal"))
        }
    }
}

fn signed_number(value: &SpannedValue, source: &str) -> Result<i128, TextError> {
    let ParsedValue::Number(number) = &value.value else {
        return Err(error_at(source, value.start, "expected an integer"));
    };
    parse_signed(number).ok_or_else(|| error_at(source, value.start, "invalid signed integer"))
}

fn unsigned_number(value: &SpannedValue, source: &str) -> Result<u64, TextError> {
    let ParsedValue::Number(number) = &value.value else {
        return Err(error_at(source, value.start, "expected an integer"));
    };
    parse_unsigned(number).ok_or_else(|| error_at(source, value.start, "invalid unsigned integer"))
}

fn parse_signed(value: &str) -> Option<i128> {
    let value = value.replace('_', "");
    let (negative, digits) = value
        .strip_prefix('-')
        .map_or((false, value.as_str()), |digits| (true, digits));
    let magnitude = if let Some(hex) = digits.strip_prefix("0x") {
        u128::from_str_radix(hex, 16).ok()?
    } else {
        digits.parse::<u128>().ok()?
    };
    if negative {
        if magnitude == (i128::MAX as u128) + 1 {
            Some(i128::MIN)
        } else {
            i128::try_from(magnitude).ok().map(|value| -value)
        }
    } else {
        i128::try_from(magnitude).ok()
    }
}

fn parse_unsigned(value: &str) -> Option<u64> {
    let value = value.replace('_', "");
    if value.starts_with('-') {
        return None;
    }
    value.strip_prefix("0x").map_or_else(
        || value.parse().ok(),
        |hex| u64::from_str_radix(hex, 16).ok(),
    )
}

fn float_number(value: &SpannedValue, source: &str) -> Result<f64, TextError> {
    let text = match &value.value {
        ParsedValue::Number(value) | ParsedValue::Identifier(value) => value.as_str(),
        _ => {
            return Err(error_at(
                source,
                value.start,
                "expected a floating-point value",
            ));
        }
    };
    match text.to_ascii_lowercase().as_str() {
        "inf" | "+inf" => Ok(f64::INFINITY),
        "-inf" => Ok(f64::NEG_INFINITY),
        "nan" | "+nan" | "-nan" => Ok(f64::NAN),
        _ => text
            .replace('_', "")
            .parse()
            .map_err(|_| error_at(source, value.start, "invalid floating-point value")),
    }
}

fn write_struct(
    value: &DynamicStruct,
    style: FormatStyle,
    depth: usize,
    output: &mut String,
) -> Result<(), TextError> {
    let structure = value
        .schema()
        .node(value.type_id())
        .and_then(|node| match &node.kind {
            NodeKind::Struct(structure) => Some(structure),
            _ => None,
        })
        .ok_or_else(|| plain_error("dynamic value has no struct schema"))?;
    let mut fields = structure.fields.iter().collect::<Vec<_>>();
    fields.sort_by_key(|field| field.code_order);
    let mut present = Vec::new();
    for field in fields {
        if value
            .is_field_present(&field.name)
            .map_err(|error| plain_error(error.to_string()))?
        {
            present.push((
                field,
                value
                    .get(&field.name)
                    .map_err(|error| plain_error(error.to_string()))?,
            ));
        }
    }
    output.push('(');
    for (index, (field, field_value)) in present.iter().enumerate() {
        if index != 0 {
            output.push(',');
        }
        if style == FormatStyle::Pretty && !present.is_empty() {
            output.push('\n');
            indent(output, depth + 1);
        } else if index != 0 {
            output.push(' ');
        }
        output.push_str(&field.name);
        output.push_str(" = ");
        write_dynamic(field_value, style, depth + 1, output)?;
    }
    if style == FormatStyle::Pretty && !present.is_empty() {
        output.push('\n');
        indent(output, depth);
    }
    output.push(')');
    Ok(())
}

fn write_list(
    value: &DynamicList,
    style: FormatStyle,
    depth: usize,
    output: &mut String,
) -> Result<(), TextError> {
    output.push('[');
    let len = value
        .len()
        .map_err(|error| plain_error(error.to_string()))?;
    for index in 0..len {
        if index != 0 {
            output.push_str(", ");
        }
        let item = value
            .get(index)
            .map_err(|error| plain_error(error.to_string()))?;
        write_dynamic(&item, style, depth + 1, output)?;
    }
    output.push(']');
    Ok(())
}

fn write_dynamic(
    value: &DynamicValue,
    style: FormatStyle,
    depth: usize,
    output: &mut String,
) -> Result<(), TextError> {
    match value {
        DynamicValue::Void => output.push_str("void"),
        DynamicValue::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
        DynamicValue::Int8(value) => {
            write!(output, "{value}").map_err(|_| plain_error("format failure"))?
        }
        DynamicValue::Int16(value) => {
            write!(output, "{value}").map_err(|_| plain_error("format failure"))?
        }
        DynamicValue::Int32(value) => {
            write!(output, "{value}").map_err(|_| plain_error("format failure"))?
        }
        DynamicValue::Int64(value) => {
            write!(output, "{value}").map_err(|_| plain_error("format failure"))?
        }
        DynamicValue::UInt8(value) => {
            write!(output, "{value}").map_err(|_| plain_error("format failure"))?
        }
        DynamicValue::UInt16(value) => {
            write!(output, "{value}").map_err(|_| plain_error("format failure"))?
        }
        DynamicValue::UInt32(value) => {
            write!(output, "{value}").map_err(|_| plain_error("format failure"))?
        }
        DynamicValue::UInt64(value) => {
            write!(output, "{value}").map_err(|_| plain_error("format failure"))?
        }
        DynamicValue::Float32(value) => write_float32(*value, output),
        DynamicValue::Float64(value) => write_float(*value, output),
        DynamicValue::Text(value) => write_quoted(value.as_bytes(), true, output),
        DynamicValue::Data(value) => write_quoted(value, false, output),
        DynamicValue::List(Some(value)) => write_list(value, style, depth, output)?,
        DynamicValue::List(None) => output.push_str("[]"),
        DynamicValue::Enum(value) => match value.name() {
            Some(name) => output.push_str(name),
            None => {
                write!(output, "{}", value.ordinal).map_err(|_| plain_error("format failure"))?
            }
        },
        DynamicValue::Struct(Some(value)) => write_struct(value, style, depth, output)?,
        DynamicValue::Struct(None) => output.push_str("()"),
        DynamicValue::Capability(None) => output.push_str("null"),
        DynamicValue::Capability(Some(index)) => {
            write!(output, "capability({index})").map_err(|_| plain_error("format failure"))?;
        }
        DynamicValue::AnyPointer(value) => match value {
            DynamicAnyPointer::Null => output.push_str("null"),
            DynamicAnyPointer::Struct(_) => output.push_str("anyPointer(struct)"),
            DynamicAnyPointer::List(_) => output.push_str("anyPointer(list)"),
            DynamicAnyPointer::Capability(index) => {
                write!(output, "anyPointer(capability {index})")
                    .map_err(|_| plain_error("format failure"))?;
            }
        },
    }
    Ok(())
}

fn write_float(value: f64, output: &mut String) {
    if value.is_nan() {
        output.push_str("nan");
    } else if value == f64::INFINITY {
        output.push_str("inf");
    } else if value == f64::NEG_INFINITY {
        output.push_str("-inf");
    } else if value == 0.0 && value.is_sign_negative() {
        output.push_str("-0");
    } else {
        write_finite_float(format!("{value:?}"), output);
    }
}

fn write_float32(value: f32, output: &mut String) {
    if value.is_nan() {
        output.push_str("nan");
    } else if value == f32::INFINITY {
        output.push_str("inf");
    } else if value == f32::NEG_INFINITY {
        output.push_str("-inf");
    } else if value == 0.0 && value.is_sign_negative() {
        output.push_str("-0");
    } else {
        write_finite_float(format!("{value:?}"), output);
    }
}

fn write_finite_float(value: String, output: &mut String) {
    output.push_str(value.strip_suffix(".0").unwrap_or(&value));
}

fn write_quoted(bytes: &[u8], text: bool, output: &mut String) {
    output.push('"');
    if text {
        if let Ok(value) = std::str::from_utf8(bytes) {
            for character in value.chars() {
                match character {
                    '\'' => output.push_str("\\'"),
                    '"' => output.push_str("\\\""),
                    '\\' => output.push_str("\\\\"),
                    '\n' => output.push_str("\\n"),
                    '\r' => output.push_str("\\r"),
                    '\t' => output.push_str("\\t"),
                    value if value.is_control() => {
                        for byte in value.to_string().bytes() {
                            let _ = write!(output, "\\{byte:03o}");
                        }
                    }
                    value => output.push(value),
                }
            }
            output.push('"');
            return;
        }
    }
    for byte in bytes {
        match *byte {
            b'\'' => output.push_str("\\'"),
            b'"' => output.push_str("\\\""),
            b'\\' => output.push_str("\\\\"),
            b'\n' => output.push_str("\\n"),
            b'\r' => output.push_str("\\r"),
            b'\t' => output.push_str("\\t"),
            0x20..=0x7e => output.push(char::from(*byte)),
            value => {
                let _ = write!(output, "\\{value:03o}");
            }
        }
    }
    output.push('"');
}

fn indent(output: &mut String, depth: usize) {
    for _ in 0..depth {
        output.push_str("  ");
    }
}

fn plain_error(message: impl Into<String>) -> TextError {
    TextError {
        byte: 0,
        line: 1,
        column: 1,
        message: message.into(),
    }
}

fn error_at(source: &str, byte: usize, message: impl Into<String>) -> TextError {
    let byte = byte.min(source.len());
    let before = &source[..byte];
    let line = before.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let line_start = before.rfind('\n').map_or(0, |index| index + 1);
    let column = source[line_start..byte].chars().count() + 1;
    TextError {
        byte,
        line,
        column,
        message: message.into(),
    }
}

struct Parser<'a> {
    source: &'a str,
    offset: usize,
    values: usize,
    limits: TextLimits,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str, limits: TextLimits) -> Result<Self, TextError> {
        if source.len() > limits.max_input_bytes {
            return Err(error_at(
                source,
                limits.max_input_bytes.min(source.len()),
                format!("text input exceeds {} bytes", limits.max_input_bytes),
            ));
        }
        Ok(Self {
            source,
            offset: 0,
            values: 0,
            limits,
        })
    }

    fn parse_all(mut self) -> Result<Vec<SpannedValue>, TextError> {
        let mut output = Vec::new();
        self.skip_trivia();
        while self.offset < self.source.len() {
            output.push(self.parse_value(0)?);
            self.skip_trivia();
        }
        Ok(output)
    }

    fn parse_value(&mut self, depth: u16) -> Result<SpannedValue, TextError> {
        if depth > self.limits.max_nesting {
            return Err(error_at(
                self.source,
                self.offset,
                "text nesting limit exceeded",
            ));
        }
        self.values = self.values.saturating_add(1);
        if self.values > self.limits.max_values {
            return Err(error_at(
                self.source,
                self.offset,
                "text value limit exceeded",
            ));
        }
        self.skip_trivia();
        let start = self.offset;
        let Some(byte) = self.peek() else {
            return Err(error_at(self.source, start, "expected a value"));
        };
        let value = match byte {
            b'(' => ParsedValue::Struct(self.parse_struct(depth + 1)?),
            b'[' => ParsedValue::List(self.parse_list(depth + 1)?),
            b'"' => ParsedValue::Bytes(self.parse_quoted()?),
            b'0' if self.source.as_bytes().get(self.offset + 1) == Some(&b'x')
                && self.source.as_bytes().get(self.offset + 2) == Some(&b'"') =>
            {
                self.offset += 2;
                ParsedValue::Bytes(self.parse_hex_bytes()?)
            }
            b'-' | b'+' | b'0'..=b'9' => ParsedValue::Number(self.parse_atom()),
            _ if is_identifier_start(byte) => {
                let atom = self.parse_atom();
                match atom.as_str() {
                    "void" => ParsedValue::Void,
                    "true" => ParsedValue::Bool(true),
                    "false" => ParsedValue::Bool(false),
                    "null" => ParsedValue::Null,
                    _ => ParsedValue::Identifier(atom),
                }
            }
            _ => return Err(error_at(self.source, start, "unexpected character")),
        };
        Ok(SpannedValue { start, value })
    }

    fn parse_struct(&mut self, depth: u16) -> Result<Vec<ParsedField>, TextError> {
        self.expect(b'(')?;
        let mut output = Vec::new();
        self.skip_trivia();
        if self.consume(b')') {
            return Ok(output);
        }
        loop {
            self.skip_trivia();
            let start = self.offset;
            let name = self.parse_identifier()?;
            self.skip_trivia();
            self.expect(b'=')?;
            let value = self.parse_value(depth)?;
            output.push(ParsedField { start, name, value });
            self.skip_trivia();
            if self.consume(b')') {
                break;
            }
            self.expect(b',')?;
            self.skip_trivia();
            if self.consume(b')') {
                break;
            }
        }
        Ok(output)
    }

    fn parse_list(&mut self, depth: u16) -> Result<Vec<SpannedValue>, TextError> {
        self.expect(b'[')?;
        let mut output = Vec::new();
        self.skip_trivia();
        if self.consume(b']') {
            return Ok(output);
        }
        loop {
            output.push(self.parse_value(depth)?);
            self.skip_trivia();
            if self.consume(b']') {
                break;
            }
            self.expect(b',')?;
            self.skip_trivia();
            if self.consume(b']') {
                break;
            }
        }
        Ok(output)
    }

    fn parse_quoted(&mut self) -> Result<Vec<u8>, TextError> {
        self.expect(b'"')?;
        let mut output = Vec::new();
        while let Some(byte) = self.peek() {
            if byte == b'"' {
                self.offset += 1;
                return Ok(output);
            }
            if byte != b'\\' {
                let character = self.source[self.offset..]
                    .chars()
                    .next()
                    .ok_or_else(|| error_at(self.source, self.offset, "invalid UTF-8 boundary"))?;
                let mut encoded = [0u8; 4];
                output.extend_from_slice(character.encode_utf8(&mut encoded).as_bytes());
                self.offset += character.len_utf8();
                continue;
            }
            let escape_start = self.offset;
            self.offset += 1;
            let escaped = self
                .peek()
                .ok_or_else(|| error_at(self.source, escape_start, "unterminated escape"))?;
            self.offset += 1;
            match escaped {
                b'a' => output.push(7),
                b'b' => output.push(8),
                b't' => output.push(b'\t'),
                b'n' => output.push(b'\n'),
                b'v' => output.push(11),
                b'f' => output.push(12),
                b'r' => output.push(b'\r'),
                b'\\' | b'\'' | b'"' => output.push(escaped),
                b'x' => {
                    let high = self.take_hex(escape_start)?;
                    let low = self.take_hex(escape_start)?;
                    output.push((high << 4) | low);
                }
                b'0'..=b'7' => {
                    let mut value = u16::from(escaped - b'0');
                    for _ in 0..2 {
                        match self.peek() {
                            Some(next @ b'0'..=b'7') => {
                                self.offset += 1;
                                value = value * 8 + u16::from(next - b'0');
                            }
                            _ => break,
                        }
                    }
                    output.push(u8::try_from(value).map_err(|_| {
                        error_at(self.source, escape_start, "octal escape exceeds one byte")
                    })?);
                }
                _ => return Err(error_at(self.source, escape_start, "unknown string escape")),
            }
        }
        Err(error_at(self.source, self.offset, "unterminated string"))
    }

    fn parse_hex_bytes(&mut self) -> Result<Vec<u8>, TextError> {
        let start = self.offset;
        self.expect(b'"')?;
        let mut digits = Vec::new();
        while let Some(byte) = self.peek() {
            if byte == b'"' {
                self.offset += 1;
                if digits.len() % 2 != 0 {
                    return Err(error_at(
                        self.source,
                        start,
                        "hex data needs pairs of digits",
                    ));
                }
                return Ok(digits
                    .chunks_exact(2)
                    .map(|pair| (pair[0] << 4) | pair[1])
                    .collect());
            }
            if byte.is_ascii_whitespace() {
                self.offset += 1;
            } else {
                digits.push(self.take_hex(start)?);
            }
        }
        Err(error_at(self.source, start, "unterminated hex data"))
    }

    fn take_hex(&mut self, start: usize) -> Result<u8, TextError> {
        let byte = self
            .peek()
            .ok_or_else(|| error_at(self.source, start, "incomplete hexadecimal escape"))?;
        let value = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            _ => {
                return Err(error_at(
                    self.source,
                    self.offset,
                    "expected hexadecimal digit",
                ));
            }
        };
        self.offset += 1;
        Ok(value)
    }

    fn parse_identifier(&mut self) -> Result<String, TextError> {
        let start = self.offset;
        if !self.peek().is_some_and(is_identifier_start) {
            return Err(error_at(self.source, start, "expected a field name"));
        }
        self.offset += 1;
        while self.peek().is_some_and(is_identifier_continue) {
            self.offset += 1;
        }
        Ok(self.source[start..self.offset].to_owned())
    }

    fn parse_atom(&mut self) -> String {
        let start = self.offset;
        while self
            .peek()
            .is_some_and(|byte| !byte.is_ascii_whitespace() && !matches!(byte, b',' | b')' | b']'))
        {
            self.offset += 1;
        }
        self.source[start..self.offset].to_owned()
    }

    fn skip_trivia(&mut self) {
        loop {
            while self.peek().is_some_and(|byte| byte.is_ascii_whitespace()) {
                self.offset += 1;
            }
            if self.source.as_bytes().get(self.offset..self.offset + 2) == Some(b"//") {
                self.offset += 2;
                while self.peek().is_some_and(|byte| byte != b'\n') {
                    self.offset += 1;
                }
                continue;
            }
            if self.peek() == Some(b'#') {
                while self.peek().is_some_and(|byte| byte != b'\n') {
                    self.offset += 1;
                }
                continue;
            }
            break;
        }
    }

    fn expect(&mut self, expected: u8) -> Result<(), TextError> {
        if self.consume(expected) {
            Ok(())
        } else {
            Err(error_at(
                self.source,
                self.offset,
                format!("expected `{}`", char::from(expected)),
            ))
        }
    }

    fn consume(&mut self, expected: u8) -> bool {
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
}

fn is_identifier_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_identifier_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;
    use capnp_io::{FrameLimits, FrameRead, encode_frame, parse_frame};

    const WIRE_TYPE: NodeId = 0x99c9_abad_7396_3922;
    const LANGUAGE_REQUEST: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../conformance/fixtures/cpp/",
        "e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/",
        "compiler-request-language-fixture.bin"
    ));
    const WIRE_REQUEST: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../conformance/fixtures/cpp/",
        "e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/",
        "compiler-request-wire-fixture.bin"
    ));
    const WIRE_FRAME: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../conformance/fixtures/cpp/",
        "e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/",
        "wire-unpacked.bin"
    ));
    const WIRE_SOURCE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../conformance/fixtures/source/wire-fixture.txt"
    ));

    fn schema(bytes: &[u8]) -> Arc<CompiledSchema> {
        Arc::new(
            CompiledSchema::from_code_generator_request(bytes, capnp_schema::LoadLimits::default())
                .expect("schema request loads"),
        )
    }

    fn frame_segments(bytes: &[u8]) -> Vec<Arc<[u8]>> {
        let FrameRead::Message { frame, remaining } =
            parse_frame(bytes, FrameLimits::default()).expect("frame parses")
        else {
            assert!(matches!(
                parse_frame(bytes, FrameLimits::default()),
                Ok(FrameRead::Message { .. })
            ));
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
    fn pinned_cpp_text_corpus_decodes_and_round_trips_exactly() {
        let schema = schema(WIRE_REQUEST);
        let source_encoded = encode_structs(&schema, WIRE_TYPE, WIRE_SOURCE, TextLimits::default())
            .expect("pinned source text encodes");
        let segments = source_encoded[0]
            .segments
            .iter()
            .map(AsRef::as_ref)
            .collect::<Vec<&[u8]>>();
        assert_eq!(
            encode_frame(&segments, FrameLimits::default()).expect("frame encodes"),
            WIRE_FRAME
        );
        let expected = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../conformance/fixtures/text/wire-short.txt"
        ))
        .trim_end();
        let actual = format_message(
            Arc::clone(&schema),
            WIRE_TYPE,
            frame_segments(WIRE_FRAME),
            FormatStyle::Short,
            ReaderLimits::default(),
        )
        .expect("fixture formats");
        assert_eq!(actual, expected);

        let pretty = format_message(
            Arc::clone(&schema),
            WIRE_TYPE,
            frame_segments(WIRE_FRAME),
            FormatStyle::Pretty,
            ReaderLimits::default(),
        )
        .expect("fixture pretty-prints");
        let pretty_encoded = encode_structs(&schema, WIRE_TYPE, &pretty, TextLimits::default())
            .expect("pretty text parses");
        let pretty_round_trip = format_message(
            Arc::clone(&schema),
            WIRE_TYPE,
            pretty_encoded[0]
                .segments
                .iter()
                .map(|segment| Arc::<[u8]>::from(segment.as_ref())),
            FormatStyle::Short,
            ReaderLimits::default(),
        )
        .expect("pretty round trip formats");
        assert_eq!(pretty_round_trip, expected);

        let encoded = encode_structs(&schema, WIRE_TYPE, expected, TextLimits::default())
            .expect("reference text encodes");
        assert_eq!(encoded.len(), 1);
        let round_trip = format_message(
            Arc::clone(&schema),
            WIRE_TYPE,
            encoded[0]
                .segments
                .iter()
                .map(|segment| Arc::<[u8]>::from(segment.as_ref())),
            FormatStyle::Short,
            ReaderLimits::default(),
        )
        .expect("native message formats");
        assert_eq!(round_trip, expected);
    }

    #[test]
    fn formatter_uses_code_order_even_if_storage_order_changes() {
        let original = schema(WIRE_REQUEST);
        let mut nodes = original.nodes().to_vec();
        let structure = nodes
            .iter_mut()
            .find(|node| node.id == WIRE_TYPE)
            .and_then(|node| match &mut node.kind {
                NodeKind::Struct(value) => Some(value),
                _ => None,
            })
            .expect("wire struct");
        structure.fields.reverse();
        let reordered = Arc::new(
            CompiledSchema::from_parts(
                original.version,
                nodes,
                original.source_infos().to_vec(),
                original.requested_files().to_vec(),
            )
            .expect("reordered schema remains valid"),
        );
        let formatted = format_message(
            reordered,
            WIRE_TYPE,
            frame_segments(WIRE_FRAME),
            FormatStyle::Short,
            ReaderLimits::default(),
        )
        .expect("reordered schema formats");
        assert!(formatted.starts_with("(voidValue = void, boolValue = true"));
    }

    #[test]
    fn diagnostics_have_locations_and_limits_fail_before_building() {
        let schema = schema(WIRE_REQUEST);
        let error = encode_structs(
            &schema,
            WIRE_TYPE,
            "(\n  boolValue = maybe)",
            TextLimits::default(),
        )
        .expect_err("invalid bool is rejected");
        assert_eq!((error.line, error.column), (2, 15));
        assert!(error.message.contains("true or false"));

        let limits = TextLimits {
            max_values: 2,
            ..TextLimits::default()
        };
        let error = encode_structs(&schema, WIRE_TYPE, "(bools = [true, false])", limits)
            .expect_err("value limit is enforced");
        assert!(error.message.contains("value limit"));

        let error = encode_structs(
            &schema,
            WIRE_TYPE,
            "(data = \"\\777\")",
            TextLimits::default(),
        )
        .expect_err("oversized octal escape is rejected");
        assert!(error.message.contains("one byte"));
    }

    #[test]
    fn deterministic_arbitrary_text_never_panics_under_limits() {
        let schema = schema(WIRE_REQUEST);
        let limits = TextLimits {
            max_input_bytes: 512,
            max_values: 128,
            max_nesting: 16,
            max_message_words: 4096,
        };
        let alphabet = b"()[]=, abcdef0123456789\\\"\n[]#";
        let mut state = 0x8b8b_8b8b_1234_5678_u64;
        for length in 0..512 {
            let mut source = String::with_capacity(length);
            for _ in 0..length {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                source.push(char::from(alphabet[state as usize % alphabet.len()]));
            }
            let _ = encode_structs(&schema, WIRE_TYPE, &source, limits);
        }
    }

    #[test]
    fn nested_constants_lists_and_field_defaults_evaluate() {
        let schema = schema(LANGUAGE_REQUEST);
        let scope = schema.requested_files()[0].id;
        assert_eq!(
            evaluate(
                Arc::clone(&schema),
                scope,
                "LanguageFixture.answer",
                FormatStyle::Short,
            )
            .expect("nested constant"),
            "42"
        );
        assert_eq!(
            evaluate(
                Arc::clone(&schema),
                scope,
                "LanguageFixture.primes[3]",
                FormatStyle::Short,
            )
            .expect("constant list item"),
            "7"
        );
        assert_eq!(
            evaluate(
                Arc::clone(&schema),
                scope,
                "LanguageFixture.sampleBox.value",
                FormatStyle::Short,
            )
            .expect("constant struct member"),
            "\"constant generic struct\""
        );
        assert_eq!(
            evaluate(
                Arc::clone(&schema),
                scope,
                "LanguageFixture.state",
                FormatStyle::Short,
            )
            .expect("enum field default"),
            "ready"
        );

        let binary = evaluate_struct_message(
            schema,
            scope,
            "LanguageFixture.sampleBox",
            TextLimits::default(),
        )
        .expect("branded struct constant evaluates to binary");
        assert_eq!(binary.segments.len(), 1);
        assert!(binary.segments[0].len() >= 16);
    }
}
