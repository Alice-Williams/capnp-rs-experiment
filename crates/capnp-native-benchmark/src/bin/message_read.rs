use std::hint::black_box;
use std::time::Instant;

use capnp_io::{BorrowedFrameRead, FrameLimits, Segment, encode_frame, parse_frame_into};
use capnp_message::{LocalTraversalBudget, MessageSegments, NestingLimit};

const SEED: u64 = 0x4d59_5df4_d0f3_3173;
const VALUE: u64 = 0x0123_4567_89ab_cdef;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mode = args
        .next()
        .ok_or("usage: message_read framing|root|isolated-root SEGMENTS PASSES")?;
    let segment_count = args
        .next()
        .ok_or("missing segment count")?
        .parse::<usize>()?;
    let passes = args.next().ok_or("missing pass count")?.parse::<usize>()?;
    if args.next().is_some()
        || !matches!(mode.as_str(), "framing" | "root" | "isolated-root")
        || !matches!(segment_count, 1 | 2 | 64)
        || passes == 0
    {
        return Err(
            "expected framing|root|isolated-root, SEGMENTS in {1,2,64}, and positive PASSES".into(),
        );
    }

    let segments = fixture_segments(segment_count);
    let views: Vec<&[u8]> = segments.iter().map(Vec::as_slice).collect();
    let encoded = encode_frame(&views, FrameLimits::default())?;
    let descriptors: Vec<Segment<'_>> = views
        .iter()
        .map(|bytes| Segment::from_bytes(bytes).expect("fixture segments are word-aligned"))
        .collect();
    let started = Instant::now();
    let checksum = if mode == "isolated-root" {
        read_isolated_roots(&descriptors, passes)?
    } else {
        read_many(&encoded, mode == "root", passes)?
    };
    println!("{}\t{}", started.elapsed().as_nanos(), checksum);
    Ok(())
}

fn read_isolated_roots(
    segments: &[Segment<'_>],
    passes: usize,
) -> Result<u64, Box<dyn std::error::Error>> {
    let mut checksum = SEED;
    for _ in 0..passes {
        let message = MessageSegments::from_descriptors(segments)?;
        let budget = LocalTraversalBudget::new(16);
        let root = message.read_root_struct(&budget, NestingLimit::new(8))?;
        let data_len = root.data_byte_len();
        let value = root.read_u64(0, 0)?;
        let fingerprint =
            segments.len() as u64 ^ (data_len as u64).rotate_left(17) ^ value.rotate_left(37);
        checksum = checksum.rotate_left(9) ^ fingerprint;
    }
    Ok(black_box(checksum))
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
            write_word(&mut segments[0], 0, 1_u64 << 32);
            write_word(&mut segments[0], 1, VALUE);
        }
        2 => {
            write_word(&mut segments[0], 0, (1_u64 << 32) | 2);
            write_word(&mut segments[1], 0, 1_u64 << 32);
            write_word(&mut segments[1], 1, VALUE);
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
            let data_len = root.data_byte_len();
            let value = root.read_u64(0, 0)?;
            fingerprint ^= (data_len as u64).rotate_left(17) ^ value.rotate_left(37);
        }
        checksum = checksum.rotate_left(9) ^ fingerprint;
    }
    Ok(black_box(checksum))
}
