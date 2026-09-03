# M53 — Message-construction performance

- Status: in-progress
- Phase: 9
- Depends on: M50, M52

## Outcome

Make schema-independent Cap'n Proto message construction at least as efficient
as pinned C++ from arena creation through emitted wire bytes. Preserve the M50
wire-write performance floor instead of using it to hide allocation, placement,
or copying overhead. Measure fresh allocation and reusable scratch storage as
separate ownership models.

## Inherited performance contract

M50 established parity for the checked scalar-store and pointer-encoding
primitives used by construction. Their final native/C++ ratios are carried into
the matching prepared-storage construction cases, with the program's 3%
measurement tolerance:

| Lower-layer operation | M50 Rust / C++ | Maximum M53 prepared-storage ratio |
| --- | ---: | ---: |
| checked scalar store | 1.000 | 1.030 |
| pointer encode | 1.006 | 1.036 |

Each complete-build benchmark also records a paired prepared-storage lower case
in the same run. After subtracting that lower case from both implementations,
the incremental Rust/C++ cost for arena allocation, object placement, or graph
copy must be no greater than 1.03. If subtraction amplifies timer noise, an
isolated component benchmark must corroborate it. A scratch-reuse result cannot
substitute for the fresh-allocation result, or vice versa.

## Comparison contract

- Pin C++ to `e7c9cd96f1505b5ae486db7821006c2f5dce5b5b`
  and use public C++ builder APIs or a clearly labeled source-derived shim only
  for otherwise inaccessible primitives.
- Use byte-identical output shapes, identical initial segment capacities,
  allocation strategies, operation counts, and semantic checksums. Compare
  serialized segment words as well as selected values so a cheaper but
  different graph cannot pass.
- Cover a one-segment direct root, a two-segment single-far placement, and a
  multi-segment double-far/landing-pad shape. Include scalar/data writes,
  schema-independent graph copy, and fresh arena creation.
- Benchmark safe scratch reset/reuse separately on both sides. Native reset must
  zero previously used bytes before reuse and must not permit old builders,
  offsets, or pointers to address the new message.
- Force all emitted coordinates, sizes, values, and checksums to be observable.
  Record two warmups, nine alternating samples, binary hashes, commits,
  toolchains, medians, ranges, cumulative ratios, and incremental ratios.
- Preserve exact allocation-limit, overflow, placement, zero-initialization,
  aliasing, and far-pointer behavior. Optimization may not introduce `unsafe`
  code.

## Implementation checklist

- [ ] Trace native and pinned C++ arena allocation, root placement, object
  allocation, far-pointer emission, graph-copy, and scratch-reuse paths.
- [ ] Add matched low-level Rust/C++ fixtures and a paired construction runner
  with wire-output and semantic checksums.
- [ ] Record and verify the unmodified M53 baseline.
- [ ] Attribute each material gap with isolated phase, allocation-count, or
  profile evidence before changing implementation code.
- [ ] Optimize only schema-independent construction paths while preserving
  safety, limits, and deterministic output.
- [ ] Record final cumulative and incremental comparisons.
- [ ] Add Bazel evidence gates and pass full Cargo/MSRV/Bazel/Miri validation.

## Required exit evidence

Every prepared-storage shape preserves its applicable M50 cumulative ceiling.
Every fresh-build and reuse shape has an incremental native/C++ construction
ratio no greater than 1.03, or an isolated component result at or below 1.03
when subtraction is too noisy. The fresh and reused cases must each satisfy
their own gate; neither is averaged with the other.

## Scope boundary

M53 may change schema-independent builder arenas, allocation and placement,
pointer emission, graph copy, clear/reset, and scratch-storage APIs. It does not
change packed encoding, generated accessor strategy, reflection, compiler
tools, or RPC behavior; those belong to later performance milestones.
