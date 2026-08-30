# M02 — Safety and concurrency model ADRs

- Status: complete
- Phase: 0
- Depends on: M00

## Outcome

Freeze reader ownership, exact budgets, builder separation, RPC actors/order/cancellation, and unsafe-code policy in ADRs plus compile-only prototypes.

## Implementation checklist

- [x] Accept ADR-0001 for owned/borrowed readers and stable wire locations.
- [x] Accept ADR-0002 for exact local/shared traversal and per-reader nesting budgets.
- [x] Accept ADR-0003 separating exclusive and partitioned builders.
- [x] Accept ADR-0004 for actor-owned RPC state and explicit scheduling policies.
- [x] Accept ADR-0005 for cancellation, disconnect, shutdown, and reconnect semantics.
- [x] Accept ADR-0006 confining unsafe optimization behind safe baselines and follow-up ADRs.
- [x] Add compile prototypes for `OwnedMessage`, `ObjectRef<T>`, `Client`, and a `Send` server future without unsafe trait implementations.
- [x] Run development/MSRV Cargo tests, Clippy, and Bazel tests in Linux.
- [x] Update the compatibility manifest.

## Required exit evidence

Evidence recorded on 2026-08-30:

- Six accepted ADRs name alternatives, invariants, consequences, and enforcing tests.
- Compile prototypes derive `Send + Sync` for owned-message/object-reference and client/server shapes with no unsafe code.
- Rust 1.98.0 tests and Clippy passed.
- Rust 1.85.0 MSRV tests passed.
- Bazel 9.2.0 passed all 11 workspace tests.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.
