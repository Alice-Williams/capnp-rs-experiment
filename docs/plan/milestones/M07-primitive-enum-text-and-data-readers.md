# M07 — Primitive, enum, text, and data readers

- Status: complete
- Phase: 1
- Depends on: M03, M05, M06

## Outcome

Expose low-level and dynamic readers for all primitive values, enums, Text, and Data.

## Implementation checklist

- [x] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [x] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [x] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [x] Run Cargo and Bazel validation in the Linux development container.
- [x] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Default XOR, unknown enums, bool bits, text NUL/UTF-8 semantics, and zero-copy borrowed reads match independent fixtures.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.
