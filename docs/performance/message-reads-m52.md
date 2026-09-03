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
The nearly identical 41 ns Rust increment for one and two segments points to a
fixed cost before pointer shape matters: `MessageSegments::new()` copies the
already-parsed segment views into a new boxed slice. C++'s reader already owns
its message context after framing, so it does not add a second descriptor
allocation at this boundary. The 64-segment Rust increment adds the descriptor
copy and the scan/far-pointer work.

The first optimization target is therefore a safe reusable or borrowed
message context over already-validated segment descriptors. It must preserve
the existing allocating convenience API, exact bounds and traversal charging,
and the workspace's no-unsafe policy.

Evidence:
`benchmarks/results/2026-09-03-m52-root-baseline-g-drive-docker/`.
