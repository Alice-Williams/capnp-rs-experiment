use std::error::Error;
use std::hint::black_box;
use std::sync::Arc;
use std::time::{Duration, Instant};

use capnp_message::{
    ExclusiveArena, ListObject, ListPartitionPlan, ObjectRef, OwnedMessage, ParallelReadOptions,
    ReaderLimits,
};

const DEFAULT_ITEMS: u32 = 262_144;
const DEFAULT_ROUNDS: u32 = 128;
const DEFAULT_SAMPLES: usize = 5;
type SharedSegments = Arc<[Arc<[u8]>]>;

fn parse_arg<T>(index: usize, default: T, name: &str) -> Result<T, Box<dyn Error>>
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

fn fixture(items: u32) -> Result<SharedSegments, Box<dyn Error>> {
    let arena_words = items.checked_add(1).ok_or("fixture is too large")?;
    let mut arena = ExclusiveArena::new(arena_words, arena_words)?;
    {
        let mut list = arena.init_root_list::<u64>(items)?;
        for index in 0..items {
            list.set(index, u64::from(index).wrapping_mul(0x9e37_79b9))?;
        }
    }
    let segments = arena
        .into_segments()
        .into_iter()
        .map(Arc::<[u8]>::from)
        .collect::<Vec<_>>();
    Ok(segments.into())
}

fn root(
    segments: &SharedSegments,
    traversal_words: u64,
) -> Result<ObjectRef<ListObject>, Box<dyn Error>> {
    Ok(OwnedMessage::new(
        segments.iter().cloned(),
        ReaderLimits {
            traversal_words,
            nesting_levels: 8,
        },
    )?
    .root_list()?
    .into_root())
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

fn run_once(
    segments: &SharedSegments,
    items: u32,
    rounds: u32,
    workers: usize,
    threshold: u32,
) -> Result<(Duration, u64, usize), Box<dyn Error>> {
    let started = Instant::now();
    let plan = ListPartitionPlan::new(
        root(segments, u64::from(items))?,
        ParallelReadOptions {
            requested_workers: workers,
            min_parallel_items: threshold,
            min_items_per_partition: 4_096,
        },
    )?;
    let partition_count = plan.partitions().len();
    let checksum = plan.map_reduce_scoped(
        |partition| {
            partition
                .with_reader(|reader, mut range| {
                    let values = reader.as_primitive::<u64>()?;
                    range.try_fold(0u64, |checksum, index| {
                        Ok::<_, capnp_message::ListReadError>(
                            checksum.wrapping_add(mix(values.get(index)?, rounds)),
                        )
                    })
                })
                .map_err(std::io::Error::other)?
                .map_err(std::io::Error::other)
        },
        || 0u64,
        u64::wrapping_add,
    )?;
    Ok((started.elapsed(), checksum, partition_count))
}

fn median_run(
    segments: &SharedSegments,
    items: u32,
    rounds: u32,
    workers: usize,
    threshold: u32,
    samples: usize,
) -> Result<(Duration, u64, usize), Box<dyn Error>> {
    let _ = run_once(segments, items, rounds, workers, threshold)?;
    let mut timings = Vec::with_capacity(samples);
    let mut expected = None;
    let mut partitions = 0;
    for _ in 0..samples {
        let (elapsed, checksum, count) = run_once(segments, items, rounds, workers, threshold)?;
        if let Some(previous) = expected {
            assert_eq!(checksum, previous, "benchmark checksum changed");
        }
        expected = Some(checksum);
        partitions = count;
        timings.push(elapsed);
    }
    timings.sort_unstable();
    Ok((
        timings[timings.len() / 2],
        expected.unwrap_or(0),
        partitions,
    ))
}

fn main() -> Result<(), Box<dyn Error>> {
    let items = parse_arg(1, DEFAULT_ITEMS, "item count")?;
    let rounds = parse_arg(2, DEFAULT_ROUNDS, "mix rounds")?;
    let workers = parse_arg(
        3,
        std::thread::available_parallelism().map_or(1, usize::from),
        "worker count",
    )?;
    let samples = parse_arg(4, DEFAULT_SAMPLES, "sample count")?;
    let threshold = parse_arg(
        5,
        ParallelReadOptions::default().min_parallel_items,
        "threshold",
    )?;
    if samples == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "sample count must be non-zero",
        )
        .into());
    }

    let segments = fixture(items)?;
    let (serial, serial_checksum, _) = median_run(&segments, items, rounds, 1, 0, samples)?;
    let (parallel, parallel_checksum, partitions) =
        median_run(&segments, items, rounds, workers, threshold, samples)?;
    assert_eq!(serial_checksum, parallel_checksum);

    println!("items\trounds\tworkers\tpartitions\tserial_ns\tparallel_ns\tspeedup\tchecksum");
    println!(
        "{items}\t{rounds}\t{workers}\t{partitions}\t{}\t{}\t{:.3}\t{serial_checksum}",
        serial.as_nanos(),
        parallel.as_nanos(),
        serial.as_secs_f64() / parallel.as_secs_f64()
    );
    Ok(())
}
