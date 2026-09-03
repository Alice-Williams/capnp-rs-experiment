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
