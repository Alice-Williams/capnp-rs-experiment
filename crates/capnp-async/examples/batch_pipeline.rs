use std::error::Error;
use std::hint::black_box;
use std::time::{Duration, Instant};

use capnp_async::{BatchJob, BatchLimits, BatchOutput, run_ordered_batch};
use capnp_io::pack;

const DEFAULT_MESSAGES: usize = 32;
const DEFAULT_WORDS: usize = 1024;
const DEFAULT_ROUNDS: u32 = 128;
const DEFAULT_SAMPLES: usize = 5;
type AnyError = Box<dyn Error>;

fn parse_arg<T>(index: usize, default: T, name: &str) -> Result<T, AnyError>
where
    T: std::str::FromStr,
    T::Err: Error + 'static,
{
    std::env::args().nth(index).map_or(Ok(default), |value| {
        value.parse().map_err(|error: T::Err| {
            Box::<dyn Error>::from(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("invalid {name}: {error}"),
            ))
        })
    })
}

#[inline(never)]
fn mix(mut value: u64, rounds: u32) -> u64 {
    for round in 0..rounds {
        value ^= value >> 30;
        value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value ^= value >> 27;
        value = value.wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^= value >> 31;
        value = value.wrapping_add(u64::from(round));
    }
    black_box(value)
}

fn fixtures(messages: usize, words: usize) -> Vec<Vec<u8>> {
    (0..messages)
        .map(|message| {
            (0..words)
                .flat_map(|word| {
                    (u64::try_from(message).unwrap_or(u64::MAX) << 32
                        | u64::try_from(word).unwrap_or(u64::MAX))
                    .to_le_bytes()
                })
                .collect()
        })
        .collect()
}

fn run_once(
    fixtures: &[Vec<u8>],
    rounds: u32,
    workers: usize,
    threshold: usize,
) -> Result<(Duration, u64, usize), AnyError> {
    let jobs = fixtures
        .iter()
        .cloned()
        .map(|input| {
            let packed_bound = input.len().saturating_add(input.len().div_ceil(8));
            BatchJob::new(input, packed_bound.saturating_mul(2))
        })
        .collect::<Vec<_>>();
    let mut checksum = 0xcbf2_9ce4_8422_2325u64;
    let mut expected_sequence = 0u64;
    let started = Instant::now();
    let stats = run_ordered_batch(
        jobs,
        BatchLimits {
            requested_workers: workers,
            min_parallel_items: threshold,
            max_in_flight_items: workers.max(1).saturating_mul(2),
            max_in_flight_bytes: 256 * 1024 * 1024,
        },
        |mut input| {
            for word in input.chunks_exact_mut(8) {
                let mut bytes = [0u8; 8];
                bytes.copy_from_slice(word);
                let value = u64::from_le_bytes(bytes);
                word.copy_from_slice(&mix(value, rounds).to_le_bytes());
            }
            let bound = input.len().saturating_add(input.len().div_ceil(8));
            let output = pack(&input, bound)?;
            let retained = output.len();
            Ok::<_, capnp_io::PackedError>(BatchOutput::new(output, retained))
        },
        |sequence, output| {
            if sequence != expected_sequence {
                return Err(std::io::Error::other("batch output reordered"));
            }
            expected_sequence += 1;
            checksum = output.iter().fold(checksum, |hash, byte| {
                hash.wrapping_mul(0x100_0000_01b3) ^ u64::from(*byte)
            });
            Ok::<_, std::io::Error>(())
        },
    )?;
    Ok((started.elapsed(), checksum, stats.workers_used))
}

fn median(
    fixtures: &[Vec<u8>],
    rounds: u32,
    workers: usize,
    threshold: usize,
    samples: usize,
) -> Result<(Duration, Duration, u64, usize), AnyError> {
    let _ = run_once(fixtures, rounds, 1, threshold)?;
    let _ = run_once(fixtures, rounds, workers, threshold)?;
    let mut serial = Vec::with_capacity(samples);
    let mut parallel = Vec::with_capacity(samples);
    let mut expected = None;
    let mut workers_used = 0;
    for _ in 0..samples {
        let (serial_elapsed, serial_checksum, _) = run_once(fixtures, rounds, 1, threshold)?;
        let (parallel_elapsed, parallel_checksum, used) =
            run_once(fixtures, rounds, workers, threshold)?;
        if serial_checksum != parallel_checksum {
            return Err(std::io::Error::other("serial and parallel outputs differ").into());
        }
        if let Some(previous) = expected {
            if serial_checksum != previous {
                return Err(std::io::Error::other("pipeline output changed").into());
            }
        }
        expected = Some(serial_checksum);
        workers_used = used;
        serial.push(serial_elapsed);
        parallel.push(parallel_elapsed);
    }
    serial.sort_unstable();
    parallel.sort_unstable();
    Ok((
        serial[serial.len() / 2],
        parallel[parallel.len() / 2],
        expected.unwrap_or(0),
        workers_used,
    ))
}

fn main() -> Result<(), AnyError> {
    let messages = parse_arg(1, DEFAULT_MESSAGES, "message count")?;
    let words = parse_arg(2, DEFAULT_WORDS, "words per message")?;
    let rounds = parse_arg(3, DEFAULT_ROUNDS, "mix rounds")?;
    let workers = parse_arg(
        4,
        std::thread::available_parallelism().map_or(1, usize::from),
        "worker count",
    )?;
    let samples = parse_arg(5, DEFAULT_SAMPLES, "sample count")?;
    let threshold = parse_arg(6, 2usize, "parallel message threshold")?;
    if samples == 0 || messages == 0 || words == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "messages, words, and samples must be non-zero",
        )
        .into());
    }
    let fixtures = fixtures(messages, words);
    let (serial, parallel, checksum, workers_used) =
        median(&fixtures, rounds, workers, threshold, samples)?;
    println!(
        "messages\twords\trounds\tworkers\tworkers_used\tserial_ns\tparallel_ns\tspeedup\tchecksum"
    );
    println!(
        "{messages}\t{words}\t{rounds}\t{workers}\t{workers_used}\t{}\t{}\t{:.3}\t{checksum}",
        serial.as_nanos(),
        parallel.as_nanos(),
        serial.as_secs_f64() / parallel.as_secs_f64()
    );
    Ok(())
}
