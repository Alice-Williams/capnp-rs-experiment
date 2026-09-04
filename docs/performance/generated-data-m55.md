# M55 generated-data API performance

## Baseline source trace

The generated Rust API is currently a typed facade over the dynamic reflection
API. This is correct and useful for interoperability, but its hot path does not
resemble generated C++:

| Operation | Generated Rust today | Pinned generated C++ |
| --- | --- | --- |
| scalar read | `DynamicStruct::get("field")`, schema lookup, dynamic enum construction/match, retained reader reconstruction, checked scalar read | inlined constant data offset and default passed to `getDataField` |
| scalar write | `DynamicStructBuilder::set("field", DynamicInput)`, schema lookup, cloned `Field`, brand/type resolution, dynamic dispatch, checked scalar write | inlined constant data offset and default passed to `setDataField` |
| text/data read | dynamic pointer lookup followed by a new `String`/`Vec<u8>` allocation and copy | borrowed `Text::Reader`/`Data::Reader` over the message |
| list read | dynamic pointer lookup, cloned runtime element `Type`, dynamic-list enum dispatch per element | statically typed list reader and constant pointer offset |
| struct/group read | dynamic lookup plus cloned schema/brand/backing handles | statically typed reader and constant pointer/group layout |
| union query | schema lookup followed by a retained-reader reconstruction and dynamic discriminant read | constant discriminant offset and generated enum switch |

The generated-source fixture confirms that every ordinary getter calls
`self.inner.get("...")`, every ordinary setter calls `self.inner.set("...",
...)`, and `which()` calls `self.inner.union_discriminant()`. The code generator
already has each field's `FieldKind`, slot offset, type, default, discriminant,
and containing struct layout when it emits these methods, so none of those
lookups are semantically required for non-generic generated fields.

The retained representation is a second, independent cost. `DynamicStruct`
stores an `Arc<CompiledSchema>`, a runtime `Brand`, and a retained object or list
coordinate. Each scalar getter reconstructs a short-lived checked reader from
that retained coordinate. M52 made this reconstruction allocation-free, but a
generated scalar sequence still reconstructs it once per field whereas direct
runtime code opens one reader and performs all constant-offset accesses.

Text and data have a larger semantic mismatch: generated Rust currently returns
owned `String` and `Vec<u8>` values, while C++ returns borrowed views. M55 must
introduce generated borrowed views whose lifetime is tied to an explicit read
context; merely making the field-name lookup faster would leave allocation and
copying in every blob getter.

## Measurement decomposition

The first paired baseline separates:

1. direct checked runtime reads from each implementation's public retained or
   borrowed root representation;
2. generated hot reads from the same pinned message;
3. direct checked construction with constant layout;
4. generated construction with identical values and allocation policy.

Scalar and blob cases are recorded independently first because they exercise
the two dominant mechanisms: repeated reflection/retained-reader overhead and
owned blob conversion. Typed lists, nested structs/groups, unions/defaults, and
evolution cases extend the same harness after those floors are stable.

No implementation path changes before this unmodified baseline is checked in.

## Initial retained-reader baseline

Evidence:
[`benchmarks/results/2026-09-03-m55-generated-reader-baseline-g-drive-docker`](../../benchmarks/results/2026-09-03-m55-generated-reader-baseline-g-drive-docker)
at native commit `b9fc9d03d0d4eeacd5aec80dcfe096c87418de15`.
The run uses 100,000 operations per sample, two warmups, and nine alternating
samples. Every direct/generated and cross-language case produces the same
semantic checksum over the pinned C++ wire fixture.

| Operation | C++ ns/op | Native ns/op | Native / C++ |
| --- | ---: | ---: | ---: |
| direct scalars | 6.7413 | 26.8044 | 3.976 |
| generated scalars | 6.0133 | 1,273.7523 | 211.822 |
| direct text/data | 29.4165 | 43.3043 | 1.472 |
| generated text/data | 24.1253 | 339.2937 | 14.064 |

The C++ direct and generated variants compile to effectively the same
constant-offset operations; their subtracted difference is below timer
resolution. Native generated scalar access adds about 1,241 ns per complete
field sequence over its direct retained-reader control. Native generated blob
access adds about 256 ns, dominated by two allocations and payload copies.

The ownership rows are intentionally explicit. C++'s generated reader borrows
stable native pointers from its message reader. Today's Rust generated reader
owns an `Arc`-retained coordinate and safely reconstructs a short-lived reader;
it cannot expose a payload borrow with the lifetime of an individual accessor
call. The 3.976 and 1.472 direct ratios therefore describe the retained Rust
model, not the lower-level borrowed reader previously qualified in M52. M55
must measure a new lifetime-bound borrowed generated reader separately rather
than silently compare unlike ownership semantics.

## Borrowed-reader scalar and blob checkpoint

Evidence:
[`benchmarks/results/2026-09-03-m55-generated-reader-borrowed-g-drive-docker`](../../benchmarks/results/2026-09-03-m55-generated-reader-borrowed-g-drive-docker)
at native commit `d61d4a6d72727edd4a85e533922914193f2203cb`.
The paired run adds a generated `BorrowedReader` whose lifetime is tied to a
validated `BorrowedMessage`. It caches the root data slice and retains the
checked root reader for pointer fields. The representation performs no
allocation or copy and contains no raw pointer or unsafe self-reference.

| Operation | C++ ns/op | Native ns/op | Native / C++ |
| --- | ---: | ---: | ---: |
| borrowed generated scalars | 3.6763 | 5.2064 | 1.416 |
| borrowed generated text/data | 18.0554 | 12.9000 | 0.714 |
| retained generated scalars | 3.9763 | 67.6683 | 17.018 |
| retained generated text/data | 17.2203 | 244.5270 | 14.200 |

This checkpoint is not an acceptance comparison: it added borrowed generated
rows before adding a borrowed direct-runtime Rust control. Consequently, its
incremental table subtracts the retained direct reader from the borrowed
generated reader. The corrected harness records separate
`borrowed-direct-*` rows so ownership, validation state, and cached view setup
are paired like-for-like.

The C++ `generated-*` and `borrowed-*` rows intentionally invoke the same
generated reader. Their timing spread is therefore an internal noise check,
not a second C++ implementation. The native borrowed blob path is already
about 1.40x faster than this pinned C++ reader while returning exact borrowed
slices without copying. This paired run confirms the lead but does not by
itself attribute it; generated assembly and phase-isolated pointer/blob reads
must confirm the mechanism before M55 treats it as a durable advantage. The
borrowed scalar sequence remains about 1.42x slower. Its remaining hot work is
the per-field checked `DataSection` read and `Result` propagation; that is the
next scalar floor to isolate before adding lists and nested pointers.

The retained API remains deliberately separate. It owns schema and message
backing and can outlive the caller's read context, which costs additional
indirection and prevents accessor-returned blob borrows. It is useful when
ownership is required, while the generated borrowed API now matches the C++
reader's lifetime and memory model for hot synchronous traversal.

## Corrected borrowed controls and scalar attribution

The original eight-case evidence first recorded a distinct borrowed direct
runtime control at
[`benchmarks/results/2026-09-03-m55-generated-reader-borrowed-paired-g-drive-docker`](../../benchmarks/results/2026-09-03-m55-generated-reader-borrowed-paired-g-drive-docker).
Subsequent composite work found that the native harness had not made its reader
input opaque on every iteration, so LLVM could still hoist known fixture loads.
The absolute native/C++ ratios in this section and the two checkpoints below
are therefore retained as development history, not acceptance evidence. The
relative API design findings remain useful. The native direct path retained the root reader's data-word
count, allowing LLVM to combine short-section checks, while the generated
cached byte slice emitted independent bounds/default branches. The generated
typed enum also constructed `Color` and immediately converted it back to its
ordinal; C++'s generated enum read is an unvalidated underlying ordinal.

The safe fix has two parts:

- `BorrowedReader` caches an optional reference to the schema's complete fixed
  data prefix. Complete current-schema messages use constant array indexes;
  short older messages use total checked reads and the same XOR defaults.
- Enum fields expose an additional `{field}_ordinal()` getter. The typed getter
  remains available and preserves `Unrecognized(u16)`, while code that needs
  the wire ordinal no longer constructs and deconstructs the richer Rust enum.

The prefix reference adds one pointer (eight bytes on this x86-64 target) to a
borrowed generated reader. It does not copy data or allocate. This is a
deliberate memory-for-codegen trade: current-schema scalar reads avoid repeated
bounds branches, while schema-evolution behavior remains intact and tested.

Final scalar/ordinal checkpoint evidence:
[`benchmarks/results/2026-09-03-m55-generated-reader-ordinal-g-drive-docker`](../../benchmarks/results/2026-09-03-m55-generated-reader-ordinal-g-drive-docker)
at native commit `ceba5d3`, using one million operations per sample:

| Operation | C++ ns/op | Native ns/op | Native / C++ |
| --- | ---: | ---: | ---: |
| borrowed direct scalars | 4.5690 | 3.7981 | 0.831 |
| borrowed generated scalars/ordinal | 3.6324 | 3.2443 | 0.893 |
| borrowed direct text/data | 17.2404 | 8.9550 | 0.519 |
| borrowed generated text/data | 20.5815 | 13.3336 | 0.648 |

The generated scalar sequence is 10.7% faster than pinned C++ and 14.6%
faster than native direct access. It does not yet satisfy the literal inherited
0.831 direct ratio plus tolerance, although both languages' generated sequence
is faster than its direct control and both subtracted increments are below
resolution. Isolated scalar and enum accessors must resolve that gate.

The borrowed blob operation remains 35.2% faster end to end, but its generated
pointer/blob increment is about 4.06 ns versus 1.18 ns in C++. That incremental
gate remains open; the next reader work isolates pointer following, text NUL
adjustment, and data view construction before moving to lists and structs.

## Borrowed scalar and blob gate

> Superseded measurement: this checkpoint predates per-iteration native reader
> black-boxing. Its checksums and API behavior are valid, but its hot-load
> timings are not used for later inherited gates.

Evidence:
[`benchmarks/results/2026-09-03-m55-generated-reader-pointer-section-g-drive-docker`](../../benchmarks/results/2026-09-03-m55-generated-reader-pointer-section-g-drive-docker)
at native commit `e2a0fba`, with one million operations per sample.

The generated blob overhead was not in pointer validation or the borrowed
views. Caching the pointer-section base alone left the smoke result near 13 ns.
The material cost was widening each successful generated getter from the
runtime's `StructReadError` to reflection-oriented `DynamicError`. Returning
the precise runtime error type reduced the generated blob sequence to 8.5349
ns without changing validation, traversal charging, or failure behavior.

| Operation | Direct native / C++ | Generated native / C++ | Inherited ceiling |
| --- | ---: | ---: | ---: |
| borrowed scalars/ordinal | 0.778 | 0.782 | 0.801 |
| borrowed text/data | 0.493 | 0.443 | 0.508 |

Both complete generated operations are faster than C++ and preserve their
paired direct-runtime advantage within the 3% tolerance. The native generated
minus direct medians are negative for both shapes, so there is no resolvable
incremental generated cost. These scalar/ordinal and text/data borrowed-reader
gates are closed. Typed lists, nested structs/groups, unions, and evolution
remain open reader work; retained ownership remains reported separately.

## Hardened group and union gate

The group benchmark uncovered the native load-hoisting issue above. The runner
now black-boxes every native reader input inside the measured loop, matching the
C++ harness's inline assembly barrier, and gives both readers effectively
unlimited traversal budgets. A deliberately longer five-million-operation run
at
[`benchmarks/results/2026-09-03-m55-generated-reader-groups-streamlined-5m-g-drive-docker`](../../benchmarks/results/2026-09-03-m55-generated-reader-groups-streamlined-5m-g-drive-docker)
records the corrected floor at native commit `c86ed14`.

| Operation | Direct native / C++ | Generated native / C++ |
| --- | ---: | ---: |
| borrowed scalars/ordinal | 1.241 | 1.106 |
| borrowed text/data | 0.725 | 0.680 |
| borrowed group/union fields | 2.427 | 1.008 |

The host was under heavier absolute load during this run, but every C++/native
pair alternates in the same sample schedule and uses the same checksum. The
ratios, rather than the absolute nanoseconds, are the acceptance signal. The
corrected generated scalar ratio sets a 1.139 inherited ceiling after the 3%
tolerance; the complete generated group/union sequence reaches 1.008. It is at
parity with generated C++ and materially faster than the raw checked native
control, so the group/union layer preserves more performance than it inherits.

The implementation uses generated constant-offset group views, total unknown
union discriminants, and explicit hot-path inlining. A trial that read the
union tag through the cached full-data option was slower and was removed; the
lean total `DataSection` tag read wins here. The hardened scalar result also
reopens the absolute scalar floor: it is 10.6% behind C++ under the corrected
barrier even though the group layer satisfies its inherited gate. Blob access
retains a clear lead. Primitive, enum, text/data, nested, and struct-list
borrowed APIs now have conformance coverage; their individual performance
gates remain next.

## Borrowed list gate

The first hardened mixed-list run measured the generated native path at 1.504x
C++ for primitive, enum, text, data, and nested primitive-list access. Two
runtime changes were retained: direct non-inline-composite list validation now
avoids the generic pointer-kind path, and primitive list readers cache their
validated segment and element coordinates. A broader pointer-list coordinate
cache made the paired result worse and was reverted.

The decisive change was explicit inlining across the generated wrapper and the
checked runtime's list open/view/index path. These functions are small but span
the codegen and message crates; leaving the decision to cross-crate heuristics
cost roughly half of the complete operation. Final evidence at native commit
`2588476` is in
[`benchmarks/results/2026-09-03-m55-generated-reader-lists-final-5m-g-drive-docker`](../../benchmarks/results/2026-09-03-m55-generated-reader-lists-final-5m-g-drive-docker),
again using five million operations, alternating implementations, opaque
per-iteration reader inputs, and equal traversal limits.

| Operation | C++ ns/op | Native ns/op | Native / C++ |
| --- | ---: | ---: | ---: |
| borrowed direct mixed lists | 98.7270 | 62.3907 | 0.632 |
| borrowed generated mixed lists | 103.0098 | 63.4680 | 0.616 |

The mixed generated operation is 38.4% faster than generated C++. More
importantly, it preserves the lower pointer/blob advantage: this run's
generated blob ratio is 0.604, giving an inherited ceiling of 0.622 with the
3% tolerance, and generated lists reach 0.616. The native generated wrapper
adds only 1.08 ns over its direct control versus 4.28 ns for C++, so static
typing does not consume the lower-layer lead. The primitive, enum, text/data,
and nested primitive-list gate is closed. Inline-composite struct-list element
access and ordinary nested struct pointers remain separate gates.

## Borrowed nested-struct gate

The first paired nested-struct run exposed the same cross-crate inlining cliff
as lists: native generated access took 1.249x C++ even though the operation is
only a checked pointer dereference followed by one scalar read. Marking the
small struct-pointer validation and view-construction path for explicit
inlining removed that boundary cost without changing validation, traversal
accounting, or generated API semantics.

Final five-million-operation evidence at native commit `253710f` is in
[`benchmarks/results/2026-09-03-m55-generated-reader-nested-final-5m-g-drive-docker`](../../benchmarks/results/2026-09-03-m55-generated-reader-nested-final-5m-g-drive-docker).
It uses the same alternating schedule, opaque per-iteration reader inputs,
unlimited traversal limits, and checksum equality as the hardened list run.

| Operation | C++ ns/op | Native ns/op | Native / C++ |
| --- | ---: | ---: | ---: |
| borrowed direct nested struct | 8.3819 | 5.5495 | 0.662 |
| borrowed generated nested struct | 10.4038 | 5.4399 | 0.523 |

Generated native nested-struct access is 47.7% faster than generated C++ and
comfortably preserves the paired direct-runtime ratio, whose 3% tolerance
allows a 0.682 cumulative ceiling. C++ adds 2.02 ns for its generated wrapper;
the native generated-minus-direct median is below timer resolution. Ordinary
nested struct pointers are therefore closed.

## Borrowed inline-composite struct-list gate

The initial paired struct-list run placed native direct element access at
1.776x C++ and generated access at 1.639x. Explicit inlining first brought both
to approximate parity. The remaining duplicated work was in the checked
runtime: every element lookup reconstructed the already validated list layout,
reselected its segment, and resliced the element data.

`StructListReader` now caches its validated segment, length, element stride,
data width, and pointer width when the list is opened. Each element lookup
performs only its required index, nesting, and checked-coordinate work and
constructs the borrowed data view once. This preserves primitive-to-struct and
pointer-to-struct upgrade semantics and adds no `unsafe` code.

Final five-million-operation evidence at native commit `964df29` is in
[`benchmarks/results/2026-09-03-m55-generated-reader-struct-lists-final-5m-g-drive-docker`](../../benchmarks/results/2026-09-03-m55-generated-reader-struct-lists-final-5m-g-drive-docker).

| Operation | C++ ns/op | Native ns/op | Native / C++ |
| --- | ---: | ---: | ---: |
| borrowed direct struct-list element | 17.3854 | 15.5757 | 0.896 |
| borrowed generated struct-list element | 17.8094 | 15.3211 | 0.860 |

Generated native struct-list access is 14.0% faster than generated C++ and
improves on its paired direct-runtime ratio. The direct ratio permits a 0.923
cumulative ceiling with tolerance; generated access reaches 0.860. Its native
generated-minus-direct median is below timer resolution, while C++ adds
0.42 ns. The inline-composite struct-list reader gate is closed.

## Borrowed schema-evolution gate

The evolution workload reads the C++ `evolution-v2` fixture through generated
`evolution-v1` bindings in both languages. It combines a constant-offset
scalar, an unknown newer enum ordinal, borrowed text, and the required
`List(Struct)` to `List(UInt32)` element upgrade. The paired direct control
uses the same v1 field layout and checked runtime operations, so newer fields
remain outside both timed paths while the compatible old view performs equal
observable work.

Final five-million-operation evidence at native commit `2a6cd59` is in
[`benchmarks/results/2026-09-03-m55-generated-reader-evolution-final-5m-g-drive-docker`](../../benchmarks/results/2026-09-03-m55-generated-reader-evolution-final-5m-g-drive-docker).

| Operation | C++ ns/op | Native ns/op | Native / C++ |
| --- | ---: | ---: | ---: |
| borrowed direct v1-over-v2 read | 25.0847 | 19.8368 | 0.791 |
| borrowed generated v1-over-v2 read | 25.3498 | 19.6268 | 0.774 |

Generated native evolution access is 22.6% faster than generated C++ and
improves on the paired direct-runtime ratio. The direct ratio permits a 0.815
cumulative ceiling with tolerance; generated access reaches 0.774. Its native
generated-minus-direct median is again below timer resolution. The old-reader
over new-writer schema-evolution reader gate is closed.

## Borrowed pointer-default gate

Borrowed generated readers now emit a constant-layout accessor for non-empty
Text schema defaults. The generated default is a static, terminated byte
literal; the checked runtime selects it only for an absent or null field and
otherwise follows and charges the message pointer normally. This avoids
schema lookup, allocation, and owned-string conversion while preserving the
wire-level distinction needed to apply pointer defaults correctly.

The benchmark uses the byte-identical minimal one-segment message in both
languages, whose root pointer is null, then observes the `defaultText` bytes.
Final five-million-operation evidence at native commit `b5f0fd7` is in
[`benchmarks/results/2026-09-03-m55-generated-reader-defaults-final-5m-g-drive-docker`](../../benchmarks/results/2026-09-03-m55-generated-reader-defaults-final-5m-g-drive-docker).

| Operation | C++ ns/op | Native ns/op | Native / C++ |
| --- | ---: | ---: | ---: |
| borrowed direct missing Text default | 2.8202 | 1.1272 | 0.400 |
| borrowed generated missing Text default | 5.6034 | 1.1538 | 0.206 |

Generated native pointer-default access is 79.4% faster than generated C++
and preserves substantially more than the paired direct-runtime advantage.
The direct ratio permits a 0.412 cumulative ceiling with tolerance; generated
access reaches 0.206. Native generated wrapping adds only 0.03 ns versus
2.78 ns for C++. Together with scalar default-XOR coverage, the generated
reader default gate is closed.

## Retained scalar-reader gate

The retained reader owns its schema, immutable message, and prepared root
coordinate. Its earlier scalar path nevertheless borrowed and resliced that
retained coordinate for every generated field call. A generated reader now
copies a schema-sized immutable data prefix once when it is constructed, then
serves constant-offset scalar and enum access directly from that local prefix.
Short evolution views retain their checked fallback. To prevent pathological
generated-reader sizes, the optimization applies only when the complete data
section is at most 128 bytes; larger schemas retain the allocation-free
prepared-reader path.

Final five-million-operation evidence for the optimized representation at
native commit `39e8c36` is in
[`benchmarks/results/2026-09-03-m55-retained-scalar-final-5m-g-drive-docker`](../../benchmarks/results/2026-09-03-m55-retained-scalar-final-5m-g-drive-docker).

| Operation | C++ ns/op | Native ns/op | Native / C++ |
| --- | ---: | ---: | ---: |
| retained direct scalar sequence | 5.3541 | 32.5785 | 6.085 |
| retained generated scalar sequence | 3.6730 | 5.1746 | 1.409 |

The generated path falls from roughly 73 ns before caching to 5.17 ns and
removes 84% of the native retained-control cost. Although retained native
access remains 40.9% slower than generated C++ in absolute terms, it easily
preserves the ownership model's paired 6.267 cumulative ceiling and adds no
resolvable generated overhead. The retained scalar generated-API gate is
closed; reducing the retained ownership floor itself remains lower-layer work.

## Scalar-builder gate

Generated scalar and enum setters now embed their wire offsets and defaults and
call the checked `StructBuilder` primitives directly. They no longer resolve a
field name, clone schema metadata, or dispatch through `DynamicInput` for an
ordinary non-union field. Union setters retain the dynamic activation path so
that writing a member still updates its discriminant.

The paired workload mutates every scalar and enum field plus the scalar-default
field on one preallocated `WireFixture` root. Values vary on every iteration,
both implementations place an optimizer barrier around the mutable builder,
and all four paths produce the same checksum. Root construction and allocation
are outside this hot-setter timing; cold construction remains a separate gate.

Final five-million-operation evidence at native commit `0a1c550` is in
[`benchmarks/results/2026-09-03-m55-generated-builder-scalars-final-5m-g-drive-docker`](../../benchmarks/results/2026-09-03-m55-generated-builder-scalars-final-5m-g-drive-docker).

| Operation | C++ ns/op | Native ns/op | Native / C++ |
| --- | ---: | ---: | ---: |
| direct scalar writes | 15.5077 | 36.7342 | 2.369 |
| generated scalar writes | 17.6091 | 38.6834 | 2.197 |

Generated Rust improves on the paired direct-runtime ratio and stays beneath
its 2.440 cumulative ceiling after the 3% tolerance. The paired incremental
generated cost is 1.13 ns in Rust versus 1.79 ns in C++, an incremental ratio
of 0.631. The generated scalar-builder layer therefore closes its M55 gate.

Inlining the small checked write path across the wire, message, and generated
crates materially reduced native setter cost. The remaining roughly 2.2x
absolute gap is shared lower-layer work: Rust revalidates the data-section
coordinate, segment, and byte range on each safe setter, while generated C++
retains a direct data pointer and emits a bounds-specialized store. It does not
come from generated lookup or metadata work. A future safe direct-data builder
view may reduce this floor, but it must not hold an invalidatable slice across
arena growth. Blob, struct, list, union, default, and evolution builder gates
remain open.

## Blob-builder baseline

The fixed-capacity blob-builder workload appends alternating seven-byte Text
and eight-byte Data values to a preallocated one-segment arena. Allocation and
root creation are excluded equally; pointer emission, zero initialization, and
payload copying remain inside each iteration. Evidence before generated
pointer-slot specialization is in
[`benchmarks/results/2026-09-03-m55-generated-builder-blobs-baseline-100k-g-drive-docker`](../../benchmarks/results/2026-09-03-m55-generated-builder-blobs-baseline-100k-g-drive-docker)
at native commit `18767fd`.

| Operation | C++ ns/op | Native ns/op | Native / C++ |
| --- | ---: | ---: | ---: |
| direct Text/Data writes | 76.8374 | 53.8327 | 0.701 |
| generated Text/Data writes | 81.8621 | 453.8511 | 5.544 |

The direct native path is 29.9% faster than its C++ control, while the generated
path loses that advantage completely. The approximately 400 ns native delta is
field-name lookup, owned field metadata, runtime type resolution, and dynamic
input dispatch performed once for Text and again for Data. This attribution is
also visible structurally in generated source; constant pointer offsets are
already available to codegen. The next change removes that reflection only for
ordinary non-union Text/Data setters.

## Blob-builder gate

Generated non-union Text and Data setters now embed their pointer offsets and
call checked `StructBuilder` blob primitives directly. They retain the same
copying, allocation, pointer emission, and output-limit behavior as the dynamic
path, but no longer perform field-name lookup, clone field metadata, resolve a
runtime type, or dispatch through `DynamicInput`. Union blob setters retain the
dynamic activation path so that their discriminants remain correct.

Final five-million-operation evidence at native commit `af9846c` is in
[`benchmarks/results/2026-09-03-m55-generated-builder-blobs-final-5m-g-drive-docker`](../../benchmarks/results/2026-09-03-m55-generated-builder-blobs-final-5m-g-drive-docker).

| Operation | C++ ns/op | Native ns/op | Native / C++ |
| --- | ---: | ---: | ---: |
| direct Text/Data writes | 28.0623 | 36.4282 | 1.298 |
| generated Text/Data writes | 27.6996 | 35.6529 | 1.287 |

The generated native path is 92.1% faster than its pre-specialization baseline
and slightly improves on the paired direct-runtime ratio. Its 1.287 cumulative
ratio stays below the inherited 1.337 ceiling including tolerance. Generated
and direct medians differ by less than one nanosecond in both languages, so the
incremental cost is below timer resolution. The ordinary Text/Data builder gate
is closed; struct, list, union, default, and evolution builder gates remain
open.

## Struct-builder baseline

The struct-builder workload initializes a fresh `Node` in the root's `node`
slot and writes its scalar value on every iteration. Both implementations use a
fixed-capacity one-segment arena, exclude root construction, and include child
allocation, zeroing, pointer emission, typed child wrapping, and the scalar
write. Evidence before generated struct-slot specialization is in
[`benchmarks/results/2026-09-03-m55-generated-builder-struct-baseline-100k-g-drive-docker`](../../benchmarks/results/2026-09-03-m55-generated-builder-struct-baseline-100k-g-drive-docker)
at native commit `e0f6ba2`.

| Operation | C++ ns/op | Native ns/op | Native / C++ |
| --- | ---: | ---: | ---: |
| direct `Node` construction | 20.6430 | 15.5357 | 0.753 |
| generated `Node` construction | 20.7850 | 120.7257 | 5.808 |

The direct native construction path is 24.7% faster than C++, but the generated
path loses that advantage to an additional 105.19 ns per operation. C++ adds
only 0.18 ns at the generated layer. Source tracing attributes the Rust delta
to field-name lookup, cloned field/type/brand metadata, runtime type
resolution, and schema-node lookup before the same checked allocation and
pointer emission. The next change embeds the pointer offset and child layout in
the generated initializer while retaining the dynamic path for reflection.

## Struct-builder gate

Generated initializers for ordinary, non-union, unbranded struct fields now
embed the pointer offset, child type ID, and child data/pointer sizes. They call
a checked runtime primitive directly, so allocation, zeroing, output limits,
and pointer emission are unchanged while field-name lookup, field cloning,
brand resolution, and schema-node lookup leave the hot path. Branded and union
fields retain the dynamic path so generic substitution and discriminant
activation remain exact.

The first constant-layout diagnostic reduced native generated construction
from 120.73 ns to 25.52 ns, but exposed roughly 6.1 ns of residual wrapper cost.
The generated child builder still carried a three-word owned `Brand` containing
an empty `Vec`. The final representation stores the common empty brand as
`None` and a non-empty brand as its boxed scopes slice. This reduces the
internal builder state without adding an allocation layer for branded scopes;
generic-brand and dynamic-builder tests continue to exercise the fallback.

Final five-million-operation evidence at native commit `f3c5fd6` is in
[`benchmarks/results/2026-09-03-m55-generated-builder-struct-final-5m-g-drive-docker`](../../benchmarks/results/2026-09-03-m55-generated-builder-struct-final-5m-g-drive-docker).

| Operation | C++ ns/op | Native ns/op | Native / C++ |
| --- | ---: | ---: | ---: |
| direct `Node` construction | 20.6200 | 15.3541 | 0.745 |
| generated `Node` construction | 20.5658 | 15.6843 | 0.763 |

Generated Rust construction is 23.7% faster than generated C++. Its 0.763
cumulative ratio stays below the direct-runtime ceiling of 0.767 after the 3%
tolerance. Native generated wrapping adds 0.31 ns while the paired C++ delta is
negative, so the incremental comparison is below timer resolution. Generated
source tests prove the initializer contains the constant layout and does not
call dynamic field lookup. The ordinary struct-builder gate is closed; list,
union, default, and evolution builder gates remain open.

## Primitive-list builder baseline

The paired primitive-list workload initializes a four-element `List(UInt16)`
in the root's `uint16s` field and writes four pass-dependent values on every
iteration. Both implementations use fixed-capacity one-segment arenas and
include list allocation, zeroing, pointer emission, list-builder construction,
and all checked element writes. Root construction remains outside the timed
region. Evidence before list specialization is in
[`benchmarks/results/2026-09-03-m55-generated-builder-list-baseline-100k-g-drive-docker`](../../benchmarks/results/2026-09-03-m55-generated-builder-list-baseline-100k-g-drive-docker)
at native commit `a89ef64`.

| Operation | C++ ns/op | Native ns/op | Native / C++ |
| --- | ---: | ---: | ---: |
| direct primitive-list construction | 16.5818 | 18.8729 | 1.138 |
| generated primitive-list construction | 15.9477 | 155.1202 | 9.727 |

The direct native path is 13.8% slower than C++, so its allocation and element
write costs must be investigated before using it as an inherited generated-API
ceiling. Generated Rust adds a further 136.40 ns per list. Source tracing
attributes that delta to field-name lookup, cloned list type metadata and brand
resolution during initialization, followed by dynamic storage/input dispatch
for each of the four elements. C++'s generated/direct delta is below timer
resolution. The next step isolates the direct allocation and element-store
costs, improves the lower-level floor, and only then generates a typed
constant-layout list initializer.

## Primitive-list builder gate

The lower-level `DataListBuilder` now retains a checked mutable slice bounded
to the allocated primitive-list payload. Its exclusive borrow prevents arena
growth for the slice's lifetime, so element writes no longer reacquire a
segment or recompute an absolute word coordinate. Primitive write
implementations are inlined across the crate boundary, while index arithmetic
and byte-range checks remain intact. This uses ordinary safe references and
adds no `unsafe` code.

For ordinary non-union primitive-list fields, generated initializers now embed
the pointer offset and instantiate the typed `DataListBuilder<T>` directly.
The public generated setter consequently accepts `T`, matching the schema,
instead of requiring `DynamicInput`; reflection remains available through the
dynamic builder API. Generated-source tests require the constant offset and
reject field-name lookup for `uint16s`.

Final five-million-operation evidence at native commit `ae37f1b` is in
[`benchmarks/results/2026-09-03-m55-generated-builder-list-final-5m-g-drive-docker`](../../benchmarks/results/2026-09-03-m55-generated-builder-list-final-5m-g-drive-docker).

| Operation | C++ ns/op | Native ns/op | Native / C++ |
| --- | ---: | ---: | ---: |
| direct primitive-list construction | 16.9785 | 13.7330 | 0.809 |
| generated primitive-list construction | 17.2460 | 14.1470 | 0.820 |

Direct Rust is 19.1% faster than direct C++ and 27.2% faster than its recorded
baseline. Generated Rust is 18.0% faster than generated C++ and 90.9% faster
than its pre-specialization baseline. Its 0.820 cumulative ratio stays below
the improved direct-runtime ceiling of 0.833 after the 3% tolerance. Native
generated wrapping adds 0.41 ns while the paired C++ delta is negative, so the
incremental comparison is below timer resolution. The ordinary primitive-list
builder gate is closed; pointer/struct lists, unions, defaults, and evolution
builder gates remain open.

## Struct-list builder baseline

The paired struct-list workload initializes two inline-composite `Node`
elements in the root's `structs` field and writes one varying `UInt32` value to
each element on every iteration. Both implementations use fixed-capacity
one-segment arenas. Allocation, inline-composite tag emission, zeroing, element
selection, and the two scalar writes are timed; root construction remains
outside the timed region.

The direct controls embed pointer slot 17 and the `Node` layout of one data word
and one pointer. C++ initializes `List(AnyStruct)` with that layout, while Rust
uses the corresponding checked `StructListBuilder`. The generated C++ path uses
`initStructs()` and typed `Node` builders. The current generated Rust path still
performs field-name lookup, clones and resolves list/brand metadata, constructs
a dynamic list enum, and repeats dynamic struct-field lookup for both elements.

Alternating 100,000-operation baseline evidence at native commit `fee6649` is
in
[`benchmarks/results/2026-09-03-m55-generated-builder-struct-list-baseline-100k-g-drive-docker`](../../benchmarks/results/2026-09-03-m55-generated-builder-struct-list-baseline-100k-g-drive-docker).

| Operation | C++ ns/op | Native ns/op | Native / C++ |
| --- | ---: | ---: | ---: |
| direct two-element struct-list construction | 56.2011 | 40.3760 | 0.718 |
| generated two-element struct-list construction | 54.5440 | 339.6351 | 6.227 |

The checked lower-level Rust path is already 28.2% faster than the direct C++
control, but generated dispatch adds roughly 299 ns and loses that advantage.
The next change generates a constant-layout typed struct-list initializer and
typed element wrapper, retaining dynamic list construction only for reflection,
unions, and branded element types.

## Struct-list builder gate

Generated initializers for ordinary, unbranded struct lists now embed the
pointer slot and element layout and return a typed list whose `get()` yields
the generated element builder. Reflection, union activation, and branded
element resolution retain their dynamic paths. The checked message builder
caches the validated element widths and stride, and inlines element indexing;
generated code enters that wire builder directly instead of returning the
large list builder through an intermediate dynamic result. No unchecked code
or weaker bounds behavior is introduced.

Because the full-operation generated-minus-direct differences are close to
timer resolution, the paired harness also retains one two-element list and
measures only element selection and the two scalar writes. This isolates the
generated typed wrapper from allocation, tag emission, and zeroing.

Final five-million-operation evidence at native commit `6b8f9f4` is in
[`benchmarks/results/2026-09-03-m55-struct-list-wire-path-5m`](../../benchmarks/results/2026-09-03-m55-struct-list-wire-path-5m).

| Operation | C++ ns/op | Native ns/op | Native / C++ |
| --- | ---: | ---: | ---: |
| direct struct-list construction and writes | 57.2777 | 34.2751 | 0.598 |
| generated struct-list construction and writes | 59.4020 | 34.8792 | 0.587 |
| retained direct element access and writes | 4.6391 | 6.6911 | 1.442 |
| retained generated element access and writes | 4.5771 | 6.6868 | 1.461 |

Generated Rust is 41.3% faster than generated C++ and slightly improves on
the paired direct-runtime ratio, closing the cumulative gate. In the isolated
workload, generated and direct native medians differ by 0.0043 ns; the paired
increment is 0.0635 ns and the C++ increment is negative, so incremental
subtraction is below timer resolution. The isolated generated/direct ratio
differs by 1.3%, within the 3% tolerance, corroborating that typed wrapping
adds no material native cost. The ordinary struct-list builder gate is closed;
pointer lists, unions, defaults, and evolution builders remain open.

## Pointer-list builder baseline

The paired pointer-list workload initializes a two-element `List(Text)` in the
root's `texts` field and writes two alternating short strings on every
iteration. Both implementations use fixed-capacity one-segment arenas and
time pointer-list allocation, zeroing, list pointer emission, both Text
allocations and copies, and both element-pointer writes. Root construction is
outside the timed region.

The direct controls embed pointer slot 15 and use their checked pointer-list
builders. Generated C++ uses `initTexts()` and typed Text setters. The current
generated Rust path still looks up `texts` by name, clones and resolves list
metadata, constructs a dynamic list storage enum, and dispatches two
`DynamicInput::Text` values.

Alternating 100,000-operation baseline evidence at native commit `b767aae` is
in
[`benchmarks/results/2026-09-03-m55-generated-builder-pointer-list-baseline-100k-v2-g-drive-docker`](../../benchmarks/results/2026-09-03-m55-generated-builder-pointer-list-baseline-100k-v2-g-drive-docker).

| Operation | C++ ns/op | Native ns/op | Native / C++ |
| --- | ---: | ---: | ---: |
| direct two-element Text-list construction | 69.9918 | 65.4265 | 0.935 |
| generated two-element Text-list construction | 70.6339 | 171.3259 | 2.426 |

The checked lower-level native path is already 6.5% faster than the direct C++
control. Generated reflection adds about 106 ns and discards that advantage.
The next change emits a constant-slot typed Text-list initializer and setter,
while preserving the dynamic list builder for reflection, nested generic
lists, unions, and other runtime-selected element types.

## Pointer-list builder gate

Generated initializers for ordinary, non-union `List(Text)` and `List(Data)`
fields now embed their pointer slot and enter the checked wire builder directly.
They return typed list builders whose setters accept `&str` and `&[u8]`, so the
hot path no longer performs field-name lookup, clones list metadata, resolves a
runtime brand, constructs dynamic storage, or wraps each value in
`DynamicInput`. Reflection, unions, nested generic lists, and runtime-selected
element types retain the dynamic path.

Generated-source tests require the `texts` initializer to contain constant
pointer slot 15 and reject dynamic `init_list("texts")` dispatch. Fixture tests
round-trip typed Text and Data lists through the checked reader. Allocation,
payload copying, bounds checks, output limits, and pointer emission remain in
the existing safe runtime primitives.

Final five-million-operation evidence at native commit `e4510e0` is in
[`benchmarks/results/2026-09-03-m55-pointer-list-typed-5m`](../../benchmarks/results/2026-09-03-m55-pointer-list-typed-5m).

| Operation | C++ ns/op | Native ns/op | Native / C++ |
| --- | ---: | ---: | ---: |
| direct two-element Text-list construction | 102.4746 | 82.8371 | 0.808 |
| generated two-element Text-list construction | 108.1263 | 83.8602 | 0.776 |

Generated Rust is 22.4% faster than generated C++ and improves on the paired
direct-runtime ratio. Native generated-minus-direct is negative, while the
C++ difference is only 0.0233 ns, so the incremental comparison is below timer
resolution. The ordinary Text/Data pointer-list builder gate is closed;
unions, defaults, and evolution builders remain open.

## Union builder baseline

The paired union workload selects the scalar `choice.number` arm and writes a
pass-dependent `UInt64`. Both direct controls write discriminant offset 19 and
payload offset 6. Generated C++ uses `getChoice().setNumber()`, while generated
Rust currently obtains the group through dynamic field lookup and then sets
`number` through a second lookup, field clone, type dispatch, and union
activation.

Alternating 100,000-operation baseline evidence at native commit `395e942` is
in
[`benchmarks/results/2026-09-03-m55-generated-builder-union-baseline-100k-g-drive-docker`](../../benchmarks/results/2026-09-03-m55-generated-builder-union-baseline-100k-g-drive-docker).

| Operation | C++ ns/op | Native ns/op | Native / C++ |
| --- | ---: | ---: | ---: |
| direct scalar-union selection and write | 0.7140 | 2.2691 | 3.178 |
| generated scalar-union selection and write | 0.7140 | 148.2940 | 207.686 |

The generated Rust path adds about 145 ns per write. The lower-level operation
is smaller than a few clock cycles in C++, so its absolute ratio is not a
stable acceptance signal by itself; the final gate needs a longer run and an
isolated generated/direct comparison. The next change emits constant-layout
group access plus a union-aware scalar setter that writes the payload and tag
through checked primitives without reflection.

## Union builder gate

Generated access to non-generic groups now embeds the group type ID, and the
`UInt64` union setter embeds the discriminant offset, arm value, payload
offset, and default. It therefore bypasses both dynamic field lookups and the
runtime value switch. The checked message builder writes the discriminant and
payload through one segment access, validates both destinations before either
is changed, and proves the complete data range with one success-path bounds
check. The failure path retains the original field-specific out-of-bounds
error. No unchecked code or weaker mutation guarantee is used.

The paired harness also retains the group builder and repeats only the union
write. This removes group construction from both languages and corroborates
the generated wrapper cost when subtraction of the full operations falls
below timer resolution. Optimized disassembly shows that the retained direct
and generated Rust success loops contain the same arena lookup, range check,
two stores, and checksum instructions; their different error representations
exist only on cold branches.

Final five-million-operation evidence at native commit `e177c0e` is in
[`benchmarks/results/2026-09-04-m55-union-builder-final-5m-v3`](../../benchmarks/results/2026-09-04-m55-union-builder-final-5m-v3).

| Operation | C++ ns/op | Native ns/op | Native / C++ |
| --- | ---: | ---: | ---: |
| direct scalar-union selection and write | 0.7421 | 1.6436 | 2.215 |
| generated scalar-union selection and write | 0.7516 | 1.5669 | 2.085 |
| retained direct union write | 0.7498 | 1.6096 | 2.147 |
| retained generated union write | 0.7556 | 1.5747 | 2.084 |

Generated Rust is slightly faster than its direct Rust control in both the
full and retained workloads. Its cumulative ratio improves on the paired
direct-runtime ceiling, and both generated-minus-direct comparisons are below
timer resolution. The result closes the generated union-builder gate.

The absolute direct operation remains roughly 0.9 ns slower than C++ because
`StructBuilder` deliberately retains a relocatable arena coordinate and
revalidates the segment range, whereas the optimized C++ `AnyStruct::Builder`
loop retains a native segment pointer and hoists its `ArrayPtr` bounds. At this
sub-two-nanosecond scale the ratio magnifies a handful of instructions; the
implementation keeps the safe coordinate representation needed for arena
growth rather than introducing a cached raw pointer. Defaults and evolution
builders remain open.

## Default-value builder gate

The default-value builder workload alternates the `defaulted` field between
its schema default (`123456`) and a pass-dependent value. Both controls perform
the required XOR before storing offset 16, and the generated source embeds the
offset and default rather than consulting reflection.

The operation is only about two native nanoseconds. Optimized disassembly is
therefore used as the isolated-accessor cross-check: the direct and generated
Rust loops have instruction-identical success paths, including the same layout
check, arena segment lookup, range check, XOR, store, and checksum. Their
different `ArenaError` and `DynamicError` encodings occur only on cold error
branches.

Final five-million-operation evidence at native commit `a0fceda` is in
[`benchmarks/results/2026-09-04-m55-builder-defaults-evolution-final-5m`](../../benchmarks/results/2026-09-04-m55-builder-defaults-evolution-final-5m).

| Operation | C++ ns/op | Native ns/op | Native / C++ |
| --- | ---: | ---: | ---: |
| direct default-XOR scalar write | 1.2145 | 2.1332 | 1.756 |
| generated default-XOR scalar write | 1.0760 | 2.1281 | 1.978 |

Native generated-minus-direct is -0.0207 ns and the two native medians differ
by 0.2%. C++ generated-minus-direct is also negative, so incremental
subtraction is below resolution; the instruction-identical isolated paths
confirm that generated Rust adds no work. The apparently worse generated
cumulative ratio is the unstable quotient produced when C++ saves 0.14 ns on
a roughly one-nanosecond operation, not hidden native wrapper work. The
default-value builder gate is closed without weakening the checked setter.

## Schema-evolution builder gate

The evolution workload builds the v1 `Record` layout using the pinned v1
schema: a varying `id`, alternating known enum state, alternating Text value,
and a two-element `List(UInt32)`. The direct controls use the same one-data-word,
two-pointer layout. Both languages use fixed-capacity scratch arenas sized
identically, and every iteration covers scalar writes, Text allocation and
copy, primitive-list allocation, and element writes.

| Operation | C++ ns/op | Native ns/op | Native / C++ |
| --- | ---: | ---: | ---: |
| direct v1 record construction | 36.0133 | 48.1404 | 1.337 |
| generated v1 record construction | 34.9036 | 48.5218 | 1.390 |

The retained generated wrapper is within 0.8% of the direct native runtime.
The paired native delta is 0.8710 ns while the C++ delta is negative, making
incremental division invalid; the isolated generated/direct ratio remains
inside the 3% tolerance. Constant generated scalar, Text, and primitive-list
paths are already the same paths qualified by the preceding builder gates.
The schema-evolution builder gate is closed.

With scalar, blob, struct, primitive-list, struct-list, pointer-list, union,
default, and schema-evolution readers and builders all covered by checked-in
paired evidence, M55 is complete.
