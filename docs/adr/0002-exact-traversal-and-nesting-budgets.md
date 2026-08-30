# ADR-0002: Exact traversal and nesting budgets

- Status: accepted
- Milestone: M02
- Date: 2026-08-30

## Context

Traversal limits are a security boundary. A relaxed load followed by a separate
store can undercount concurrent reads, allowing aggregate work beyond the
configured limit. A shared mutable nesting counter would also couple otherwise
independent branches.

## Decision

- Shared budgets use an atomic compare-exchange/fetch-update operation that
  deducts a complete charge or fails without changing the balance.
- Local readers use a non-atomic budget type with identical charge semantics.
- A target is charged before any view into it is returned.
- Zero-sized lists are charged at least one word per element to bound
  amplification.
- Remaining nesting depth is copied into each child reader and decremented when
  following a pointer; it is not shared mutable state.
- Message-size/allocation limits and traversal-work limits are distinct.
- Targets without required atomics expose local-only reading explicitly.

Bounded lease optimization is permitted only if reservations are atomic, the
sum of outstanding leases cannot exceed the global maximum, and abandoned
leases are safely returned or remain conservatively charged.

## Alternatives considered

- Racy relaxed load/store accounting: rejected because it weakens a hard limit.
- One mutex around traversal: exact but rejected as the default shared-read hot
  path; it may remain a correctness comparison implementation.
- Approximate per-thread counters: rejected unless backed by exact bounded
  reservations.
- One shared nesting counter: rejected because parallel branches interfere.

## Consequences

Concurrent traversal cannot exceed the configured work allowance. Each pointer
follow pays an atomic cost in shared mode, so benchmarks must establish whether
bounded leases are worthwhile.

## Enforcement

M06 adds edge, amplification, cycle/overlap, and Loom exhaustion tests. Property
tests must show every successful concurrent charge set sums to no more than the
initial allowance. Benchmarks compare exact atomic, local, and any lease design.
