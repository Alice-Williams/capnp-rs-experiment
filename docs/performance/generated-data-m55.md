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

1. direct checked runtime reads from already-opened roots;
2. generated hot reads from the same pinned message;
3. direct checked construction with constant layout;
4. generated construction with identical values and allocation policy.

Scalar and blob cases are recorded independently first because they exercise
the two dominant mechanisms: repeated reflection/retained-reader overhead and
owned blob conversion. Typed lists, nested structs/groups, unions/defaults, and
evolution cases extend the same harness after those floors are stable.

No implementation path changes before this unmodified baseline is checked in.
