# Derived from the pinned C++ benchmark schema at commit
# e7c9cd96f1505b5ae486db7821006c2f5dce5b5b. The C++-only namespace annotation
# is omitted; the file ID, declarations, ordinals, and wire layout are unchanged.

@0xe12dc4c3e70e9eda;

enum Operation {
  add @0;
  subtract @1;
  multiply @2;
  divide @3;
  modulus @4;
}

struct Expression {
  op @0 :Operation;

  left :union {
    value @1 :Int32;
    expression @2 :Expression;
  }

  right :union {
    value @3 :Int32;
    expression @4 :Expression;
  }
}

struct EvaluationResult {
  value @0 :Int32;
}
