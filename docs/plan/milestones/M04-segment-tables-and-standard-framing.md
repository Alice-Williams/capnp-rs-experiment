# M04 — Segment tables and standard framing

- Status: complete
- Phase: 1
- Depends on: M03

## Outcome

Parse and write standard unpacked message framing into immutable segment descriptors under configured limits.

## Implementation checklist

- [x] Restate the pinned `serialize.h`/`serialize.c++` framing rules and
  explicit non-goals in `capnp-io` module documentation.
- [x] Implement bounded complete-slice parsing and encoding with immutable
  borrowed segment descriptors in `capnp-io`.
- [x] Add reproducible pinned-C++ one-, two-, and many-segment fixtures plus
  truncation, overflow, limit, alignment, concatenation, and table-shape tests.
- [x] Run development/MSRV Cargo tests, Clippy, and Bazel validation in the
  Linux development container.
- [x] Record evidence and update `compatibility/manifest.toml`.

## Required exit evidence

Clean EOF differs from truncation; limits/overflow/padding/short bodies fail safely; reference one-, two-, and many-segment frames cross-read.

Evidence recorded on 2026-08-30:

- Pinned C++ frames containing one, two, and 33 segments parse successfully and
  re-encode byte-for-byte; the two-segment fixture is generated reproducibly by
  the M01 oracle tooling with `--segment-size=64`.
- Empty input returns clean EOF, while partial headers, missing table padding,
  incomplete tables, and short bodies return distinct errors.
- The reference 512-segment ceiling, caller segment/word limits, count overflow,
  unaligned writer inputs, and empty writer inputs fail before body slicing.
- Concatenated frames return the exact unconsumed remainder.
- Rust 1.98.0 and Rust 1.85.0 workspace tests passed; Clippy passed with
  warnings denied. Bazel 9.2.0 analyzed 31 targets and all 18 tests passed.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.
