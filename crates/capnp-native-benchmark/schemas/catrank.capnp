# Derived from the pinned C++ benchmark schema at commit
# e7c9cd96f1505b5ae486db7821006c2f5dce5b5b. The C++-only namespace annotation
# is omitted; the file ID, declarations, ordinals, and wire layout are unchanged.

@0x82beb8e37ff79aba;

struct SearchResultList {
  results @0 :List(SearchResult);
}

struct SearchResult {
  url @0 :Text;
  score @1 :Float64;
  snippet @2 :Text;
}
