# Bottom-up performance program

Performance work proceeds upward only after the current layer has a checked-in,
matched C++ comparison and reaches parity within measurement noise. Every layer
keeps raw samples, exact producer commits, toolchain/host metadata, semantic
checksums, and isolated phase or profile evidence.

1. **M50 — Wire primitives:** little-endian scalar loads/stores, checked byte
   ranges, wire words, and pointer bitfields.
2. **M51 — Framing:** segment-table parse/encode and standard stream adapters.
3. **M52 — Message reads:** pointer validation, traversal accounting, retained
   object references, primitive fields, and borrowed blobs.
4. **M53 — Message construction:** arena allocation, pointer emission, copying,
   and scratch reuse.
5. **M54 — Packing:** packed encode/decode and buffered streaming.
6. **M55 — Generated data APIs:** constant-offset typed readers/builders,
   lists, unions, defaults, and schema evolution.
7. **M56 — Schema/compiler/text/JSON:** reflection where required and native
   tooling throughput.
8. **M57 — RPC wire and actor:** control codecs, tables, driver scheduling,
   transport, and capability lifecycle.
9. **M58 — End-to-end performance gate:** rerun CarSales, CatRank, Eval, and
   Ping against the pinned C++ implementation and publish the cumulative result.

An isolated native speedup does not advance a layer unless the matched C++
scenario, correctness fixture, and relevant safety limits remain intact.

## Inherited performance floor

A completed lower layer's speedup is not a budget that a higher layer may
spend. For every workload shape, each milestone records the closest lower-layer
measurement and carries its native/C++ ratio forward as the cumulative ceiling
for the next layer (allowing only the stated measurement-noise tolerance). If a
framing path is 2x faster than C++, the corresponding framing-plus-message-read
path must remain at least 2x faster before the program advances.

Each higher-layer benchmark also reports the incremental cost after subtracting
the paired lower-layer time from both implementations. That incremental
native/C++ ratio must be no greater than 1.03. Both gates are required: the
cumulative ceiling prevents later work from consuming an existing advantage,
while the incremental comparison prevents a fast foundation from hiding a slow
new component.

When subtraction would amplify timer noise, the milestone must add an isolated
benchmark for the new component and cross-check it against the cumulative
scenario. Different safety or ownership semantics are reported separately and
never substituted silently for a matched comparison.
