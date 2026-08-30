# M25 — Native CodeGeneratorRequest and self-hosted generation

- Status: complete
- Phase: 4
- Depends on: M20, M24

## Outcome

Emit standard CodeGeneratorRequest values natively and regenerate project schemas with the Rust backend.

## Implementation checklist

- [x] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [x] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [x] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [x] Run Cargo and Bazel validation in the Linux development container.
- [x] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Native/reference requests agree semantically; regeneration is deterministic; clean Cargo builds need no system capnp; bootstrap is tested.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.

## Evidence

- `capnp-compiler::request` constructs owned schema models and standard framed
  requests for every pinned fixture, including imports, brands, annotations,
  interfaces, streaming methods, source metadata, and aggregate constants.
- Native compilation of the pinned upstream `schema.capnp` agrees with the C++
  oracle, reloads after native serialization, and produces identical canonical
  Rust code. The checked-in request is reflection bootstrap data, not generated
  implementation source.
- The generated fixture build scripts consume `.capnp` sources through the
  native resolver/compiler/code generator and do not execute a system `capnp`.
- Repeated model serialization and Rust generation are deterministic. Codegen
  canonicalizes schema-node identity order so equivalent requests do not leak
  producer-specific traversal order into generated source.
- Validation: Rust 1.98 and MSRV 1.85 workspace tests, Clippy with warnings
  denied, public API documentation, fixture/upstream hashes, and the complete
  Bazel suite in the Linux development container.
