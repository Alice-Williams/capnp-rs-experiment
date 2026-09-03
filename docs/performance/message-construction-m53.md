# M53 message-construction performance

M53 is in progress. Its first checked-in baseline isolates prepared word writes
from fresh arena construction for a direct root and a forced single-far root.
It uses the pinned C++ commit
`e7c9cd96f1505b5ae486db7821006c2f5dce5b5b`, identical logical values, the
same used word counts, two warmups, nine alternating samples, and 100,000
messages per sample.

## Initial baseline

Evidence: `benchmarks/results/2026-09-03-m53-build-baseline-g-drive-docker`

| Case | Shape | C++ ns/message | Rust ns/message | Rust / C++ |
| --- | --- | ---: | ---: | ---: |
| prepared storage | direct `[3]` | 15.0794 | 19.5479 | 1.296 |
| fresh arena | direct `[3]` | 82.8520 | 109.1055 | 1.317 |
| prepared storage | far `[1,3]` | 21.1730 | 26.0695 | 1.231 |
| fresh arena | far `[1,3]` | 227.7699 | 218.3830 | 0.959 |

Paired subtraction attributes 65.9633 ns/message to C++ direct construction
and 88.5565 ns/message to native direct construction, a 1.343 ratio. For the
single-far shape it attributes 208.7911 ns/message to C++ and 196.3179 ns/message
to native, a 0.940 ratio.

These are diagnostic baselines, not passing M53 results. In particular, the
prepared cases currently include a small-buffer byte hash whose Rust/C++ code
generation differs; that observation overhead is a large fraction of the
measurement and must be normalized or isolated before using prepared ratios as
the inherited M50 gate. The fresh results still expose a clear direct-arena gap
and a distinct far-allocation result worth profiling independently.

## Traced paths

- Native fresh construction creates an `ExclusiveArena`, a segment-descriptor
  `Vec`, and one byte `Vec` per segment. Each object extends the used byte length
  and `emit_struct()` chooses a direct, single-far, or double-far pointer.
- Pinned C++ `MallocMessageBuilder` lazily creates its internal `BuilderArena`.
  It uses `calloc()` for owned segments. When an object does not fit, C++ asks
  the new segment for the object plus a one-word landing pad in one allocation.
- Native's forced single-far fixture first allocates the object in a new segment
  and then appends its landing pad. C++ places the landing pad before the object.
  The graphs and used segment sizes match, but their physical word order does
  not; semantic checksums are therefore compared across implementations while
  stable wire checksums are retained per implementation.

The next attribution step equalizes the Rust and C++ benchmark function
boundaries (the initial Rust prepared path had two extra non-inlined calls),
then records allocation counts/capacities before changing arena representation
or allocation policy.

## Word-level observation baseline

Evidence:
`benchmarks/results/2026-09-03-m53-build-word-observation-g-drive-docker`

The corrected harness gives each implementation one non-inlined iteration
boundary and uses the M50-style rotate/XOR word checksum rather than hashing
every byte. Longer 500,000-message samples produce:

| Case | Shape | C++ ns/message | Rust ns/message | Rust / C++ |
| --- | --- | ---: | ---: | ---: |
| prepared storage | direct `[3]` | 4.9418 | 5.6894 | 1.151 |
| fresh arena | direct `[3]` | 115.2537 | 200.4135 | 1.739 |
| prepared storage | far `[1,3]` | 6.6297 | 6.1793 | 0.932 |
| fresh arena | far `[1,3]` | 485.7785 | 464.5458 | 0.956 |

The direct prepared difference is only 0.7476 ns/message, so subtraction is
noise-sensitive, but the 85.1598 ns fresh-path gap is not. Disassembly shows
the benchmark calling native `ExclusiveArena::new`, `init_root_struct`, and
both scalar setters across crate boundaries, while the equivalent C++ wrapper
inlines its small header-defined operations around the builder library calls.
Native also allocates its segment-descriptor `Vec` separately from the segment
bytes.
The first implementation experiment therefore exports the small hot builder
chain for cross-crate inlining before changing storage representation.

## Cross-crate inlining result

Evidence: `benchmarks/results/2026-09-03-m53-build-inline-g-drive-docker`

Adding ordinary `#[inline]` hints to the small public construction chain and
its private checked helpers removes the indirect cross-crate calls without
changing the arena representation or any safety check:

| Case | Shape | C++ ns/message | Rust ns/message | Rust / C++ |
| --- | --- | ---: | ---: | ---: |
| fresh arena | direct `[3]` | 46.5355 | 45.1542 | 0.970 |
| fresh arena | far `[1,3]` | 161.0687 | 127.6302 | 0.792 |

Paired fresh-minus-prepared construction is 0.940 of C++ for the direct shape
and 0.787 for the single-far shape. This clears the 1.03 incremental gate for
the first two construction shapes without adding `unsafe` or changing storage.

The tiny prepared cases report 1.062 and 1.080 ratios, but the absolute gaps
are only 0.1738 and 0.2368 ns/message and the workload is dominated by a single
iteration call and timer sensitivity. M50's longer isolated word/pointer tests
already demonstrate parity. Before the final M53 gate, the prepared case will
batch enough word operations per observation to make its cumulative ratio
statistically meaningful rather than weakening the ceiling.

## Direct and far-pointer sublayer

Evidence: `benchmarks/results/2026-09-03-m53-build-double-far-g-drive-docker`

The benchmark now forces all three schema-independent pointer-placement paths.
For the double-far case, pinned C++ allocates an unattached two-word generated
orphan into a full segment and adopts it at the root; native uses an equivalent
two-word low-level struct allocation. Both produce the exact `[1,2,2]` word
segments and the same wire checksum.

| Shape | Prepared Rust / C++ | Fresh Rust / C++ | Incremental Rust / C++ |
| --- | ---: | ---: | ---: |
| direct `[3]` | 0.945 | 0.930 | 0.917 |
| single-far `[1,3]` | 0.929 | 0.705 | 0.712 |
| double-far `[1,2,2]` | 0.998 | 0.590 | 0.585 |

Every pointer-placement shape now preserves the M50 prepared-write ceiling and
clears the 1.03 fresh and incremental gates. The increasingly large advantage
for far placement is concrete rather than an optimizer artifact: both binaries
materialize and checksum every output word, and the double-far wire bytes match
exactly. C++'s general builder arena/orphan machinery carries more bookkeeping
and virtual allocation calls for these deliberately tiny forced segments;
native's exclusive deterministic arena has less machinery on the same graph.

## Safe scratch reuse

Evidence: `benchmarks/results/2026-09-03-m53-build-reuse-g-drive-docker`

`ExclusiveArena::reset()` now provides scoped scratch reuse. It requires
exclusive access, zeros every used byte before releasing extra segments,
retains the first segment's allocation, returns to the root-only state, and
assigns a new arena identity so copied offsets cannot address the next message.
The paired C++ case uses `MallocMessageBuilder`'s documented caller-provided
first-segment scratch constructor, whose destructor zeros the used words.

For the three-word direct message, median reuse cost is 46.1471 ns/message in
C++ and 28.1443 ns/message in native Rust, a 0.610 ratio. Both implementations
materialize and checksum the same wire words on every iteration, and both
zero-state guarantees are checked outside the timed region as well as by native
unit tests. Fresh direct construction remains independently measured at a
0.976 cumulative ratio and 0.990 paired incremental ratio in the same run, so
the reuse win does not substitute for or conceal fresh-allocation behavior.

The prepared cases again show how unstable sub-nanosecond denominator gaps are:
all three are slower by only 0.25–0.27 ns yet produce 1.07–1.09 ratios. The
earlier pointer-sublayer evidence remains the performance gate, while the final
runner will batch prepared operations before enforcing their numeric ceiling.

## Schema-independent graph-copy baseline

Evidence:
`benchmarks/results/2026-09-03-m53-build-copy-baseline-g-drive-docker`

The first graph-copy fixture is an exact one-segment, 11-word graph containing
a one-word root and a 64-byte data child. The prepared lower case copies and
checksums the same 88 bytes without arena allocation or pointer traversal. Both
implementations produce the same semantic and wire checksums.

| Case | C++ ns/message | Rust ns/message | Rust / C++ |
| --- | ---: | ---: | ---: |
| prepared 88-byte copy | 8.5592 | 7.0293 | 0.821 |
| validated graph copy | 94.4765 | 172.9730 | 1.831 |
| paired graph-copy increment | 87.3452 | 166.5819 | 1.907 |

This is a real native gap. Source tracing identifies two avoidable costs in the
success path: `copy_words_from()` allocates a temporary `Vec` for each copied
region even though safe callers cannot mutably alias the borrowed source, and
the iterative copier allocates its work-list `Vec` for this two-object graph.
The first graph-copy optimization removes the per-region byte copies while
retaining the existing rollback and hostile-input behavior.

## Schema-independent graph-copy result

Evidence:
`benchmarks/results/2026-09-03-m53-build-one-pass-zero-g-drive-docker`

The optimized path removes the temporary region buffer, keeps its first copy
task and rollback checkpoint inline, specializes direct root-struct and byte-list
validation, and retains the arena's first segment descriptor inline. Segment
storage now has a separate used-length cursor: reset zeros each used byte once,
while later safe growth can reuse that already-initialized storage without a
second fill. Fresh arenas initialize their complete first segment in one pass,
matching C++'s one-shot `calloc()` behavior. No optimization adds `unsafe`.

The paired scratch-copy case was essential attribution evidence. Before the
arena changes, reusable native graph copy was already close to or faster than
C++, while fresh native copy retained a large gap. That isolated the remaining
cost in arena allocation and initialization rather than wire copying. The final
paired run reports:

| Case | C++ ns/message | Rust ns/message | Rust / C++ |
| --- | ---: | ---: | ---: |
| prepared 88-byte copy | 17.9570 | 14.1403 | 0.787 |
| reusable validated graph copy | 240.0116 | 234.1260 | 0.975 |
| fresh validated graph copy | 265.7662 | 264.3411 | 0.995 |
| paired fresh-copy increment | 244.1322 | 250.3023 | 1.025 |

All copy-specific cumulative and incremental ratios clear the 1.03 M53 ceiling.
Both implementations materialize the same 11 wire words and report identical
semantic and wire checksums. The high absolute times in this run reflect shared
host load, but the runner alternates implementation order within every sample;
the gate uses paired medians and is supported by interleaved CPU-pinned
before/after measurements. Those measurements attribute about 9% to the compact
root-validation fast path, 8–17% (depending on message size) to one-pass segment
initialization, and about 13% on reused graph copy to avoiding a second zero fill.

The inline first descriptor also reduces one-segment arena memory overhead by
one heap allocation and one allocation header. Multi-segment arenas still use a
growable `Vec` for additional descriptors, so their asymptotic descriptor memory
is unchanged. Retaining initialized bytes after reset does not increase allocated
capacity—the old implementation already retained that capacity—but it keeps the
safe `Vec` length at its high-water mark internally while exposing only the
used prefix through every public segment view and ownership conversion.

## Public scalar and Data construction

Evidence: `benchmarks/results/2026-09-03-m53-build-data-inline-g-drive-docker`

The final user-facing construction shape builds an 11-word one-segment message
through public APIs: one root `UInt64` plus one 64-byte `Data` field. Pinned C++
uses generated `BuildGraph` setters; native uses `init_root_struct()`, `set_u64()`,
and `set_data()`. The prepared lower case emits and observes the identical wire
words without an arena. Semantic and wire checksums match across implementations.

The unmodified public-path baseline measured native fresh construction at 1.079
of C++ and its paired incremental cost at 1.153. Optimized Rust still called the
small `set_data()` chain across its crate boundary, whereas generated C++ inlined
its header-defined setter before entering the runtime. Adding ordinary inline
exports to native byte-list allocation, pointer-slot validation, pointer emission,
and `set_data()`/`set_text()` removes those calls without removing a bounds,
overflow, allocation, or pointer check.

| Case | C++ ns/message | Rust ns/message | Rust / C++ |
| --- | ---: | ---: | ---: |
| prepared scalar + Data words | 11.6239 | 9.3700 | 0.806 |
| fresh public scalar + Data build | 88.1349 | 64.9792 | 0.737 |
| paired construction increment | 72.8052 | 58.6438 | 0.805 |

This level therefore preserves the faster lower layers rather than spending
their margin: the complete public Data build is 26.3% faster and the incremental
builder work is 19.5% faster than pinned C++. In the same run, fresh graph copy
is 0.904, reusable graph copy is 0.810, and its paired increment is 0.911. The
separate pointer-placement gate remains authoritative for the sub-nanosecond
prepared direct case, whose ratio is timer-sensitive in mixed runs; its complete
fresh and incremental construction results remain comfortably faster here.
