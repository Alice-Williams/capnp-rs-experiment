# M00 — Project charter and compatibility manifest

- Status: in-progress
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
- [x] Verify all four schema hashes against the pinned upstream commit.
- [x] Run Cargo tests/Clippy on Rust 1.98.0, Cargo tests on Rust 1.85.0, and
  all Bazel tests in the Linux development container.
- [ ] Select and add the repository license.
- [ ] Observe the new workflow passing on GitHub after this foundation is
  pushed.

## Required exit evidence

Local evidence recorded on 2026-08-30:

- Four pinned schema SHA-256 values matched upstream.
- Rust 1.98.0 workspace tests and Clippy passed.
- Rust 1.85.0 MSRV workspace tests passed.
- Bazel 9.2.0 analyzed 22 targets and all 11 tests passed.

Completion is blocked only on the owner license choice and first hosted CI run.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.
