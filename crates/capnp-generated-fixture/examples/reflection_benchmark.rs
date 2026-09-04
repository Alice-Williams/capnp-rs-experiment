use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

use capnp_generated_fixture::wire::wire_fixture;
use capnp_io::{FrameLimits, FrameRead, parse_frame};
use capnp_message::{OwnedMessage, ReaderLimits};
use capnp_schema::{CompiledSchema, DynamicStruct, DynamicValue, LoadLimits, NodeKind};

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
        "usage: reflection_benchmark schema-name|schema-index|dynamic-name|dynamic-index|dynamic-field|dynamic-blobs-borrowed|dynamic-blobs-owned PASSES",
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
            "dynamic-blobs-borrowed" | "dynamic-blobs-owned" => {
                dynamic_blobs(&dynamic, text_field, data_field)?
            }
            _ => unreachable!(),
        };
        checksum = checksum.rotate_left(9) ^ observed;
    }
    println!("{}\t{}", started.elapsed().as_nanos(), black_box(checksum));
    Ok(())
}

fn dynamic_blobs(
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
