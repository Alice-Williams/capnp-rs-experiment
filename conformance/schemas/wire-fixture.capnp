@0xe267bb828280f0a2;

enum Color @0xd5e4ed5f9f36445f {
  red @0;
  green @1;
  blue @2;
}

struct Empty @0xde63f394299a28cc {}

struct Node @0xae37c0cc5acf02c6 {
  value @0 :UInt32;
  next @1 :Node;
}

interface Callback @0xbd62ac775e48d993 {
  notify @0 (value :UInt64) -> (accepted :Bool);
}

struct WireFixture @0x99c9abad73963922 {
  voidValue @0 :Void;
  boolValue @1 :Bool;
  int8Value @2 :Int8;
  int16Value @3 :Int16;
  int32Value @4 :Int32;
  int64Value @5 :Int64;
  uint8Value @6 :UInt8;
  uint16Value @7 :UInt16;
  uint32Value @8 :UInt32;
  uint64Value @9 :UInt64;
  float32Value @10 :Float32;
  float64Value @11 :Float64;
  color @12 :Color;
  text @13 :Text;
  data @14 :Data;
  empty @15 :Empty;

  bools @16 :List(Bool);
  int8s @17 :List(Int8);
  int16s @18 :List(Int16);
  int32s @19 :List(Int32);
  int64s @20 :List(Int64);
  uint8s @21 :List(UInt8);
  uint16s @22 :List(UInt16);
  uint32s @23 :List(UInt32);
  uint64s @24 :List(UInt64);
  float32s @25 :List(Float32);
  float64s @26 :List(Float64);
  colors @27 :List(Color);
  texts @28 :List(Text);
  dataBlobs @29 :List(Data);
  structs @30 :List(Node);
  nestedLists @31 :List(List(UInt16));
  anyPointer @32 :AnyPointer;
  callback @33 :Callback;

  choice :union {
    none @34 :Void;
    number @35 :UInt64;
    words @36 :List(Text);
  }

  metadata :group {
    created @37 :UInt64;
    valid @38 :Bool;
  }

  node @39 :Node;
  callbacks @40 :List(Callback);
  emptyStructs @41 :List(Empty);
  defaulted @42 :UInt32 = 123456;
  defaultText @43 :Text = "default text";
  anyStruct @44 :AnyStruct;
  anyList @45 :AnyList;
}
