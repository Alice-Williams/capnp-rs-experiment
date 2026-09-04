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

## Lowest-layer wire values

M50 compares checked native scalar access and native `Word` access with the
pinned C++ implementation's actual `capnp::_::WireValue<uint64_t>` type:

```console
bash benchmarks/run-wire-value-baselines.sh benchmarks/results/<run-name>
```

Inputs, operation order, and checksums are identical. This deliberately keeps
Rust's checked slice boundary visible while measuring C++'s unchecked internal
wire value separately.

## Standard framing

M51 compares one-, two-, and 64-segment parse and encode operations using the
actual pinned C++ `FlatArrayMessageReader` and `messageToFlatArray` APIs:

```console
bash benchmarks/run-framing-baselines.sh benchmarks/results/<run-name>
```

Fixture construction and process launch are outside the timed region. Every
shape uses identical deterministic segment contents and semantic checksums.

## Native workspace versus C++

M49 ports the same carsales, catrank, and expression-evaluation workload logic
to this native workspace. It deliberately uses `no-reuse` for both binaries
because `ExclusiveArena` does not yet expose a reset/reuse operation:

```console
bash benchmarks/run-native-cpp-baselines.sh benchmarks/results/<run-name>
```

The runner alternates which implementation executes first, retains raw samples,
and reports the median native/C++ ratio. The schemas keep the pinned C++ file
IDs and wire layout; only the C++ namespace annotation is omitted from the
native compiler inputs. Each executable validates its response against the
same deterministic RNG-derived expectation. Serialized byte counts can differ
because the allocators choose different segment layouts, so they are recorded
as context rather than treated as a cross-implementation checksum.

Set `CAPNP_BENCH_PROFILE=1` on the native executable to collect aggregate
`Instant` timings for request construction, request encode/decode, request
handling, response encode/decode, and response checking. The reproducible phase
runner adds a child-process wall timer so arena construction, owned-message
conversion, schema loading, and loop overhead remain visible as unattributed
time:

```console
bash benchmarks/run-native-phase-breakdown.sh benchmarks/results/<run-name>
```

The sequential native RPC comparison uses the same Ping interface, one
bootstrap, and the same UInt64 request/result loop as C++. The native
`MemoryTransport` passes owned message envelopes directly, whereas the C++ KJ
pipe transports bytes. The result is therefore a useful lower-bound comparison
for the native RPC state machine, not evidence that its framing cost matches
C++:

```console
bash benchmarks/run-native-cpp-rpc.sh benchmarks/results/<run-name>
```

An opt-in `Instant` breakdown attributes the native request loop without
changing ordinary benchmark timing:

```console
bash benchmarks/run-native-rpc-phase-breakdown.sh benchmarks/results/<run-name>
```

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

The default uses 31 odd samples per mode because the one-message no-pool
control intentionally enforces a tight 5% bound between two identical serial
branches; seven samples proved too sensitive to host scheduling noise during
the M48 release audit.

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

## M55 generated-data API comparison

`run-generated-api-baselines.sh` begins the generated API comparison with hot
scalar and text/data reads from the pinned C++ `wire-fixture.capnp` message.
Each generated case is paired with an already-opened, direct checked runtime
reader over the same bytes, operation order, and checksum. This exposes the
incremental typed-API cost without hiding it behind framing or root opening.

Run it inside the Linux development container from a committed worktree:

```sh
benchmarks/run-generated-api-baselines.sh \
  benchmarks/results/DATE-m55-generated-api-baseline-g-drive-docker
```

The initial reader checkpoint is intentionally narrower than the completed M55
gate. Builder, list, struct/group, union/default, and evolution cases join this
same runner before the milestone advances.

## M56 schema and dynamic-reflection comparison

`run-reflection-baselines.sh` starts M56 at the lowest tooling layer. It
compares field-name lookup, field-index access, dynamic reads by name, and
dynamic reads through the closest cached-field path over the same four scalar
fields and pinned wire message. Schema loading, framing, and root construction
are outside the timed region in both implementations.

Run it inside the Linux development container from a committed worktree:

```sh
benchmarks/run-reflection-baselines.sh \
  benchmarks/results/DATE-m56-reflection-baseline-g-drive-docker
```

## M54 low-level packing comparison

`run-packing-baselines.sh` compares fresh-output `pack` and `unpack` operations
with the pinned C++ `PackedOutputStream` and `PackedInputStream`. Long zero runs,
long raw runs, deterministic mixed/sparse words, and a repeated pinned wire
fixture are measured independently. Paired byte-copy cases expose the inherited
allocation and observation floor for both directions; the summarizer reports
both cumulative and subtracted incremental native/C++ ratios.

Run it inside the Linux development container from a committed worktree:

```sh
benchmarks/run-packing-baselines.sh \
  benchmarks/results/DATE-m54-packing-baseline-g-drive-docker
```
