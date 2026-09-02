use std::hint::black_box;
use std::io::Cursor;
use std::time::Instant;

use capnp_io::{
    BorrowedFrameRead, FrameLimits, PreparedSegments, Segment, encode_frame, encode_prepared_frame,
    parse_frame_into, read_frame, write_frame, write_prepared_frame,
};

const SEED: u64 = 0x4d59_5df4_d0f3_3173;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mode = args
        .next()
        .ok_or("usage: framing_benchmark MODE SEGMENTS PASSES")?;
    let segment_count = args
        .next()
        .ok_or("missing segment count")?
        .parse::<usize>()?;
    let passes = args.next().ok_or("missing pass count")?.parse::<usize>()?;
    if args.next().is_some() || !matches!(segment_count, 1 | 2 | 64) || passes == 0 {
        return Err(
            "expected parse|encode|encode-prepared, SEGMENTS in {1,2,64}, and positive PASSES"
                .into(),
        );
    }

    let segments = fixture_segments(segment_count);
    let views: Vec<&[u8]> = segments.iter().map(Vec::as_slice).collect();
    let limits = FrameLimits::default();
    let encoded_len =
        (views.len() / 2 + 1) * 8 + views.iter().map(|segment| segment.len()).sum::<usize>();
    let (elapsed_ns, checksum) = match mode.as_str() {
        "parse" => {
            let encoded = encode_frame(&views, limits)?;
            let mut storage = [Segment::EMPTY; 64];
            measure(|| parse_many(&encoded, limits, &mut storage, passes))?
        }
        "encode" => measure(|| encode_many(&views, limits, passes))?,
        "encode-prepared" => {
            let prepared = PreparedSegments::new(&views, limits)?;
            measure(|| encode_prepared_many(&prepared, passes))?
        }
        "stream-read" => {
            let encoded = encode_frame(&views, limits)?;
            measure_io(|| stream_read_many(&encoded, limits, passes))?
        }
        "stream-write" => measure_io(|| stream_write_many(&views, limits, encoded_len, passes))?,
        "stream-write-prepared" => {
            let prepared = PreparedSegments::new(&views, limits)?;
            measure_io(|| stream_write_prepared_many(&prepared, passes))?
        }
        _ => return Err("unknown benchmark mode".into()),
    };
    println!("{elapsed_ns}\t{checksum}");
    Ok(())
}

fn fixture_segments(count: usize) -> Vec<Vec<u8>> {
    let word_counts: Vec<usize> = match count {
        1 => vec![8],
        2 => vec![3, 5],
        64 => vec![1; 64],
        _ => unreachable!(),
    };
    let mut state = SEED ^ count as u64;
    word_counts
        .into_iter()
        .map(|words| {
            let mut bytes = Vec::with_capacity(words * 8);
            for _ in 0..words {
                state = xorshift(state);
                bytes.extend_from_slice(&state.to_le_bytes());
            }
            bytes
        })
        .collect()
}

fn measure(
    operation: impl FnOnce() -> Result<u64, capnp_io::FrameError>,
) -> Result<(u128, u64), capnp_io::FrameError> {
    let started = Instant::now();
    let checksum = operation()?;
    Ok((started.elapsed().as_nanos(), checksum))
}

fn measure_io(
    operation: impl FnOnce() -> Result<u64, capnp_io::IoFrameError>,
) -> Result<(u128, u64), capnp_io::IoFrameError> {
    let started = Instant::now();
    let checksum = operation()?;
    Ok((started.elapsed().as_nanos(), checksum))
}

fn parse_many<'input>(
    encoded: &'input [u8],
    limits: FrameLimits,
    storage: &mut [Segment<'input>],
    passes: usize,
) -> Result<u64, capnp_io::FrameError> {
    let mut checksum = SEED;
    for _ in 0..passes {
        let BorrowedFrameRead::Message { frame, remaining } =
            parse_frame_into(encoded, limits, storage)?
        else {
            unreachable!();
        };
        let segments = frame.segments();
        let first = segments[0];
        let last = segments[segments.len() - 1];
        let fingerprint = (segments.len() as u64)
            ^ (frame.table_len() as u64).rotate_left(11)
            ^ (frame.encoded_len() as u64).rotate_left(23)
            ^ u64::from(first.word_count()).rotate_left(37)
            ^ u64::from(last.word_count()).rotate_left(49)
            ^ u64::from(first.bytes()[0]).rotate_left(7)
            ^ u64::from(last.bytes()[last.bytes().len() - 1]).rotate_left(19)
            ^ remaining.len() as u64;
        checksum = checksum.rotate_left(9) ^ fingerprint;
    }
    Ok(black_box(checksum))
}

fn encode_many(
    segments: &[&[u8]],
    limits: FrameLimits,
    passes: usize,
) -> Result<u64, capnp_io::FrameError> {
    let mut checksum = SEED;
    for _ in 0..passes {
        let encoded = encode_frame(segments, limits)?;
        let fingerprint = (encoded.len() as u64)
            ^ u64::from(encoded[0]).rotate_left(7)
            ^ u64::from(encoded[encoded.len() - 1]).rotate_left(19);
        checksum = checksum.rotate_left(9) ^ fingerprint;
        black_box(encoded);
    }
    Ok(black_box(checksum))
}

fn encode_prepared_many(
    segments: &PreparedSegments<'_>,
    passes: usize,
) -> Result<u64, capnp_io::FrameError> {
    let mut checksum = SEED;
    for _ in 0..passes {
        let encoded = encode_prepared_frame(segments);
        let fingerprint = (encoded.len() as u64)
            ^ u64::from(encoded[0]).rotate_left(7)
            ^ u64::from(encoded[encoded.len() - 1]).rotate_left(19);
        checksum = checksum.rotate_left(9) ^ fingerprint;
        black_box(encoded);
    }
    Ok(black_box(checksum))
}

fn stream_read_many(
    encoded: &[u8],
    limits: FrameLimits,
    passes: usize,
) -> Result<u64, capnp_io::IoFrameError> {
    let mut checksum = SEED;
    for _ in 0..passes {
        let mut input = Cursor::new(encoded);
        let frame = read_frame(&mut input, limits)?.expect("fixture contains one frame");
        let fingerprint = (frame.len() as u64)
            ^ u64::from(frame[0]).rotate_left(7)
            ^ u64::from(frame[frame.len() - 1]).rotate_left(19)
            ^ u64::from(u32::from_le_bytes(
                frame[4..8].try_into().expect("frame header"),
            ))
            .rotate_left(31);
        checksum = checksum.rotate_left(9) ^ fingerprint;
        black_box(frame);
    }
    Ok(black_box(checksum))
}

fn stream_write_many(
    segments: &[&[u8]],
    limits: FrameLimits,
    encoded_len: usize,
    passes: usize,
) -> Result<u64, capnp_io::IoFrameError> {
    let mut checksum = SEED;
    for _ in 0..passes {
        let output = Vec::with_capacity(encoded_len);
        let frame = write_frame(output, segments, limits, usize::MAX)?;
        let fingerprint = (frame.len() as u64)
            ^ u64::from(frame[0]).rotate_left(7)
            ^ u64::from(frame[frame.len() - 1]).rotate_left(19);
        checksum = checksum.rotate_left(9) ^ fingerprint;
        black_box(frame);
    }
    Ok(black_box(checksum))
}

fn stream_write_prepared_many(
    segments: &PreparedSegments<'_>,
    passes: usize,
) -> Result<u64, capnp_io::IoFrameError> {
    let mut checksum = SEED;
    for _ in 0..passes {
        let output = Vec::with_capacity(segments.encoded_len());
        let frame = write_prepared_frame(output, segments, usize::MAX)?;
        let fingerprint = (frame.len() as u64)
            ^ u64::from(frame[0]).rotate_left(7)
            ^ u64::from(frame[frame.len() - 1]).rotate_left(19);
        checksum = checksum.rotate_left(9) ^ fingerprint;
        black_box(frame);
    }
    Ok(black_box(checksum))
}

const fn xorshift(mut value: u64) -> u64 {
    value ^= value << 13;
    value ^= value >> 7;
    value ^= value << 17;
    value
}
