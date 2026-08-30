# Project charter

## Mission

Build a conformance-first, Rust-native implementation of the Cap'n Proto
platform with immutable shared readers, explicit partitioned construction, and
actor-owned thread-safe RPC state. Compatibility follows the pinned C++
implementation and normative formats; it does not preserve architectural
limitations of the current Rust implementation.

## First release boundary

The first release sequence is secure wire reading (M00–M10), complete
serialization (M11–M16), generated data APIs (M17–M21), and a native compiler
(M22–M28). Parallel data APIs follow those safety foundations. RPC reaches a
hardened two-party Level 1 before Level 3 handoff, Level 4 Join, and persistent
capabilities.

## Support matrix

| Dimension | M00 policy |
|---|---|
| Development host | Any host that can run the repository's Linux Dev Container |
| Primary execution target | Linux x86-64 |
| Cargo MSRV | Rust 1.85.0 |
| Pinned development/Bazel Rust | Rust 1.98.0 |
| Rust edition | 2024 |
| Bazel | Bazelisk 1.29.0 selecting Bazel 9.2.0 |
| Bazel Rust rules | rules_rust 0.74.0 |
| Runtime modes | std first; no_std + alloc and no-allocation wire reading are required later |
| Endianness/alignment | Little-endian fast path; big-endian and strict/unaligned correctness are release requirements |
| Async | Executor-neutral core with first-party adapters later |

The MSRV is distinct from the pinned development compiler: Cargo must continue
to check 1.85.0 while normal development and Bazel use 1.98.0.

## Compatibility dimensions

- Wire: bit-exact encoding, framing, limits, evolution, packing, and canonical form.
- Generated API: equivalent data/interface semantics, not identical Rust names or lifetimes.
- Tooling: native schema compilation and documented replacement CLI workflows.
- RPC: explicitly versioned protocol levels and pinned schemas.
- Product surface: compatibility adapters and capability utilities tracked separately.

## Non-goals

- A line-for-line port of capnproto-rust or KJ.
- Pervasive locks as a substitute for an ownership/concurrency design.
- Arbitrary concurrent mutation of one object graph.
- Parallelizing ordered frame emission, canonical preorder, or one connection's protocol transitions.
- Claiming compatibility from Rust-only round trips.
- Stabilizing a facade API before wire and ownership invariants are proven.

## Open owner decision

The repository license has not been selected. M00 remains in progress until
the owner chooses it; no license text or SPDX expression will be guessed.
