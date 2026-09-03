# M55 — Generated data API performance

- Status: in-progress
- Phase: 8
- Depends on: M50, M52, M53

## Outcome

Make generated, statically typed data readers and builders preserve the native
performance advantage already established by the wire, message-read, and
message-construction layers. Generated accessors compile field layout into the
call site; ordinary typed operations must not pay for string lookup, cloned
schema metadata, or owned blob conversion. Reflection remains a supported,
explicit dynamic API rather than the implementation of the generated fast
path.

## Inherited performance contract

Each generated-API workload has a paired direct-runtime workload that performs
the same underlying checked wire access or construction and observes the same
result. The direct-runtime native/C++ ratio, plus the program's 3% measurement
tolerance, is the cumulative ceiling for the corresponding generated workload.
After subtracting the paired lower-layer times, the incremental native/C++ cost
of the generated API must be no greater than 1.03. Where subtraction is below
timer resolution, an isolated accessor benchmark must corroborate the result.

Reader and builder results are separate gates. Different field kinds, ownership
models, list element representations, union states, defaults, or schema
versions are not averaged together. Borrowed and retained readers are reported
separately when their lifetime or traversal-accounting costs differ.

## Comparison contract

- Pin C++ to `e7c9cd96f1505b5ae486db7821006c2f5dce5b5b`
  and generate both languages from byte-identical schemas whose layouts are
  already covered by the conformance corpus.
- Exercise constant-offset scalar and enum fields, pointer-backed text/data and
  structs, primitive and pointer lists, defaults, groups/unions (including
  unknown discriminants), and old-reader/new-writer schema evolution.
- Pair generated reads with equivalent direct checked readers and generated
  writes with equivalent direct checked builders. Match validation state,
  traversal limits, allocation/scratch policy, operation order, and observable
  checksums.
- Require byte-identical results where allocator layout is fixed, otherwise
  require cross-language semantic round trips and equivalent observable work.
- Record cold construction/opening separately from repeated hot access. Do not
  hide schema lookup, reader creation, allocation, or blob copying by placing it
  outside only one language's timed region.
- Record two warmups, nine alternating optimized samples, binary hashes,
  producer commits, toolchains, medians, ranges, cumulative ratios, and
  incremental ratios.
- Preserve checked arithmetic, traversal/output limits, default XOR semantics,
  unknown enum/union behavior, aliasing rules, and schema-evolution behavior.
  Optimization may not introduce `unsafe` code.

## Implementation checklist

- [ ] Trace generated Rust and pinned C++ accessor code for every required field
  shape and identify reflection, metadata, allocation, and ownership costs.
- [ ] Add matched generated/direct-runtime Rust and C++ fixtures plus a paired
  benchmark runner with exact semantic checksums.
- [ ] Record and verify the unmodified M55 baseline.
- [ ] Attribute every material gap with isolated phase, allocation-count, or
  profile evidence before changing runtime or generated code.
- [ ] Generate constant-layout typed fast paths and borrowed blob views while
  keeping reflection available through the explicit dynamic API.
- [ ] Optimize typed lists, groups/unions, defaults, and evolution paths without
  weakening validation or ownership guarantees.
- [ ] Record final cumulative and incremental comparisons for every required
  reader, builder, field-shape, ownership, and evolution gate.
- [ ] Add Bazel evidence gates and pass full Cargo/MSRV/Bazel/Miri validation.

## Required exit evidence

Every generated workload preserves its paired direct-runtime cumulative
ceiling. Every isolated or subtracted incremental accessor ratio is no greater
than 1.03. Scalar, blob, struct, list, union/default, and schema-evolution cases
must each pass their own read and/or build gate rather than being averaged.
Generated-source tests must prove that the hot path embeds layout constants and
does not call dynamic field lookup.

## Scope boundary

M55 may change generated data bindings, their code generator, and narrowly
required checked runtime primitives. It does not optimize general reflection,
the schema compiler, text/JSON codecs, RPC control messages or actor scheduling,
or end-to-end application workloads; those belong to M56–M58.
