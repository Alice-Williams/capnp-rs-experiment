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
