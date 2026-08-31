# Parallel immutable reads

M29 adds zero-copy scheduling primitives for immutable owned messages. It does
not add a runtime dependency: callers may use the standard-library scoped
map/reduce helper, Rayon, or another scoped executor.

The compatibility sources are the pinned C++ traversal limiter and list-reader
rules at commit `e7c9cd96f1505b5ae486db7821006c2f5dce5b5b`, together with the exact
shared-budget rule in [ADR-0002](../adr/0002-exact-traversal-and-nesting-budgets.md).
Partition boundaries are native scheduling policy and do not change Cap'n
Proto bytes or claim compatibility with a C++ executor.

## Safety and accounting

`OwnedMessage` holds immutable `Arc` segment buffers. `ObjectRef` and every
partition contain only an `Arc`, validated wire coordinates, and copied
nesting state, so `Send + Sync` comes from their representation rather than an
unsafe implementation. Planning a list validates and charges its body once.
Workers reopen that same approved list without copying bytes or charging its
body again. Any pointer followed from an element still charges the message's
one atomic budget exactly.

Creating another plan is another logical dereference and charges the list
again. Invalid scheduling options fail before any charge. Readers exist only
inside `with_reader` callbacks and cannot escape; mutation is absent from this
API. M30 owns parallel construction.

## Scoped map/reduce

The checked-in example builds one `List(UInt64)`, shares it, partitions it, and
computes a deterministic checksum:

```console
cargo run --release -p capnp-message --example parallel_read -- \
  262144 128 4 7 16384
```

`ListPartitionPlan::map_reduce_scoped` stays on the caller for a one-partition
plan. Otherwise it spawns scoped workers and reduces results in partition
order. A map error is returned; a panic is converted to
`MapReduceError::WorkerPanicked` with the partition ordinal. The API does not
provide cancellation or guarantee that sibling workers stop after an error.

Rayon users can instead consume the independent partitions:

```text
plan.partitions().par_iter()
    .map(|partition| partition.with_reader(|reader, range| { ... }))
    .reduce(...)
```

No Rayon feature is needed in `capnp-message`; the application owns its
executor and scheduling policy.

## Subtree planning

`SubtreePlan` accepts retained typed `ObjectRef`s and caller-supplied word work
estimates. It applies deterministic longest-processing-time-first binning,
then restores original ordinal order within each batch. Batches retain the
same objects and backing allocations. The planner does not inspect or charge
the subtrees: workers charge them only when they actually dereference them.

`min_parallel_items` applies to the number of independent subtree work items;
`min_items_per_partition` limits the useful bin count. Estimates affect load
balance only and are saturating hints, not security limits.

## Threshold and recorded scaling

Defaults are 16,384 list items before parallel work and at least 4,096 items
per partition. The runner records medians including owned-view and plan
creation and refuses to overwrite evidence:

```console
bash benchmarks/run-m29-parallel-read.sh \
  benchmarks/results/<new-run-name>
```

The checked-in 2026-08-31 Docker/WSL2 run used four workers on an Intel
i7-6700K. The 1,024- and 8,192-item cases stayed on the serial path and were
within 0.4% of their one-worker comparison. The first parallel size, 16,384,
reached 3.243x; 65,536 through 1,048,576 items reached 3.618x to 3.706x. These
are qualification data for this workload and machine, not universal latency
claims. Applications should benchmark their own per-element work and increase
the threshold for cheaper operations.

Loom separately models the single root precharge followed by competing nested
charges. Stress tests repeatedly reopen every partition while asserting the
original list charge is neither refunded nor duplicated.

## Explicit non-goals

- parallel or shared mutation;
- a built-in thread pool, Rayon dependency, work stealing, or async executor;
- subtree cost discovery or automatic schema-aware splitting;
- wire-format, canonicalization, or traversal-limit changes;
- panic cancellation of already-running sibling partitions.
