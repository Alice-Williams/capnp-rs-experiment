use std::hint::black_box;

use capnp_wire::{Word, WordSlice, WordSliceMut, read_u64_le, write_u64_le};

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

    let checksum = match mode.as_str() {
        "checked-read" => checked_read(&bytes, passes)?,
        "word-read" => word_read(&bytes, passes)?,
        "validated-read" => validated_read(&bytes, passes)?,
        "word-array-read" => word_array_read(&bytes, passes)?,
        "checked-write" => checked_write(&mut bytes, passes)?,
        "word-write" => word_write(&mut bytes, passes)?,
        "validated-write" => validated_write(&mut bytes, passes)?,
        "word-array-write" => word_array_write(&bytes, passes)?,
        _ => return Err("unknown benchmark mode".into()),
    };
    println!("{checksum}");
    Ok(())
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

fn word_array_read(bytes: &[u8], passes: usize) -> Result<u64, capnp_wire::WireError> {
    let words = collect_words(bytes)?;
    let mut checksum = SEED;
    for _ in 0..passes {
        for word in &words {
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

fn word_array_write(bytes: &[u8], passes: usize) -> Result<u64, capnp_wire::WireError> {
    let mut words = collect_words(bytes)?;
    let mut state = SEED;
    for _ in 0..passes {
        for word in &mut words {
            state = xorshift(state);
            word.set(state);
        }
    }
    checksum_words(&words)
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
