# M51 — Standard framing performance

- Status: in-progress
- Phase: 9
- Depends on: M50

## Outcome

Bring standard unpacked segment-table parsing and encoding to performance parity
with pinned Cap'n Proto C++ before optimizing any message traversal, field API,
packing, schema, or RPC layer.

## Comparison contract

- Pin C++ to `e7c9cd96f1505b5ae486db7821006c2f5dce5b5b`.
- Exercise actual C++ flat-message framing APIs where they isolate the same
  work; use a clearly labeled source-derived shim only if a private primitive
  prevents a matched low-level case.
- Cover one-segment, two-segment, and many-segment tables separately so fixed
  overhead is not hidden by body size.
- Use identical segment sizes, deterministic body bytes, limits, operation
  counts, and checksums; exclude fixture generation and process launch from the
  timed region.
- Separate caller-storage/no-allocation parsing from allocating convenience
  parsing and encoding. Add in-memory synchronous stream adapters only after
  the byte-slice cases are understood.
- Record at least two warmups, nine alternating-order samples, binary hashes,
  exact commits/toolchains, medians and ranges. Treat native/C++ <= 1.03 as
  parity on this host.

## Implementation checklist

- [ ] Trace Rust and pinned C++ framing paths and define exact matched work.
- [ ] Add one-, two-, and many-segment parse/encode benchmarks with checksum
  equivalence.
- [ ] Record and verify the unmodified M51 baseline.
- [ ] Inspect assembly or profiles for material gaps.
- [ ] Optimize only framing/table code while preserving all size, overflow,
  truncation, allocation, and caller-storage guarantees.
- [ ] Record final comparisons and explain residual variance.
- [ ] Add Bazel evidence gates and pass full Cargo/MSRV/Bazel/Miri validation.

## Required exit evidence

Every framing case has identical semantic checksums and a median native/C++
ratio no greater than 1.03, or an explicitly documented semantic mismatch that
requires a user decision before advancing to message traversal.

## Scope boundary

M51 may change `capnp-io` standard framing and synchronous frame adapters. It
does not change pointer validation, traversal accounting, message arenas,
packing, generated APIs, reflection, schema tools, or RPC behavior.
