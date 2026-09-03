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
