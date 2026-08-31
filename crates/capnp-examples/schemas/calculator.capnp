# Derived from the Cap'n Proto C++ sample at pinned commit
# e7c9cd96f1505b5ae486db7821006c2f5dce5b5b under the MIT license.

@0x85150b117366d14b;

interface Calculator @0x97983392df35cc36 {
  evaluate @0 (expression :Expression) -> (value :Value);

  struct Expression @0xd438d7caf5548d15 {
    union {
      literal @0 :Float64;
      previousResult @1 :Value;
      parameter @2 :UInt32;
      call :group {
        function @3 :Function;
        params @4 :List(Expression);
      }
    }
  }

  interface Value @0xc3e69d34d3ee48d2 {
    read @0 () -> (value :Float64);
  }

  defFunction @1 (paramCount :Int32, body :Expression) -> (func :Function);

  interface Function @0xede83a3d96840394 {
    call @0 (params :List(Float64)) -> (value :Float64);
  }

  getOperator @2 (op :Operator) -> (func :Function);

  enum Operator @0x8793407861e6dfe6 {
    add @0;
    subtract @1;
    multiply @2;
    divide @3;
  }
}
