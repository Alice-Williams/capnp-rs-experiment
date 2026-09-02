use std::hint::black_box;
use std::time::Instant;

use capnp_wire::{
    ElementSize, PointerKind, WirePointer, Word, WordSlice, WordSliceMut, read_u64_le, write_u64_le,
};

const SEED: u64 = 0x4d59_5df4_d0f3_3173;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args().skip(1);
    let mode = arguments
        .next()
        .ok_or("usage: wire_value_benchmark MODE WORDS PASSES")?;
    let words = arguments
        .next()
        .ok_or("missing word count")?
        .parse::<usize>()?;
    let passes = arguments
        .next()
        .ok_or("missing pass count")?
        .parse::<usize>()?;
    if arguments.next().is_some() || words == 0 || passes == 0 {
        return Err("expected a mode and positive WORDS/PASSES".into());
    }
    let byte_len = words.checked_mul(8).ok_or("byte length overflow")?;
    let mut bytes = vec![0_u8; byte_len];
    fill(&mut bytes)?;

    let (elapsed_ns, checksum) = match mode.as_str() {
        "checked-read" => measure(|| checked_read(&bytes, passes))?,
        "word-read" => measure(|| word_read(&bytes, passes))?,
        "validated-read" => measure(|| validated_read(&bytes, passes))?,
        "word-array-read" => {
            let words = collect_words(&bytes)?;
            measure(|| word_array_read(&words, passes))?
        }
        "checked-write" => measure(|| checked_write(&mut bytes, passes))?,
        "word-write" => measure(|| word_write(&mut bytes, passes))?,
        "validated-write" => measure(|| validated_write(&mut bytes, passes))?,
        "word-array-write" => {
            let mut words = collect_words(&bytes)?;
            measure(|| word_array_write(&mut words, passes))?
        }
        "pointer-decode" => {
            let pointers = pointer_inputs(words);
            measure(|| pointer_decode(&pointers, passes))?
        }
        "pointer-encode" => {
            let mut pointers = vec![WirePointer::NULL; words];
            measure(|| pointer_encode(&mut pointers, passes))?
        }
        _ => return Err("unknown benchmark mode".into()),
    };
    println!("{elapsed_ns}\t{checksum}");
    Ok(())
}

fn measure(
    operation: impl FnOnce() -> Result<u64, capnp_wire::WireError>,
) -> Result<(u128, u64), capnp_wire::WireError> {
    let started = Instant::now();
    let checksum = operation()?;
    Ok((started.elapsed().as_nanos(), checksum))
}

fn fill(bytes: &mut [u8]) -> Result<(), capnp_wire::WireError> {
    let mut state = SEED ^ u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    for offset in (0..bytes.len()).step_by(8) {
        state = xorshift(state);
        write_u64_le(bytes, offset, state)?;
    }
    Ok(())
}

fn checked_read(bytes: &[u8], passes: usize) -> Result<u64, capnp_wire::WireError> {
    let mut checksum = SEED;
    for _ in 0..passes {
        for offset in (0..bytes.len()).step_by(8) {
            checksum = checksum.rotate_left(7) ^ read_u64_le(bytes, offset)?;
        }
    }
    Ok(black_box(checksum))
}

fn word_read(bytes: &[u8], passes: usize) -> Result<u64, capnp_wire::WireError> {
    let mut checksum = SEED;
    for _ in 0..passes {
        for offset in (0..bytes.len()).step_by(8) {
            checksum = checksum.rotate_left(7) ^ Word::read_from(bytes, offset)?.get();
        }
    }
    Ok(black_box(checksum))
}

fn validated_read(bytes: &[u8], passes: usize) -> Result<u64, capnp_wire::WireError> {
    let words = WordSlice::new(bytes)?;
    let mut checksum = SEED;
    for _ in 0..passes {
        for word in words {
            checksum = checksum.rotate_left(7) ^ word.get();
        }
    }
    Ok(black_box(checksum))
}

fn word_array_read(words: &[Word], passes: usize) -> Result<u64, capnp_wire::WireError> {
    let mut checksum = SEED;
    for _ in 0..passes {
        for word in words {
            checksum = checksum.rotate_left(7) ^ word.get();
        }
    }
    Ok(black_box(checksum))
}

fn checked_write(bytes: &mut [u8], passes: usize) -> Result<u64, capnp_wire::WireError> {
    let mut state = SEED;
    for _ in 0..passes {
        for offset in (0..bytes.len()).step_by(8) {
            state = xorshift(state);
            write_u64_le(bytes, offset, state)?;
        }
    }
    checksum(bytes)
}

fn word_write(bytes: &mut [u8], passes: usize) -> Result<u64, capnp_wire::WireError> {
    let mut state = SEED;
    for _ in 0..passes {
        for offset in (0..bytes.len()).step_by(8) {
            state = xorshift(state);
            Word::from_u64(state).write_to(bytes, offset)?;
        }
    }
    checksum(bytes)
}

fn validated_write(bytes: &mut [u8], passes: usize) -> Result<u64, capnp_wire::WireError> {
    let mut words = WordSliceMut::new(bytes)?;
    let mut state = SEED;
    for _ in 0..passes {
        for slot in words.iter_mut() {
            state = xorshift(state);
            slot.set(Word::from_u64(state));
        }
    }
    checksum(bytes)
}

fn word_array_write(words: &mut [Word], passes: usize) -> Result<u64, capnp_wire::WireError> {
    let mut state = SEED;
    for _ in 0..passes {
        for word in &mut *words {
            state = xorshift(state);
            word.set(state);
        }
    }
    checksum_words(words)
}

fn collect_words(bytes: &[u8]) -> Result<Vec<Word>, capnp_wire::WireError> {
    Ok(WordSlice::new(bytes)?.into_iter().collect())
}

fn checksum_words(words: &[Word]) -> Result<u64, capnp_wire::WireError> {
    let mut checksum = SEED;
    for word in words {
        checksum = checksum.rotate_left(7) ^ word.get();
    }
    Ok(black_box(checksum))
}

fn pointer_inputs(count: usize) -> Vec<WirePointer> {
    let mut state = SEED ^ u64::try_from(count.saturating_mul(8)).unwrap_or(u64::MAX);
    (0..count)
        .map(|_| {
            state = xorshift(state);
            WirePointer::from_word(Word::from_u64(state))
        })
        .collect()
}

fn pointer_decode(pointers: &[WirePointer], passes: usize) -> Result<u64, capnp_wire::WireError> {
    let mut checksum = SEED;
    for _ in 0..passes {
        for pointer in pointers {
            let fingerprint = match pointer.kind() {
                PointerKind::Struct => {
                    let fields = pointer.struct_fields().expect("kind was checked");
                    u64::from(fields.offset as u32)
                        ^ u64::from(fields.data_words).rotate_left(17)
                        ^ u64::from(fields.pointer_count).rotate_left(41)
                }
                PointerKind::List => {
                    let fields = pointer.list_fields().expect("kind was checked");
                    u64::from(fields.offset as u32)
                        ^ (fields.element_size as u64).rotate_left(13)
                        ^ u64::from(fields.count).rotate_left(29)
                }
                PointerKind::Far => {
                    let fields = pointer.far_fields().expect("kind was checked");
                    u64::from(fields.landing_pad_word)
                        ^ u64::from(fields.double_far).rotate_left(17)
                        ^ u64::from(fields.segment_id).rotate_left(31)
                }
                PointerKind::Other => pointer.capability_index().map_or_else(
                    || u64::from(pointer.lower32()),
                    |index| u64::from(index).rotate_left(23),
                ),
            };
            checksum = checksum.rotate_left(7) ^ fingerprint;
        }
    }
    Ok(black_box(checksum))
}

fn pointer_encode(
    pointers: &mut [WirePointer],
    passes: usize,
) -> Result<u64, capnp_wire::WireError> {
    let mut state = SEED;
    for _ in 0..passes {
        for (index, pointer) in pointers.iter_mut().enumerate() {
            state = xorshift(state);
            let lower = state as u32;
            let upper = (state >> 32) as u32;
            *pointer = match index & 3 {
                0 => WirePointer::new_struct(
                    (lower as i32) >> 2,
                    upper as u16,
                    (upper >> 16) as u16,
                )?,
                1 => WirePointer::new_list(
                    (lower as i32) >> 2,
                    ElementSize::ALL[(upper & 7) as usize],
                    upper >> 3,
                )?,
                2 => WirePointer::new_far((lower & 4) != 0, lower >> 3, upper)?,
                _ => WirePointer::new_capability(upper),
            };
        }
    }
    checksum_pointers(pointers)
}

fn checksum_pointers(pointers: &[WirePointer]) -> Result<u64, capnp_wire::WireError> {
    let mut checksum = SEED;
    for pointer in pointers {
        checksum = checksum.rotate_left(7) ^ pointer.raw();
    }
    Ok(black_box(checksum))
}

fn checksum(bytes: &[u8]) -> Result<u64, capnp_wire::WireError> {
    let mut checksum = SEED;
    for offset in (0..bytes.len()).step_by(8) {
        checksum = checksum.rotate_left(7) ^ read_u64_le(bytes, offset)?;
    }
    Ok(black_box(checksum))
}

const fn xorshift(mut value: u64) -> u64 {
    value ^= value << 13;
    value ^= value >> 7;
    value ^= value << 17;
    value
}
