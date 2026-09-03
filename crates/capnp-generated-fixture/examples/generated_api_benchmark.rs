use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

use capnp_generated_fixture::wire::{Color, choice, wire_fixture};
use capnp_io::{FrameLimits, FrameRead, parse_frame};
use capnp_message::{BorrowedMessage, OwnedMessage, ReaderLimits, StructReadError};
use capnp_schema::{CompiledSchema, LoadLimits};

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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mode = args.next().ok_or(
        "usage: generated_api_benchmark direct-scalars|generated-scalars|borrowed-direct-scalars|borrowed-scalars|direct-blobs|generated-blobs|borrowed-direct-blobs|borrowed-blobs|borrowed-direct-groups|borrowed-groups PASSES",
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
        )
    {
        return Err("expected a known mode and positive PASSES".into());
    }

    let schema = Arc::new(CompiledSchema::from_code_generator_request(
        REQUEST,
        LoadLimits::default(),
    )?);
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
        _ => unreachable!(),
    };
    println!("{}\t{}", started.elapsed().as_nanos(), checksum);
    Ok(())
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
    let text = reader.read_text(0, None)?;
    let data = reader.read_data(1, None)?;
    Ok(blob_fingerprint(text.as_bytes(), data.as_bytes()))
}

fn generated_blob_fingerprint(
    reader: &wire_fixture::Reader,
) -> Result<u64, Box<dyn std::error::Error>> {
    let text = reader.text()?;
    let data = reader.data()?;
    Ok(blob_fingerprint(text.as_bytes(), &data))
}

fn borrowed_blob_fingerprint<B: capnp_message::TraversalBudget>(
    reader: &wire_fixture::BorrowedReader<'_, '_, B>,
) -> Result<u64, Box<dyn std::error::Error>> {
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
