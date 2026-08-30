# M03 — Words, endian, and checked wire integers

- Status: in-progress
- Phase: 1
- Depends on: M00, M02

## Outcome

Implement no_std wire words, scalar endian conversion, checked integer/range helpers, and pointer bitfield encoding/decoding.

## Implementation checklist

- [ ] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [ ] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [ ] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [ ] Run Cargo and Bazel validation in the Linux development container.
- [ ] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Exact scalar and NaN fixtures, signed 30-bit edge tests, pointer-field properties, and big-endian/unaligned byte-path tests pass.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.
