# M51 standard-framing performance

## Final comparison

The final run compares native Rust with Cap'n Proto C++ commit
`e7c9cd96f1505b5ae486db7821006c2f5dce5b5b`. Both binaries receive identical
deterministic one-, two-, and 64-segment frames, execute 500,000 operations per
sample, and record two warmups followed by nine alternating-order samples.
Every Rust/C++ pair produces the same checksum.

The acceptance paths compare equivalent reusable or already-validated inputs:

| Operation | Segments | C++ ns/frame | Rust ns/frame | Rust / C++ |
| --- | ---: | ---: | ---: | ---: |
| public flat parse | 1 | 13.9843 | 4.4824 | 0.321 |
| public flat parse | 2 | 36.8564 | 6.1593 | 0.167 |
| public flat parse | 64 | 423.5603 | 143.6055 | 0.339 |
| prepared encode | 1 | 21.7668 | 20.1142 | 0.924 |
| prepared encode | 2 | 27.5904 | 22.1437 | 0.803 |
| prepared encode | 64 | 220.2535 | 196.3506 | 0.891 |
| reusable stream read | 1 | 19.5419 | 11.3677 | 0.582 |
| reusable stream read | 2 | 21.4035 | 14.3446 | 0.670 |
| reusable stream read | 64 | 64.4599 | 53.5124 | 0.830 |
| prepared stream write | 1 | 38.6144 | 29.4936 | 0.764 |
| prepared stream write | 2 | 46.4119 | 32.6140 | 0.703 |
| prepared stream write | 64 | 534.2647 | 209.9836 | 0.393 |

All acceptance paths beat the 1.03 parity ceiling. M52 must carry these
per-shape ratios forward as cumulative ceilings; the speedups are not a budget
that message traversal may consume.

## Why the public multi-segment parse is much faster

The large result is real, but it is principally an API and allocation result,
not evidence that Rust decodes each table entry several times faster.

Pinned C++ `FlatArrayMessageReader::init()` allocates its `moreSegments` array
whenever the message contains more than one segment. Native Rust
`parse_frame_into()` instead writes two-word slice descriptors into a
caller-provided array and borrows the original frame. Compacting `Segment` to
exactly one slice removed redundant IDs and stored word counts. The parser also
specializes one- and two-segment tables and scans larger tables as contiguous
four-byte chunks.

The allocation explanation is directly testable. A source-derived C++ parser
that writes descriptors into caller storage reduces the C++ medians to 3.7248,
5.4706, and 131.0637 ns. Against that diagnostic kernel Rust takes 5.0074,
6.2491, and 149.6342 ns (1.344x, 1.142x, and 1.142x). Therefore the report does
not claim that Rust's raw descriptor loop is faster: the public win comes from
avoiding the C++ public reader's allocation and representation overhead.

This diagnostic is not substituted for the public comparison. It is a labeled
source-derived shim rather than a C++ API, accepts aligned `word` storage,
throws instead of returning Rust's exact bounded error surface, and does not
return clean EOF or a trailing-frame remainder. It remains checked in so later
work cannot accidentally describe the allocation win as a primitive-parser
win.

Caller storage owns only the descriptors, not the message payload. On this
64-bit target each descriptor is a 16-byte pointer/length slice: 64 slots occupy
1,024 bytes and the protocol maximum of 512 occupies 8,192 bytes. Reusing heap
or arena scratch avoids both allocation churn and stack pressure. For callers
that prefer simpler lifetimes, the allocating `parse_frame()` API remains
available and sizes its descriptor allocation to the message. C++'s ownership
model is therefore more convenient by default; Rust exposes both that tradeoff
and the predictable reusable fast path.

## Optimization audit

The benchmark makes each descriptor observable before the checksum loop: Rust
uses `black_box()` on the returned segment slice and C++ uses a compiler memory
barrier on its caller-storage array. The checksum then consumes every segment's
index, word count, first byte, and last byte. Stream cases verify output length,
header, first byte, and last byte, and the complete read/write occurs across the
real library boundary. Distinct binary hashes and all raw samples are recorded.

The no-allocation diagnostic reversing the apparent parse advantage is the
strongest guard against dead-code elimination: when the C++ allocation is
removed, the multi-segment gap collapses from a 2.95x Rust win to a 1.14x Rust
loss. Compiler optimization alone cannot explain that change.

## Stronger convenience paths

The remaining measured modes intentionally do more work on the Rust side and
are retained as diagnostics:

| Operation | Segments | Rust / C++ | Extra Rust contract |
| --- | ---: | ---: | --- |
| checked byte encode | 1 / 2 / 64 | 0.945 / 0.924 / 1.148 | validates byte alignment, u32 word counts, aggregate limits |
| allocating raw read | 1 / 2 / 64 | 1.022 / 1.097 / 0.939 | safely initializes new `Vec` storage; KJ allocates trivial bytes uninitialized |
| checked stream write | 1 / 2 / 64 | 0.891 / 0.787 / 0.497 | repeats byte-slice validation absent from typed C++ word views |

`PreparedSegments` is the appropriate counterpart to C++'s typed
`ArrayPtr<const word>` inputs: it validates arbitrary Rust byte slices once and
keeps immutable borrowed descriptors for repeated encoding. Likewise,
`read_frame_reusing()` is the safe high-throughput counterpart to a reusable C++
scratch allocation. The checked convenience APIs remain available rather than
weakening hostile-input validation to improve a non-equivalent number.

## Evidence and validation

- Acceptance data:
  `benchmarks/results/2026-09-03-m51-framing-final-audited-g-drive-docker/`
- Pinned source inspected:
  `c++/src/capnp/serialize.c++` and `c++/src/kj/array.h` at the oracle commit.
- Earlier baseline and optimization checkpoints remain under
  `benchmarks/results/2026-09-02-m51-*` and
  `benchmarks/results/2026-09-03-m51-{batched-table,prepared,stream,prepared-stream}-g-drive-docker/`.

The final gate covers workspace tests and doctests, formatting, strict Clippy,
Rust 1.85, Bazel, and focused Miri framing and synchronous-I/O tests.
