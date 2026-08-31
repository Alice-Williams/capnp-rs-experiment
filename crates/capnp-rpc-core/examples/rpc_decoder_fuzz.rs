use std::error::Error;
use std::sync::Arc;
use std::time::{Duration, Instant};

use capnp_message::{OwnedMessage, ReaderLimits};
use capnp_rpc_core::{
    ProtocolLimits, RpcException, encode_abort, encode_bootstrap, encode_finish_with_options,
    encode_release, read_protocol_message_with_limits,
};

const DEFAULT_CASES: u64 = 100_000;
const DEFAULT_SECONDS: u64 = 60;
const MAX_SEGMENTS: usize = 4;
const MAX_WORDS_PER_SEGMENT: usize = 128;

fn main() -> Result<(), Box<dyn Error>> {
    let mut minimum_cases = DEFAULT_CASES;
    let mut duration = Duration::from_secs(DEFAULT_SECONDS);
    let mut seed = 0x6d34_305f_7270_6351_u64;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        let value = arguments
            .next()
            .ok_or_else(|| format!("missing value after {argument}"))?;
        match argument.as_str() {
            "--minimum-cases" => minimum_cases = value.parse()?,
            "--duration-seconds" => duration = Duration::from_secs(value.parse()?),
            "--seed" => seed = value.parse()?,
            _ => return Err(format!("unknown argument: {argument}").into()),
        }
    }

    let limits = ProtocolLimits {
        max_message_words: 4096,
        max_reason_bytes: 4096,
        max_trace_bytes: 4096,
        max_cap_table_entries: 64,
        max_pipeline_ops: 16,
        max_embargo_id_bytes: 256,
    };
    let seeds = seed_messages(limits)?;
    let mut random = Random::new(seed);
    let started = Instant::now();
    let mut cases = 0_u64;
    let mut accepted = 0_u64;

    while cases < minimum_cases || started.elapsed() < duration {
        let message = if random.next_u64() % 2 == 0 {
            mutate_seed(&seeds, &mut random)?
        } else {
            arbitrary_message(&mut random)?
        };
        if let Ok(message) = message {
            if read_protocol_message_with_limits(message, limits).is_ok() {
                accepted = accepted.saturating_add(1);
            }
        }
        cases = cases.saturating_add(1);
    }

    println!(
        "m40-rpc-decoder-fuzz-ok cases={cases} accepted={accepted} rejected={} seed={seed} elapsed_ms={}",
        cases.saturating_sub(accepted),
        started.elapsed().as_millis()
    );
    Ok(())
}

fn seed_messages(limits: ProtocolLimits) -> Result<Vec<Arc<OwnedMessage>>, Box<dyn Error>> {
    Ok(vec![
        encode_abort(
            &RpcException::new("seed", capnp_rpc_core::ExceptionType::Failed),
            limits,
        )?,
        encode_bootstrap(7, limits)?,
        encode_finish_with_options(9, true, false, limits)?,
        encode_release(11, 3, limits)?,
    ])
}

fn mutate_seed(
    seeds: &[Arc<OwnedMessage>],
    random: &mut Random,
) -> Result<Result<Arc<OwnedMessage>, capnp_message::ValidationError>, Box<dyn Error>> {
    let seed_index = random.index(seeds.len())?;
    let seed = &seeds[seed_index];
    let mut segments = Vec::with_capacity(seed.segment_count());
    for index in 0..seed.segment_count() {
        let id = u32::try_from(index)?;
        let segment = seed
            .segment(id)
            .ok_or_else(|| format!("seed segment {index} disappeared"))?;
        segments.push(segment.to_vec());
    }
    let mutation_count = 1 + random.index(8)?;
    for _ in 0..mutation_count {
        let segment_index = random.index(segments.len())?;
        let byte_index = random.index(segments[segment_index].len())?;
        segments[segment_index][byte_index] ^= random.next_u64() as u8 | 1;
    }
    Ok(OwnedMessage::new(segments, fuzz_reader_limits()))
}

fn arbitrary_message(
    random: &mut Random,
) -> Result<Result<Arc<OwnedMessage>, capnp_message::ValidationError>, Box<dyn Error>> {
    let segment_count = 1 + random.index(MAX_SEGMENTS)?;
    let mut segments = Vec::with_capacity(segment_count);
    for _ in 0..segment_count {
        let words = 1 + random.index(MAX_WORDS_PER_SEGMENT)?;
        let mut bytes = vec![0_u8; words * 8];
        for chunk in bytes.chunks_mut(8) {
            chunk.copy_from_slice(&random.next_u64().to_le_bytes());
        }
        segments.push(bytes);
    }
    Ok(OwnedMessage::new(segments, fuzz_reader_limits()))
}

const fn fuzz_reader_limits() -> ReaderLimits {
    ReaderLimits {
        traversal_words: 4096,
        nesting_levels: 32,
    }
}

struct Random(u64);

impl Random {
    fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    fn next_u64(&mut self) -> u64 {
        let mut value = self.0;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.0 = value;
        value
    }

    fn index(&mut self, upper: usize) -> Result<usize, Box<dyn Error>> {
        if upper == 0 {
            return Err("random index requested for an empty range".into());
        }
        let upper = u64::try_from(upper)?;
        Ok(usize::try_from(self.next_u64() % upper)?)
    }
}
