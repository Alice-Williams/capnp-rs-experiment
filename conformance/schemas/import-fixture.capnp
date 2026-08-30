@0xc0dcf61499d65f2a;

using Wire = import "wire-fixture.capnp";
using Language = import "language-fixture.capnp";

struct ImportFixture @0xe8a7d52207520de1 {
  wire @0 :Wire.WireFixture;
  language @1 :Language.LanguageFixture;
  service @2 :Language.GenericService(Text);
}
