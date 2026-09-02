# M51 standard-framing performance

## Flat byte-slice checkpoint

The benchmark uses the pinned C++ `FlatArrayMessageReader` and
`messageToFlatArray` APIs at commit
`e7c9cd96f1505b5ae486db7821006c2f5dce5b5b`. Rust and C++ receive identical
deterministic one-, two-, and 64-segment fixtures. Allocation and fixture
construction occur outside parse timing; each encode operation includes its
returned output allocation and copy. Every case uses 50,000 operations, two
warmups, nine recorded samples, internal monotonic clocks, and equal semantic
checksums.

| Operation | Segments | C++ ns/frame | Rust ns/frame | Rust / C++ |
| --- | ---: | ---: | ---: | ---: |
| parse into caller storage | 1 | 20.5905 | 8.3522 | 0.406 |
| parse into caller storage | 2 | 33.8929 | 11.1023 | 0.328 |
| parse into caller storage | 64 | 315.8420 | 112.3729 | 0.356 |
| encode prepared segments | 1 | 23.2646 | 19.3525 | 0.832 |
| encode prepared segments | 2 | 24.9566 | 21.2765 | 0.853 |
| encode prepared segments | 64 | 284.7472 | 290.8354 | 1.021 |

Rust parsing is 2.5–3.0x faster. The semantically matched prepared-segment
encoder is faster for small tables and within 2.1% for 64 segments.

## Why prepared segments matter

C++ accepts `ArrayPtr<const word>` segment descriptors, so byte alignment and
32-bit word counts are established by its input types before encoding. Rust's
original convenience API accepts arbitrary byte slices and must recheck
alignment, per-segment word counts, aggregate size, and configured limits on
every call. `PreparedSegments` performs those checks once and retains immutable
borrowed descriptors for repeated encoding; `encode_prepared_frame()` then
matches the typed C++ work without `unsafe`.

The ordinary checked byte API remains intact. Removing its redundant temporary
size vector made one- and two-segment encoding faster than C++; safe batched
table emission brought 64-segment checked encoding to 1.030x C++. A later mixed
run showed host variance in the one-segment checked case, so it is reported as
an intentionally stronger semantic path rather than substituted for the typed
comparison.

Evidence:

- Baseline: `benchmarks/results/2026-09-02-m51-framing-baseline-g-drive-docker/`
- Allocation-free checkpoint:
  `benchmarks/results/2026-09-02-m51-framing-no-size-vec-g-drive-docker/`
- Batched-table checkpoint:
  `benchmarks/results/2026-09-03-m51-framing-batched-table-g-drive-docker/`
- Prepared-segment checkpoint:
  `benchmarks/results/2026-09-03-m51-framing-prepared-g-drive-docker/`

M51 remains open for matched synchronous in-memory stream adapters and the
final full-workspace validation gate.
