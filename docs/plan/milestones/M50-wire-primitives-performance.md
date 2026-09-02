# M50 — Wire primitives performance

- Status: in-progress
- Phase: 9
- Depends on: M49

## Outcome

Make the lowest native wire operations—little-endian scalar loads/stores,
checked ranges, wire words, and pointer bitfields—at least performance-equivalent
to the corresponding pinned C++ primitives without weakening safety or
portability.

## Comparison contract

- Pin C++ to `e7c9cd96f1505b5ae486db7821006c2f5dce5b5b` and exercise its
  `capnp::_::WireValue<T>` implementation from `capnp/endian.h` directly.
- Use identical runtime-generated inputs, operation order, counts, and output
  checksums in optimized single-thread binaries.
- Measure raw wire-value operations separately from Rust's checked slice API so
  bounds/safety cost remains visible rather than silently removed from scope.
- Add pointer-bitfield cases only from exact pinned `WirePointer` operations;
  label any source-derived C++ shim that is required because `WirePointer` is a
  translation-unit-private implementation type.
- Record warmups, at least nine alternating-order samples, medians, ranges,
  binary hashes, CPU/container context, and exact commits.
- Treat a median native/C++ ratio at or below 1.03 as parity within this host's
  measurement noise; the preferred outcome is at or below 1.00.

## Implementation checklist

- [x] Add matched C++ and native endian scalar read/write benchmarks.
- [x] Record and verify the unmodified M49/M50 baseline.
- [x] Inspect optimized assembly or profiles for any material gap.
- [x] Optimize the narrowest owning wire primitive without adding `unsafe` or
  weakening checked bounds, unaligned access, or endian behavior.
- [x] Add matched wire-word and pointer-bitfield cases.
- [x] Record the final comparison and explain any residual variance.
- [ ] Add runner/result checks to Bazel and run full Cargo/Bazel validation.

## Scalar checkpoint

The stable internally timed scalar run uses 4,096 words, 10,000 passes, two
warmups, and nine recorded samples. All eight scalar cases meet the 1.03 parity
bound. The closest semantic analogue to C++ `WireValue<uint64_t>`, a contiguous
Rust `Word` array, is 0.996x C++ on reads and 1.010x on writes. Checked
arbitrary-offset access is 1.011x on reads and 1.000x on writes. Pointer
bitfields subsequently reached 0.972x C++ for decode and 1.006x for checked
encode.

## Required exit evidence

Every lowest-layer case has equivalent checksums and a median native/C++ ratio
no greater than 1.03, with all hostile-boundary and endian tests still passing.
If a case cannot meet that bound without violating repository safety policy,
record the exact reason and obtain a user decision before moving up the stack.

## Scope boundary

M50 does not change framing, message validation, arenas, generated APIs,
reflection, packing, or RPC behavior. Those layers retain their M49 baselines
until their own performance milestone begins.
