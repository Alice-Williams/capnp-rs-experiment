# Dossier analysis

## Product boundary

The target is a Rust-native implementation of the Cap'n Proto platform as
realized by the pinned C++ repository, not a transliteration of the existing
Rust implementation. It contains four coupled products:

1. A hostile-input-safe wire/message runtime.
2. Reflection, generated Rust APIs, a native schema compiler, and CLI tools.
3. Explicit parallel APIs for immutable traversal, independent messages, and
   partitioned construction.
4. A thread-safe capability/RPC system reaching Level 1 first and Levels 3/4
   plus persistent capabilities later.

Wire and generated semantics are compatibility obligations. Rust type names,
lifetime quirks, executor choices, and the current Rust crate's single-threaded
RPC architecture are not.

## Architectural invariants

- Reader values hold immutable segment coordinates, not cached unchecked
  native pointers. Owned messages use stable shared backing storage.
- Every offset, count, word/byte conversion, and allocation size is checked
  before slicing or allocating.
- Shared traversal accounting is exact under concurrency; nesting depth is
  carried by reader values.
- Ordinary builders are exclusively mutable. Parallel builders expose only
  type-proven disjoint partitions and independently allocated lanes.
- Each RPC connection has one actor-owned ordered state machine. Application
  handlers may run concurrently, but table transitions and E-order do not.
- Public clients and owned messages derive `Send + Sync` from their
  representation. Thread safety is not asserted manually.
- Generated code contains typed accessors and dispatch glue, not protocol
  state logic.

## Serial islands that remain intentional

Frame ordering on one stream, connection table transitions, promise
disembargo/E-order, deterministic canonical emission, and arbitrary mutation
of one builder remain serial. Parallelism surrounds these islands: complete
messages, immutable subtrees, handler futures, separate connections, and
disjoint build lanes can execute concurrently.

## Verification model

No implementation may be its own sole oracle. Evidence is layered:

- exact hand-authored word fixtures;
- pinned C++ fixtures and cross-language reads/writes;
- current Rust fixtures only as secondary regression evidence;
- property/fuzz/compile-fail/Miri/Loom tests appropriate to the boundary;
- deterministic RPC state-machine simulation;
- checked-in benchmarks with hardware and before/after context.

Security gates require bounded allocation/CPU, exact shared budgets, checked
arithmetic, structured limit failures, reviewed unsafe code, and exactly-once
wakeup/resource release on cancellation or disconnect.

## Dependency analysis

The dossier's 49 milestones are ordered correctly but span several years of
product work. The wire preview (M00–M10) is the first credible proving ground.
The highest-risk early decisions are ownership, exact shared budgeting,
builder partitioning, and the RPC actor boundary; M02 freezes those before M03.

Generated interfaces depend on reflection and wire ownership. RPC depends on
generated interfaces and owned messages. Native compilation depends on layout
compatibility, not RPC. Parallel reading can begin after M10; parallel building
must wait for multi-segment allocation and orphan/copy semantics.

## Decisions adopted provisionally

- Internal crates use the new `capnp-*` workspace names from the dossier;
  compatibility facades come later.
- The runtime core is designed for `no_std`, with `alloc` and `std` layered on.
- Async cores remain executor-neutral; Tokio support is an adapter.
- Non-`Send` servers, if retained, live behind one `LocalServer` adapter.
- Unsafe optimizations require a safe baseline, benchmark evidence, a narrow
  module, and a documented invariant.
- Bazel is the reproducible Linux build/test orchestrator. Cargo remains the
  native Rust package/build interface and MSRV checker.

The software license is deliberately unresolved because that is an owner/legal
choice. M00 cannot be marked complete until the repository license is chosen.
