# M18 — Dynamic value, struct, and list API

- Status: complete
- Phase: 3
- Depends on: M11, M17

## Outcome

Provide reflection-driven dynamic readers/builders, typed downcast, and generic stringification.

## Implementation checklist

- [x] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [x] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [x] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [x] Run Cargo and Bazel validation in the Linux development container.
- [x] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Dynamic/generated views agree; field/union/list/enum operations work; schemas need no leaked lifetime or global mutable registry.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.

## Completion evidence

- Owned dynamic struct and list views retain both compiled schema and checked
  message coordinates without a global registry, leaked lifetime, or
  self-referential borrow.
- Dynamic reads and writes cover scalar defaults, text, data, structs, all list
  encodings, groups, unions, known and unknown enum ordinals, capabilities, and
  any-pointer values. Generic brand substitution is exercised by the pinned
  `Box(Text)` fixture and aggregate schema constants remain readable after
  compilation.
- A generated-style low-level reader and the dynamic reader observe identical
  field values; typed downcasts validate schema IDs, inactive union members fail
  closed, and dynamic values have deterministic generic formatting.
- Dynamic message views are `Send + Sync`, while exclusive child builders are
  statically prevented from aliasing by a compile-fail documentation test.
- Rust 1.98.0 and Rust 1.85.0 workspace tests passed; public API docs and
  Clippy with warnings denied passed; Bazel passed the complete test suite.
