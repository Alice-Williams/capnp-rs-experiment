@0xd9aecdd76e564c31;

struct BuilderFixture {
  id @0 :UInt64;
  name @1 :Text;
  payload @2 :Data;
  numbers @3 :List(UInt16);
  labels @4 :List(Text);
  child @5 :Child;
  children @6 :List(Child);
  nested @7 :List(List(UInt16));

  struct Child {
    value @0 :UInt32;
    note @1 :Text;
  }
}
