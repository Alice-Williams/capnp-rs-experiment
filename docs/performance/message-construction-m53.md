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
