# Bounded ordered batch pipelines

M31 processes independent message work concurrently while retaining one
deterministic write order. Framing and packed bytes keep the pinned C++
semantics established by M04/M15/M16; the scheduler is native policy and does
not change a stream or RPC ordering rule.

## Reservation window

Each `BatchJob<I>` carries a byte reservation. `BatchLimits` caps both jobs and
reserved bytes in flight. The coordinator does not release either charge when
a worker finishes: an out-of-order result retains its reservation until every
earlier sequence has been emitted. Consequently a blocked or slow emitter
halts further submission instead of growing an unbounded reorder buffer.

The generic processor returns `BatchOutput<O>` with its retained byte count.
An output larger than its reservation fails before emission. Callers are
responsible for reserving the input plus the processor's maximum transient and
retained allocation; the supplied pack/unpack helpers calculate conservative
input/output reservations from explicit codec bounds.

`BatchStats` reports emitted items, worker count, and peak item/byte
reservations. Oversized jobs, output under-reservation, worker panic, processing
errors, sink errors, and pool failure are sequence-located errors.

## Ordering and scheduling

`run_ordered_batch` accepts an exact-size iterator. Below
`min_parallel_items`, with one requested worker, or for a single item, it calls
the processor and emitter directly on the caller without creating a thread.
Otherwise scoped workers pull from a shared queue. They may finish in any
order; only the coordinator invokes `emit(sequence, output)`, serially and in
ascending input sequence.

This makes the utility suitable for independent read/validate/build/pack work
whose resulting frames must return to one stream. Opaque RPC frame work can use
the same scheduler only when it is independently safe to process; output is
still emitted in arrival sequence. The scheduler never infers that two RPC
messages may be reordered.

```rust
use capnp_async::{BatchJob, BatchLimits, BatchOutput, run_ordered_batch};
use std::convert::Infallible;

let jobs = vec![
    BatchJob::new(vec![1_u8], 16),
    BatchJob::new(vec![2_u8], 16),
];
let mut output = Vec::new();
run_ordered_batch(
    jobs,
    BatchLimits::default(),
    |bytes| Ok::<_, Infallible>(BatchOutput::new(bytes, 1)),
    |sequence, bytes| {
        output.push((sequence, bytes));
        Ok::<_, Infallible>(())
    },
)?;
assert_eq!(output[0].0, 0);
# Ok::<(), capnp_async::BatchError<Infallible, Infallible>>(())
```

`pack_messages_ordered` computes a worst-case packed bound for each
word-aligned message. `unpack_messages_ordered` requires an explicit maximum
unpacked size per message and includes it in every reservation.

## Evidence and threshold

Tests deliberately make later opaque/RPC-like jobs finish first and still
observe input order. A slow emitter holds a four-item/4,096-byte window while
40 worker results complete, with measured peaks never exceeding either limit.
Panic and under-reservation tests identify the exact sequence. Packed batches
round-trip in order.

The checked-in 2026-08-31 Docker/WSL2 i7-6700K benchmark transforms and packs
1,024-word messages. One message stayed on the caller and was 4.4% faster than
the explicit one-worker comparison. Four workers reached 3.186x for 32
messages and 3.180x for 64 messages. These are qualifying workload results,
not a claim that tiny messages should use threads.

```console
bash benchmarks/run-m31-batch-pipeline.sh \
  benchmarks/results/<new-run-name>
```

## Explicit non-goals

- a persistent/global thread pool or executor dependency;
- concurrent calls to the output sink;
- automatic RPC dependency, E-order, or capability analysis;
- cancellation of jobs already submitted to workers;
- hiding application input/transient allocation sizes;
- replacing M37 transport acknowledgements and adaptive flow control.
