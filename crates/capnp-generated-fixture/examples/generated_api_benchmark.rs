use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

use capnp_generated_fixture::evolution_v1;
use capnp_generated_fixture::wire::{Color, choice, wire_fixture};
use capnp_io::{FrameLimits, FrameRead, parse_frame};
use capnp_message::{
    BorrowedMessage, ExclusiveArena, OwnedMessage, ReaderLimits, StructBuilder, StructReadError,
};
use capnp_schema::{CompiledSchema, LoadLimits, NodeKind};

const SEED: u64 = 0x4d59_5df4_d0f3_3173;
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
const EVOLUTION_FRAME: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../conformance/fixtures/cpp/",
    "e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/",
    "evolution-v2-unpacked.bin"
));
const EMPTY_FRAME: &[u8] = &[0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mode = args.next().ok_or(
        "usage: generated_api_benchmark direct-scalars|generated-scalars|borrowed-direct-scalars|borrowed-scalars|direct-blobs|generated-blobs|borrowed-direct-blobs|borrowed-blobs|borrowed-direct-groups|borrowed-groups|borrowed-direct-lists|borrowed-lists|borrowed-direct-nested|borrowed-nested|borrowed-direct-struct-lists|borrowed-struct-lists|borrowed-direct-evolution|borrowed-evolution|borrowed-direct-defaults|borrowed-defaults|direct-builder-scalars|generated-builder-scalars|direct-builder-blobs|generated-builder-blobs|direct-builder-struct|generated-builder-struct|direct-builder-list|generated-builder-list PASSES",
    )?;
    let passes = args.next().ok_or("missing passes")?.parse::<usize>()?;
    if args.next().is_some()
        || passes == 0
        || !matches!(
            mode.as_str(),
            "direct-scalars"
                | "generated-scalars"
                | "borrowed-direct-scalars"
                | "borrowed-scalars"
                | "direct-blobs"
                | "generated-blobs"
                | "borrowed-direct-blobs"
                | "borrowed-blobs"
                | "borrowed-direct-groups"
                | "borrowed-groups"
                | "borrowed-direct-lists"
                | "borrowed-lists"
                | "borrowed-direct-nested"
                | "borrowed-nested"
                | "borrowed-direct-struct-lists"
                | "borrowed-struct-lists"
                | "borrowed-direct-evolution"
                | "borrowed-evolution"
                | "borrowed-direct-defaults"
                | "borrowed-defaults"
                | "direct-builder-scalars"
                | "generated-builder-scalars"
                | "direct-builder-blobs"
                | "generated-builder-blobs"
                | "direct-builder-struct"
                | "generated-builder-struct"
                | "direct-builder-list"
                | "generated-builder-list"
        )
    {
        return Err("expected a known mode and positive PASSES".into());
    }

    let schema = Arc::new(CompiledSchema::from_code_generator_request(
        REQUEST,
        LoadLimits::default(),
    )?);
    if mode.contains("builder-") {
        return run_builder_benchmark(&mode, passes, &schema);
    }
    let FrameRead::Message { frame, remaining } = parse_frame(FRAME, FrameLimits::default())?
    else {
        return Err("fixture is empty".into());
    };
    if !remaining.is_empty() {
        return Err("fixture has trailing bytes".into());
    }
    let borrowed_segments = frame
        .segments()
        .iter()
        .map(|segment| segment.bytes())
        .collect::<Vec<_>>();
    let borrowed_message = BorrowedMessage::new(
        &borrowed_segments,
        ReaderLimits {
            traversal_words: u64::MAX,
            nesting_levels: 64,
        },
    )?;
    let borrowed_direct = borrowed_message.root_struct()?;
    let borrowed = wire_fixture::BorrowedReader::from_root(&borrowed_message)?;
    let evolution_message = parse_borrowed_message(EVOLUTION_FRAME)?;
    let evolution_direct = evolution_message.root_struct()?;
    let evolution = evolution_v1::record::BorrowedReader::from_root(&evolution_message)?;
    let default_message = parse_borrowed_message(EMPTY_FRAME)?;
    let default_direct = default_message.root_struct()?;
    let default_reader = wire_fixture::BorrowedReader::from_root(&default_message)?;
    let message = owned_frame()?;
    let direct = message.root_struct()?.into_root();
    let generated = wire_fixture::Reader::from_root(schema, Arc::clone(&message))?;

    let started = Instant::now();
    let checksum = match mode.as_str() {
        "direct-scalars" => measure(passes, || {
            direct
                .with_reader(direct_scalar_fingerprint)?
                .map_err(Into::into)
        })?,
        "generated-scalars" => measure(passes, || generated_scalar_fingerprint(&generated))?,
        "borrowed-direct-scalars" => measure(passes, || {
            direct_scalar_fingerprint(borrowed_direct).map_err(Into::into)
        })?,
        "borrowed-scalars" => measure(passes, || borrowed_scalar_fingerprint(&borrowed))?,
        "direct-blobs" => measure(passes, || {
            direct
                .with_reader(direct_blob_fingerprint)?
                .map_err(Into::into)
        })?,
        "generated-blobs" => measure(passes, || generated_blob_fingerprint(&generated))?,
        "borrowed-direct-blobs" => measure(passes, || {
            direct_blob_fingerprint(borrowed_direct).map_err(Into::into)
        })?,
        "borrowed-blobs" => measure(passes, || borrowed_blob_fingerprint(&borrowed))?,
        "borrowed-direct-groups" => measure(passes, || {
            direct_group_fingerprint(borrowed_direct).map_err(Into::into)
        })?,
        "borrowed-groups" => measure(passes, || Ok(borrowed_group_fingerprint(&borrowed)))?,
        "borrowed-direct-lists" => measure(passes, || direct_list_fingerprint(borrowed_direct))?,
        "borrowed-lists" => measure(passes, || borrowed_list_fingerprint(&borrowed))?,
        "borrowed-direct-nested" => measure(passes, || {
            direct_nested_fingerprint(borrowed_direct).map_err(Into::into)
        })?,
        "borrowed-nested" => measure(passes, || {
            borrowed_nested_fingerprint(&borrowed).map_err(Into::into)
        })?,
        "borrowed-direct-struct-lists" => {
            measure(passes, || direct_struct_list_fingerprint(borrowed_direct))?
        }
        "borrowed-struct-lists" => measure(passes, || borrowed_struct_list_fingerprint(&borrowed))?,
        "borrowed-direct-evolution" => {
            measure(passes, || direct_evolution_fingerprint(evolution_direct))?
        }
        "borrowed-evolution" => measure(passes, || borrowed_evolution_fingerprint(&evolution))?,
        "borrowed-direct-defaults" => measure(passes, || {
            direct_default_fingerprint(default_direct).map_err(Into::into)
        })?,
        "borrowed-defaults" => measure(passes, || {
            borrowed_default_fingerprint(&default_reader).map_err(Into::into)
        })?,
        _ => unreachable!(),
    };
    println!("{}\t{}", started.elapsed().as_nanos(), checksum);
    Ok(())
}

fn run_builder_benchmark(
    mode: &str,
    passes: usize,
    schema: &CompiledSchema,
) -> Result<(), Box<dyn std::error::Error>> {
    let max_words = if mode.ends_with("builder-blobs")
        || mode.ends_with("builder-struct")
        || mode.ends_with("builder-list")
    {
        u32::try_from(
            passes
                .checked_mul(2)
                .and_then(|words| words.checked_add(64))
                .ok_or("builder benchmark arena size overflow")?,
        )?
    } else {
        1_024
    };
    let mut arena = ExclusiveArena::new(max_words, max_words)?;
    let started;
    let checksum = if mode == "direct-builder-scalars" {
        let node = schema
            .node(wire_fixture::TYPE_ID)
            .ok_or("WireFixture schema is missing")?;
        let NodeKind::Struct(structure) = &node.kind else {
            return Err("WireFixture is not a struct".into());
        };
        let mut builder =
            arena.init_root_struct(structure.data_word_count, structure.pointer_count)?;
        started = Instant::now();
        measure_builder(
            passes,
            &mut builder,
            write_direct_scalars,
            scalar_builder_fingerprint,
        )?
    } else if mode == "generated-builder-scalars" {
        let mut builder = wire_fixture::Builder::init_root(schema, &mut arena)?;
        started = Instant::now();
        measure_builder(
            passes,
            &mut builder,
            write_generated_scalars,
            scalar_builder_fingerprint,
        )?
    } else if mode == "direct-builder-blobs" {
        let node = schema
            .node(wire_fixture::TYPE_ID)
            .ok_or("WireFixture schema is missing")?;
        let NodeKind::Struct(structure) = &node.kind else {
            return Err("WireFixture is not a struct".into());
        };
        let mut builder =
            arena.init_root_struct(structure.data_word_count, structure.pointer_count)?;
        started = Instant::now();
        measure_builder(
            passes,
            &mut builder,
            write_direct_blobs,
            blob_builder_fingerprint,
        )?
    } else if mode == "generated-builder-blobs" {
        let mut builder = wire_fixture::Builder::init_root(schema, &mut arena)?;
        started = Instant::now();
        measure_builder(
            passes,
            &mut builder,
            write_generated_blobs,
            blob_builder_fingerprint,
        )?
    } else if mode == "direct-builder-struct" {
        let node = schema
            .node(wire_fixture::TYPE_ID)
            .ok_or("WireFixture schema is missing")?;
        let NodeKind::Struct(structure) = &node.kind else {
            return Err("WireFixture is not a struct".into());
        };
        let mut builder =
            arena.init_root_struct(structure.data_word_count, structure.pointer_count)?;
        started = Instant::now();
        measure_builder(
            passes,
            &mut builder,
            write_direct_struct,
            struct_builder_fingerprint,
        )?
    } else if mode == "generated-builder-struct" {
        let mut builder = wire_fixture::Builder::init_root(schema, &mut arena)?;
        started = Instant::now();
        measure_builder(
            passes,
            &mut builder,
            write_generated_struct,
            struct_builder_fingerprint,
        )?
    } else if mode == "direct-builder-list" {
        let node = schema
            .node(wire_fixture::TYPE_ID)
            .ok_or("WireFixture schema is missing")?;
        let NodeKind::Struct(structure) = &node.kind else {
            return Err("WireFixture is not a struct".into());
        };
        let mut builder =
            arena.init_root_struct(structure.data_word_count, structure.pointer_count)?;
        started = Instant::now();
        measure_builder(
            passes,
            &mut builder,
            write_direct_list,
            list_builder_fingerprint,
        )?
    } else {
        let mut builder = wire_fixture::Builder::init_root(schema, &mut arena)?;
        started = Instant::now();
        measure_builder(
            passes,
            &mut builder,
            write_generated_list,
            list_builder_fingerprint,
        )?
    };
    println!("{}\t{}", started.elapsed().as_nanos(), checksum);
    Ok(())
}

fn measure_builder<B, E>(
    passes: usize,
    builder: &mut B,
    mut write: impl FnMut(&mut B, usize) -> Result<(), E>,
    fingerprint: impl Fn(usize) -> u64,
) -> Result<u64, E> {
    let mut checksum = SEED;
    for pass in 0..passes {
        write(black_box(&mut *builder), pass)?;
        checksum = checksum.rotate_left(9) ^ fingerprint(pass);
    }
    Ok(black_box(checksum))
}

const BUILDER_TEXT: [&str; 2] = ["capnp-a", "capnp-b"];
const BUILDER_DATA: [[u8; 8]; 2] = [*b"data---a", *b"data---b"];

fn write_direct_blobs(
    builder: &mut StructBuilder<'_>,
    pass: usize,
) -> Result<(), capnp_message::ArenaError> {
    let selected = pass & 1;
    builder.set_text(0, BUILDER_TEXT[selected])?;
    builder.set_data(1, &BUILDER_DATA[selected])
}

fn write_generated_blobs(
    builder: &mut wire_fixture::Builder<'_, '_>,
    pass: usize,
) -> Result<(), capnp_schema::DynamicError> {
    let selected = pass & 1;
    builder.set_text(BUILDER_TEXT[selected])?;
    builder.set_data(&BUILDER_DATA[selected])
}

fn blob_builder_fingerprint(pass: usize) -> u64 {
    let selected = pass & 1;
    let text = BUILDER_TEXT[selected].as_bytes();
    let data = &BUILDER_DATA[selected];
    let mut value = (text.len() as u64).rotate_left(11) ^ (data.len() as u64).rotate_left(23);
    value ^= u64::from(text[0]).rotate_left(31) ^ u64::from(text[text.len() - 1]).rotate_left(37);
    value ^ u64::from(data[0]).rotate_left(43) ^ u64::from(data[data.len() - 1]).rotate_left(47)
}

fn write_direct_struct(
    builder: &mut StructBuilder<'_>,
    pass: usize,
) -> Result<(), capnp_message::ArenaError> {
    let mut node = builder.init_struct(22, 1, 1)?;
    node.set_u32(0, pass as u32, 0)
}

fn write_generated_struct(
    builder: &mut wire_fixture::Builder<'_, '_>,
    pass: usize,
) -> Result<(), capnp_schema::DynamicError> {
    builder.init_node()?.set_value(pass as u32)
}

fn struct_builder_fingerprint(pass: usize) -> u64 {
    (pass as u64).rotate_left(29) ^ 0xae37_c0cc_5acf_02c6
}

fn list_builder_values(pass: usize) -> [u16; 4] {
    let value = pass as u16;
    [
        value,
        value ^ 0x55aa,
        value.rotate_left(3),
        value.wrapping_add(7),
    ]
}

fn write_direct_list(
    builder: &mut StructBuilder<'_>,
    pass: usize,
) -> Result<(), capnp_message::ArenaError> {
    let values = list_builder_values(pass);
    let mut list = builder.init_list::<u16>(9, 4)?;
    for (index, value) in values.into_iter().enumerate() {
        list.set(index as u32, value)?;
    }
    Ok(())
}

fn write_generated_list(
    builder: &mut wire_fixture::Builder<'_, '_>,
    pass: usize,
) -> Result<(), capnp_schema::DynamicError> {
    let values = list_builder_values(pass);
    let mut list = builder.init_uint16s(4)?;
    for (index, value) in values.into_iter().enumerate() {
        list.set(index as u32, capnp_schema::DynamicInput::UInt16(value))?;
    }
    Ok(())
}

fn list_builder_fingerprint(pass: usize) -> u64 {
    let values = list_builder_values(pass);
    let mut fingerprint = u64::from(values[0]);
    fingerprint = fingerprint.rotate_left(11) ^ u64::from(values[1]);
    fingerprint = fingerprint.rotate_left(17) ^ u64::from(values[2]);
    fingerprint.rotate_left(23) ^ u64::from(values[3])
}

fn write_direct_scalars(
    builder: &mut StructBuilder<'_>,
    pass: usize,
) -> Result<(), capnp_message::ArenaError> {
    let values = ScalarBuilderValues::new(pass);
    builder.set_bool(0, values.bool_value, false)?;
    builder.set_i8(1, values.int8_value, 0)?;
    builder.set_i16(1, values.int16_value, 0)?;
    builder.set_i32(1, values.int32_value, 0)?;
    builder.set_i64(1, values.int64_value, 0)?;
    builder.set_u8(16, values.uint8_value, 0)?;
    builder.set_u16(9, values.uint16_value, 0)?;
    builder.set_u32(5, values.uint32_value, 0)?;
    builder.set_u64(3, values.uint64_value, 0)?;
    builder.set_f32(8, values.float32_value, 0.0)?;
    builder.set_f64(5, values.float64_value, 0.0)?;
    builder.set_u16(18, values.color.ordinal(), 0)?;
    builder.set_u32(16, values.defaulted, 123_456)?;
    Ok(())
}

fn write_generated_scalars(
    builder: &mut wire_fixture::Builder<'_, '_>,
    pass: usize,
) -> Result<(), capnp_schema::DynamicError> {
    let values = ScalarBuilderValues::new(pass);
    builder.set_bool_value(values.bool_value)?;
    builder.set_int8_value(values.int8_value)?;
    builder.set_int16_value(values.int16_value)?;
    builder.set_int32_value(values.int32_value)?;
    builder.set_int64_value(values.int64_value)?;
    builder.set_uint8_value(values.uint8_value)?;
    builder.set_uint16_value(values.uint16_value)?;
    builder.set_uint32_value(values.uint32_value)?;
    builder.set_uint64_value(values.uint64_value)?;
    builder.set_float32_value(values.float32_value)?;
    builder.set_float64_value(values.float64_value)?;
    builder.set_color(values.color)?;
    builder.set_defaulted(values.defaulted)?;
    Ok(())
}

struct ScalarBuilderValues {
    raw: u64,
    bool_value: bool,
    int8_value: i8,
    int16_value: i16,
    int32_value: i32,
    int64_value: i64,
    uint8_value: u8,
    uint16_value: u16,
    uint32_value: u32,
    uint64_value: u64,
    float32_value: f32,
    float64_value: f64,
    color: Color,
    defaulted: u32,
}

impl ScalarBuilderValues {
    fn new(pass: usize) -> Self {
        let raw = SEED.wrapping_add((pass as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15));
        Self {
            raw,
            bool_value: raw & 1 != 0,
            int8_value: (raw & 0x7f) as i8,
            int16_value: (raw & 0x7fff) as i16,
            int32_value: (raw & 0x7fff_ffff) as i32,
            int64_value: (raw & 0x7fff_ffff_ffff_ffff) as i64,
            uint8_value: raw as u8,
            uint16_value: raw as u16,
            uint32_value: raw as u32,
            uint64_value: raw,
            float32_value: f32::from_bits(0x3f80_0000 | (raw as u32 & 0x007f_ffff)),
            float64_value: f64::from_bits(0x3ff0_0000_0000_0000 | (raw & 0x000f_ffff_ffff_ffff)),
            color: match raw % 3 {
                0 => Color::Red,
                1 => Color::Green,
                _ => Color::Blue,
            },
            defaulted: (raw >> 17) as u32,
        }
    }
}

fn scalar_builder_fingerprint(pass: usize) -> u64 {
    let values = ScalarBuilderValues::new(pass);
    let mut value = u64::from(values.bool_value);
    value = value.rotate_left(5) ^ values.int8_value as u8 as u64;
    value = value.rotate_left(7) ^ values.int16_value as u16 as u64;
    value = value.rotate_left(11) ^ values.int32_value as u32 as u64;
    value = value.rotate_left(13) ^ values.int64_value as u64;
    value = value.rotate_left(17) ^ u64::from(values.uint8_value);
    value = value.rotate_left(19) ^ u64::from(values.uint16_value);
    value = value.rotate_left(23) ^ u64::from(values.uint32_value);
    value = value.rotate_left(29) ^ values.uint64_value;
    value = value.rotate_left(31) ^ u64::from(values.float32_value.to_bits());
    value = value.rotate_left(37) ^ values.float64_value.to_bits();
    value = value.rotate_left(41) ^ u64::from(values.color.ordinal());
    value.rotate_left(43) ^ u64::from(values.defaulted) ^ values.raw.rotate_left(47)
}

fn measure(
    passes: usize,
    mut fingerprint: impl FnMut() -> Result<u64, Box<dyn std::error::Error>>,
) -> Result<u64, Box<dyn std::error::Error>> {
    let mut checksum = SEED;
    for _ in 0..passes {
        checksum = checksum.rotate_left(9) ^ fingerprint()?;
    }
    Ok(black_box(checksum))
}

fn direct_scalar_fingerprint<B: capnp_message::TraversalBudget>(
    reader: capnp_message::StructReader<'_, '_, B>,
) -> Result<u64, StructReadError> {
    let reader = black_box(reader);
    let data = reader.data_section()?;
    let mut value = u64::from(data.read_bool(0, false)?);
    value = value.rotate_left(5) ^ u64::from(data.read_i8(1, 0)? as u8);
    value = value.rotate_left(7) ^ u64::from(data.read_i16(1, 0)? as u16);
    value = value.rotate_left(11) ^ u64::from(data.read_i32(1, 0)? as u32);
    value = value.rotate_left(13) ^ data.read_i64(1, 0)? as u64;
    value = value.rotate_left(17) ^ u64::from(data.read_u8(16, 0)?);
    value = value.rotate_left(19) ^ u64::from(data.read_u16(9, 0)?);
    value = value.rotate_left(23) ^ u64::from(data.read_u32(5, 0)?);
    value = value.rotate_left(29) ^ data.read_u64(3, 0)?;
    value = value.rotate_left(31) ^ u64::from(data.read_f32(8, 0.0)?.to_bits());
    value = value.rotate_left(37) ^ data.read_f64(5, 0.0)?.to_bits();
    value = value.rotate_left(41) ^ u64::from(data.read_u16(18, 0)?);
    value = value.rotate_left(43) ^ u64::from(data.read_u32(16, 123_456)?);
    Ok(value)
}

fn generated_scalar_fingerprint(
    reader: &wire_fixture::Reader,
) -> Result<u64, Box<dyn std::error::Error>> {
    let reader = black_box(reader);
    let mut value = u64::from(reader.bool_value()?);
    value = value.rotate_left(5) ^ u64::from(reader.int8_value()? as u8);
    value = value.rotate_left(7) ^ u64::from(reader.int16_value()? as u16);
    value = value.rotate_left(11) ^ u64::from(reader.int32_value()? as u32);
    value = value.rotate_left(13) ^ reader.int64_value()? as u64;
    value = value.rotate_left(17) ^ u64::from(reader.uint8_value()?);
    value = value.rotate_left(19) ^ u64::from(reader.uint16_value()?);
    value = value.rotate_left(23) ^ u64::from(reader.uint32_value()?);
    value = value.rotate_left(29) ^ reader.uint64_value()?;
    value = value.rotate_left(31) ^ u64::from(reader.float32_value()?.to_bits());
    value = value.rotate_left(37) ^ reader.float64_value()?.to_bits();
    value = value.rotate_left(41)
        ^ u64::from(match reader.color()? {
            Color::Red => 0,
            Color::Green => 1,
            Color::Blue => 2,
            Color::Unrecognized(value) => value,
        });
    value = value.rotate_left(43) ^ u64::from(reader.defaulted()?);
    Ok(value)
}

fn borrowed_scalar_fingerprint<B: capnp_message::TraversalBudget>(
    reader: &wire_fixture::BorrowedReader<'_, '_, B>,
) -> Result<u64, Box<dyn std::error::Error>> {
    let reader = black_box(reader);
    let mut value = u64::from(reader.bool_value());
    value = value.rotate_left(5) ^ u64::from(reader.int8_value() as u8);
    value = value.rotate_left(7) ^ u64::from(reader.int16_value() as u16);
    value = value.rotate_left(11) ^ u64::from(reader.int32_value() as u32);
    value = value.rotate_left(13) ^ reader.int64_value() as u64;
    value = value.rotate_left(17) ^ u64::from(reader.uint8_value());
    value = value.rotate_left(19) ^ u64::from(reader.uint16_value());
    value = value.rotate_left(23) ^ u64::from(reader.uint32_value());
    value = value.rotate_left(29) ^ reader.uint64_value();
    value = value.rotate_left(31) ^ u64::from(reader.float32_value().to_bits());
    value = value.rotate_left(37) ^ reader.float64_value().to_bits();
    value = value.rotate_left(41) ^ u64::from(reader.color_ordinal());
    value = value.rotate_left(43) ^ u64::from(reader.defaulted());
    Ok(value)
}

fn direct_blob_fingerprint<B: capnp_message::TraversalBudget>(
    reader: capnp_message::StructReader<'_, '_, B>,
) -> Result<u64, StructReadError> {
    let reader = black_box(reader);
    let text = reader.read_text(0, None)?;
    let data = reader.read_data(1, None)?;
    Ok(blob_fingerprint(text.as_bytes(), data.as_bytes()))
}

fn generated_blob_fingerprint(
    reader: &wire_fixture::Reader,
) -> Result<u64, Box<dyn std::error::Error>> {
    let reader = black_box(reader);
    let text = reader.text()?;
    let data = reader.data()?;
    Ok(blob_fingerprint(text.as_bytes(), &data))
}

fn borrowed_blob_fingerprint<B: capnp_message::TraversalBudget>(
    reader: &wire_fixture::BorrowedReader<'_, '_, B>,
) -> Result<u64, Box<dyn std::error::Error>> {
    let reader = black_box(reader);
    let text = reader.text()?;
    let data = reader.data()?;
    Ok(blob_fingerprint(text.as_bytes(), data.as_bytes()))
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

fn direct_group_fingerprint<B: capnp_message::TraversalBudget>(
    reader: capnp_message::StructReader<'_, '_, B>,
) -> Result<u64, StructReadError> {
    let reader = black_box(reader);
    let data = reader.data_section()?;
    Ok(group_fingerprint(
        data.read_u16(19, 0)?,
        data.read_u64(6, 0)?,
        data.read_u64(7, 0)?,
        data.read_bool(1, false)?,
    ))
}

fn borrowed_group_fingerprint<B: capnp_message::TraversalBudget>(
    reader: &wire_fixture::BorrowedReader<'_, '_, B>,
) -> u64 {
    let reader = black_box(reader);
    let choice = reader.choice();
    let tag = match choice.which() {
        choice::Which::None => 0,
        choice::Which::Number => 1,
        choice::Which::Words => 2,
        choice::Which::Unrecognized(value) => value,
    };
    let metadata = reader.metadata();
    group_fingerprint(tag, choice.number(), metadata.created(), metadata.valid())
}

fn group_fingerprint(tag: u16, number: u64, created: u64, valid: bool) -> u64 {
    let mut value = u64::from(tag);
    value = value.rotate_left(17) ^ number;
    value = value.rotate_left(23) ^ created;
    value.rotate_left(29) ^ u64::from(valid)
}

fn direct_list_fingerprint<B: capnp_message::TraversalBudget>(
    reader: capnp_message::StructReader<'_, '_, B>,
) -> Result<u64, Box<dyn std::error::Error>> {
    let reader = black_box(reader);
    let pointers = reader.pointer_section()?;
    let integer = pointers.read_list(9)?.as_primitive::<u16>()?.get(2)?;
    let color = pointers.read_list(14)?.as_primitive::<u16>()?.get(2)?;
    let text = pointers.read_list(15)?.as_pointers()?.read_text(2)?;
    let data = pointers.read_list(16)?.as_pointers()?.read_data(1)?;
    let nested = pointers
        .read_list(18)?
        .as_pointers()?
        .get_list(0)?
        .as_primitive::<u16>()?
        .get(2)?;
    Ok(list_fingerprint(
        integer,
        color,
        text.as_bytes(),
        data.as_bytes(),
        nested,
    ))
}

fn borrowed_list_fingerprint<B: capnp_message::TraversalBudget>(
    reader: &wire_fixture::BorrowedReader<'_, '_, B>,
) -> Result<u64, Box<dyn std::error::Error>> {
    let reader = black_box(reader);
    let integer = reader.uint16s()?.get(2)?;
    let color = reader.colors()?.get(2)?.ordinal();
    let text = reader.texts()?.get(2)?;
    let data = reader.data_blobs()?.get(1)?;
    let nested = reader.nested_lists()?.get(0)?.get(2)?;
    Ok(list_fingerprint(
        integer,
        color,
        text.as_bytes(),
        data.as_bytes(),
        nested,
    ))
}

fn list_fingerprint(integer: u16, color: u16, text: &[u8], data: &[u8], nested: u16) -> u64 {
    let mut value = u64::from(integer);
    value = value.rotate_left(11) ^ u64::from(color);
    value = value.rotate_left(17) ^ text.len() as u64;
    value = value.rotate_left(23) ^ u64::from(text.last().copied().unwrap_or_default());
    value = value.rotate_left(29) ^ data.len() as u64;
    value = value.rotate_left(31) ^ u64::from(data.last().copied().unwrap_or_default());
    value.rotate_left(37) ^ u64::from(nested)
}

fn direct_nested_fingerprint<B: capnp_message::TraversalBudget>(
    reader: capnp_message::StructReader<'_, '_, B>,
) -> Result<u64, StructReadError> {
    let reader = black_box(reader);
    let nested = reader.pointer_section()?.read_struct(22)?;
    Ok(u64::from(nested.data_section()?.get_u32(0, 0)))
}

fn borrowed_nested_fingerprint<B: capnp_message::TraversalBudget>(
    reader: &wire_fixture::BorrowedReader<'_, '_, B>,
) -> Result<u64, StructReadError> {
    let reader = black_box(reader);
    Ok(u64::from(reader.node()?.value()))
}

fn direct_struct_list_fingerprint<B: capnp_message::TraversalBudget>(
    reader: capnp_message::StructReader<'_, '_, B>,
) -> Result<u64, Box<dyn std::error::Error>> {
    let reader = black_box(reader);
    let node = reader
        .pointer_section()?
        .read_list(17)?
        .as_structs()?
        .get(1)?;
    Ok(u64::from(node.data_section()?.get_u32(0, 0)))
}

fn borrowed_struct_list_fingerprint<B: capnp_message::TraversalBudget>(
    reader: &wire_fixture::BorrowedReader<'_, '_, B>,
) -> Result<u64, Box<dyn std::error::Error>> {
    let reader = black_box(reader);
    Ok(u64::from(reader.structs()?.get(1)?.value()))
}

fn direct_evolution_fingerprint<B: capnp_message::TraversalBudget>(
    reader: capnp_message::StructReader<'_, '_, B>,
) -> Result<u64, Box<dyn std::error::Error>> {
    let reader = black_box(reader);
    let data = reader.data_section()?;
    let pointers = reader.pointer_section()?;
    evolution_fingerprint(
        data.get_u32(0, 0),
        data.get_u16(2, 0),
        pointers.read_text(0)?.as_bytes(),
        pointers.read_list(1)?.as_primitive::<u32>()?.get(1)?,
    )
}

fn borrowed_evolution_fingerprint<B: capnp_message::TraversalBudget>(
    reader: &evolution_v1::record::BorrowedReader<'_, '_, B>,
) -> Result<u64, Box<dyn std::error::Error>> {
    let reader = black_box(reader);
    evolution_fingerprint(
        reader.id(),
        reader.state_ordinal(),
        reader.name()?.as_bytes(),
        reader.values()?.get(1)?,
    )
}

fn evolution_fingerprint(
    id: u32,
    state: u16,
    name: &[u8],
    second_value: u32,
) -> Result<u64, Box<dyn std::error::Error>> {
    let mut value = u64::from(id);
    value = value.rotate_left(13) ^ u64::from(state);
    value = value.rotate_left(19) ^ name.len() as u64;
    value = value.rotate_left(23) ^ u64::from(name.last().copied().unwrap_or_default());
    Ok(value.rotate_left(29) ^ u64::from(second_value))
}

fn direct_default_fingerprint<B: capnp_message::TraversalBudget>(
    reader: capnp_message::StructReader<'_, '_, B>,
) -> Result<u64, StructReadError> {
    let reader = black_box(reader);
    let text = reader
        .pointer_section()?
        .read_text_with_default(25, b"default text\0")?;
    Ok(text_fingerprint(text.as_bytes()))
}

fn borrowed_default_fingerprint<B: capnp_message::TraversalBudget>(
    reader: &wire_fixture::BorrowedReader<'_, '_, B>,
) -> Result<u64, StructReadError> {
    let reader = black_box(reader);
    Ok(text_fingerprint(reader.default_text()?.as_bytes()))
}

fn text_fingerprint(text: &[u8]) -> u64 {
    let value = (text.len() as u64).rotate_left(17);
    value ^ u64::from(text.last().copied().unwrap_or_default()).rotate_left(31)
}

fn parse_borrowed_message(bytes: &[u8]) -> Result<BorrowedMessage<'_>, Box<dyn std::error::Error>> {
    let FrameRead::Message { frame, remaining } = parse_frame(bytes, FrameLimits::default())?
    else {
        return Err("fixture is empty".into());
    };
    if !remaining.is_empty() {
        return Err("fixture has trailing bytes".into());
    }
    let segments = frame
        .segments()
        .iter()
        .map(|segment| segment.bytes())
        .collect::<Vec<_>>();
    BorrowedMessage::new(
        &segments,
        ReaderLimits {
            traversal_words: u64::MAX,
            nesting_levels: 64,
        },
    )
    .map_err(Into::into)
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
