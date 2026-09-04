# M56 schema, reflection, compiler, text, and JSON performance

## Dynamic-reflection baseline

Evidence:
[`benchmarks/results/2026-09-04-m56-reflection-baseline-corrected-5m`](../../benchmarks/results/2026-09-04-m56-reflection-baseline-corrected-5m)
at native commit `da1bae8cc0c1b0dcfa30673ce2a983703eaeadee`.
The run uses the pinned `WireFixture` compiler request and unpacked message,
five million operations per sample, two warmups, and nine alternating samples.
Each implementation cycles through the same four unsigned scalar fields and
produces identical semantic checksums.

| Operation | C++ ns/op | Native ns/op | Native / C++ |
| --- | ---: | ---: | ---: |
| schema field by index | 6.4954 | 1.7056 | 0.263 |
| schema field by name | 107.3891 | 11.3447 | 0.106 |
| dynamic scalar by index | 51.7943 | 57.8904 | 1.118 |
| dynamic scalar by name | 151.8729 | 71.0583 | 0.468 |

The native owned schema model is already materially faster for both direct
field access and name lookup. Name lookup adds about 9.6 ns over indexed schema
access in Rust and 100.9 ns in C++. At the complete dynamic-read level, Rust is
53.2% faster by name. The name-based reflection gate therefore passes and its
incremental lookup cost is substantially below C++.

The indexed dynamic case is the open floor: Rust is 11.8% slower. Both sides
perform an indexed field-list access before dynamic dispatch in this corrected
comparison. The current Rust path then repeats the containing-node lookup,
resolves the field type into an owned value, reconstructs a retained checked
reader, obtains its data section, and creates `DynamicValue`; the pinned C++
field descriptor carries its containing schema and its dynamic reader retains a
direct internal struct reader. The next isolation adds a safe cached native
field descriptor and separately measures descriptor dispatch, type resolution,
and retained-reader reconstruction. No runtime implementation was changed
before this baseline.
