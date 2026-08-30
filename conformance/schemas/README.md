# Project-owned conformance schemas

- `wire-fixture.capnp` exercises every scalar, blob, list element width,
  structs, nested lists, AnyPointer, capabilities, unions, groups, defaults,
  recursive pointers, and zero-sized struct lists.
- `language-fixture.capnp` exercises annotations, generics, nested generic
  scopes, aliases, constants, enums, interfaces, inheritance, and generic
  methods.
- `evolution-v1.capnp`, `evolution-v2.capnp`, and `evolution-v3.capnp` retain
  file/type IDs while appending fields/enumerants, adding union/group members,
  growing structs, exercising primitive-list-to-struct-list upgrades, renaming
  declarations without changing IDs/ordinals, moving an existing field into a
  union, and making an existing type generic by appending a parameter field.
- `import-fixture.capnp` exercises relative imports and cross-file generic
  references.
- `streaming-fixture.capnp` exercises streaming methods, backpressure-shaped
  results, capability returns, and lists of data chunks.

The pinned C++ compiler is the final acceptance oracle. The container's system
compiler may be used as an early syntax check, but its version is not provenance
for committed generated fixtures.

Run that early check inside the development container with:

```console
bash tools/check-schema-syntax.sh
```
