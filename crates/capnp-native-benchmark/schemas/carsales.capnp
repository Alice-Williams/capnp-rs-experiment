# Derived from the pinned C++ benchmark schema at commit
# e7c9cd96f1505b5ae486db7821006c2f5dce5b5b. The C++-only namespace annotation
# is omitted; the file ID, declarations, ordinals, and wire layout are unchanged.

@0xff75ddc6a36723c9;

struct ParkingLot {
  cars @0 :List(Car);
}

struct TotalValue {
  amount @0 :UInt64;
}

struct Car {
  make @0 :Text;
  model @1 :Text;
  color @2 :Color;
  seats @3 :UInt8;
  doors @4 :UInt8;
  wheels @5 :List(Wheel);
  length @6 :UInt16;
  width @7 :UInt16;
  height @8 :UInt16;
  weight @9 :UInt32;
  engine @10 :Engine;
  fuelCapacity @11 :Float32;
  fuelLevel @12 :Float32;
  hasPowerWindows @13 :Bool;
  hasPowerSteering @14 :Bool;
  hasCruiseControl @15 :Bool;
  cupHolders @16 :UInt8;
  hasNavSystem @17 :Bool;
}

enum Color {
  black @0;
  white @1;
  red @2;
  green @3;
  blue @4;
  cyan @5;
  magenta @6;
  yellow @7;
  silver @8;
}

struct Wheel {
  diameter @0 :UInt16;
  airPressure @1 :Float32;
  snowTires @2 :Bool;
}

struct Engine {
  horsepower @0 :UInt16;
  cylinders @1 :UInt8;
  cc @2 :UInt32;
  usesGas @3 :Bool;
  usesElectric @4 :Bool;
}
