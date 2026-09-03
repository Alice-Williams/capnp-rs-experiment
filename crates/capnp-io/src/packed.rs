//! Incremental Cap'n Proto packed encoding.
//!
//! This follows the pinned C++ `serialize-packed.c++` implementation: each
//! word starts with a nonzero-byte tag, tag zero carries up to 255 additional
//! zero words, and tag `0xff` carries up to 255 additional raw words. The raw
//! run continues while a word has at most one zero byte, exactly matching the
//! reference encoder's size heuristic.
//!
//! Chunk boundaries are not format boundaries. The encoder retains at most a
//! partial word and one bounded run; the decoder retains only its current tag
//! state. Both enforce output limits before extending their output. Message
//! framing, async I/O, and no-allocation caller buffers belong to M16.

use core::fmt;

use alloc::vec::Vec;

const WORD_BYTES: usize = 8;
const MAX_RUN_WORDS: usize = u8::MAX as usize;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PackedError {
    OutputLimit { requested: usize, limit: usize },
    UnalignedInput { trailing_bytes: usize },
    TruncatedWord { tag: u8, missing_bytes: u8 },
    MissingRunLength { tag: u8 },
    TruncatedRawRun { remaining_bytes: usize },
    PreviousFailure,
}

impl fmt::Display for PackedError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl core::error::Error for PackedError {}

#[derive(Debug)]
enum EncodeRun {
    None,
    Zero { additional_words: u8 },
    Raw { words: Vec<u8> },
}

/// Incremental, byte-chunk-independent packed encoder.
#[derive(Debug)]
pub struct PackedEncoder {
    output: Vec<u8>,
    max_output_bytes: usize,
    partial: [u8; WORD_BYTES],
    partial_len: usize,
    run: EncodeRun,
    failed: bool,
}

impl PackedEncoder {
    pub const fn new(max_output_bytes: usize) -> Self {
        Self {
            output: Vec::new(),
            max_output_bytes,
            partial: [0; WORD_BYTES],
            partial_len: 0,
            run: EncodeRun::None,
            failed: false,
        }
    }

    /// Consumes an arbitrary input chunk. Only `finish()` requires the total
    /// input length to be word-aligned.
    pub fn push(&mut self, mut input: &[u8]) -> Result<(), PackedError> {
        if self.failed {
            return Err(PackedError::PreviousFailure);
        }
        let result = self.push_inner(&mut input);
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    fn push_inner(&mut self, input: &mut &[u8]) -> Result<(), PackedError> {
        if self.partial_len != 0 {
            let copied = (WORD_BYTES - self.partial_len).min(input.len());
            self.partial[self.partial_len..self.partial_len + copied]
                .copy_from_slice(&input[..copied]);
            self.partial_len += copied;
            *input = &input[copied..];
            if self.partial_len != WORD_BYTES {
                return Ok(());
            }
            let word = self.partial;
            self.partial_len = 0;
            self.process_word(word)?;
        }

        let mut words = input.chunks_exact(WORD_BYTES);
        for chunk in &mut words {
            let mut word = [0_u8; WORD_BYTES];
            word.copy_from_slice(chunk);
            self.process_word(word)?;
        }
        let remainder = words.remainder();
        self.partial[..remainder.len()].copy_from_slice(remainder);
        self.partial_len = remainder.len();
        Ok(())
    }

    fn process_word(&mut self, word: [u8; WORD_BYTES]) -> Result<(), PackedError> {
        loop {
            match &mut self.run {
                EncodeRun::Zero { additional_words }
                    if word.iter().all(|byte| *byte == 0)
                        && usize::from(*additional_words) < MAX_RUN_WORDS =>
                {
                    check_output_limit(self.output.len(), 2, self.max_output_bytes)?;
                    *additional_words += 1;
                    if usize::from(*additional_words) == MAX_RUN_WORDS {
                        self.flush_run()?;
                    }
                    return Ok(());
                }
                EncodeRun::Raw { words }
                    if zero_byte_count(&word) <= 1
                        && words.len() / WORD_BYTES - 1 < MAX_RUN_WORDS =>
                {
                    let projected = 2_usize
                        .checked_add(words.len())
                        .and_then(|value| value.checked_add(WORD_BYTES))
                        .ok_or(PackedError::OutputLimit {
                            requested: usize::MAX,
                            limit: self.max_output_bytes,
                        })?;
                    check_output_limit(self.output.len(), projected, self.max_output_bytes)?;
                    words.extend_from_slice(&word);
                    if words.len() / WORD_BYTES - 1 == MAX_RUN_WORDS {
                        self.flush_run()?;
                    }
                    return Ok(());
                }
                EncodeRun::None => {}
                EncodeRun::Zero { .. } | EncodeRun::Raw { .. } => {
                    self.flush_run()?;
                    continue;
                }
            }

            let tag = word_tag(&word);
            if tag == 0 {
                self.check_projected(2)?;
                self.run = EncodeRun::Zero {
                    additional_words: 0,
                };
            } else if tag == u8::MAX {
                self.check_projected(2 + WORD_BYTES)?;
                self.run = EncodeRun::Raw {
                    words: word.to_vec(),
                };
            } else {
                let encoded_len = 1 + tag.count_ones() as usize;
                self.check_projected(encoded_len)?;
                self.output.push(tag);
                self.output
                    .extend(word.into_iter().filter(|byte| *byte != 0));
            }
            return Ok(());
        }
    }

    fn check_projected(&self, pending_bytes: usize) -> Result<(), PackedError> {
        check_output_limit(self.output.len(), pending_bytes, self.max_output_bytes)
    }

    fn flush_run(&mut self) -> Result<(), PackedError> {
        let run = core::mem::replace(&mut self.run, EncodeRun::None);
        match run {
            EncodeRun::None => Ok(()),
            EncodeRun::Zero { additional_words } => {
                self.check_projected(2)?;
                self.output.extend_from_slice(&[0, additional_words]);
                Ok(())
            }
            EncodeRun::Raw { words } => {
                let additional_words = words.len() / WORD_BYTES - 1;
                let encoded_len =
                    2_usize
                        .checked_add(words.len())
                        .ok_or(PackedError::OutputLimit {
                            requested: usize::MAX,
                            limit: self.max_output_bytes,
                        })?;
                self.check_projected(encoded_len)?;
                self.output.push(u8::MAX);
                self.output.extend_from_slice(&words[..WORD_BYTES]);
                self.output.push(
                    u8::try_from(additional_words)
                        .expect("raw run is capped at the one-byte format limit"),
                );
                self.output.extend_from_slice(&words[WORD_BYTES..]);
                Ok(())
            }
        }
    }

    pub fn finish(mut self) -> Result<Vec<u8>, PackedError> {
        if self.failed {
            return Err(PackedError::PreviousFailure);
        }
        if self.partial_len != 0 {
            return Err(PackedError::UnalignedInput {
                trailing_bytes: self.partial_len,
            });
        }
        self.flush_run()?;
        Ok(self.output)
    }
}

#[derive(Debug)]
enum DecodeState {
    Tag,
    Word {
        tag: u8,
        next_byte: u8,
        remaining_payload: u8,
        word: [u8; WORD_BYTES],
    },
    ZeroCount,
    RawCount,
    Raw {
        remaining_bytes: usize,
    },
}

/// Incremental decoder for an unframed packed byte stream.
#[derive(Debug)]
pub struct PackedDecoder {
    output: Vec<u8>,
    max_output_bytes: usize,
    state: DecodeState,
    failed: bool,
}

impl PackedDecoder {
    pub const fn new(max_output_bytes: usize) -> Self {
        Self {
            output: Vec::new(),
            max_output_bytes,
            state: DecodeState::Tag,
            failed: false,
        }
    }

    pub fn push(&mut self, input: &[u8]) -> Result<(), PackedError> {
        if self.failed {
            return Err(PackedError::PreviousFailure);
        }
        let result = self.push_inner(input);
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    fn push_inner(&mut self, input: &[u8]) -> Result<(), PackedError> {
        let mut cursor = 0;
        while cursor < input.len() {
            match &mut self.state {
                DecodeState::Tag => {
                    let tag = input[cursor];
                    cursor += 1;
                    self.ensure_output(WORD_BYTES)?;
                    if tag == 0 {
                        self.output.extend_from_slice(&[0; WORD_BYTES]);
                        self.state = DecodeState::ZeroCount;
                    } else {
                        self.state = DecodeState::Word {
                            tag,
                            next_byte: 0,
                            remaining_payload: tag.count_ones() as u8,
                            word: [0; WORD_BYTES],
                        };
                    }
                }
                DecodeState::Word {
                    tag,
                    next_byte,
                    remaining_payload,
                    word,
                } => {
                    while *next_byte < WORD_BYTES as u8 {
                        let mask = 1_u8 << *next_byte;
                        if *tag & mask != 0 {
                            if cursor == input.len() {
                                break;
                            }
                            word[usize::from(*next_byte)] = input[cursor];
                            cursor += 1;
                            *remaining_payload -= 1;
                        }
                        *next_byte += 1;
                    }
                    if *next_byte == WORD_BYTES as u8 {
                        debug_assert_eq!(*remaining_payload, 0);
                        self.output.extend_from_slice(word);
                        self.state = if *tag == u8::MAX {
                            DecodeState::RawCount
                        } else {
                            DecodeState::Tag
                        };
                    }
                }
                DecodeState::ZeroCount => {
                    let words = usize::from(input[cursor]);
                    cursor += 1;
                    let bytes = words
                        .checked_mul(WORD_BYTES)
                        .ok_or(PackedError::OutputLimit {
                            requested: usize::MAX,
                            limit: self.max_output_bytes,
                        })?;
                    self.ensure_output(bytes)?;
                    self.output.resize(self.output.len() + bytes, 0);
                    self.state = DecodeState::Tag;
                }
                DecodeState::RawCount => {
                    let bytes = usize::from(input[cursor]) * WORD_BYTES;
                    cursor += 1;
                    self.ensure_output(bytes)?;
                    self.state = if bytes == 0 {
                        DecodeState::Tag
                    } else {
                        DecodeState::Raw {
                            remaining_bytes: bytes,
                        }
                    };
                }
                DecodeState::Raw { remaining_bytes } => {
                    let copied = (*remaining_bytes).min(input.len() - cursor);
                    self.output
                        .extend_from_slice(&input[cursor..cursor + copied]);
                    cursor += copied;
                    *remaining_bytes -= copied;
                    if *remaining_bytes == 0 {
                        self.state = DecodeState::Tag;
                    }
                }
            }
        }
        Ok(())
    }

    fn ensure_output(&self, additional: usize) -> Result<(), PackedError> {
        let requested =
            self.output
                .len()
                .checked_add(additional)
                .ok_or(PackedError::OutputLimit {
                    requested: usize::MAX,
                    limit: self.max_output_bytes,
                })?;
        if requested > self.max_output_bytes {
            Err(PackedError::OutputLimit {
                requested,
                limit: self.max_output_bytes,
            })
        } else {
            Ok(())
        }
    }

    pub fn finish(self) -> Result<Vec<u8>, PackedError> {
        if self.failed {
            return Err(PackedError::PreviousFailure);
        }
        match self.state {
            DecodeState::Tag => Ok(self.output),
            DecodeState::Word {
                tag,
                remaining_payload,
                ..
            } => Err(PackedError::TruncatedWord {
                tag,
                missing_bytes: remaining_payload,
            }),
            DecodeState::ZeroCount => Err(PackedError::MissingRunLength { tag: 0 }),
            DecodeState::RawCount => Err(PackedError::MissingRunLength { tag: u8::MAX }),
            DecodeState::Raw { remaining_bytes } => {
                Err(PackedError::TruncatedRawRun { remaining_bytes })
            }
        }
    }
}

/// Packs a complete word-aligned byte slice with an explicit output bound.
///
/// ```
/// use capnp_io::{PackedEncoder, pack, unpack};
///
/// let words = [0_u8; 16];
/// assert_eq!(pack(&words, 4)?, [0, 1]);
/// assert_eq!(unpack(&[0, 1], words.len())?, words);
///
/// let mut streaming = PackedEncoder::new(4);
/// streaming.push(&words[..3])?;
/// streaming.push(&words[3..])?;
/// assert_eq!(streaming.finish()?, [0, 1]);
/// # Ok::<(), capnp_io::PackedError>(())
/// ```
pub fn pack(input: &[u8], max_output_bytes: usize) -> Result<Vec<u8>, PackedError> {
    let mut output = Vec::new();
    let complete_bytes = input.len() / WORD_BYTES * WORD_BYTES;
    let mut offset = 0;
    while offset < complete_bytes {
        let word = &input[offset..offset + WORD_BYTES];
        let tag = word_tag_slice(word);
        if tag == 0 {
            check_output_limit(output.len(), 2, max_output_bytes)?;
            let mut additional_words = 0;
            let mut next = offset + WORD_BYTES;
            while additional_words < MAX_RUN_WORDS
                && next < complete_bytes
                && word_is_zero(&input[next..next + WORD_BYTES])
            {
                additional_words += 1;
                next += WORD_BYTES;
            }
            output.reserve(2);
            output.extend_from_slice(&[
                0,
                u8::try_from(additional_words).expect("zero run is capped at the format limit"),
            ]);
            offset = next;
        } else if tag == u8::MAX {
            let run_start = offset + WORD_BYTES;
            let mut next = run_start;
            let mut additional_words = 0;
            while additional_words < MAX_RUN_WORDS
                && next < complete_bytes
                && zero_byte_count_slice(&input[next..next + WORD_BYTES]) <= 1
            {
                additional_words += 1;
                next += WORD_BYTES;
            }
            let encoded_len = 2 + WORD_BYTES + additional_words * WORD_BYTES;
            check_raw_output_limit(output.len(), encoded_len, max_output_bytes)?;
            output.reserve(encoded_len);
            output.push(u8::MAX);
            output.extend_from_slice(word);
            output.push(
                u8::try_from(additional_words).expect("raw run is capped at the format limit"),
            );
            output.extend_from_slice(&input[run_start..next]);
            offset = next;
        } else {
            let output_len = 1 + tag.count_ones() as usize;
            check_output_limit(output.len(), output_len, max_output_bytes)?;
            let mut encoded = [0_u8; 1 + WORD_BYTES];
            encoded[0] = tag;
            let mut encoded_len = 1;
            for byte in word {
                if *byte != 0 {
                    encoded[encoded_len] = *byte;
                    encoded_len += 1;
                }
            }
            output.extend_from_slice(&encoded[..encoded_len]);
            offset += WORD_BYTES;
        }
    }
    if complete_bytes != input.len() {
        return Err(PackedError::UnalignedInput {
            trailing_bytes: input.len() - complete_bytes,
        });
    }
    Ok(output)
}

pub fn unpack(input: &[u8], max_output_bytes: usize) -> Result<Vec<u8>, PackedError> {
    let mut output = Vec::new();
    let mut cursor = 0;
    while cursor < input.len() {
        let tag = input[cursor];
        cursor += 1;
        ensure_output_limit(output.len(), WORD_BYTES, max_output_bytes)?;
        if tag == 0 {
            output.resize(output.len() + WORD_BYTES, 0);
            let additional_words = *input
                .get(cursor)
                .ok_or(PackedError::MissingRunLength { tag: 0 })?;
            cursor += 1;
            let additional_bytes = usize::from(additional_words) * WORD_BYTES;
            ensure_output_limit(output.len(), additional_bytes, max_output_bytes)?;
            output.resize(output.len() + additional_bytes, 0);
        } else {
            let payload_bytes = tag.count_ones() as usize;
            let available = input.len() - cursor;
            if available < payload_bytes {
                return Err(PackedError::TruncatedWord {
                    tag,
                    missing_bytes: (payload_bytes - available) as u8,
                });
            }
            let output_start = output.len();
            output.resize(output_start + WORD_BYTES, 0);
            let mut payload = cursor;
            for lane in 0..WORD_BYTES {
                if tag & (1 << lane) != 0 {
                    output[output_start + lane] = input[payload];
                    payload += 1;
                }
            }
            cursor = payload;

            if tag == u8::MAX {
                let additional_words = *input
                    .get(cursor)
                    .ok_or(PackedError::MissingRunLength { tag: u8::MAX })?;
                cursor += 1;
                let raw_bytes = usize::from(additional_words) * WORD_BYTES;
                ensure_output_limit(output.len(), raw_bytes, max_output_bytes)?;
                let available = input.len() - cursor;
                if available < raw_bytes {
                    return Err(PackedError::TruncatedRawRun {
                        remaining_bytes: raw_bytes - available,
                    });
                }
                output.extend_from_slice(&input[cursor..cursor + raw_bytes]);
                cursor += raw_bytes;
            }
        }
    }
    Ok(output)
}

fn check_raw_output_limit(
    current: usize,
    encoded_len: usize,
    limit: usize,
) -> Result<(), PackedError> {
    let first_word = current
        .checked_add(2 + WORD_BYTES)
        .ok_or(PackedError::OutputLimit {
            requested: usize::MAX,
            limit,
        })?;
    if first_word > limit {
        return Err(PackedError::OutputLimit {
            requested: first_word,
            limit,
        });
    }
    let requested = current
        .checked_add(encoded_len)
        .ok_or(PackedError::OutputLimit {
            requested: usize::MAX,
            limit,
        })?;
    if requested <= limit {
        return Ok(());
    }
    let additional_that_fit = (limit - first_word) / WORD_BYTES;
    let requested = additional_that_fit
        .checked_add(1)
        .and_then(|words| words.checked_mul(WORD_BYTES))
        .and_then(|bytes| first_word.checked_add(bytes))
        .unwrap_or(usize::MAX);
    Err(PackedError::OutputLimit { requested, limit })
}

fn ensure_output_limit(current: usize, additional: usize, limit: usize) -> Result<(), PackedError> {
    check_output_limit(current, additional, limit)
}

fn word_tag(word: &[u8; WORD_BYTES]) -> u8 {
    word.iter().enumerate().fold(0, |tag, (index, byte)| {
        tag | (u8::from(*byte != 0) << index)
    })
}

fn word_tag_slice(word: &[u8]) -> u8 {
    u8::from(word[0] != 0)
        | (u8::from(word[1] != 0) << 1)
        | (u8::from(word[2] != 0) << 2)
        | (u8::from(word[3] != 0) << 3)
        | (u8::from(word[4] != 0) << 4)
        | (u8::from(word[5] != 0) << 5)
        | (u8::from(word[6] != 0) << 6)
        | (u8::from(word[7] != 0) << 7)
}

fn word_is_zero(word: &[u8]) -> bool {
    word == [0; WORD_BYTES]
}

fn zero_byte_count(word: &[u8; WORD_BYTES]) -> usize {
    word.iter().filter(|byte| **byte == 0).count()
}

fn zero_byte_count_slice(word: &[u8]) -> usize {
    word.iter().filter(|byte| **byte == 0).count()
}

fn check_output_limit(current: usize, additional: usize, limit: usize) -> Result<(), PackedError> {
    let requested = current
        .checked_add(additional)
        .ok_or(PackedError::OutputLimit {
            requested: usize::MAX,
            limit,
        })?;
    if requested > limit {
        Err(PackedError::OutputLimit { requested, limit })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    const CPP_UNPACKED: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../conformance/fixtures/cpp/",
        "e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/wire-unpacked.bin"
    ));
    const CPP_PACKED: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../conformance/fixtures/cpp/",
        "e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/wire-packed.bin"
    ));

    fn assert_vector(unpacked: &[u8], packed: &[u8]) {
        assert_eq!(pack(unpacked, usize::MAX).expect("packs"), packed);
        assert_eq!(unpack(packed, usize::MAX).expect("unpacks"), unpacked);
    }

    #[test]
    fn exact_reference_vectors_cover_every_tag_and_run() {
        assert_vector(&[], &[]);
        assert_vector(&[0; 8], &[0, 0]);
        assert_vector(&[0, 0, 12, 0, 0, 34, 0, 0], &[0x24, 12, 34]);
        assert_vector(
            &[1, 3, 2, 4, 5, 7, 6, 8],
            &[0xff, 1, 3, 2, 4, 5, 7, 6, 8, 0],
        );
        assert_vector(
            &[
                8, 0, 100, 6, 0, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 1, 0, 2, 0, 3, 1,
            ],
            &[0xed, 8, 100, 6, 1, 1, 2, 0, 2, 0xd4, 1, 2, 3, 1],
        );
    }

    #[test]
    fn pinned_cpp_message_is_byte_exact_in_both_directions() {
        assert_eq!(
            pack(CPP_UNPACKED, CPP_PACKED.len()),
            Ok(CPP_PACKED.to_vec())
        );
        assert_eq!(
            unpack(CPP_PACKED, CPP_UNPACKED.len()),
            Ok(CPP_UNPACKED.to_vec())
        );
    }

    #[test]
    fn every_input_and_output_chunk_boundary_is_equivalent() {
        let expected = pack(CPP_UNPACKED, usize::MAX).expect("fixture packs");
        for chunk_size in 1..=CPP_UNPACKED.len() + 1 {
            let mut encoder = PackedEncoder::new(expected.len());
            for chunk in CPP_UNPACKED.chunks(chunk_size) {
                encoder.push(chunk).expect("chunk packs");
            }
            assert_eq!(encoder.finish().expect("encoder finishes"), expected);
        }
        for chunk_size in 1..=expected.len() + 1 {
            let mut decoder = PackedDecoder::new(CPP_UNPACKED.len());
            for chunk in expected.chunks(chunk_size) {
                decoder.push(chunk).expect("chunk unpacks");
            }
            assert_eq!(decoder.finish().expect("decoder finishes"), CPP_UNPACKED);
        }
    }

    #[test]
    fn deterministic_random_words_round_trip_across_chunks() {
        let mut state = 0x4d59_5df4_d0f3_3173_u64;
        for words in 0..400 {
            let mut input = vec![0_u8; words * WORD_BYTES];
            for byte in &mut input {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                *byte = if state & 3 == 0 { 0 } else { state as u8 };
            }
            let packed = pack(&input, input.len().saturating_mul(2).saturating_add(2))
                .expect("random input packs");
            assert_eq!(
                unpack(&packed, input.len()).expect("random input unpacks"),
                input
            );
        }
    }

    #[test]
    fn run_counts_split_at_the_one_byte_limit() {
        let zeros = vec![0_u8; 257 * WORD_BYTES];
        assert_eq!(pack(&zeros, 4).expect("zeros pack"), [0, 255, 0, 0]);

        let raw = vec![1_u8; 257 * WORD_BYTES];
        let packed = pack(&raw, raw.len() + 20).expect("raw words pack");
        assert_eq!(packed[0], u8::MAX);
        assert_eq!(packed[9], u8::MAX);
        assert_eq!(packed[2050], u8::MAX);
        assert_eq!(packed[2059], 0);
        assert_eq!(unpack(&packed, raw.len()).expect("raw words unpack"), raw);
    }

    #[test]
    fn limits_and_truncation_fail_without_unbounded_growth() {
        assert_eq!(
            pack(&[1; 8], 9),
            Err(PackedError::OutputLimit {
                requested: 10,
                limit: 9,
            })
        );
        assert_eq!(
            pack(&[1; 7], 100),
            Err(PackedError::UnalignedInput { trailing_bytes: 7 })
        );
        assert_eq!(
            unpack(&[0], 100),
            Err(PackedError::MissingRunLength { tag: 0 })
        );
        assert_eq!(
            unpack(&[0xff, 1], 100),
            Err(PackedError::TruncatedWord {
                tag: 0xff,
                missing_bytes: 7,
            })
        );
        assert_eq!(
            unpack(&[0xff, 1, 2, 3, 4, 5, 6, 7, 8], 100),
            Err(PackedError::MissingRunLength { tag: 0xff })
        );
        assert_eq!(
            unpack(&[0xff, 1, 2, 3, 4, 5, 6, 7, 8, 1, 9], 100),
            Err(PackedError::TruncatedRawRun { remaining_bytes: 7 })
        );
        assert_eq!(
            unpack(&[0, 255], 8),
            Err(PackedError::OutputLimit {
                requested: 2_048,
                limit: 8,
            })
        );

        let mut decoder = PackedDecoder::new(8);
        assert!(decoder.push(&[0, 1]).is_err());
        assert_eq!(decoder.push(&[]), Err(PackedError::PreviousFailure));
        let mut encoder = PackedEncoder::new(1);
        assert!(encoder.push(&[0; 8]).is_err());
        assert_eq!(encoder.push(&[]), Err(PackedError::PreviousFailure));
    }

    #[test]
    fn arbitrary_packed_bytes_always_terminate_under_the_output_bound() {
        let mut state = 0xa076_1d64_78bd_642f_u64;
        for length in 0..512 {
            let mut packed = vec![0_u8; length];
            for byte in &mut packed {
                state ^= state << 7;
                state ^= state >> 9;
                state ^= state << 8;
                *byte = state as u8;
            }
            if let Ok(output) = unpack(&packed, 4_096) {
                assert!(output.len() <= 4_096);
                assert_eq!(output.len() % WORD_BYTES, 0);
            }
        }
    }

    #[test]
    fn one_shot_fast_paths_preserve_streaming_results_and_errors() {
        let inputs = [
            vec![0_u8; 264 * WORD_BYTES],
            vec![1_u8; 264 * WORD_BYTES],
            CPP_UNPACKED.repeat(17),
        ];
        for input in &inputs {
            let expected = pack(input, usize::MAX).expect("fixture packs");
            for limit in 0..=expected.len() + WORD_BYTES {
                let mut encoder = PackedEncoder::new(limit);
                let streaming = match encoder.push(input) {
                    Ok(()) => encoder.finish(),
                    Err(error) => Err(error),
                };
                assert_eq!(pack(input, limit), streaming);
            }
        }

        let mut state = 0xd6e8_feb8_6659_fd93_u64;
        for length in 0..128 {
            let mut packed = vec![0_u8; length];
            for byte in &mut packed {
                state = xorshift_for_test(state);
                *byte = state as u8;
            }
            for limit in [0, 8, 64, 512] {
                let mut decoder = PackedDecoder::new(limit);
                let streaming = packed
                    .chunks(3)
                    .try_for_each(|chunk| decoder.push(chunk))
                    .and_then(|()| decoder.finish());
                assert_eq!(unpack(&packed, limit), streaming);
            }
        }
    }

    fn xorshift_for_test(mut value: u64) -> u64 {
        value ^= value << 13;
        value ^= value >> 7;
        value ^ (value << 17)
    }
}
