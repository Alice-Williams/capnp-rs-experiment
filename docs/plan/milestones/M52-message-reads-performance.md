# M52 — Message-read performance

- Status: in-progress
- Phase: 9
- Depends on: M51

## Outcome

Make borrowed Cap'n Proto message traversal and common reads at least as
efficient as pinned C++, without spending the framing advantage established by
M51. Retained/owned reads are measured separately so ownership costs cannot be
hidden inside the borrowed result.

## Inherited performance contract

The closest M51 public flat-parse medians are the cumulative ceilings for
parse-plus-read workloads, with the program's 3% measurement tolerance:

| Segments | M51 Rust / C++ | Maximum M52 cumulative ratio |
| ---: | ---: | ---: |
| 1 | 0.321 | 0.331 |
| 2 | 0.167 | 0.172 |
| 64 | 0.339 | 0.349 |

Every cumulative benchmark also records its paired framing-only time in the
same run. After subtracting framing from both implementations, the incremental
Rust/C++ read cost must be no greater than 1.03. If subtraction is dominated by
timer noise, an isolated component benchmark must corroborate it. Neither gate
may be replaced by an unmatched ownership or safety model.

## Comparison contract

- Pin C++ to `e7c9cd96f1505b5ae486db7821006c2f5dce5b5b` and use generated C++ readers
  or a clearly labeled source-derived shim only for private primitives.
- Use byte-identical, C++-generated messages and identical root types, pointer
  paths, field values, traversal/nesting limits, operation counts, and semantic
  checksums.
- Cover a one-segment direct root, a two-segment far pointer, and a 64-segment
  far-pointer/table shape without timing fixture construction.
- Measure framing-only, framing plus root validation, primitive fields,
  borrowed text/data, and retained/owned reopening as separate cases.
- Force returned coordinates and all selected values to be observable. Record
  two warmups, nine alternating samples, binary hashes, commits, toolchains,
  medians, ranges, cumulative ratios, and incremental ratios.
- Preserve exact traversal charging, nesting, bounds, far-pointer, alignment,
  and malformed-input behavior. Optimization may not introduce `unsafe` code.

## Implementation checklist

- [ ] Trace the native and pinned C++ root, direct-pointer, far-pointer,
  traversal-budget, scalar, text, and data read paths.
- [ ] Add byte-identical fixtures and a paired framing/message-read runner with
  checksums that observe every requested result.
- [ ] Record and verify the unmodified M52 baseline.
- [ ] Attribute each material incremental gap with assembly or isolated phase
  evidence before changing implementation code.
- [ ] Optimize only message validation/read paths while preserving hostile-input
  and exact-budget guarantees.
- [ ] Record final cumulative and incremental comparisons.
- [ ] Add Bazel evidence gates and pass full Cargo/MSRV/Bazel/Miri validation.

## Required exit evidence

Every shape preserves its M51 cumulative ceiling and has an incremental
native/C++ read ratio no greater than 1.03. Any semantic mismatch remains a
separately labeled diagnostic and cannot advance the layer.

## Scope boundary

M52 may change `capnp-message` validation, traversal budgeting, borrowed field
reads, and retained read-context construction. It does not change builders,
packing, schema reflection, generated accessor strategy, compiler tools, or
RPC behavior; those belong to later performance milestones.
