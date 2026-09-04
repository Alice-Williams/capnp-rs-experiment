use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

use capnp_generated_fixture::wire::wire_fixture;
use capnp_io::{FrameLimits, FrameRead, parse_frame};
use capnp_message::{ExclusiveArena, OwnedMessage, ReaderLimits};
use capnp_schema::{
    CompiledSchema, DynamicScalarValue, DynamicStruct, DynamicValue, FieldKind, LoadLimits,
    NodeKind, StructSchema,
};

const SEED: u64 = 0x4d59_5df4_d0f3_3173;
const SCALAR_NAMES: [&str; 4] = ["uint8Value", "uint16Value", "uint32Value", "uint64Value"];
const REQUEST: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../conformance/fixtures/cpp/",
    "e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/",
    "compiler-request-wire-fixture.bin"
));
const FRAME: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../conformance/fixtures/cpp/",
    "e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/",
    "wire-unpacked.bin"
));

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mode = args.next().ok_or(
        "usage: reflection_benchmark schema-name|schema-index|dynamic-name|dynamic-index|dynamic-field|dynamic-blobs-borrowed|dynamic-blobs-owned|dynamic-primitive-list|dynamic-nested-struct|dynamic-struct-list|dynamic-nested-list|dynamic-enum|dynamic-default|dynamic-union-active|dynamic-union-unknown PASSES",
    )?;
    let passes = args.next().ok_or("missing passes")?.parse::<usize>()?;
    if args.next().is_some()
        || passes == 0
        || !matches!(
            mode.as_str(),
            "schema-name"
                | "schema-index"
                | "dynamic-name"
                | "dynamic-index"
                | "dynamic-field"
                | "dynamic-blobs-borrowed"
                | "dynamic-blobs-owned"
                | "dynamic-primitive-list"
                | "dynamic-nested-struct"
                | "dynamic-struct-list"
                | "dynamic-nested-list"
                | "dynamic-enum"
                | "dynamic-default"
                | "dynamic-union-active"
                | "dynamic-union-unknown"
        )
    {
        return Err("expected a known mode and positive PASSES".into());
    }

    let schema = Arc::new(CompiledSchema::from_code_generator_request(
        REQUEST,
        LoadLimits::default(),
    )?);
    let node = schema
        .node(wire_fixture::TYPE_ID)
        .ok_or("WireFixture schema is missing")?;
    let NodeKind::Struct(structure) = &node.kind else {
        return Err("WireFixture is not a struct".into());
    };
    let field_indexes = SCALAR_NAMES.map(|name| {
        structure
            .fields
            .iter()
            .position(|field| field.name == name)
            .expect("benchmark field is present")
    });
    let message = owned_frame()?;
    let dynamic = DynamicStruct::root(
        Arc::clone(&schema),
        Arc::clone(&message),
        wire_fixture::TYPE_ID,
    )?;
    let dynamic_fields = SCALAR_NAMES.map(|name| {
        dynamic
            .field(name)
            .expect("benchmark dynamic field is present")
    });
    let text_field = dynamic.field("text")?;
    let data_field = dynamic.field("data")?;
    let typed_text_field = dynamic.text_field("text")?;
    let typed_data_field = dynamic.data_field("data")?;
    let primitive_list_field = dynamic.list_field("uint16s")?;
    let nested_struct_field = dynamic.struct_field("node")?;
    let struct_list_field = dynamic.list_field("structs")?;
    let nested_list_field = dynamic.list_field("nestedLists")?;
    let color_field = dynamic.scalar_field("color")?;
    let defaulted_field = dynamic.scalar_field("defaulted")?;
    let choice_field = dynamic.struct_field("choice")?;
    let DynamicValue::Struct(Some(nested_probe)) = dynamic.get("node")? else {
        return Err("dynamic nested struct field has the wrong type".into());
    };
    let nested_value_field = nested_probe.field("value")?;
    let DynamicValue::Struct(Some(choice_probe)) = dynamic.get("choice")? else {
        return Err("dynamic union group has the wrong type".into());
    };
    let choice_number_field = choice_probe.scalar_field("number")?;
    let choice_schema = group_schema(&schema, structure, "choice")?;
    let unknown_message = unknown_union_message(structure, choice_schema.discriminant_offset)?;
    let unknown_dynamic =
        DynamicStruct::root(Arc::clone(&schema), unknown_message, wire_fixture::TYPE_ID)?;

    let started = Instant::now();
    let mut checksum = SEED;
    for pass in 0..passes {
        let selector = pass & 3;
        let observed = match mode.as_str() {
            "schema-name" => {
                let name = black_box(SCALAR_NAMES[selector]);
                u64::from(
                    black_box(structure)
                        .field(name)
                        .ok_or("benchmark field lookup failed")?
                        .code_order,
                )
            }
            "schema-index" => u64::from(
                black_box(structure)
                    .field_by_index(black_box(field_indexes[selector]))
                    .ok_or("benchmark field index failed")?
                    .code_order,
            ),
            "dynamic-name" => dynamic_scalar(
                black_box(&dynamic).get(black_box(SCALAR_NAMES[selector]))?,
                selector,
            )?,
            "dynamic-index" => dynamic_scalar(
                black_box(&dynamic).get_by_index(black_box(field_indexes[selector]))?,
                selector,
            )?,
            "dynamic-field" => dynamic_scalar(
                black_box(&dynamic).get_field(black_box(dynamic_fields[selector]))?,
                selector,
            )?,
            "dynamic-blobs-borrowed" => black_box(&dynamic).with_view(|view| {
                view.with_text(&typed_text_field, |text| {
                    view.with_data(&typed_data_field, |data| {
                        blob_fingerprint(text.as_bytes(), data)
                    })
                })?
            })?,
            "dynamic-blobs-owned" => dynamic_blobs_owned(&dynamic, text_field, data_field)?,
            "dynamic-primitive-list" => black_box(&dynamic).with_view(|view| {
                view.with_list(&primitive_list_field, |list| {
                    Ok(u64::from(list.get_u16(2)?))
                })
            })?,
            "dynamic-nested-struct" => black_box(&dynamic).with_view(|view| {
                view.with_struct(&nested_struct_field, |child| {
                    nested_u32(&child, nested_value_field)
                })
            })?,
            "dynamic-struct-list" => black_box(&dynamic).with_view(|view| {
                view.with_list(&struct_list_field, |list| {
                    list.with_struct(1, |child| nested_u32(&child, nested_value_field))
                })
            })?,
            "dynamic-nested-list" => black_box(&dynamic).with_view(|view| {
                view.with_list(&nested_list_field, |outer| {
                    outer.with_list(0, |inner| Ok(u64::from(inner.get_u16(2)?)))
                })
            })?,
            "dynamic-enum" => {
                let DynamicScalarValue::Enum { ordinal, .. } =
                    black_box(&dynamic).get_scalar(&color_field)?
                else {
                    return Err("dynamic enum field has the wrong type".into());
                };
                u64::from(ordinal)
            }
            "dynamic-default" => {
                let DynamicScalarValue::UInt32(value) =
                    black_box(&dynamic).get_scalar(&defaulted_field)?
                else {
                    return Err("dynamic default field has the wrong type".into());
                };
                u64::from(value)
            }
            "dynamic-union-active" => black_box(&dynamic).with_view(|view| {
                view.with_struct(&choice_field, |choice| {
                    let active = choice.active_union_field()?.ok_or(
                        capnp_schema::DynamicError::TypeMismatch {
                            expected: "known dynamic union field",
                        },
                    )?;
                    let discriminant = active.discriminant_value.ok_or(
                        capnp_schema::DynamicError::TypeMismatch {
                            expected: "dynamic union discriminant",
                        },
                    )?;
                    let DynamicScalarValue::UInt64(value) =
                        choice.get_scalar(&choice_number_field)?
                    else {
                        return Err(capnp_schema::DynamicError::TypeMismatch {
                            expected: "UInt64 dynamic union value",
                        });
                    };
                    Ok(u64::from(discriminant).rotate_left(17) ^ value)
                })
            })?,
            "dynamic-union-unknown" => black_box(&unknown_dynamic).with_view(|view| {
                view.with_struct(&choice_field, |choice| {
                    if choice.active_union_field()?.is_some() {
                        return Err(capnp_schema::DynamicError::TypeMismatch {
                            expected: "unknown dynamic union field",
                        });
                    }
                    choice.union_discriminant()?.map(u64::from).ok_or(
                        capnp_schema::DynamicError::TypeMismatch {
                            expected: "raw dynamic union discriminant",
                        },
                    )
                })
            })?,
            _ => unreachable!(),
        };
        checksum = checksum.rotate_left(9) ^ observed;
    }
    println!("{}\t{}", started.elapsed().as_nanos(), black_box(checksum));
    Ok(())
}

fn group_schema<'schema>(
    schema: &'schema CompiledSchema,
    structure: &StructSchema,
    name: &str,
) -> Result<&'schema StructSchema, Box<dyn std::error::Error>> {
    let FieldKind::Group { type_id } = structure
        .field(name)
        .ok_or("benchmark group field is missing")?
        .kind
    else {
        return Err("benchmark field is not a group".into());
    };
    let NodeKind::Struct(group) = &schema
        .node(type_id)
        .ok_or("benchmark group schema is missing")?
        .kind
    else {
        return Err("benchmark group schema is not a struct".into());
    };
    Ok(group)
}

fn unknown_union_message(
    structure: &StructSchema,
    discriminant_offset: u32,
) -> Result<Arc<OwnedMessage>, Box<dyn std::error::Error>> {
    let mut arena = ExclusiveArena::new(8, 256)?;
    arena
        .init_root_struct(structure.data_word_count, structure.pointer_count)?
        .set_u16(discriminant_offset, 55, 0)?;
    Ok(OwnedMessage::new(
        arena.into_segments(),
        ReaderLimits {
            traversal_words: u64::MAX,
            nesting_levels: 64,
        },
    )?)
}

#[inline(always)]
fn nested_u32(
    child: &capnp_schema::DynamicStructView<'_>,
    field: capnp_schema::DynamicField<'_>,
) -> Result<u64, capnp_schema::DynamicError> {
    let DynamicValue::UInt32(value) = child.get_scalar_field(field)? else {
        return Err(capnp_schema::DynamicError::TypeMismatch {
            expected: "UInt32 benchmark field",
        });
    };
    Ok(u64::from(value))
}

#[inline(always)]
fn dynamic_blobs_owned(
    dynamic: &DynamicStruct,
    text_field: capnp_schema::DynamicField<'_>,
    data_field: capnp_schema::DynamicField<'_>,
) -> Result<u64, Box<dyn std::error::Error>> {
    let DynamicValue::Text(text) = black_box(dynamic).get_field(text_field)? else {
        return Err("dynamic Text field has the wrong type".into());
    };
    let DynamicValue::Data(data) = black_box(dynamic).get_field(data_field)? else {
        return Err("dynamic Data field has the wrong type".into());
    };
    Ok(blob_fingerprint(text.as_bytes(), &data))
}

fn blob_fingerprint(text: &[u8], data: &[u8]) -> u64 {
    let mut value = (text.len() as u64).rotate_left(11) ^ (data.len() as u64).rotate_left(23);
    if let (Some(first), Some(last)) = (text.first(), text.last()) {
        value ^= u64::from(*first).rotate_left(31) ^ u64::from(*last).rotate_left(37);
    }
    if let (Some(first), Some(last)) = (data.first(), data.last()) {
        value ^= u64::from(*first).rotate_left(43) ^ u64::from(*last).rotate_left(47);
    }
    value
}

fn dynamic_scalar(value: DynamicValue, selector: usize) -> Result<u64, &'static str> {
    match (selector, value) {
        (0, DynamicValue::UInt8(value)) => Ok(u64::from(value)),
        (1, DynamicValue::UInt16(value)) => Ok(u64::from(value)),
        (2, DynamicValue::UInt32(value)) => Ok(u64::from(value)),
        (3, DynamicValue::UInt64(value)) => Ok(value),
        _ => Err("dynamic benchmark value has the wrong type"),
    }
}

fn owned_frame() -> Result<Arc<OwnedMessage>, Box<dyn std::error::Error>> {
    let FrameRead::Message { frame, remaining } = parse_frame(FRAME, FrameLimits::default())?
    else {
        return Err("fixture is empty".into());
    };
    if !remaining.is_empty() {
        return Err("fixture has trailing bytes".into());
    }
    Ok(OwnedMessage::new(
        frame.segments().iter().map(|segment| segment.bytes()),
        ReaderLimits {
            traversal_words: u64::MAX,
            nesting_levels: 64,
        },
    )?)
}
