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

The pinned C++ benchmark sources need a three-line compatibility adaptation for
the pinned commit's newer KJ output-stream API. The build applies the reviewed
patch from `benchmarks/patches/` to an isolated copy; neither oracle checkout is
modified.

Results are baselines, not universal performance claims. Compare revisions on
the same hardware and container setup. The first corpus deliberately does not
claim an RPC baseline yet; that needs a common transport/workload harness and
remains an explicit M01 exit gap.
