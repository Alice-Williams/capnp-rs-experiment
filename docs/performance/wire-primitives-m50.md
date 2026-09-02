# M50 wire-primitives performance

## Scalar checkpoint

The scalar checkpoint compares native Rust with the pinned Cap'n Proto C++
`capnp::_::WireValue<uint64_t>` at commit
`e7c9cd96f1505b5ae486db7821006c2f5dce5b5b`. Each binary measures its primitive
loop with a monotonic clock after allocation and input generation. The runner
uses 4,096 words, 10,000 passes, two warmups, nine alternating-order recorded
samples, and checksum equality for every case.

| Case | C++ ns/op | Rust ns/op | Rust / C++ |
| --- | ---: | ---: | ---: |
| checked read | 0.5154 | 0.5210 | 1.011 |
| checked write | 1.4966 | 1.4966 | 1.000 |
| validate-once read | 0.5069 | 0.5014 | 0.989 |
| validate-once write | 1.5090 | 1.5124 | 1.002 |
| contiguous `Word` read | 0.4994 | 0.4974 | 0.996 |
| contiguous `Word` write | 1.4972 | 1.5114 | 1.010 |
| checked `Word` read | 0.5126 | 0.5265 | 1.027 |
| checked `Word` write | 1.4877 | 1.4843 | 0.998 |

All scalar cases meet M50's 1.03 parity threshold. The direct word array is the
closest representation-level match and is slightly faster on reads and within
1% on writes.

## Findings

- Small endian helpers needed explicit cross-crate inlining; otherwise optimized
  callers retained helper calls around the hot access path.
- `WordSlice` and `WordSliceMut` validate a complete byte region once and retain
  safe, unaligned byte-backed iteration without weakening hostile-input checks.
- A contiguous `Word` array lets LLVM emit a fully unrolled scalar loop and is
  the appropriate comparison with C++'s contiguous `WireValue<uint64_t>` array.
- The original runner measured around child-process launch. At ten million
  operations, Rust loader/startup cost looked like a 1.47x primitive gap even
  though longer whole-process trials converged. Timing inside both binaries
  removed that systematic error; increasing the measured window to 40.96
  million operations reduced remaining scheduler noise.

## Evidence

- Acceptance data:
  `benchmarks/results/2026-09-02-m50-wire-stable-timing-g-drive-docker/`
- First internal-timing run:
  `benchmarks/results/2026-09-02-m50-wire-internal-timing-g-drive-docker/`
- External-timing result that exposed startup bias:
  `benchmarks/results/2026-09-02-m50-wire-word-array-g-drive-docker/`
- Pre-optimization baseline and intermediate data remain under
  `benchmarks/results/2026-09-02-m50-wire-*-g-drive-docker/`.

M50 remains open for matched pointer-bitfield decode and encode cases.
