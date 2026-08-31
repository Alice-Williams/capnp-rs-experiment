use std::error::Error;
use std::hint::black_box;
use std::time::{Duration, Instant};

use capnp_message::{
    ExclusiveArena, ParallelBuildOptions, PartitionedPrimitiveList, PrimitiveBuildPartition,
};

const DEFAULT_ITEMS: u32 = 262_144;
const DEFAULT_ROUNDS: u32 = 128;
const DEFAULT_SAMPLES: usize = 5;
type AnyError = Box<dyn Error>;
type SerialRun = (Duration, Box<[u8]>);
type ParallelRun = (Duration, Box<[u8]>, usize);

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

fn fill_partition(
    partition: &mut PrimitiveBuildPartition<'_, u64>,
    rounds: u32,
) -> Result<(), capnp_message::ParallelBuildError> {
    let start = partition.range().start;
    for local_index in 0..partition.len() {
        let global_index = start
            .checked_add(local_index)
            .ok_or(capnp_message::ParallelBuildError::RangeOverflow)?;
        partition.set(local_index, mix(u64::from(global_index), rounds))?;
    }
    Ok(())
}

fn serial_once(items: u32, rounds: u32) -> Result<SerialRun, AnyError> {
    let arena_words = items.checked_add(1).ok_or("fixture is too large")?;
    let started = Instant::now();
    let mut arena = ExclusiveArena::new(arena_words, arena_words)?;
    {
        let mut list = arena.init_root_list::<u64>(items)?;
        for index in 0..items {
            list.set(index, mix(u64::from(index), rounds))?;
        }
    }
    let bytes = arena.into_segment()?;
    Ok((started.elapsed(), bytes))
}

fn parallel_once(
    items: u32,
    rounds: u32,
    workers: usize,
    threshold: u32,
) -> Result<ParallelRun, AnyError> {
    let started = Instant::now();
    let mut builder = PartitionedPrimitiveList::<u64>::new(items)?;
    let mut partitions = builder.partitions(ParallelBuildOptions {
        requested_workers: workers,
        min_parallel_items: threshold,
        min_items_per_partition: 4_096,
    })?;
    let partition_count = partitions.len();
    if partitions.len() <= 1 {
        if let Some(partition) = partitions.first_mut() {
            fill_partition(partition, rounds)?;
        }
    } else {
        std::thread::scope(|scope| {
            let handles = partitions
                .into_iter()
                .map(|mut partition| scope.spawn(move || fill_partition(&mut partition, rounds)))
                .collect::<Vec<_>>();
            for handle in handles {
                handle
                    .join()
                    .map_err(|_| std::io::Error::other("parallel build worker panicked"))?
                    .map_err(std::io::Error::other)?;
            }
            Ok::<_, std::io::Error>(())
        })?;
    }
    let mut segments = builder.finish()?;
    let bytes = segments.pop().ok_or("missing output segment")?;
    Ok((started.elapsed(), bytes, partition_count))
}

fn checksum(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        hash.wrapping_mul(0x100_0000_01b3) ^ u64::from(*byte)
    })
}

fn median(
    items: u32,
    rounds: u32,
    workers: usize,
    threshold: u32,
    samples: usize,
) -> Result<(Duration, Duration, u64, usize), AnyError> {
    let _ = serial_once(items, rounds)?;
    let _ = parallel_once(items, rounds, workers, threshold)?;
    let mut serial_times = Vec::with_capacity(samples);
    let mut parallel_times = Vec::with_capacity(samples);
    let mut expected = None;
    let mut partitions = 0;
    for _ in 0..samples {
        let (serial, serial_bytes) = serial_once(items, rounds)?;
        let (parallel, parallel_bytes, count) = parallel_once(items, rounds, workers, threshold)?;
        if serial_bytes != parallel_bytes {
            return Err(std::io::Error::other("serial and partitioned bytes differ").into());
        }
        let output_checksum = checksum(&serial_bytes);
        if let Some(previous) = expected {
            if output_checksum != previous {
                return Err(std::io::Error::other("benchmark output changed").into());
            }
        }
        expected = Some(output_checksum);
        partitions = count;
        serial_times.push(serial);
        parallel_times.push(parallel);
    }
    serial_times.sort_unstable();
    parallel_times.sort_unstable();
    Ok((
        serial_times[serial_times.len() / 2],
        parallel_times[parallel_times.len() / 2],
        expected.unwrap_or(0),
        partitions,
    ))
}

fn main() -> Result<(), AnyError> {
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
        ParallelBuildOptions::default().min_parallel_items,
        "threshold",
    )?;
    if samples == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "sample count must be non-zero",
        )
        .into());
    }

    let (serial, parallel, output_checksum, partitions) =
        median(items, rounds, workers, threshold, samples)?;
    println!("items\trounds\tworkers\tpartitions\tserial_ns\tparallel_ns\tspeedup\tchecksum");
    println!(
        "{items}\t{rounds}\t{workers}\t{partitions}\t{}\t{}\t{:.3}\t{output_checksum}",
        serial.as_nanos(),
        parallel.as_nanos(),
        serial.as_secs_f64() / parallel.as_secs_f64()
    );
    Ok(())
}
