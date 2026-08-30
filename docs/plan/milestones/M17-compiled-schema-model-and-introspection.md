# M17 — Compiled schema model and introspection

- Status: complete
- Phase: 3
- Depends on: M10, M16

## Outcome

Represent schema nodes, types, brands, values, annotations, fields, enums, and source information at runtime.

## Implementation checklist

- [x] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [x] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [x] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [x] Run Cargo and Bazel validation in the Linux development container.
- [x] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Pinned CodeGeneratorRequest fixtures load; all lookup paths work; malformed metadata fails; every conformance feature is describable.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.

## Completion evidence

- All seven pinned C++ `CodeGeneratorRequest` fixtures load through native
  framing, traversal, schema decoding, and owned-model validation.
- Indexed node, nested declaration, source-info, requested-file, field,
  enumerant, and method lookup paths are exercised against language and wire
  fixtures. Generics, brands, annotations, constants, groups, unions, source
  identifiers, and every schema type/value discriminant are representable.
- Duplicate and inconsistent lookup metadata, invalid prefixes, unknown tags,
  invalid framing, and exhausted metadata limits fail closed.
- Rust 1.98.0 and Rust 1.85.0 workspace tests passed; public API docs and
  Clippy with warnings denied passed; Bazel passed 18/18 tests.
