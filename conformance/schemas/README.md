# Project-owned conformance schemas

- `wire-fixture.capnp` exercises every scalar, blob, list element width,
  structs, nested lists, AnyPointer, capabilities, unions, groups, defaults,
  recursive pointers, and zero-sized struct lists.
- `language-fixture.capnp` exercises annotations, generics, nested generic
  scopes, aliases, constants, enums, interfaces, inheritance, and generic
  methods.
- `evolution-v1.capnp` and `evolution-v2.capnp` retain file/type IDs while
  appending fields/enumerants, adding union/group members, growing structs, and
  exercising the primitive-list-to-struct-list upgrade representation.
- `import-fixture.capnp` exercises relative imports and cross-file generic
  references.

The pinned C++ compiler is the final acceptance oracle. The container's system
compiler may be used as an early syntax check, but its version is not provenance
for committed generated fixtures.

Run that early check inside the development container with:

```console
bash tools/check-schema-syntax.sh
```
