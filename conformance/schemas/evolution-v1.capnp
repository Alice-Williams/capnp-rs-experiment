@0xa953d29c91bd70e5;

enum State @0xa19b291b66f957dc {
  unknown @0;
  active @1;
}

struct Item @0xba07e32b8a341317 {
  value @0 :UInt32;
}

struct Record @0x81787eedde27c411 {
  id @0 :UInt32;
  name @1 :Text;
  state @2 :State;
  values @3 :List(UInt32);
}
