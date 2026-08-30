@0xcf5b582d672451a9;

struct OrphanFixture {
  oldChild @0 :Child;
  newChild @1 :Child;
  oldValues @2 :List(UInt16);
  newValues @3 :List(UInt16);

  struct Child {
    value @0 :UInt32;
    note @1 :Text;
  }
}
