# M20 — Codegen: generics, brands, constants, and annotations

- Status: complete
- Phase: 3
- Depends on: M19

## Outcome

Complete non-RPC generation for generic scopes, brands, constants, annotations, and cross-crate imports.

## Implementation checklist

- [x] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [x] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [x] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [x] Run Cargo and Bazel validation in the Linux development container.
- [x] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Generic/unbound cases and pointer constants compile/run; annotation metadata is typed; large-schema compile growth is benchmarked.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.

## Completion evidence

- Generated readers and builders retain outer and inner generic scopes as Rust
  type parameters. `Box(String)`, `Box(Vec<u8>)`, and
  `Box(T).Pair(String, Vec<u8>)` decode the pinned C++ language fixture, while
  defaulted unbound parameters expose the lossless dynamic value.
- `GeneratedType` creates concrete runtime brands at generic roots and
  `FieldInput` gives generic scalar/pointer setters a typed path into the
  exclusive native builder. A generated `Box(String)` builder round-trips its
  value through the native wire runtime.
- Scalar, Text, and Data constants are emitted as Rust constants. Pointer list
  and branded struct constants open through schema-owning functions and retain
  their request backing without a static registry or leaked lifetime.
- Annotation declarations emit typed value decoders and exact target metadata.
  Import file IDs can map to external Rust paths; a separate generated crate
  composes wire and language APIs from another crate for both reads and builds.
- The reproducible compile-growth harness measured 1,976 to 15,808 generated
  lines (105,794 to 846,352 bytes): warmed `cargo check` grew from 761 ms to
  2,603 ms for 1x to 8x source on the G-drive Linux environment.
- Rust 1.98.0 and Rust 1.85.0 workspace tests passed; public API docs and
  Clippy with warnings denied passed; Bazel passed the complete test suite.
