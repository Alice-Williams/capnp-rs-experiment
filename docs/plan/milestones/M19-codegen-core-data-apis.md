# M19 — Codegen: structs, enums, unions, and lists

- Status: complete
- Phase: 3
- Depends on: M17, M18

## Outcome

Generate core typed Rust data APIs from a pinned reference CodeGeneratorRequest.

## Implementation checklist

- [x] Restate the compatibility sources, invariants, and explicit non-goals in module or design documentation.
- [x] Implement only this milestone's deliverable behind the narrowest crate boundary that owns the invariant.
- [x] Add the independent fixtures and positive, negative, property, compile, concurrency, fuzz, or benchmark coverage appropriate to this boundary.
- [x] Run Cargo and Bazel validation in the Linux development container.
- [x] Record evidence and update compatibility/manifest.toml.

## Required exit evidence

Generated crates avoid the C++ runtime and cover accessors/defaults/unions/groups/unknown enums/lists/imports/docs with cross-language round trips.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.

## Completion evidence

- Deterministic generation from pinned `CodeGeneratorRequest` fixtures emits
  typed struct/group readers and builders, typed enum and recursive list
  readers, union `Which` values, source documentation, stable type IDs, and an
  explicit import table without invoking the C++ runtime.
- Unknown enum ordinals and union discriminants remain observable through
  collision-safe fallback variants. Scalar and pointer defaults use the same
  native evolution semantics as dynamic access.
- A build-script fixture compiles generated output for wire, evolution-v2, and
  imported schemas. Its typed reader decodes the pinned C++ frame, including
  lists, groups, defaults, and nested structs; generated builders round-trip
  the same features through the native runtime.
- The pinned C++ decoder accepts a 360-byte standard frame emitted through the
  generated Rust builder and observes its scalar, unknown enum, text, list,
  union, and nested-struct values.
- Rust 1.98.0 and Rust 1.85.0 workspace tests passed; public API docs and
  Clippy with warnings denied passed; Bazel passed the complete test suite.
