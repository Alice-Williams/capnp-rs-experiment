use std::hint::black_box;
use std::time::Instant;

use capnp_io::{BorrowedFrameRead, FrameLimits, Segment, encode_frame, parse_frame_into};
use capnp_message::{
    DataSection, LocalTraversalBudget, MessageSegments, NestingLimit, PrimitiveError,
    StructReadError, StructReader,
};

const SEED: u64 = 0x4d59_5df4_d0f3_3173;
const VALUE: u64 = 0x0123_4567_89ab_cdef;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mode = args.next().ok_or(
            "usage: message_read framing|root|scalars|blobs|isolated-root|isolated-scalars|isolated-blobs|scalar-only|blob-only SEGMENTS PASSES",
    )?;
    let segment_count = args
        .next()
        .ok_or("missing segment count")?
        .parse::<usize>()?;
    let passes = args.next().ok_or("missing pass count")?.parse::<usize>()?;
    if args.next().is_some()
        || !matches!(
            mode.as_str(),
            "framing"
                | "root"
                | "scalars"
                | "blobs"
                | "isolated-root"
                | "isolated-scalars"
                | "isolated-blobs"
                | "scalar-only"
                | "blob-only"
        )
        || !matches!(segment_count, 1 | 2 | 64)
        || passes == 0
    {
        return Err("expected a known read mode, SEGMENTS in {1,2,64}, and positive PASSES".into());
    }

    let segments = fixture_segments(segment_count);
    let views: Vec<&[u8]> = segments.iter().map(Vec::as_slice).collect();
    let encoded = encode_frame(&views, FrameLimits::default())?;
    let descriptors: Vec<Segment<'_>> = views
        .iter()
        .map(|bytes| Segment::from_bytes(bytes).expect("fixture segments are word-aligned"))
        .collect();
    if mode == "scalar-only" {
        let (elapsed, checksum) = read_scalar_only(&descriptors, passes)?;
        println!("{}\t{}", elapsed.as_nanos(), checksum);
        return Ok(());
    }
    if mode == "blob-only" {
        let (elapsed, checksum) = read_blob_only(&descriptors, passes)?;
        println!("{}\t{}", elapsed.as_nanos(), checksum);
        return Ok(());
    }
    let started = Instant::now();
    let checksum = if matches!(
        mode.as_str(),
        "isolated-root" | "isolated-scalars" | "isolated-blobs"
    ) {
        read_isolated_roots(
            &descriptors,
            mode == "isolated-scalars",
            mode == "isolated-blobs",
            passes,
        )?
    } else {
        read_many(
            &encoded,
            matches!(mode.as_str(), "root" | "scalars" | "blobs"),
            mode == "scalars",
            mode == "blobs",
            passes,
        )?
    };
    println!("{}\t{}", started.elapsed().as_nanos(), checksum);
    Ok(())
}

fn read_scalar_only(
    segments: &[Segment<'_>],
    passes: usize,
) -> Result<(std::time::Duration, u64), Box<dyn std::error::Error>> {
    let message = MessageSegments::from_descriptors(segments)?;
    let budget = LocalTraversalBudget::new(16);
    let root = message.read_root_struct(&budget, NestingLimit::new(8))?;
    let data = root.data_section()?;
    let mut checksum = SEED;
    let started = Instant::now();
    for _ in 0..passes {
        checksum = checksum.rotate_left(9) ^ scalar_fingerprint(black_box(data))?;
    }
    Ok((started.elapsed(), black_box(checksum)))
}

fn read_blob_only(
    segments: &[Segment<'_>],
    passes: usize,
) -> Result<(std::time::Duration, u64), Box<dyn std::error::Error>> {
    let message = MessageSegments::from_descriptors(segments)?;
    let budget = LocalTraversalBudget::new(u64::MAX);
    let root = message.read_root_struct(&budget, NestingLimit::new(8))?;
    let mut checksum = SEED;
    let started = Instant::now();
    for _ in 0..passes {
        checksum = checksum.rotate_left(9) ^ blob_fingerprint(black_box(&root))?;
    }
    Ok((started.elapsed(), black_box(checksum)))
}

fn read_isolated_roots(
    segments: &[Segment<'_>],
    read_scalars: bool,
    read_blobs: bool,
    passes: usize,
) -> Result<u64, Box<dyn std::error::Error>> {
    let mut checksum = SEED;
    for _ in 0..passes {
        let message = MessageSegments::from_descriptors(segments)?;
        let budget = LocalTraversalBudget::new(16);
        let root = message.read_root_struct(&budget, NestingLimit::new(8))?;
        let fingerprint = if read_scalars {
            segments.len() as u64 ^ scalar_fingerprint(root.data_section()?)?
        } else if read_blobs {
            segments.len() as u64 ^ blob_fingerprint(&root)?
        } else {
            segments.len() as u64
                ^ (root.data_byte_len() as u64).rotate_left(17)
                ^ root.read_u64(0, 0)?.rotate_left(37)
        };
        checksum = checksum.rotate_left(9) ^ fingerprint;
    }
    Ok(black_box(checksum))
}

#[inline(always)]
fn blob_fingerprint(
    root: &StructReader<'_, '_, LocalTraversalBudget>,
) -> Result<u64, StructReadError> {
    if root.pointer_count() < 2 {
        return Ok(0);
    }
    let text = root.read_text(0, None)?;
    let data = root.read_data(1, None)?;
    let mut fingerprint = (text.len() as u64).rotate_left(11) ^ (data.len() as u64).rotate_left(23);
    if let Some(first) = text.as_bytes().first() {
        fingerprint ^= u64::from(*first).rotate_left(31);
    }
    if let Some(last) = text.as_bytes().last() {
        fingerprint ^= u64::from(*last).rotate_left(37);
    }
    if let Some(first) = data.as_bytes().first() {
        fingerprint ^= u64::from(*first).rotate_left(43);
    }
    if let Some(last) = data.as_bytes().last() {
        fingerprint ^= u64::from(*last).rotate_left(47);
    }
    Ok(fingerprint)
}

#[inline(always)]
fn scalar_fingerprint(data: DataSection<'_>) -> Result<u64, PrimitiveError> {
    let mut fingerprint = u64::from(data.read_bool(0, true)?);
    fingerprint = fingerprint.rotate_left(7) ^ u64::from(data.read_u8(0, 0x5a)?);
    fingerprint = fingerprint.rotate_left(11) ^ u64::from(data.read_u16(0, 0xa55a)?);
    fingerprint = fingerprint.rotate_left(13) ^ u64::from(data.read_u32(0, 0x1357_9bdf)?);
    fingerprint = fingerprint.rotate_left(17) ^ data.read_u64(0, 0xfedc_ba98_7654_3210)?;
    fingerprint = fingerprint.rotate_left(19) ^ u64::from(data.read_i32(0, -123_456)? as u32);
    fingerprint = fingerprint.rotate_left(23) ^ u64::from(data.read_f32(0, 1.25)?.to_bits());
    fingerprint = fingerprint.rotate_left(29) ^ data.read_f64(0, -3.5)?.to_bits();
    fingerprint = fingerprint.rotate_left(31) ^ u64::from(data.read_u16(0, 7)?);
    Ok(fingerprint)
}

fn fixture_segments(count: usize) -> Vec<Vec<u8>> {
    let sizes = match count {
        1 => vec![8],
        2 => vec![3, 5],
        64 => vec![1; 64],
        _ => unreachable!(),
    };
    let mut segments: Vec<Vec<u8>> = sizes.into_iter().map(|words| vec![0; words * 8]).collect();

    match count {
        1 => {
            write_word(&mut segments[0], 0, 0x0002_0001_u64 << 32);
            write_word(&mut segments[0], 1, VALUE);
            write_word(&mut segments[0], 2, (0x42_u64 << 32) | 5);
            write_word(&mut segments[0], 3, (0x42_u64 << 32) | 1);
            segments[0][32..40].copy_from_slice(b"capnp!!\0");
        }
        2 => {
            write_word(&mut segments[0], 0, (1_u64 << 32) | 2);
            write_word(&mut segments[1], 0, 0x0002_0001_u64 << 32);
            write_word(&mut segments[1], 1, VALUE);
            write_word(&mut segments[1], 2, (0x42_u64 << 32) | 5);
            write_word(&mut segments[1], 3, (0x42_u64 << 32) | 1);
            segments[1][32..40].copy_from_slice(b"capnp!!\0");
        }
        64 => {
            write_word(&mut segments[0], 0, (63_u64 << 32) | 2);
            // Segment 63's zero landing-pad word describes an empty struct.
        }
        _ => unreachable!(),
    }
    segments
}

fn write_word(segment: &mut [u8], index: usize, value: u64) {
    segment[index * 8..index * 8 + 8].copy_from_slice(&value.to_le_bytes());
}

fn read_many(
    encoded: &[u8],
    read_root: bool,
    read_scalars: bool,
    read_blobs: bool,
    passes: usize,
) -> Result<u64, Box<dyn std::error::Error>> {
    let limits = FrameLimits::default();
    let mut storage = [Segment::EMPTY; 64];
    let mut checksum = SEED;

    for _ in 0..passes {
        let BorrowedFrameRead::Message { frame, remaining } =
            parse_frame_into(encoded, limits, &mut storage)?
        else {
            unreachable!();
        };
        let segments = black_box(frame.segments());
        let mut fingerprint = (segments.len() as u64)
            ^ (frame.table_len() as u64).rotate_left(11)
            ^ (frame.encoded_len() as u64).rotate_left(23)
            ^ remaining.len() as u64;
        for (index, segment) in segments.iter().enumerate() {
            fingerprint = fingerprint.rotate_left(9)
                ^ index as u64
                ^ u64::from(segment.word_count()).rotate_left(13)
                ^ u64::from(segment.bytes()[0]).rotate_left(29)
                ^ u64::from(segment.bytes()[segment.bytes().len() - 1]).rotate_left(47);
        }

        if read_root {
            let message = MessageSegments::from_descriptors(segments)?;
            let budget = LocalTraversalBudget::new(16);
            let root = message.read_root_struct(&budget, NestingLimit::new(8))?;
            fingerprint ^= if read_scalars {
                scalar_fingerprint(root.data_section()?)?
            } else if read_blobs {
                blob_fingerprint(&root)?
            } else {
                (root.data_byte_len() as u64).rotate_left(17) ^ root.read_u64(0, 0)?.rotate_left(37)
            };
        }
        checksum = checksum.rotate_left(9) ^ fingerprint;
    }
    Ok(black_box(checksum))
}
