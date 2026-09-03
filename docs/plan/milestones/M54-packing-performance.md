# M54 — Packing performance

- Status: in-progress
- Phase: 8
- Depends on: M50, M51, M53

## Outcome

Make Cap'n Proto packed encoding and decoding at least as efficient as the
pinned C++ implementation without spending the performance already established
for wire access, framing, or construction. Measure the complete one-shot codec
separately from arbitrarily chunked streaming so incremental-state guarantees
cannot make either ownership model look artificially cheap.

## Inherited performance contract

Each matched packing workload records a paired lower-layer case that observes
or copies the exact unpacked input or output bytes without performing the
packing transform. The native/C++ ratio of that lower case, plus the program's
3% measurement tolerance, is the cumulative ceiling for the matching
lower-layer-plus-codec case. After subtracting the lower case on both sides,
the incremental native/C++ pack or unpack cost must be no greater than 1.03.
If subtraction amplifies timer noise, an isolated codec benchmark must
corroborate the result.

One-shot and chunked-streaming results are separate gates. A result for one
input distribution, direction, chunking model, or allocation model cannot
substitute for another.

Encoding inherits the unpacked-input copy/emission floor established below the
packing layer. Decoding inherits materialization of its exact unpacked output;
copying only the compressed input would omit the allocation and byte writes
that both decoders necessarily perform and is not a valid lower-layer case.

## Comparison contract

- Pin C++ to `e7c9cd96f1505b5ae486db7821006c2f5dce5b5b`
  and use its public packed-stream APIs, or a clearly labeled source-derived
  shim only where a codec primitive is otherwise inaccessible.
- Use byte-identical aligned word inputs and require byte-identical packed
  output plus semantic checksums. Cover long zero runs, long `0xff` raw runs,
  mixed/sparse words, and the pinned realistic C++ wire fixture.
- Match operation counts, input and output capacity, output limits, allocation
  or scratch-reuse policy, and observable work. Report one-shot fresh output,
  reusable output where both implementations support it, and chunked streaming
  independently.
- Benchmark encode and decode independently. Include chunk sizes that split
  tags, word payloads, run counts, and raw data; do not average them into the
  one-shot result.
- Force packed bytes, unpacked words, lengths, and checksums to be observable.
  Record two warmups, nine alternating samples, binary hashes, commits,
  toolchains, medians, ranges, cumulative ratios, and incremental ratios.
- Preserve exact output-limit, alignment, truncation, run-boundary, arbitrary
  chunking, and failure-state behavior. Optimization may not introduce
  `unsafe` code.

## Implementation checklist

- [x] Trace native and pinned C++ packed encode/decode paths, buffering,
  allocation, run detection, and streaming state.
- [x] Add matched low-level Rust/C++ fixtures and a paired packing runner with
  exact-output and semantic checksums.
- [x] Record and verify the unmodified M54 baseline.
- [x] Attribute each material gap with isolated phase, allocation-count, or
  profile evidence before changing implementation code.
- [x] Optimize one-shot and streaming paths while preserving safety, limits,
  and deterministic output.
- [x] Record final cumulative and incremental comparisons for every required
  direction, distribution, and ownership model.
- [ ] Add Bazel evidence gates and pass full Cargo/MSRV/Bazel/Miri validation.

## Required exit evidence

Every one-shot and streaming packing shape preserves its paired lower-layer
cumulative ceiling. Every isolated or subtracted incremental encode and decode
ratio is no greater than 1.03. Zero-heavy, raw-heavy, mixed/sparse, and realistic
fixtures must each pass their own gate rather than being averaged together.

## Scope boundary

M54 may change schema-independent packed codecs, buffering, scratch reuse, and
packed stream adapters. It does not change generated accessors, reflection,
compiler tools, JSON/text codecs, RPC behavior, or end-to-end schema workloads;
those belong to later performance milestones.
