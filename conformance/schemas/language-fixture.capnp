@0xcd6bfdd088fa4545;

annotation fixtureTag @0xed8d2c51fd2ff6dd (*) :Text;

$fixtureTag("language-fixture");

struct Box @0x82b15e53797a8580 (T) $fixtureTag("generic-box") {
  value @0 :T;

  struct Pair(U) {
    first @0 :T;
    second @1 :U;
  }
}

using TextBox = Box(Text);

struct LanguageFixture @0xb4c435b21aa9b116 {
  boxedText @0 :TextBox;
  boxedData @1 :Box(Data);
  nestedGeneric @2 :Box(Text).Pair(Data);
  anyPointer @3 :AnyPointer;

  enum State {
    unknown @0;
    ready @1 $fixtureTag("ready-state");
    future @2;
  }

  state @4 :State = ready;
  const answer :UInt64 = 42;
  const greeting :Text = "hello";
  const signature :Data = 0x"00cafeff";
  const primes :List(UInt16) = [2, 3, 5, 7, 11];
  const sampleBox :Box(Text) = (value = "constant generic struct");
}

interface BaseService @0xb02e0a639958c628 {
  ping @0 () -> (value :UInt32);
}

interface GenericService @0xa452e51fe34f10ac (T) extends(BaseService) {
  get @0 (key :Text) -> (value :T);
  set @1 (key :Text, value :T) -> ();
  transform @2 [U] (value :U) -> (result :Box(U));
}
