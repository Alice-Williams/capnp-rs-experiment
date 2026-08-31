# Derived from the Cap'n Proto C++ sample at pinned commit
# e7c9cd96f1505b5ae486db7821006c2f5dce5b5b under the MIT license.

@0x9eb32e19f86ee174;

struct Person @0x98808e9832e8bc18 {
  id @0 :UInt32;
  name @1 :Text;
  email @2 :Text;
  phones @3 :List(PhoneNumber);

  struct PhoneNumber @0x814e90b29c9e8ad0 {
    number @0 :Text;
    type @1 :Type;

    enum Type @0x91e0bd04d585062f {
      mobile @0;
      home @1;
      work @2;
    }
  }

  employment :union {
    unemployed @4 :Void;
    employer @5 :Text;
    school @6 :Text;
    selfEmployed @7 :Void;
  }
}

struct AddressBook @0xf934d9b354a8a134 {
  people @0 :List(Person);
}
