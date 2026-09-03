# M52 message-read performance

## Root-read baseline

The first checkpoint measures identical one-, two-, and 64-segment standard
frames. `framing` parses and observes every segment descriptor. `root` repeats
that exact work, creates the implementation's message-reader context, follows
the root struct pointer, applies traversal limits, and reads its data section.
The two-segment case follows a single-far pointer; the 64-segment case follows a
single-far pointer to an empty struct in segment 63. Each sample contains
100,000 operations, with two warmups and nine alternating recorded runs.

| Case | Segments | C++ ns/message | Rust ns/message | Rust / C++ |
| --- | ---: | ---: | ---: | ---: |
| framing | 1 | 13.9636 | 4.0712 | 0.292 |
| root | 1 | 42.3849 | 45.1501 | 1.065 |
| framing | 2 | 45.8401 | 5.4062 | 0.118 |
| root | 2 | 153.6480 | 46.5471 | 0.303 |
| framing | 64 | 405.7314 | 155.7151 | 0.384 |
| root | 64 | 545.0687 | 259.4168 | 0.476 |

Subtracting the paired framing medians isolates the added message-read work:

| Segments | C++ incremental ns | Rust incremental ns | Rust / C++ |
| ---: | ---: | ---: | ---: |
| 1 | 28.4213 | 41.0789 | 1.445 |
| 2 | 107.8079 | 41.1409 | 0.382 |
| 64 | 139.3373 | 103.7017 | 0.744 |

This baseline fails M52's cumulative inherited-speedup gate for every shape.
The nearly identical 41 ns Rust increment for one and two segments initially
suggested that allocating descriptor storage in `MessageSegments::new()` was
the dominant fixed cost. The next checkpoint disproved that attribution.

Evidence:
`benchmarks/results/2026-09-03-m52-root-baseline-g-drive-docker/`.

## Inline small-context checkpoint

`MessageSegments` now stores one- and two-segment descriptors inline while
retaining owned descriptor storage for larger messages. The same paired run
measured:

| Case | Segments | C++ ns/message | Rust ns/message | Rust / C++ |
| --- | ---: | ---: | ---: | ---: |
| framing | 1 | 12.3646 | 4.0512 | 0.328 |
| root | 1 | 49.5744 | 40.3939 | 0.815 |
| framing | 2 | 37.7568 | 6.4043 | 0.170 |
| root | 2 | 164.3219 | 48.6703 | 0.296 |
| framing | 64 | 394.8309 | 169.4291 | 0.429 |
| root | 64 | 575.2095 | 282.8635 | 0.492 |

The paired incremental costs were 36.3427 ns versus 37.2098 ns for one
segment, 42.2660 ns versus 126.5651 ns for two segments, and 113.4344 ns
versus 180.3786 ns for 64 segments. Removing the small-message allocation
improved the stable one-segment Rust root median from 45.1501 ns to 40.3939 ns,
but left most of the increment intact. The dominant cost is therefore in
validation and accessor work rather than allocation. Source tracing next found
that the bounded-pointer path read and checked the root pointer twice and that
several public hot methods were not available for cross-crate inlining.

Evidence:
`benchmarks/results/2026-09-03-m52-inline-small-contexts-g-drive-docker/`.

## Single-read and inlining checkpoint

The bounded root path previously decoded and bounds-checked the same root word
twice. It now carries the decoded word into validation, and the checked
message, budget, data-section, and primitive helpers are available for
cross-crate inlining. This reduced the paired Rust root medians substantially:

| Case | Segments | C++ ns/message | Rust ns/message | Rust / C++ |
| --- | ---: | ---: | ---: | ---: |
| framing | 1 | 21.5606 | 6.7892 | 0.315 |
| root | 1 | 72.9262 | 25.9398 | 0.356 |
| framing | 2 | 106.0102 | 10.2523 | 0.097 |
| root | 2 | 409.7002 | 41.5332 | 0.101 |
| framing | 64 | 883.6094 | 277.9353 | 0.315 |
| root | 64 | 1232.8038 | 506.9421 | 0.411 |

The paired incremental Rust/C++ ratios are 0.373, 0.103, and 0.656. The
two-segment shape passes its cumulative gate; the one- and 64-segment shapes
remain above their inherited ceilings. The remaining 64-segment increment
includes allocating and copying a second descriptor table after framing; the
next step is to borrow already-owned descriptors. The direct-root validation
path also still performs generic range work that can be specialized without
changing checked-coordinate semantics.

Evidence:
`benchmarks/results/2026-09-03-m52-streamlined-root-g-drive-docker/`.

## Shared-descriptor root checkpoint

Framing and message reading now share one safe `Segment` descriptor whose
private representation proves word alignment. The message context borrows the
already-validated framing table directly. A specialized struct-root path also
reuses the source segment while checking a direct pointer, while far pointers
retain the general checked-coordinate path. No native pointer is cached and no
unsafe code is used.

| Case | Segments | C++ ns/message | Rust ns/message | Rust / C++ | Ceiling |
| --- | ---: | ---: | ---: | ---: | ---: |
| framing | 1 | 13.2911 | 4.2493 | 0.320 | — |
| root | 1 | 44.1166 | 12.7450 | 0.289 | 0.331 |
| framing | 2 | 57.4517 | 5.7215 | 0.100 | — |
| root | 2 | 168.2798 | 26.5202 | 0.158 | 0.172 |
| framing | 64 | 436.4597 | 185.6482 | 0.425 | — |
| root | 64 | 566.7974 | 192.0447 | 0.339 | 0.349 |

Every root shape now passes the inherited cumulative gate. Paired subtraction
gives incremental Rust/C++ ratios of 0.276, 0.188, and 0.049. The 64-segment
increment is only 6.3965 ns and the framing/root sample ranges overlap, so M52
does not treat that subtraction alone as a stable component claim; an isolated
root-read benchmark must corroborate it before this layer is final.

Evidence:
`benchmarks/results/2026-09-03-m52-shared-descriptors-g-drive-docker/`.

## Isolated-root evidence

An additional `isolated-root` case constructs a reader over prevalidated
segment descriptors and performs the same charged root/data/value read without
standard-frame parsing. It uses C++ `SegmentArrayMessageReader` and Rust
`MessageSegments::from_descriptors`, so neither side receives framing work.

| Segments | C++ ns/read | Rust ns/read | Rust / C++ |
| ---: | ---: | ---: | ---: |
| 1 | 33.2673 | 17.0007 | 0.511 |
| 2 | 146.5764 | 26.0796 | 0.178 |
| 64 | 166.4644 | 24.1414 | 0.145 |

This independently confirms that the root component is faster for every
shape, including the noisy 64-segment subtraction. In the same run the
two- and 64-segment cumulative ratios remained within their ceilings at 0.166
and 0.340. The one-segment cumulative ratio varied to 0.378, above its 0.331
ceiling, despite its component remaining 1.96x faster. Because the lower-layer
advantage must hold across repeated runs rather than one favorable checkpoint,
segment-zero lookup remains an optimization target.

Evidence:
`benchmarks/results/2026-09-03-m52-root-isolated-g-drive-docker/`.

## Cached-primary checkpoint

Caching the immutable segment-zero descriptor in the coordinate context avoids
an enum/table lookup for the overwhelmingly common root segment. A longer
one-million-operation run reduced isolated root ratios to 0.285, 0.155, and
0.158. The 64-segment cumulative ratio passed at 0.328, but cumulative one- and
two-segment ratios still measured 0.364 and 0.183. The component and framing
ratios show that the remaining loss is small-message composition/codegen cost,
not pointer traversal itself. This checkpoint therefore does not close the
root step.

Evidence:
`benchmarks/results/2026-09-03-m52-root-final-cached-g-drive-docker/`.

## Final root result

The explicit `read_root_struct()` API now keeps the complete checked direct-root
path available for guaranteed cross-crate inlining. `StructReader` also offers
a direct checked word accessor, avoiding construction and reslicing of an
intermediate data-section view for generated scalar accessors. The general
coordinate reader and `DataSection` APIs remain available.

One million operations per sample produced:

| Case | Segments | C++ ns/message | Rust ns/message | Rust / C++ | Ceiling |
| --- | ---: | ---: | ---: | ---: | ---: |
| framing | 1 | 13.3078 | 4.0907 | 0.307 | — |
| root | 1 | 42.1029 | 9.3457 | 0.222 | 0.331 |
| framing | 2 | 35.5842 | 5.6812 | 0.160 | — |
| root | 2 | 161.5577 | 25.0177 | 0.155 | 0.172 |
| framing | 64 | 404.8284 | 150.2039 | 0.371 | — |
| root | 64 | 530.7771 | 167.3190 | 0.315 | 0.349 |

All cumulative gates pass with headroom. Paired incremental ratios are 0.182,
0.153, and 0.136. The independent isolated-root ratios are 0.112, 0.149, and
0.143, so the subtraction result is corroborated for every shape. This closes
the root-read substep; primitive-field breadth, borrowed blobs, and retained
reads remain open M52 work.

Evidence:
`benchmarks/results/2026-09-03-m52-root-final-inlined-g-drive-docker/`.

## Scalar-read baseline

The next layer reads nine scalar views from the root data word: bool, unsigned
8/16/32/64-bit values, signed 32-bit, float32, float64, and an enum-width
ordinal. Nonzero schema defaults exercise XOR semantics, and the empty
64-segment root exercises missing-field defaults. C++ uses the same checked
data-section bounds and `WireValue` loads. Checksums match for every shape.

| Segments | Cumulative C++ ns | Cumulative Rust ns | Rust / C++ | Scalar-only Rust / C++ |
| ---: | ---: | ---: | ---: | ---: |
| 1 | 49.1035 | 23.0350 | 0.469 | 4.865 |
| 2 | 176.9105 | 42.4760 | 0.240 | 2.112 |
| 64 | 520.6610 | 179.8548 | 0.345 | 2.119 |

The cumulative workloads are faster than C++, but this is not acceptable under
the inherited-efficiency rule: paired scalar-minus-root medians show that Rust
spends 13.2873–17.2624 ns while C++ spends 2.7312–8.1738 ns. The isolated
scalar workload remains faster only because it includes Rust's much faster
root opening. The gap is localized to `DataSection` scalar accessors; most are
public cross-crate methods without inline annotations and each independently
recomputes checked byte ranges.

The summarizer now uses medians of same-run paired differences rather than
subtracting independent medians. This prevents scheduler noise from producing
a false negative increment while retaining the raw and marginal medians.

Evidence:
`benchmarks/results/2026-09-03-m52-scalars-baseline-g-drive-docker/`.

## Audited scalar checkpoint

Inlining the checked `DataSection` accessors allows one bounds/byte path to be
optimized as a unit. A dedicated `scalar-only` case keeps the data slice opaque
on every iteration using symmetric Rust black-box and C++ compiler-memory
barriers, preventing either compiler from hoisting the nine reads.

| Segments | C++ scalar-only ns | Rust scalar-only ns | Rust / C++ | Cumulative Rust / C++ |
| ---: | ---: | ---: | ---: | ---: |
| 1 | 4.4145 | 3.5985 | 0.815 | 0.263 |
| 2 | 3.8275 | 3.5125 | 0.918 | 0.180 |
| 64 | 1.8954 | 1.5421 | 0.814 | 0.332 |

The scalar component itself now passes the 1.03 parity gate for every shape.
One- and 64-segment cumulative workloads preserve the inherited ceilings. The
two-segment cumulative ratio is 0.180 versus its 0.172 ceiling, so this remains
a checkpoint rather than the final scalar result. The scalar work is already
faster; the remaining headroom must come from the checked far-pointer root path
below it.

Evidence:
`benchmarks/results/2026-09-03-m52-scalars-final-audited-g-drive-docker/`.

## Final scalar result

Keeping the checked single- and double-far helpers in the same optimized unit
as root opening restored enough two-segment headroom without changing scalar
semantics.

| Segments | Cumulative C++ ns | Cumulative Rust ns | Rust / C++ | Ceiling | Scalar-only Rust / C++ |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 47.8719 | 11.7484 | 0.245 | 0.331 | 0.848 |
| 2 | 162.8375 | 25.4403 | 0.156 | 0.172 | 0.965 |
| 64 | 564.7714 | 170.2120 | 0.301 | 0.349 | 0.671 |

All cumulative and direct component gates pass. Same-run scalar-minus-root
subtraction remains noisy because the component is only a few nanoseconds;
the opaque `scalar-only` case is the acceptance measurement for that component.
Borrowed text/data and retained reads remain open.

Evidence:
`benchmarks/results/2026-09-03-m52-scalars-final-far-inline-g-drive-docker/`.

## Borrowed text/data baseline

The blob fixture gives the non-empty roots two pointers to the same eight-byte
byte list. Field zero is read as seven-byte Text after checking its trailing
NUL; field one is read as eight-byte Data. The empty 64-segment root exercises
schema-evolution defaults. Both implementations observe lengths and endpoint
bytes without copying payloads.

| Segments | Cumulative C++ ns | Cumulative Rust ns | Rust / C++ | Blob-only Rust / C++ |
| ---: | ---: | ---: | ---: | ---: |
| 1 | 60.0182 | 66.1424 | 1.102 | 3.172 |
| 2 | 175.1435 | 89.4111 | 0.511 | 3.340 |
| 64 | 539.3382 | 182.3098 | 0.338 | 7.242 |

This fails both the cumulative one-segment gate and the direct component gate.
Tracing localizes the fixed cost: `StructReader::read_text()` and `read_data()`
call `select_pointer()`, which validates the pointer to decide whether a schema
default applies, then call the blob reader, which validates and follows the
same pointer again. With no schema default, null already maps to the empty blob,
so that first validation is redundant. These public paths also lack the
cross-crate inline annotations needed by such small checked accessors.

Evidence:
`benchmarks/results/2026-09-03-m52-blobs-baseline-g-drive-docker/`.

## Final borrowed text/data result

The no-schema-default field path now avoids validating a pointer merely to
select that same pointer a second time. Direct byte-list pointers use a compact
success/slow-path discriminator: the success path decodes the pointer once,
checks its source and target ranges, verifies byte element size and nesting,
applies the exact traversal charge, and returns the borrowed slice. Nulls stay
empty. Far pointers, other pointer kinds, malformed targets, and failed limits
fall back to the general validator, which retains the detailed error behavior.
No native pointer is cached and no unsafe code is used.

The common successful accessors inline only this compact checked path. Rich
error construction and schema-default selection remain out of line, preventing
the general far/error machinery from expanding twice into each caller. The
benchmark also checks the encoded pointer count once before the two field reads,
matching C++ `AnyStruct::Reader::getPointerSection()` for the empty-root case.

| Segments | Cumulative C++ ns | Cumulative Rust ns | Rust / C++ | Ceiling | Blob-only Rust / C++ |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 63.1261 | 19.5834 | 0.310 | 0.331 | 0.696 |
| 2 | 202.8314 | 30.2811 | 0.149 | 0.172 | 0.779 |
| 64 | 570.5540 | 177.0350 | 0.310 | 0.349 | 0.650 |

Every cumulative and direct component gate passes. The independently isolated
parse-free blob workloads report Rust/C++ ratios of 0.318, 0.145, and 0.087,
corroborating the composition result. The regression suite also exercises the
slow-path single-far byte list and verifies its landing-pad-plus-target charge.
Retained/owned reopening remains the final open read category.

Evidence:
`benchmarks/results/2026-09-03-m52-blobs-final-direct-fast-g-drive-docker/`.
