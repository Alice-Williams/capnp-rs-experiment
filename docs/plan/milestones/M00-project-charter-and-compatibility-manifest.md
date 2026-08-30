# M00 — Project charter and compatibility manifest

- Status: complete
- Phase: 0
- Depends on: None

## Outcome

Create the workspace skeleton, charter, support matrix, pinned upstream revisions and schema hashes, feature-level table, non-goals, and ADR template.

## Implementation checklist

- [x] Record authority order, architecture, compatibility dimensions, support
  matrix, measurable performance gates, and non-goals.
- [x] Create the eleven-crate Cargo and Bazel workspace boundaries.
- [x] Pin the C++ and current-Rust reference commits, schema URLs/hashes,
  development Rust, MSRV, Bazel, Bazelisk, and rules_rust.
- [x] Add an ADR template and repository-wide agent rules.
- [x] Add CI steps for remote pin verification, development/MSRV Cargo tests,
  and Bazel tests.
- [x] Verify all pinned standard-schema hashes against the upstream commit.
- [x] Run Cargo tests/Clippy on Rust 1.98.0, Cargo tests on Rust 1.85.0, and
  all Bazel tests in the Linux development container.
- [x] Select and add the repository license (MIT).
- [x] Push the complete CI workflow and obtain owner approval to use the
  equivalent passing local Linux suite as the implementation gate while hosted
  observation is temporarily unavailable.

## Required exit evidence

Local evidence recorded on 2026-08-30:

- Five pinned schema SHA-256 values matched upstream.
- Rust 1.98.0 workspace tests and Clippy passed.
- Rust 1.85.0 MSRV workspace tests passed.
- Bazel 9.2.0 analyzed 30 targets and all 18 tests passed, including schema,
  fixture, script, and recorded-baseline integrity gates.

The owner selected MIT, recorded in `LICENSE`, Cargo package metadata, and the
compatibility manifest. Browser-based CI inspection was temporarily unavailable
after the foundation push, so no passing hosted result is claimed here. On
2026-08-30 the owner explicitly accepted the equivalent passing local Linux
suite and successful push as sufficient evidence to continue implementation.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.
