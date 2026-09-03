use std::hint::black_box;
use std::time::Instant;

use capnp_message::{
    ExclusiveArena, LocalTraversalBudget, MessageSegments, NestingLimit, WireLocation,
};

const SEED: u64 = 0x4d59_5df4_d0f3_3173;
const VALUE: u64 = 0x0123_4567_89ab_cdef;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mode = args
        .next()
        .ok_or(
            "usage: message_build prepared|fresh|reuse|copy-prepared|copy|copy-reuse direct|far|double-far|graph PASSES",
        )?;
    let shape = args.next().ok_or("missing shape")?;
    let passes = args.next().ok_or("missing pass count")?.parse::<usize>()?;
    let copy_mode = matches!(mode.as_str(), "copy-prepared" | "copy" | "copy-reuse");
    if args.next().is_some()
        || !matches!(
            mode.as_str(),
            "prepared" | "fresh" | "reuse" | "copy-prepared" | "copy" | "copy-reuse"
        )
        || !matches!(shape.as_str(), "direct" | "far" | "double-far" | "graph")
        || (mode == "reuse" && shape != "direct")
        || copy_mode != (shape == "graph")
        || passes == 0
    {
        return Err("expected a compatible build mode/shape and positive PASSES".into());
    }

    let shape = match shape.as_str() {
        "direct" => 0,
        "far" => 1,
        "double-far" => 2,
        "graph" => 3,
        _ => unreachable!(),
    };
    let mut semantic = SEED;
    let mut wire = SEED;
    let mut reuse_arena = match mode.as_str() {
        "reuse" => Some(ExclusiveArena::new(3, 3)?),
        "copy-reuse" => Some(ExclusiveArena::new(11, 11)?),
        _ => None,
    };
    let source_bytes = graph_fixture();
    let source_views = [source_bytes.as_slice()];
    let source = MessageSegments::new(&source_views)?;
    let started = Instant::now();
    for pass in 0..passes {
        let first = VALUE ^ pass as u64;
        let second = first.rotate_left(23);
        let fingerprint = match mode.as_str() {
            "prepared" => prepared_iteration(shape, first, second),
            "reuse" => reuse_iteration(
                reuse_arena.as_mut().expect("reuse mode creates its arena"),
                first,
                second,
            )?,
            "copy-prepared" => copy_prepared_iteration(&source_bytes),
            "copy" => copy_iteration(&source)?,
            "copy-reuse" => copy_reuse_iteration(
                reuse_arena
                    .as_mut()
                    .expect("copy-reuse mode creates its arena"),
                &source,
            )?,
            _ => fresh_iteration(shape, first, second)?,
        };
        semantic = semantic.rotate_left(9) ^ first ^ second.rotate_left(13);
        wire = wire.rotate_left(11) ^ fingerprint;
    }
    let elapsed = started.elapsed();
    if let Some(arena) = &reuse_arena
        && arena.as_segment() != [0; 8]
    {
        return Err("reused arena root storage was not cleared".into());
    }
    println!(
        "{}\t{}\t{}",
        elapsed.as_nanos(),
        black_box(semantic),
        black_box(wire)
    );
    Ok(())
}

#[inline(never)]
fn prepared_iteration(shape: u8, first: u64, second: u64) -> u64 {
    let mut words = [0_u8; 40];
    if shape == 2 {
        set_word(&mut words, 0, (2_u64 << 32) | 6);
        set_word(&mut words, 1, first);
        set_word(&mut words, 2, second);
        set_word(&mut words, 3, (1_u64 << 32) | 2);
        set_word(&mut words, 4, 2_u64 << 32);
        hash_prepared(&words, 5, 3)
    } else if shape == 1 {
        set_word(&mut words, 0, (1_u64 << 32) | 2);
        set_word(&mut words, 1, 2_u64 << 32);
        set_word(&mut words, 2, first);
        set_word(&mut words, 3, second);
        hash_prepared(&words, 4, 2)
    } else {
        set_word(&mut words, 0, 2_u64 << 32);
        set_word(&mut words, 1, first);
        set_word(&mut words, 2, second);
        hash_prepared(&words, 3, 1)
    }
}

#[inline(never)]
fn fresh_iteration(shape: u8, first: u64, second: u64) -> Result<u64, capnp_message::ArenaError> {
    let mut arena = match shape {
        0 => ExclusiveArena::new(3, 3)?,
        1 => ExclusiveArena::new_segmented(1, 3, 2, 4)?,
        2 => ExclusiveArena::new_segmented(1, 2, 3, 5)?,
        _ => unreachable!(),
    };
    {
        let mut root = arena.init_root_struct(2, 0)?;
        root.set_u64(0, first, 0)?;
        root.set_u64(1, second, 0)?;
    }
    Ok(hash_segments(&arena))
}

#[inline(never)]
fn reuse_iteration(
    arena: &mut ExclusiveArena,
    first: u64,
    second: u64,
) -> Result<u64, capnp_message::ArenaError> {
    {
        let mut root = arena.init_root_struct(2, 0)?;
        root.set_u64(0, first, 0)?;
        root.set_u64(1, second, 0)?;
    }
    let fingerprint = hash_segments(arena);
    arena.reset()?;
    debug_assert_eq!(arena.as_segment(), &[0; 8]);
    Ok(fingerprint)
}

#[inline(never)]
fn copy_prepared_iteration(source: &[u8; 88]) -> u64 {
    let output = *black_box(source);
    hash_prepared(&output, 11, 1)
}

#[inline(never)]
fn copy_iteration(source: &MessageSegments<'_>) -> Result<u64, capnp_message::GraphError> {
    let mut arena = ExclusiveArena::new(11, 11)?;
    let budget = LocalTraversalBudget::new(16);
    arena.copy_root(source, WireLocation::ROOT, &budget, NestingLimit::new(8))?;
    Ok(hash_segments(&arena))
}

#[inline(never)]
fn copy_reuse_iteration(
    arena: &mut ExclusiveArena,
    source: &MessageSegments<'_>,
) -> Result<u64, capnp_message::GraphError> {
    let budget = LocalTraversalBudget::new(16);
    arena.copy_root(source, WireLocation::ROOT, &budget, NestingLimit::new(8))?;
    let fingerprint = hash_segments(arena);
    arena.reset()?;
    debug_assert_eq!(arena.as_segment(), &[0; 8]);
    Ok(fingerprint)
}

fn graph_fixture() -> [u8; 88] {
    let mut words = [0_u8; 88];
    set_word(&mut words, 0, 0x0001_0001_u64 << 32);
    set_word(&mut words, 1, VALUE);
    set_word(&mut words, 2, (514_u64 << 32) | 1);
    for (index, byte) in words[24..].iter_mut().enumerate() {
        *byte = (index as u8) ^ 0xa5;
    }
    words
}

fn set_word(bytes: &mut [u8], index: usize, value: u64) {
    let start = index * 8;
    bytes[start..start + 8].copy_from_slice(&value.to_le_bytes());
}

#[inline(always)]
fn hash_prepared(bytes: &[u8], word_count: usize, segment_count: usize) -> u64 {
    let mut hash =
        SEED ^ (segment_count as u64).rotate_left(17) ^ (word_count as u64).rotate_left(31);
    for word in black_box(bytes)[..word_count * 8].chunks_exact(8) {
        hash = hash.rotate_left(7) ^ read_word(word);
    }
    hash
}

#[inline(always)]
fn hash_segments(arena: &ExclusiveArena) -> u64 {
    let segments = black_box(arena).segments();
    let mut hash = SEED ^ (segments.len() as u64).rotate_left(17);
    for (index, segment) in segments.enumerate() {
        hash ^= (index as u64).rotate_left(7) ^ ((segment.len() / 8) as u64).rotate_left(31);
        for word in segment.chunks_exact(8) {
            hash = hash.rotate_left(7) ^ read_word(word);
        }
    }
    hash
}

#[inline(always)]
fn read_word(bytes: &[u8]) -> u64 {
    u64::from_le_bytes(
        bytes
            .try_into()
            .expect("word chunks are exactly eight bytes"),
    )
}
