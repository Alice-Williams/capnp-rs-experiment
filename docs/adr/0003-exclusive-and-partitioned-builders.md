# ADR-0003: Exclusive and partitioned builders

- Status: accepted
- Milestone: M02
- Date: 2026-08-30

## Context

Ordinary Cap'n Proto construction mutates one arena and an interconnected
object graph. Making every setter lockable would obscure aliasing, add
contention, and make pointer/capability bookkeeping difficult to audit.
Parallel construction is useful only where disjointness is explicit.

## Decision

The ordinary builder is exclusively borrowed through `&mut self`. It allocates
zeroed storage and exposes typed offsets rather than long-lived raw pointers.

The separate parallel builder works in phases:

1. A coordinator creates the root/container and reserves disjoint slots.
2. Split APIs transfer non-overlapping ranges to worker-owned partitions.
3. Each worker allocates child objects through an independent `BuildLane`,
   normally producing its own segment(s).
4. Workers seal fragments; sealed fragments cannot be mutated.
5. The coordinator links fragments with far pointers and deterministically
   verifies/finalizes every reserved slot.

Cancellation or panic leaves unfilled slots as valid zero/default values.
Capability registration is excluded from the first partitioned-builder release
unless a separately audited synchronized service is introduced.

## Alternatives considered

- `Arc<Mutex<Builder>>`: rejected because it serializes hot mutation while
  presenting a misleading parallel API.
- Concurrent arbitrary setters over shared raw memory: rejected as unsound and
  incompatible with auditable pointer ownership.
- Build separate messages then deep-copy: safe but rejected as the sole API
  because it loses the intended zero-copy construction benefit.

## Consequences

Parallel output may use more segments and far pointers. Canonical compaction is
a later copying step. The type system, not runtime convention, carries ownership
of each writable range.

## Enforcement

M11 adds borrow/alias compile-fail tests. M12 proves deterministic far-pointer
layouts. M30 adds disjointness compile tests, cancellation/Miri/Loom coverage,
reference reads, and scaling benchmarks.
