@0xdfc5c5c07458d5ad;

interface ByteStream @0xaedc8f2ab4a9ab7a {
  write @0 (bytes :Data) -> stream;
  writeMany @1 (chunks :List(Data)) -> stream;
  finish @2 () -> (totalBytes :UInt64);
}

interface StreamFactory @0xc57d10e05eed0e42 {
  open @0 (name :Text) -> (stream :ByteStream);
}
