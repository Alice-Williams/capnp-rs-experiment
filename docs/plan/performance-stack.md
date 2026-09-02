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
