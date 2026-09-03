@0xd9e8b6678c7a1f45;

# A fixed two-word payload used only to make the pinned C++ orphan allocator
# expose a schema-independent double-far adoption path.
struct BuildRoot {
  first @0 :UInt64;
  second @1 :UInt64;
}
