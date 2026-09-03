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
