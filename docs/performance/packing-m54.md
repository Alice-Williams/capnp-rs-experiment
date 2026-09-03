# M54 packed-codec performance

M54 compares schema-independent packed encoding and decoding with Cap'n Proto
C++ pinned at `e7c9cd96f1505b5ae486db7821006c2f5dce5b5b`. Each transform has a paired
fresh byte-copy floor. Results are nanoseconds per eight-byte input word and use
two warmups followed by nine alternating C++/native samples.

## Unmodified baseline

Evidence:
[`benchmarks/results/2026-09-03-m54-packing-baseline-g-drive-docker`](../../benchmarks/results/2026-09-03-m54-packing-baseline-g-drive-docker)
at native commit `05e463dfc93c25dd6fb9bfa3718000f4184d57ee`.

| Operation | Shape | C++ ns/word | Native ns/word | Native / C++ |
| --- | --- | ---: | ---: | ---: |
| pack | zero | 0.8550 | 6.2282 | 7.284 |
| pack | raw | 1.3583 | 8.7968 | 6.476 |
| pack | mixed | 6.2738 | 16.1760 | 2.578 |
| pack | realistic | 5.5284 | 14.6064 | 2.642 |
| unpack | zero | 0.2624 | 0.1899 | 0.724 |
| unpack | raw | 0.4189 | 0.2893 | 0.690 |
| unpack | mixed | 5.5412 | 10.8145 | 1.952 |
| unpack | realistic | 5.0108 | 9.1430 | 1.825 |

The distribution-isolated cases and source trace make the first bottlenecks
unambiguous:

- One-shot native `pack()` passes every aligned word through the arbitrary-byte
  chunk state machine. Zero runs therefore perform per-word array copies, byte
  scans, enum dispatch, and repeated projected-limit checks, while C++ scans
  consecutive zero words as native 64-bit values.
- A native `0xff` run allocates and grows an independent `Vec`, copies every raw
  word into it, then copies the completed run into the result. C++ scans the run
  in place and bulk-copies it directly to the output stream. A 4,096-word raw
  input creates sixteen maximum-length temporary native run buffers.
- Ordinary native tags use iterator folds and filters plus the streaming state
  transition for every word. The C++ fast path writes the tag and eight
  conditionally advanced bytes in an unrolled loop.
- Native unpacking already wins the bulk zero and raw cases. The remaining
  mixed/realistic gap is in ordinary-tag reconstruction: the native decoder
  resumes a general incremental state machine for each payload byte, whereas
  C++ uses an unrolled contiguous-input fast path whenever ten input bytes are
  available.

The paired copy ratios range from 0.828 to 1.299. M54 therefore evaluates both
the cumulative ceiling inherited from each exact copy case and the isolated
incremental codec ratio; no aggregate average hides an individual shape.

## Prepared-streaming candidate

Evidence:
[`benchmarks/results/2026-09-03-m54-packing-prepared-decode-g-drive-docker`](../../benchmarks/results/2026-09-03-m54-packing-prepared-decode-g-drive-docker)
at native commit `9ea8992fb64173a98dfe5c2ffe86d7da1f4a54d0`.

| Operation | Shape | C++ ns/word | Native ns/word | Native / C++ | Incremental native / C++ |
| --- | --- | ---: | ---: | ---: | ---: |
| pack | zero | 0.5457 | 0.5681 | 1.041 | 0.958 |
| pack | raw | 1.3659 | 1.0281 | 0.753 | 0.729 |
| pack | mixed | 7.1754 | 5.4070 | 0.754 | 0.750 |
| pack | realistic | 5.7485 | 4.4412 | 0.773 | 0.762 |
| unpack | zero | 0.2763 | 0.2471 | 0.894 | 0.901 |
| unpack | raw | 0.4156 | 0.2294 | 0.552 | below timer resolution |
| unpack | mixed | 6.0466 | 5.0816 | 0.840 | 0.838 |
| unpack | realistic | 6.0041 | 4.8355 | 0.805 | 0.805 |
| pack stream | zero | 0.6443 | 0.4256 | 0.661 | 0.451 |
| pack stream | raw | 1.3053 | 0.6515 | 0.499 | 0.428 |
| pack stream | mixed | 7.4080 | 6.1163 | 0.826 | 0.824 |
| pack stream | realistic | 6.4155 | 5.1559 | 0.804 | 0.795 |
| unpack stream | zero | 0.3881 | 0.1765 | 0.455 | 0.450 |
| unpack stream | raw | 0.6736 | 0.4224 | 0.627 | 0.400 |
| unpack stream | mixed | 7.4864 | 6.3337 | 0.846 | 0.844 |
| unpack stream | realistic | 6.6857 | 5.8872 | 0.881 | 0.881 |

The one-shot zero pack ratio is above 1.0 but preserves its paired lower-layer
ceiling (`1.041 <= 1.214 * 1.03`) and its isolated incremental transform is
faster than C++. All other cumulative and resolvable incremental ratios are
below 1.0. Raw one-shot unpack is so close to its allocation/copy floor that
the native subtraction is negative; its complete isolated codec ratio of
0.552 corroborates that this is timer resolution, not hidden slow work.

The final implementation:

- bypasses the arbitrary-chunk state machine for complete one-shot input;
- scans zero words at 64-bit granularity without `unsafe` code;
- writes raw runs directly instead of allocating and recopying a temporary
  run `Vec`;
- derives all eight nonzero-byte tag bits in parallel with safe integer SWAR;
- walks only populated payload lanes for ordinary tags;
- directly handles aligned streaming chunks that end at a canonical run or
  ordinary-word boundary;
- decodes complete streamed items without materializing resumable enum state;
- permits a caller with a trusted expected size to pre-size decoder capacity,
  while retaining the conservative default constructor and all output limits.

The streaming comparison uses exact canonical bytes and deliberately
misaligned 1,025-byte decode feeds. C++'s pull-oriented stream may bypass its
input buffer when it requests the remainder of a raw run; Rust's push-oriented
decoder cannot make that same call. Prepared output capacity is matched, and
the byte-at-a-time correctness tests remain permanent, but pathological tiny
external call overhead is not presented as codec throughput.

This run is retained as optimization evidence rather than the final gate. Its
32-byte zero-stream copy floor measured at a native/C++ ratio of 0.691, while
complete zero-stream unpack measured 0.894. The transform itself is faster
(`0.901` incremental), but the cumulative result does not preserve that
sub-microbenchmark's unusually low ratio. A longer final run must resolve the
short-output allocation noise or the zero decoder must improve further.

That candidate also exposed a lower-case modeling error: unpack had been paired
with copying its compressed input rather than materializing its unpacked
output. The final summarizer corrects unpack and unpack-stream to use the exact
unpacked-output copy floor. The candidate files remain immutable evidence of
the earlier analysis and are not used by the final ratio gate.

## Final qualified comparison

Evidence:
[`benchmarks/results/2026-09-03-m54-packing-final-corrected-floor-g-drive-docker`](../../benchmarks/results/2026-09-03-m54-packing-final-corrected-floor-g-drive-docker)
at native commit `48aeaf6b116c97f77d08234a80e01c4188cc6656`.
This longer run uses 5,000 passes per sample and the corrected unpacked-output
lower case.

| Operation | Shape | C++ ns/word | Native ns/word | Native / C++ | Incremental native / C++ |
| --- | --- | ---: | ---: | ---: | ---: |
| pack | zero | 0.4409 | 0.3918 | 0.889 | 0.744 |
| pack | raw | 1.2812 | 1.0369 | 0.809 | 0.794 |
| pack | mixed | 5.9392 | 5.0336 | 0.848 | 0.843 |
| pack | realistic | 5.2123 | 4.7581 | 0.913 | 0.910 |
| unpack | zero | 0.2940 | 0.2051 | 0.697 | 0.092 |
| unpack | raw | 0.3664 | 0.2471 | 0.674 | 0.430 |
| unpack | mixed | 5.5018 | 4.5073 | 0.819 | 0.814 |
| unpack | realistic | 5.1291 | 3.8799 | 0.756 | 0.748 |
| pack stream | zero | 0.4946 | 0.3050 | 0.617 | 0.347 |
| pack stream | raw | 1.2214 | 0.6918 | 0.566 | 0.505 |
| pack stream | mixed | 7.0128 | 6.0610 | 0.864 | 0.861 |
| pack stream | realistic | 5.2647 | 4.7184 | 0.896 | 0.893 |
| unpack stream | zero | 0.2925 | 0.1752 | 0.599 | below timer resolution |
| unpack stream | raw | 0.5064 | 0.2449 | 0.484 | 0.231 |
| unpack stream | mixed | 5.9182 | 5.7690 | 0.975 | 0.974 |
| unpack stream | realistic | 5.2225 | 5.0275 | 0.963 | 0.961 |

All sixteen complete-operation ratios are below 1.0. Every cumulative ratio is
within its paired lower-layer ceiling and every resolvable incremental ratio is
below 1.0. Zero streaming unpack again falls below its measured unpacked-copy
floor on the native side; its 0.599 isolated complete-operation ratio
corroborates the below-resolution subtraction.

## Qualification

The final implementation and checked-in benchmark evidence passed:

- pinned upstream and schema-file verification;
- all locked workspace targets and documentation tests on the current stable
  toolchain, plus formatting and Clippy with warnings denied;
- the Level-1 RPC fuzz and soak gates, the full-platform soak gate, and all
  three targeted Loom concurrency models;
- all locked workspace targets on the Rust 1.85 minimum supported toolchain;
- all 71 Bazel tests, including the new packing baseline, final-results, and
  runner-syntax evidence gates;
- all 17 `capnp-wire` tests under Miri and the disjoint primitive-partition
  aliasing test in `capnp-message` under Miri.

The packing implementation remains entirely safe Rust. Nightly Miri emitted a
deprecation warning for `AtomicU64::fetch_update` in the pre-existing traversal
budget code; it did not affect the run and is outside M54's packing scope.
