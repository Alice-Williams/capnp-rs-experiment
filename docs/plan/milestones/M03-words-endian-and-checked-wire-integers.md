# M03 — Words, endian, and checked wire integers

- Status: complete
- Phase: 1
- Depends on: M00, M02

## Outcome

Implement no_std wire words, scalar endian conversion, checked integer/range helpers, and pointer bitfield encoding/decoding.

## Implementation checklist

- [x] Restate the compatibility sources, invariants, and explicit non-goals in
  `capnp-wire` module documentation.
- [x] Implement allocation-free byte/endian access, checked byte/word ranges,
  checked signed displacement, and raw pointer bitfields in `capnp-wire`.
- [x] Add exact scalar/NaN/oracle-pointer fixtures, signed 30-bit edge tests,
  29/30-bit rejection tests, field round-trip properties, and
  host-endian-independent unaligned byte-path tests.
- [x] Run development/MSRV Cargo tests, Clippy, and Bazel validation in the
  Linux development container.
- [x] Record evidence and update `compatibility/manifest.toml`.

## Required exit evidence

Exact scalar and NaN fixtures, signed 30-bit edge tests, pointer-field properties, and big-endian/unaligned byte-path tests pass.

Evidence recorded on 2026-08-30:

- The root pointer bytes from the pinned C++ `wire-unpacked.bin` fixture decode
  as zero offset, nine data words, and 28 pointer words.
- Fourteen `capnp-wire` tests cover exact integer/floating bytes, NaN payload
  preservation, arithmetic failures, signed 30-bit limits, every list element
  size, inline-composite tags, far/double-far pointers, capabilities, reserved
  `OTHER` values, and unaligned I/O.
- Rust 1.98.0 and Rust 1.85.0 workspace tests passed; Clippy passed with
  warnings denied.
- Bazel 9.2.0 analyzed 30 targets and all 18 tests passed.
- The crate remains `no_std`, allocation-free, and free of unsafe code.

## Scope boundary

Later milestone behavior may be anticipated in types only where required to avoid a known compatibility dead end. It must not be implemented or claimed here.
