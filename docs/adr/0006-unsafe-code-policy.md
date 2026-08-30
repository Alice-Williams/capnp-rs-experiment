# ADR-0006: Unsafe code policy

- Status: accepted
- Milestone: M02
- Date: 2026-08-30

## Context

Endian loads, alignment fast paths, arena allocation, and I/O may eventually
benefit from unsafe code. Introducing it before safe semantics and benchmarks
exist would enlarge the trusted computing base at the most hostile boundaries.

## Decision

- Workspace crates forbid unsafe code by default.
- A safe implementation is required before an unsafe optimization is proposed.
- Enabling unsafe requires a follow-up ADR naming the module, measurable need,
  exact preconditions, aliasing/provenance/alignment/lifetime invariants, and
  safe fallback.
- Unsafe blocks are confined to small private modules and each block has a
  `SAFETY` explanation tied to tests.
- Public `unsafe impl Send` and `unsafe impl Sync` are prohibited. Public types
  derive auto traits from their fields. Any future exception requires a
  dedicated owner-reviewed ADR and compile tests.
- Hostile-input validation and checked arithmetic remain outside unsafe fast
  paths; unsafe code cannot bypass resource limits.

## Alternatives considered

- Ban unsafe forever: retained as the current state, but not made irrevocable
  because measured alignment/allocator paths may justify narrow use.
- Permit unsafe anywhere with code review: rejected because the audit boundary
  would grow continuously.
- Use unsafe trait implementations to force thread safety: rejected because it
  can hide unsound representation choices.

## Consequences

Early implementations may be slower but establish a trustworthy baseline.
Optimization work bears the cost of proof and regression evidence.

## Enforcement

The workspace `unsafe_code = "forbid"` lint is active under Cargo. Bazel targets
must retain equivalent rustc linting before unsafe can be enabled. Approved
unsafe modules require Miri/sanitizer coverage and boundary fuzzing.
