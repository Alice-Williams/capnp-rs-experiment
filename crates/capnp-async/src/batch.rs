//! Bounded concurrent work with deterministic per-stream emission.
//!
//! The scheduler owns a finite reservation window. Workers may pull and finish
//! jobs in any order, but completed outputs retain their reservations until the
//! coordinator emits every preceding sequence. A slow emitter therefore stops
//! submission instead of allowing an unbounded reorder buffer.
//!
//! Standard framing and packing semantics remain those of the pinned C++
//! implementation and M15/M16. This native scheduler does not create a
//! persistent pool, infer RPC independence, reorder a stream, or cancel work
//! already submitted to a worker.

use core::fmt;
use core::panic::AssertUnwindSafe;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

use capnp_io::{PackedError, pack, unpack};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BatchLimits {
    pub requested_workers: usize,
    pub min_parallel_items: usize,
    pub max_in_flight_items: usize,
    pub max_in_flight_bytes: usize,
}

impl Default for BatchLimits {
    fn default() -> Self {
        Self {
            requested_workers: std::thread::available_parallelism().map_or(1, usize::from),
            min_parallel_items: 2,
            max_in_flight_items: 64,
            max_in_flight_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Debug)]
pub struct BatchJob<I> {
    input: I,
    reserved_bytes: usize,
}

impl<I> BatchJob<I> {
    pub const fn new(input: I, reserved_bytes: usize) -> Self {
        Self {
            input,
            reserved_bytes,
        }
    }

    pub const fn reserved_bytes(&self) -> usize {
        self.reserved_bytes
    }

    pub fn into_input(self) -> I {
        self.input
    }
}

#[derive(Debug)]
pub struct BatchOutput<O> {
    value: O,
    retained_bytes: usize,
}

impl<O> BatchOutput<O> {
    pub const fn new(value: O, retained_bytes: usize) -> Self {
        Self {
            value,
            retained_bytes,
        }
    }

    pub const fn retained_bytes(&self) -> usize {
        self.retained_bytes
    }

    pub fn into_value(self) -> O {
        self.value
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BatchStats {
    pub emitted_items: u64,
    pub workers_used: usize,
    pub peak_in_flight_items: usize,
    pub peak_reserved_bytes: usize,
}

#[derive(Debug)]
pub enum BatchError<ProcessError, EmitError> {
    ZeroWorkers,
    ZeroParallelThreshold,
    ZeroInFlightItems,
    ZeroInFlightBytes,
    SequenceOverflow,
    ReservationTooLarge {
        sequence: u64,
        requested: usize,
        limit: usize,
    },
    OutputExceedsReservation {
        sequence: u64,
        retained: usize,
        reserved: usize,
    },
    Process {
        sequence: u64,
        error: ProcessError,
    },
    Emit {
        sequence: u64,
        error: EmitError,
    },
    WorkerPanicked {
        sequence: u64,
    },
    WorkerPoolClosed,
}

impl<P: fmt::Display, E: fmt::Display> fmt::Display for BatchError<P, E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroWorkers => formatter.write_str("batch worker count must be non-zero"),
            Self::ZeroParallelThreshold => {
                formatter.write_str("batch parallel threshold must be non-zero")
            }
            Self::ZeroInFlightItems => {
                formatter.write_str("batch in-flight item limit must be non-zero")
            }
            Self::ZeroInFlightBytes => {
                formatter.write_str("batch in-flight byte limit must be non-zero")
            }
            Self::SequenceOverflow => formatter.write_str("batch sequence number overflowed"),
            Self::ReservationTooLarge {
                sequence,
                requested,
                limit,
            } => write!(
                formatter,
                "batch item {sequence} reserves {requested} bytes; limit is {limit}"
            ),
            Self::OutputExceedsReservation {
                sequence,
                retained,
                reserved,
            } => write!(
                formatter,
                "batch item {sequence} retained {retained} bytes; reservation is {reserved}"
            ),
            Self::Process { sequence, error } => {
                write!(formatter, "batch item {sequence} failed: {error}")
            }
            Self::Emit { sequence, error } => {
                write!(formatter, "batch output {sequence} failed: {error}")
            }
            Self::WorkerPanicked { sequence } => {
                write!(formatter, "batch worker panicked on item {sequence}")
            }
            Self::WorkerPoolClosed => formatter.write_str("batch worker pool closed unexpectedly"),
        }
    }
}

impl<P: std::error::Error + 'static, E: std::error::Error + 'static> std::error::Error
    for BatchError<P, E>
{
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Process { error, .. } => Some(error),
            Self::Emit { error, .. } => Some(error),
            Self::ZeroWorkers
            | Self::ZeroParallelThreshold
            | Self::ZeroInFlightItems
            | Self::ZeroInFlightBytes
            | Self::SequenceOverflow
            | Self::ReservationTooLarge { .. }
            | Self::OutputExceedsReservation { .. }
            | Self::WorkerPanicked { .. }
            | Self::WorkerPoolClosed => None,
        }
    }
}

struct Work<I> {
    sequence: u64,
    reserved_bytes: usize,
    input: I,
}

enum WorkFailure<E> {
    Process(E),
    Panicked,
}

struct Completed<O, E> {
    sequence: u64,
    reserved_bytes: usize,
    result: Result<BatchOutput<O>, WorkFailure<E>>,
}

/// Runs independently reserved jobs concurrently and emits in input order.
///
/// The input iterator must know its remaining length so a below-threshold batch
/// can avoid creating worker threads altogether. `emit` always runs on the
/// caller and is never invoked concurrently.
pub fn run_ordered_batch<Jobs, Process, Emit, I, O, ProcessError, EmitError>(
    jobs: Jobs,
    limits: BatchLimits,
    process: Process,
    mut emit: Emit,
) -> Result<BatchStats, BatchError<ProcessError, EmitError>>
where
    Jobs: IntoIterator<Item = BatchJob<I>>,
    Jobs::IntoIter: ExactSizeIterator,
    Process: Fn(I) -> Result<BatchOutput<O>, ProcessError> + Sync,
    Emit: FnMut(u64, O) -> Result<(), EmitError>,
    I: Send,
    O: Send,
    ProcessError: Send,
{
    validate_limits(limits)?;
    let mut jobs = jobs.into_iter();
    let job_count = jobs.len();
    if job_count < limits.min_parallel_items || job_count <= 1 || limits.requested_workers == 1 {
        return run_serial(&mut jobs, limits, &process, &mut emit);
    }
    run_parallel(jobs, job_count, limits, &process, &mut emit)
}

fn run_serial<Jobs, Process, Emit, I, O, ProcessError, EmitError>(
    jobs: &mut Jobs,
    limits: BatchLimits,
    process: &Process,
    emit: &mut Emit,
) -> Result<BatchStats, BatchError<ProcessError, EmitError>>
where
    Jobs: Iterator<Item = BatchJob<I>>,
    Process: Fn(I) -> Result<BatchOutput<O>, ProcessError>,
    Emit: FnMut(u64, O) -> Result<(), EmitError>,
{
    let mut stats = BatchStats {
        workers_used: usize::from(jobs.size_hint().0 != 0),
        ..BatchStats::default()
    };
    for (index, job) in jobs.enumerate() {
        let sequence = u64::try_from(index).map_err(|_| BatchError::SequenceOverflow)?;
        validate_reservation(sequence, job.reserved_bytes, limits)?;
        stats.peak_in_flight_items = 1;
        stats.peak_reserved_bytes = stats.peak_reserved_bytes.max(job.reserved_bytes);
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| process(job.input)))
            .map_err(|_| BatchError::WorkerPanicked { sequence })?
            .map_err(|error| BatchError::Process { sequence, error })?;
        if result.retained_bytes > job.reserved_bytes {
            return Err(BatchError::OutputExceedsReservation {
                sequence,
                retained: result.retained_bytes,
                reserved: job.reserved_bytes,
            });
        }
        emit(sequence, result.value).map_err(|error| BatchError::Emit { sequence, error })?;
        stats.emitted_items = stats
            .emitted_items
            .checked_add(1)
            .ok_or(BatchError::SequenceOverflow)?;
    }
    Ok(stats)
}

fn run_parallel<Jobs, Process, Emit, I, O, ProcessError, EmitError>(
    mut jobs: Jobs,
    job_count: usize,
    limits: BatchLimits,
    process: &Process,
    emit: &mut Emit,
) -> Result<BatchStats, BatchError<ProcessError, EmitError>>
where
    Jobs: Iterator<Item = BatchJob<I>>,
    Process: Fn(I) -> Result<BatchOutput<O>, ProcessError> + Sync,
    Emit: FnMut(u64, O) -> Result<(), EmitError>,
    I: Send,
    O: Send,
    ProcessError: Send,
{
    thread::scope(|scope| {
        let worker_count = limits
            .requested_workers
            .min(job_count)
            .min(limits.max_in_flight_items)
            .max(1);
        let (work_sender, work_receiver) = mpsc::channel::<Work<I>>();
        let work_receiver = Arc::new(Mutex::new(work_receiver));
        let (result_sender, result_receiver) = mpsc::channel::<Completed<O, ProcessError>>();
        let handles = (0..worker_count)
            .map(|_| {
                let work_receiver = Arc::clone(&work_receiver);
                let result_sender = result_sender.clone();
                scope.spawn(move || {
                    loop {
                        let work = match work_receiver.lock() {
                            Ok(receiver) => match receiver.recv() {
                                Ok(work) => work,
                                Err(_) => return,
                            },
                            Err(_) => return,
                        };
                        let result = match std::panic::catch_unwind(AssertUnwindSafe(|| {
                            process(work.input)
                        })) {
                            Ok(Ok(output)) => Ok(output),
                            Ok(Err(error)) => Err(WorkFailure::Process(error)),
                            Err(_) => Err(WorkFailure::Panicked),
                        };
                        if result_sender
                            .send(Completed {
                                sequence: work.sequence,
                                reserved_bytes: work.reserved_bytes,
                                result,
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                })
            })
            .collect::<Vec<_>>();
        drop(result_sender);

        let result = coordinate(
            &mut jobs,
            limits,
            &work_sender,
            &result_receiver,
            emit,
            worker_count,
        );
        drop(work_sender);
        let infrastructure_panicked = handles.into_iter().any(|handle| handle.join().is_err());
        if infrastructure_panicked && result.is_ok() {
            Err(BatchError::WorkerPoolClosed)
        } else {
            result
        }
    })
}

fn coordinate<Jobs, Emit, I, O, ProcessError, EmitError>(
    jobs: &mut Jobs,
    limits: BatchLimits,
    work_sender: &mpsc::Sender<Work<I>>,
    result_receiver: &mpsc::Receiver<Completed<O, ProcessError>>,
    emit: &mut Emit,
    worker_count: usize,
) -> Result<BatchStats, BatchError<ProcessError, EmitError>>
where
    Jobs: Iterator<Item = BatchJob<I>>,
    Emit: FnMut(u64, O) -> Result<(), EmitError>,
{
    let mut stats = BatchStats {
        workers_used: worker_count,
        ..BatchStats::default()
    };
    let mut pending = None;
    let mut input_exhausted = false;
    let mut next_submit = 0u64;
    let mut next_emit = 0u64;
    let mut in_flight_items = 0usize;
    let mut in_flight_bytes = 0usize;
    let mut completed = BTreeMap::new();

    loop {
        while in_flight_items < limits.max_in_flight_items {
            if pending.is_none() && !input_exhausted {
                pending = jobs.next();
                input_exhausted = pending.is_none();
            }
            let Some(job) = pending.as_ref() else {
                break;
            };
            validate_reservation(next_submit, job.reserved_bytes, limits)?;
            let requested = in_flight_bytes.checked_add(job.reserved_bytes).ok_or(
                BatchError::ReservationTooLarge {
                    sequence: next_submit,
                    requested: usize::MAX,
                    limit: limits.max_in_flight_bytes,
                },
            )?;
            if requested > limits.max_in_flight_bytes {
                break;
            }
            let job = pending.take().ok_or(BatchError::WorkerPoolClosed)?;
            let reserved_bytes = job.reserved_bytes;
            work_sender
                .send(Work {
                    sequence: next_submit,
                    reserved_bytes,
                    input: job.input,
                })
                .map_err(|_| BatchError::WorkerPoolClosed)?;
            next_submit = next_submit
                .checked_add(1)
                .ok_or(BatchError::SequenceOverflow)?;
            in_flight_items += 1;
            in_flight_bytes = requested;
            stats.peak_in_flight_items = stats.peak_in_flight_items.max(in_flight_items);
            stats.peak_reserved_bytes = stats.peak_reserved_bytes.max(in_flight_bytes);
        }

        if in_flight_items == 0 {
            if input_exhausted {
                return Ok(stats);
            }
            return Err(BatchError::WorkerPoolClosed);
        }

        let item = result_receiver
            .recv()
            .map_err(|_| BatchError::WorkerPoolClosed)?;
        if completed.insert(item.sequence, item).is_some() {
            return Err(BatchError::WorkerPoolClosed);
        }
        while let Some(item) = completed.remove(&next_emit) {
            let output = match item.result {
                Ok(output) => output,
                Err(WorkFailure::Process(error)) => {
                    return Err(BatchError::Process {
                        sequence: next_emit,
                        error,
                    });
                }
                Err(WorkFailure::Panicked) => {
                    return Err(BatchError::WorkerPanicked {
                        sequence: next_emit,
                    });
                }
            };
            if output.retained_bytes > item.reserved_bytes {
                return Err(BatchError::OutputExceedsReservation {
                    sequence: next_emit,
                    retained: output.retained_bytes,
                    reserved: item.reserved_bytes,
                });
            }
            emit(next_emit, output.value).map_err(|error| BatchError::Emit {
                sequence: next_emit,
                error,
            })?;
            in_flight_items -= 1;
            in_flight_bytes = in_flight_bytes
                .checked_sub(item.reserved_bytes)
                .ok_or(BatchError::WorkerPoolClosed)?;
            stats.emitted_items = stats
                .emitted_items
                .checked_add(1)
                .ok_or(BatchError::SequenceOverflow)?;
            next_emit = next_emit
                .checked_add(1)
                .ok_or(BatchError::SequenceOverflow)?;
        }
    }
}

fn validate_limits<P, E>(limits: BatchLimits) -> Result<(), BatchError<P, E>> {
    if limits.requested_workers == 0 {
        Err(BatchError::ZeroWorkers)
    } else if limits.min_parallel_items == 0 {
        Err(BatchError::ZeroParallelThreshold)
    } else if limits.max_in_flight_items == 0 {
        Err(BatchError::ZeroInFlightItems)
    } else if limits.max_in_flight_bytes == 0 {
        Err(BatchError::ZeroInFlightBytes)
    } else {
        Ok(())
    }
}

fn validate_reservation<P, E>(
    sequence: u64,
    requested: usize,
    limits: BatchLimits,
) -> Result<(), BatchError<P, E>> {
    if requested > limits.max_in_flight_bytes {
        Err(BatchError::ReservationTooLarge {
            sequence,
            requested,
            limit: limits.max_in_flight_bytes,
        })
    } else {
        Ok(())
    }
}

/// Packs word-aligned messages concurrently and emits packed bytes in order.
pub fn pack_messages_ordered<Messages, Emit, EmitError>(
    messages: Messages,
    limits: BatchLimits,
    emit: Emit,
) -> Result<BatchStats, BatchError<PackedError, EmitError>>
where
    Messages: IntoIterator<Item = Vec<u8>>,
    Messages::IntoIter: ExactSizeIterator,
    Emit: FnMut(u64, Vec<u8>) -> Result<(), EmitError>,
{
    let jobs = messages.into_iter().map(|input| {
        let output_bound = input.len().saturating_add(input.len().div_ceil(8));
        BatchJob::new(input, output_bound.saturating_mul(2))
    });
    run_ordered_batch(
        jobs,
        limits,
        |input| {
            let output_bound = input.len().saturating_add(input.len().div_ceil(8));
            let output = pack(&input, output_bound)?;
            let retained = output.len();
            Ok(BatchOutput::new(output, retained))
        },
        emit,
    )
}

/// Unpacks messages concurrently under one explicit per-message output bound.
pub fn unpack_messages_ordered<Messages, Emit, EmitError>(
    messages: Messages,
    max_output_bytes_per_message: usize,
    limits: BatchLimits,
    emit: Emit,
) -> Result<BatchStats, BatchError<PackedError, EmitError>>
where
    Messages: IntoIterator<Item = Vec<u8>>,
    Messages::IntoIter: ExactSizeIterator,
    Emit: FnMut(u64, Vec<u8>) -> Result<(), EmitError>,
{
    let jobs = messages.into_iter().map(|input| {
        let reservation = input.len().saturating_add(max_output_bytes_per_message);
        BatchJob::new(input, reservation)
    });
    run_ordered_batch(
        jobs,
        limits,
        |input| {
            let output = unpack(&input, max_output_bytes_per_message)?;
            let retained = output.len();
            Ok(BatchOutput::new(output, retained))
        },
        emit,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::convert::Infallible;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn limits(workers: usize, items: usize, bytes: usize) -> BatchLimits {
        BatchLimits {
            requested_workers: workers,
            min_parallel_items: 2,
            max_in_flight_items: items,
            max_in_flight_bytes: bytes,
        }
    }

    #[test]
    fn one_item_never_creates_a_worker_thread() {
        let caller = thread::current().id();
        let process_thread = Mutex::new(None);
        let stats = run_ordered_batch(
            [BatchJob::new(7u64, 8)],
            limits(4, 4, 32),
            |value| {
                *process_thread.lock().expect("thread slot") = Some(thread::current().id());
                Ok::<_, Infallible>(BatchOutput::new(value * 2, 8))
            },
            |sequence, value| {
                assert_eq!((sequence, value), (0, 14));
                Ok::<_, Infallible>(())
            },
        )
        .expect("batch");
        assert_eq!(*process_thread.lock().expect("thread slot"), Some(caller));
        assert_eq!(stats.workers_used, 1);
    }

    #[test]
    fn work_stealing_completion_never_reorders_rpc_bytes() {
        let jobs = (0u8..12)
            .map(|value| BatchJob::new(value, 1))
            .collect::<Vec<_>>();
        let mut emitted = Vec::new();
        let stats = run_ordered_batch(
            jobs,
            limits(4, 8, 8),
            |value| {
                thread::sleep(Duration::from_millis(u64::from(12 - value)));
                Ok::<_, Infallible>(BatchOutput::new(vec![value], 1))
            },
            |sequence, bytes| {
                emitted.push((sequence, bytes[0]));
                Ok::<_, Infallible>(())
            },
        )
        .expect("batch");
        assert_eq!(
            emitted,
            (0u64..12)
                .map(|sequence| (sequence, sequence as u8))
                .collect::<Vec<_>>()
        );
        assert!(stats.workers_used > 1);
    }

    #[test]
    fn slow_writer_keeps_the_exact_reservation_window_bounded() {
        let active = AtomicUsize::new(0);
        let peak_active = AtomicUsize::new(0);
        let jobs = (0..40)
            .map(|value| BatchJob::new(value, 1024))
            .collect::<Vec<_>>();
        let stats = run_ordered_batch(
            jobs,
            limits(4, 4, 4096),
            |value| {
                let now = active.fetch_add(1, Ordering::AcqRel) + 1;
                peak_active.fetch_max(now, Ordering::AcqRel);
                let output = vec![u8::try_from(value).expect("test value"); 1024];
                active.fetch_sub(1, Ordering::AcqRel);
                Ok::<_, Infallible>(BatchOutput::new(output, 1024))
            },
            |_, _| {
                thread::sleep(Duration::from_millis(1));
                Ok::<_, Infallible>(())
            },
        )
        .expect("batch");
        assert_eq!(stats.emitted_items, 40);
        assert_eq!(stats.peak_in_flight_items, 4);
        assert_eq!(stats.peak_reserved_bytes, 4096);
        assert!(peak_active.load(Ordering::Acquire) <= 4);
    }

    #[test]
    fn packed_batches_round_trip_in_input_order() {
        let inputs = (1usize..20)
            .map(|words| {
                (0..words * 8)
                    .map(|index| u8::try_from((index * words) % 251).expect("byte"))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let mut packed = Vec::new();
        let pack_stats =
            pack_messages_ordered(inputs.clone(), limits(4, 6, 64 * 1024), |_, item| {
                packed.push(item);
                Ok::<_, Infallible>(())
            })
            .expect("pack");
        let mut unpacked = Vec::new();
        unpack_messages_ordered(packed, 4096, limits(4, 6, 64 * 1024), |_, item| {
            unpacked.push(item);
            Ok::<_, Infallible>(())
        })
        .expect("unpack");
        assert_eq!(unpacked, inputs);
        assert_eq!(pack_stats.emitted_items, 19);
    }

    #[test]
    #[allow(clippy::panic)]
    fn panic_and_under_reserved_output_are_reported_by_sequence() {
        let panic = run_ordered_batch(
            vec![BatchJob::new(0u8, 1), BatchJob::new(1, 1)],
            limits(2, 2, 2),
            |value| {
                if value == 1 {
                    panic!("worker panic");
                }
                Ok::<_, Infallible>(BatchOutput::new(value, 1))
            },
            |_, _| Ok::<_, Infallible>(()),
        );
        assert!(matches!(
            panic,
            Err(BatchError::WorkerPanicked { sequence: 1 })
        ));

        let oversized = run_ordered_batch(
            [BatchJob::new(0u8, 1)],
            limits(1, 1, 1),
            |_| Ok::<_, Infallible>(BatchOutput::new(vec![0, 1], 2)),
            |_, _| Ok::<_, Infallible>(()),
        );
        assert!(matches!(
            oversized,
            Err(BatchError::OutputExceedsReservation {
                sequence: 0,
                retained: 2,
                reserved: 1
            })
        ));
    }
}
