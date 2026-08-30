# M05 — Struct, list, far, and capability pointer validation

- Status: complete
- Phase: 1
- Depends on: M03, M04

## Outcome

Follow every pointer kind lazily with bounds-checked concrete wire reader results.

## Implementation checklist

- [x] Restate pinned `WirePointer`/`followFars` invariants and ADR-0001's
  coordinate-only reader rule in implementation documentation.
- [x] Implement bounded struct, list, inline-composite, single-far, double-far,
  capability, null, and reserved-pointer validation in `capnp-message`.
- [x] Add positive, malformed, boundary, and randomized multi-segment coverage.
- [x] Run development/MSRV Cargo tests, Clippy, and Bazel validation in Linux.
- [x] Record evidence and update `compatibility/manifest.toml`.

## Required exit evidence

Null, struct, all list sizes, inline composite, far/double-far, capability, reserved, and malformed cases are tested; fuzzing cannot escape segments.

Evidence recorded on 2026-08-30:

- Successful results contain only stable segment/word coordinates, never cached
  native pointers.
- Ten `capnp-message` tests cover every pointer kind and list size, valid and
  overrunning inline composites, unknown segments, malformed landing pads, and
  reserved `OTHER` values.
- Two deterministic 10,000-case malformed-word campaigns include randomized
  two-segment far-pointer graphs and assert every returned extent is in bounds.
- Rust 1.98.0 and Rust 1.85.0 workspace tests passed; Clippy passed with
  warnings denied. Bazel 9.2.0 analyzed 31 targets and all 18 tests passed.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.
