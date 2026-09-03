use std::hint::black_box;
use std::time::Instant;

use capnp_io::{PackedDecoder, PackedEncoder, pack, unpack};

const SEED: u64 = 0x4d59_5df4_d0f3_3173;
const STREAM_DECODE_CHUNK_BYTES: usize = 1_025;
const CPP_FIXTURE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../conformance/fixtures/cpp/",
    "e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/wire-unpacked.bin"
));

fn parse_size(value: Option<String>) -> Result<usize, Box<dyn std::error::Error>> {
    let parsed = value.ok_or("missing positive integer")?.parse::<usize>()?;
    if parsed == 0 {
        return Err("sizes must be positive integers".into());
    }
    Ok(parsed)
}

fn xorshift(mut value: u64) -> u64 {
    value ^= value << 13;
    value ^= value >> 7;
    value ^ (value << 17)
}

fn make_input(shape: &str, words: usize) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let byte_len = words.checked_mul(8).ok_or("input size overflow")?;
    if shape == "realistic" {
        return Ok((0..byte_len)
            .map(|index| CPP_FIXTURE[index % CPP_FIXTURE.len()])
            .collect());
    }

    let mut result = vec![0_u8; byte_len];
    let mut state = SEED ^ words as u64;
    for (index, byte) in result.iter_mut().enumerate() {
        state = xorshift(state);
        let word = index / 8;
        let lane = index % 8;
        let nonzero = match shape {
            "zero" => false,
            "raw" => true,
            "mixed" => match word % 8 {
                0 => false,
                1 => lane == 0 || lane == 3,
                2 => true,
                3 => lane != 4,
                4 => lane >= 2,
                5 => lane % 2 == 0,
                6 => true,
                _ => lane == 7,
            },
            _ => return Err("shape must be zero, raw, mixed, or realistic".into()),
        };
        if nonzero {
            *byte = state as u8 | 1;
        }
    }
    Ok(result)
}

fn observe(mut checksum: u64, bytes: &[u8]) -> u64 {
    checksum = checksum.rotate_left(9) ^ bytes.len() as u64;
    if let Some(first) = bytes.first() {
        checksum ^= u64::from(*first).rotate_left(7);
    }
    if let Some(last) = bytes.last() {
        checksum ^= u64::from(*last).rotate_left(19);
    }
    checksum
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn stream_chunk_words(shape: &str) -> usize {
    match shape {
        "zero" | "raw" => 256,
        "mixed" => 8,
        "realistic" => 100,
        _ => unreachable!("validated by make_input"),
    }
}

fn pack_streaming(
    input: &[u8],
    shape: &str,
    max_packed: usize,
) -> Result<Vec<u8>, capnp_io::PackedError> {
    let mut encoder = PackedEncoder::new(max_packed);
    for chunk in input.chunks(stream_chunk_words(shape) * 8) {
        encoder.push(chunk)?;
    }
    encoder.finish()
}

fn unpack_streaming(packed: &[u8], max_output: usize) -> Result<Vec<u8>, capnp_io::PackedError> {
    let mut decoder = PackedDecoder::new(max_output);
    for chunk in packed.chunks(STREAM_DECODE_CHUNK_BYTES) {
        decoder.push(chunk)?;
    }
    decoder.finish()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mode = args
        .next()
        .ok_or("usage: packing_benchmark MODE SHAPE WORDS PASSES")?;
    let shape = args.next().ok_or("missing shape")?;
    let words = parse_size(args.next())?;
    let passes = parse_size(args.next())?;
    if args.next().is_some() {
        return Err("too many arguments".into());
    }

    let input = make_input(&shape, words)?;
    let max_packed = input
        .len()
        .checked_mul(2)
        .and_then(|size| size.checked_add(2))
        .ok_or("packed size limit overflow")?;
    let packed = pack(&input, max_packed)?;
    if unpack(&packed, input.len())? != input {
        return Err("packing fixture did not round trip".into());
    }
    if pack_streaming(&input, &shape, max_packed)? != packed {
        return Err("stream chunks changed the canonical packed bytes".into());
    }
    if unpack_streaming(&packed, input.len())? != input {
        return Err("streaming packing fixture did not round trip".into());
    }

    let mut checksum = SEED;
    let started = Instant::now();
    match mode.as_str() {
        "copy-unpacked" => {
            for _ in 0..passes {
                let output = black_box(black_box(&input).to_vec());
                checksum = observe(checksum, &output);
            }
        }
        "copy-packed" => {
            for _ in 0..passes {
                let output = black_box(black_box(&packed).to_vec());
                checksum = observe(checksum, &output);
            }
        }
        "pack" => {
            for _ in 0..passes {
                let output = black_box(pack(black_box(&input), max_packed)?);
                checksum = observe(checksum, &output);
            }
        }
        "unpack" => {
            for _ in 0..passes {
                let output = black_box(unpack(black_box(&packed), input.len())?);
                checksum = observe(checksum, &output);
            }
        }
        "pack-stream" => {
            for _ in 0..passes {
                let output = black_box(pack_streaming(black_box(&input), &shape, max_packed)?);
                checksum = observe(checksum, &output);
            }
        }
        "unpack-stream" => {
            for _ in 0..passes {
                let output = black_box(unpack_streaming(black_box(&packed), input.len())?);
                checksum = observe(checksum, &output);
            }
        }
        _ => return Err("unknown benchmark mode".into()),
    }
    let elapsed = started.elapsed();
    let canonical = if mode == "copy-packed" || mode == "pack" || mode == "pack-stream" {
        &packed
    } else {
        &input
    };
    checksum ^= fnv1a(canonical);
    println!("{}\t{checksum}", elapsed.as_nanos());
    Ok(())
}
