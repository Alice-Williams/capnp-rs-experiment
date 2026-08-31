@0xd8f6f071b3dbb2ab;

using Json = import "/capnp/compat/json.capnp";

enum Tone {
  quiet @0;
  loud @1 $Json.name("LOUD");
}

struct Detail {
  count @0 :UInt16;
  displayName @1 :Text $Json.name("display_name");
}

struct JsonFixture $Json.discriminator(name = "kind", valueName = "value") {
  renamed @0 :Text $Json.name("external_name");
  encoded @1 :Data $Json.base64;
  hexed @2 :Data $Json.hex;
  detail @3 :Detail $Json.flatten(prefix = "detail_");
  tone @4 :Tone;

  union {
    none @5 :Void;
    amount @6 :Int64 $Json.name("amount64");
  }
}
