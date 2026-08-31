# Oracle baselines

These benchmarks compare the pinned primary C++ product oracle with the pinned
current-Rust secondary oracle using the same upstream carsales, catrank, and
expression-evaluation workloads. They exercise in-memory build/read paths,
unpacked serialization, and packed serialization.

Run them only in the Linux development container:

```console
bash benchmarks/run-oracle-baselines.sh benchmarks/results/<run-name>
```

The runner refuses to overwrite a result directory. `BENCH_WARMUPS` and
`BENCH_RUNS` control repetition. Every run records raw wall-clock samples,
per-iteration means, hardware/container context, exact producer commits, and
toolchain versions.

The common two-party RPC baseline is added to an existing run directory with:

```console
bash benchmarks/run-rpc-oracle-baselines.sh benchmarks/results/<run-name>
```

It uses the same `Ping.ping(UInt64) -> UInt64` schema and sequential call
pattern in both implementations. Each runs a single-thread event loop over an
in-memory bidirectional byte stream, avoiding network variability while still
exercising framing, RPC tables, dispatch, and response delivery.

The pinned C++ benchmark sources need a three-line compatibility adaptation for
the pinned commit's newer KJ output-stream API. The build applies the reviewed
patch from `benchmarks/patches/` to an isolated copy; neither oracle checkout is
modified.

Results are baselines, not universal performance claims. Compare revisions on
the same hardware and container setup.

## Exact traversal-budget microbenchmark

M06 adds a single-thread comparison of complete local and atomic shared budget
charges. Run it in the development container; an optional argument selects the
number of charges:

```console
cargo run --release -p capnp-message --example traversal_budget -- 10000000
```

This is a guardrail for the accounting design, not a parallel scaling claim.
Later parallel-read milestones own representative workload and scaling gates.

## Parallel immutable-read benchmark

M29 adds a deterministic CPU-bound `List(UInt64)` map/reduce workload. It
records serial and four-worker medians, partition counts, checksums, and host
context, verifies that below-threshold inputs stay on one partition without a
greater-than-5% regression, and requires a qualifying four-worker size to
reach 3.0x:

```console
bash benchmarks/run-m29-parallel-read.sh benchmarks/results/<run-name>
```

The checked-in qualification run is
`results/2026-08-31-m29-g-drive-docker`. Work-per-item, workers, samples,
threshold, and sizes can be changed with the `M29_BENCH_*` environment
variables; altered runs are evidence for that configuration, not the default
gate.

## Partitioned-build benchmark

M30 compares the ordinary exclusive arena with byte-identical construction
through disjoint mutable primitive-list partitions. The deterministic workload
includes CPU work representative of independent application encoding, then
checks every serial and parallel output byte. Four-worker qualification is
2.5x and below-threshold cases must stay on one partition within 5%:

```console
bash benchmarks/run-m30-parallel-build.sh benchmarks/results/<run-name>
```

The checked-in qualification run is
`results/2026-08-31-m30-g-drive-docker`. `M30_BENCH_*` variables control work,
workers, samples, threshold, and sizes for new non-overwriting runs.

## Ordered batch-pipeline benchmark

M31 measures independent per-message transform and packed encoding through the
bounded ordered scheduler. The serial and four-worker paths consume identical
fixtures, verify output order and checksums, and include scheduling plus
ordered emission. A single message must stay on the caller without a pool and
within 5%; a qualifying multi-message batch must reach 3.0x:

```console
bash benchmarks/run-m31-batch-pipeline.sh benchmarks/results/<run-name>
```

## RPC server-scheduling benchmark

`server_scheduling` submits one burst through a single service/executor adapter
and records throughput, p50/p99 dispatch-to-completion latency, and the maximum
consecutive completion run for four evenly loaded keys. The runner compares one
and four CPU workers, then records serial and keyed policies under the same
load:

```sh
bash benchmarks/run-m39-server-scheduling.sh benchmarks/results/<run-name>
```

The checked-in qualification run is
`results/2026-08-31-m31-g-drive-docker`. New configurations use the
`M31_BENCH_*` variables and a fresh output directory.
