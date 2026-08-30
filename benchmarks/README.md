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
