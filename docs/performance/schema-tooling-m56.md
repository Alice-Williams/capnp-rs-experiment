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

## Zero-copy dynamic blob gate

Final evidence:
[`benchmarks/results/2026-09-04-m56-reflection-blobs-final-5m`](../../benchmarks/results/2026-09-04-m56-reflection-blobs-final-5m)
at native commit `a73e87cb736a9cb2b2ec86d4cbcce3e15f9a57f3`.

| Operation | C++ ns/op | Native ns/op | Native / C++ |
| --- | ---: | ---: | ---: |
| dynamic Text + Data, borrowed views | 98.6367 | 40.6437 | 0.412 |
| dynamic Text + Data, owned copies | 140.6952 | 82.6401 | 0.587 |
| dynamic scalar by cached field | 47.2791 | 25.9077 | 0.548 |

`DynamicTextField` and `DynamicDataField` resolve names, runtime types, pointer
offsets, schema defaults, and union metadata once. Every use still checks the
originating schema allocation and type ID, and active union fields still check
their discriminant. A foreign typed descriptor is rejected. Empty schema blob
defaults share the direct null-as-empty wire path; non-empty defaults retain
their exact fallback behavior.

`DynamicStruct::with_view` lends a callback-scoped `DynamicStructView`. It
borrows the retained message context once for a batch, and its higher-ranked
callback API prevents Text or Data slices from escaping. This makes the natural
API zero-copy without adding self-references, leaked backing, or `unsafe` code.
The existing owned `DynamicValue` API remains available and now uses the same
direct wire read before allocating its result. Shared traversal accounting is
unchanged; only its tiny trait methods and the reflection dispatch chain are
made visible to the optimizer across crate boundaries.

The natural borrowed path is 58.8% below C++ and also improves on the scalar
reflection ratio in the same run (0.412x versus 0.548x), so the cumulative blob
gate preserves the lower layer's advantage. Ownership adds 41.9964 ns in Rust
and 42.0585 ns in C++, an incremental ratio of 0.999. That like-for-like control
therefore closes the allocation/copy gate as well. Relative to the recorded
baseline, native borrowed access falls from 167.8527 to 40.6437 ns and owned
access from 162.9723 to 82.6401 ns. List and nested-struct reflection are the
next open shapes.

## Nested reflection baseline

Evidence:
[`benchmarks/results/2026-09-04-m56-reflection-nested-baseline-5m`](../../benchmarks/results/2026-09-04-m56-reflection-nested-baseline-5m)
at native commit `072b4d8dda402ba49c4d21e6e472bdaac9f7a5ce`.
Each case starts from the same cached root field descriptor and reads the same
fixture value. Nested-struct cases also cache the `Node.value` descriptor before
timing. Checksums agree for every implementation and shape.

| Operation | C++ ns/op | Native ns/op | Native / C++ |
| --- | ---: | ---: | ---: |
| dynamic primitive-list element | 89.5004 | 130.0117 | 1.453 |
| dynamic nested-struct scalar | 106.5742 | 131.7143 | 1.236 |
| dynamic struct-list scalar | 150.4634 | 233.5283 | 1.552 |
| dynamic nested-list scalar | 144.3452 | 258.6036 | 1.792 |

All four gates are open. The current root pointer read materializes an owned
`DynamicList` or `DynamicStruct`, cloning retained message/schema handles and
resolved type state. `DynamicList::get` then opens the retained list once to
check its length and again to read the element. Struct-list and nested-list
cases repeat that process at the second hop. C++ keeps short-lived dynamic
readers and list schemas by value, so no equivalent ownership reconstruction is
required between these immediately consumed operations. The next isolation
extends the callback-scoped dynamic view to typed list and struct access plans,
while retaining the owned dynamic values for callers that need them to escape.

## Callback-scoped nested reflection gate

Final evidence:
[`benchmarks/results/2026-09-04-m56-reflection-nested-final-5m`](../../benchmarks/results/2026-09-04-m56-reflection-nested-final-5m)
at native commit `ee96ce4399b5ce7308bc58ba7736c43d7f6d128f`.

| Operation | C++ ns/op | Native ns/op | Native / C++ |
| --- | ---: | ---: | ---: |
| dynamic primitive-list element | 90.1311 | 29.3392 | 0.326 |
| dynamic nested-struct scalar | 100.1701 | 28.9179 | 0.289 |
| dynamic struct-list scalar | 143.0482 | 46.1999 | 0.323 |
| dynamic nested-list scalar | 136.4062 | 43.8162 | 0.321 |

`DynamicListField` and `DynamicStructField` resolve the field's runtime type,
pointer offset, union metadata, and child brand once. Their callback-scoped
views keep the original checked message and shared traversal budget borrowed
across every nested hop. Primitive lists read directly from the open list;
struct-list elements borrow their inline-composite reader; pointer-list
elements borrow the nested list reader. Cached scalar fields read from the
already-open child data section. Bounds, wire-kind, schema-allocation,
containing-type, active-union, nesting, traversal, and schema-evolution checks
remain on those paths. The access plans currently require the normal null
pointer default; callers needing a non-null aggregate schema default retain the
existing owned dynamic path.

This removes the baseline's repeated `Arc`, runtime `Type`/`Brand`, and retained
coordinate construction without changing the observed work or using `unsafe`.
All Rust/C++ checksums agree. The four native medians fall by 77.4%, 78.0%,
80.2%, and 83.1% respectively from the recorded baseline, and each cumulative
ratio is better than the 0.413 borrowed-blob control in this same evidence run.
The primitive-list, nested-struct, struct-list, and nested-list reflection gates
are therefore closed. Active/unknown unions, defaults/evolution, enums, and
builder reflection remain the next reflection shapes.

## Enum, default, and active-union reflection baseline

Evidence:
[`benchmarks/results/2026-09-04-m56-reflection-enum-union-baseline-5m`](../../benchmarks/results/2026-09-04-m56-reflection-enum-union-baseline-5m)
at native commit `f57b81a1b2b48a56cfbeec22396ae335c353ecde`.
The enum case reads the fixture's cached `color` descriptor, the default case
reads the XOR-encoded `defaulted` UInt32 slot, and the union case discovers the
active `choice` member and reads its UInt64 payload. All samples produce
identical C++ and Rust checksums.

| Operation | C++ ns/op | Native ns/op | Native / C++ |
| --- | ---: | ---: | ---: |
| dynamic defaulted UInt32 | 38.9151 | 16.6888 | 0.429 |
| dynamic enum ordinal | 52.3984 | 40.9478 | 0.781 |
| dynamic active-union member | 128.1883 | 88.2827 | 0.689 |
| borrowed Text + Data control | 99.7677 | 40.7923 | 0.409 |

All three native paths are faster than C++, but none yet preserves the best
applicable lower-layer ratio within the 3% tolerance. Defaulted scalar access
is close; its prepared root still opens a general reader instead of borrowing
the already validated data section directly. Enum access additionally creates
an owned `DynamicEnum` and clones its schema `Arc` merely to consume the raw
ordinal. Union access creates a retained group `DynamicStruct`, including
schema, brand, backing, and coordinate state, before reading the shared parent
data section. The next isolation adds a copy-only prepared scalar result and a
callback-scoped group view. Unknown discriminants and schema evolution remain
separate correctness/performance shapes.

## Prepared scalar and scoped active-union gate

Final evidence:
[`benchmarks/results/2026-09-04-m56-reflection-enum-union-final-5m`](../../benchmarks/results/2026-09-04-m56-reflection-enum-union-final-5m)
at native commit `5be4a244da0ef4a0353fcc5faf61a384b4c6202f`.

| Operation | C++ ns/op | Native ns/op | Native / C++ |
| --- | ---: | ---: | ---: |
| dynamic defaulted UInt32 | 36.2630 | 5.9474 | 0.164 |
| dynamic enum ordinal | 56.2489 | 7.2722 | 0.129 |
| dynamic active-union member | 132.7292 | 27.3053 | 0.206 |
| borrowed Text + Data control | 98.1068 | 41.5686 | 0.424 |

`DynamicScalarField` prepares the resolved scalar kind, default, offset, union
metadata, containing type, and originating schema once. Reads return the
copy-only `DynamicScalarValue`, so consuming an enum ordinal no longer clones a
schema `Arc` or constructs an owned `DynamicEnum`. Pointer-backed structs read
their already validated data section directly; element-backed structs use the
same scoped checked reader. Default XOR semantics are unchanged.

`DynamicStructField` now also represents groups. A group view copies the
parent's short-lived reader because Cap'n Proto groups share their parent's
exact storage; pointer struct fields continue to descend through the checked
pointer path. `DynamicStructView::active_union_field()` preserves known versus
unknown discriminant behavior, and prepared union-member reads still reject an
inactive member before reading its payload. Cross-schema scalar plans are
rejected, and all C++/Rust checksums agree.

Relative to the recorded baseline, native default, enum, and active-union
medians fall by 64.4%, 82.2%, and 69.1%. Each cumulative ratio is better than
the 0.424 borrowed-value control in the same run, so the scalar-default, enum,
group, and known active-union reader gates are closed. Unknown discriminants,
schema evolution, and builder reflection remain open.

## Unknown-union discriminant gate

Baseline evidence:
[`benchmarks/results/2026-09-04-m56-reflection-unknown-union-baseline-5m`](../../benchmarks/results/2026-09-04-m56-reflection-unknown-union-baseline-5m)
at native commit `710fecb01e8c55cb7c83fb7cfe882fe8c3fd0def`.
The fixture contains the raw discriminant `55`, which neither schema knows.
Both implementations first prove that dynamic union dispatch does not expose a
stale known payload and then preserve the raw ordinal in the checksum. Fixture
construction remains outside the timed region.

| Operation | C++ ns/op | Native ns/op | Native / C++ |
| --- | ---: | ---: | ---: |
| unknown-union recognition + raw ordinal | 68.7100 | 36.3235 | 0.529 |
| borrowed Text + Data control | 99.4407 | 42.6773 | 0.429 |

The initial native path is 47.1% faster than C++, but it misses the inherited
control ratio. Each call on the callback-scoped group view repeats a compiled
schema node lookup even though the containing struct and prepared group
descriptor already establish the exact child type.

Final evidence:
[`benchmarks/results/2026-09-04-m56-reflection-unknown-union-final-5m`](../../benchmarks/results/2026-09-04-m56-reflection-unknown-union-final-5m)
at native commit `f1295bb673c29c65907b1ed92c1b78436a932f1b`.

| Operation | C++ ns/op | Native ns/op | Native / C++ |
| --- | ---: | ---: | ---: |
| unknown-union recognition + raw ordinal | 74.8997 | 20.8419 | 0.278 |
| borrowed Text + Data control | 106.0105 | 47.4992 | 0.448 |

Root callback views now borrow their already-resolved `StructSchema` and
prepared struct/group descriptors carry the resolved child structure into the
scoped view. Inline-composite list elements resolve their structure once when
the child view opens. Union recognition and raw-discriminant access therefore
avoid repeated schema-index lookups while retaining the same checked data read,
schema identity, and callback lifetime; no `unsafe` code is used.

The native median falls by 42.6% from baseline. Its 0.278 cumulative ratio is
also materially better than the 0.448 borrowed-value control in the same run,
so the unknown-union reader gate is closed. Schema evolution and builder
reflection remain open.

## Schema-evolution reader gate

Final evidence:
[`benchmarks/results/2026-09-04-m56-reflection-evolution-final-5m`](../../benchmarks/results/2026-09-04-m56-reflection-evolution-final-5m)
at native commit `efbe22ff`.

The v2 writer fixture is read through the compiled v1 schema on both sides.
Each timed iteration dynamically reads the exact scalar and text values written
by v2, preserves the v2-only enum value as unknown raw ordinal `2`, and views a
v2 `List(Item)` field through v1's compatible `List(UInt32)` declaration. The
second upgraded-list element is `42`. Schema loading, framing, root creation,
field lookup, and fixture generation remain outside the timed region, and all
C++/Rust checksums agree.

| Operation | C++ ns/op | Native ns/op | Native / C++ |
| --- | ---: | ---: | ---: |
| dynamic v1-read-v2 evolution | 299.2500 | 100.2990 | 0.335 |
| borrowed Text + Data control | 134.4009 | 60.6836 | 0.452 |

The native evolution path is 66.5% faster than C++. Its 0.335 cumulative ratio
is also better than the 0.452 borrowed-value control in the same run, so the
schema-evolution reader gate is closed. Builder reflection remains open.
