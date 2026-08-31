# Partitioned parallel construction

M30 adds an opt-in construction path for work that can be divided before any
worker receives mutable storage. The ordinary `ExclusiveArena` remains the
default and never becomes internally synchronized.

The wire compatibility source is the pinned C++ `layout.c++` implementation at
commit `e7c9cd96f1505b5ae486db7821006c2f5dce5b5b`. Ownership phases and exclusions
follow [ADR-0003](../adr/0003-exclusive-and-partitioned-builders.md).
Partition sizes and worker scheduling are native policy, not wire behavior.

## Primitive list partitions

`PartitionedPrimitiveList<T>` allocates one zeroed root list. Calling
`partitions()` exclusively borrows the builder and repeatedly uses safe slice
splitting to return non-overlapping `PrimitiveBuildPartition`s. Each partition
is `Send`, not `Clone`, exposes only its own local/global indexes, and writes
through the same little-endian primitive implementations as the exclusive
arena. Bit-list boundaries are rounded to whole bytes so two workers never
share a storage byte. Void lists remain one logical partition.

The builder cannot be split again or finalized while partitions are live.
Unwritten elements remain their valid zero/default wire representation. The
checked-in `parallel_build` example shows scoped construction and produces
bytes identical to `ExclusiveArena`.

## Pointer lanes and sealed fragments

`PartitionedPointerList` creates a zeroed root pointer list and issues one set
of non-cloneable `BuildLane`s. A lane owns a unique slot range and builds each
requested child in a private, bounded, single-segment `ExclusiveArena`.
Installation into the lane is transactional: a builder error or panic drops
the private arena and leaves that slot absent. A slot cannot be initialized
twice.

`seal()` consumes the mutable lane. Finalization accepts sealed lanes in any
arrival order, rejects foreign/duplicate lanes, verifies aggregate segment and
word limits before mutation, and links fragments in ascending slot order with
single-far pointers. Missing lanes and slots stay null. Fragment root bytes are
moved into the final segment vector rather than graph-copied.

The finalizer validates every reachable pointer in each fragment and rejects
capability pointers. This first release also deliberately limits a fragment to
one segment; applications needing a larger child should increase that lane
slot's word bound rather than create internal far segments.

## Failure and determinism guarantees

- The type system prevents simultaneous overlapping primitive slices and
  duplicate ownership of a lane.
- All allocation is zero-initialized before workers receive it.
- Panic/cancellation can leave completed values, but every unfilled primitive
  or pointer slot remains a valid default.
- Sealed lanes have no mutation API.
- Final segment order depends on root slot order, never worker completion
  order.
- Segment-count and total-word limits are checked before the coordinator writes
  any far pointer.

Loom explores both worker completion orders and verifies identical valid
finalization. Miri executes the actual scoped mutable-slice split/write path.
The pinned C++ reader decodes the native seven-segment `List(Text)` fixture and
checks all values.

## Scaling and threshold

The default threshold is 16,384 elements with at least 4,096 elements per
partition. On the recorded 2026-08-31 Docker/WSL2 i7-6700K run, 1,024 and 8,192
elements stayed on one partition and were within 2.1% of the exclusive arena.
Four workers reached 3.134x at 16,384 items and 3.574x to 3.836x from 65,536 to
1,048,576 items, exceeding the 2.5x gate. These results include arena creation,
writes, and finalization; applications must retune for cheaper field work.

```console
bash benchmarks/run-m30-parallel-build.sh \
  benchmarks/results/<new-run-name>
```

## Explicit non-goals

- arbitrary concurrent mutation of one graph or `Arc<Mutex<Builder>>`;
- capability-table registration;
- multi-segment fragments inside one lane slot;
- canonical single-segment output;
- automatic retry, cancellation, or work stealing;
- batch codec scheduling, which belongs to M31.
