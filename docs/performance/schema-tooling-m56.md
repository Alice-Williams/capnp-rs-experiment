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

## Prepared scalar-reflection gate

Final scalar evidence:
[`benchmarks/results/2026-09-04-m56-reflection-prepared-5m`](../../benchmarks/results/2026-09-04-m56-reflection-prepared-5m)
at native commit `d8aa68307c6886155244fd2855326976d71e81ac`.

| Operation | C++ ns/op | Native ns/op | Native / C++ |
| --- | ---: | ---: | ---: |
| schema field by index | 6.6251 | 1.5383 | 0.232 |
| schema field by name | 107.0160 | 12.1217 | 0.113 |
| dynamic scalar by cached field | 44.6208 | 27.4706 | 0.616 |
| dynamic scalar by index | 51.5813 | 33.3380 | 0.646 |
| dynamic scalar by name | 149.5544 | 45.5933 | 0.305 |

Rust now exposes a lifetime-bound `DynamicField` that retains references to the
resolved struct and field metadata. Dispatch checks both the originating
schema allocation and type ID, so a descriptor from another schema version
cannot apply an offset to this value. This mirrors C++'s containing-schema
check without raw pointers or unsafe code.

The other material cost was repeated retained-root reconstruction. A dynamic
struct now replaces its checked `ObjectRef` with a `PreparedStructRef`: root
pointer resolution and traversal charging happen once, while every field read
reuses validated relocatable coordinates. Pointer-valued child reads continue
to validate and charge the child normally. The prepared reference retains one
`Arc` and wire coordinates; it neither copies message data nor sits alongside a
duplicate retained root. Generated retained readers use the same single
prepared representation, so this does not add a second cache to M55's reader.

The isolated cached-field path is 38.4% faster than C++. The like-for-like
indexed path improves from 1.118x to 0.646x, and name-based dynamic access
improves from 0.468x to 0.305x. After subtracting schema access, indexed dynamic
dispatch is about 31.80 ns in Rust versus 44.96 ns in C++ (0.707x); name-based
dispatch is about 33.47 ns versus 42.54 ns (0.787x). Both the cumulative and
incremental scalar-reflection gates are closed. Blob, list, nested struct,
union, evolution, and builder reflection remain separate gates.

## Dynamic blob-ownership baseline

Evidence:
[`benchmarks/results/2026-09-04-m56-reflection-blobs-baseline-5m`](../../benchmarks/results/2026-09-04-m56-reflection-blobs-baseline-5m)
at native commit `beaa376e980381967ee4cfc6d1c6e526c4f784bf`.
The two blob cases read the same Text and Data fields through cached field
descriptors. The borrowed C++ case consumes its normal zero-copy views; the
owned C++ control explicitly allocates and copies both payloads so that it
matches the ownership contract of Rust's current `DynamicValue::Text(String)`
and `DynamicValue::Data(Vec<u8>)`. Checksums are identical across both
implementations and ownership modes.

| Operation | C++ ns/op | Native ns/op | Native / C++ |
| --- | ---: | ---: | ---: |
| dynamic Text + Data, borrowed C++ view | 104.3371 | 167.8527 | 1.609 |
| dynamic Text + Data, owned copies | 143.7810 | 162.9723 | 1.133 |

The native owned path is 13.3% slower than the matched owned-copy control, but
60.9% slower than C++'s natural reflection API. The 39.4 ns difference between
the two C++ cases also demonstrates that allocation and payload copying are a
material part of the apparent language gap. Scalar reflection remains faster
in this same run, including the cached-field path at 0.577x C++, so the open
work is specifically blob representation and conversion rather than descriptor
dispatch. The next gate adds a lifetime-bound, zero-copy dynamic blob view while
retaining the owned API for callers that need values to escape the reader.
