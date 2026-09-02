#include <capnp/endian.h>

#include <bit>
#include <charconv>
#include <chrono>
#include <cstdint>
#include <iostream>
#include <string_view>
#include <vector>

namespace {

constexpr uint64_t SEED = 0x4d595df4d0f33173ull;

// Source-derived from the translation-unit-private WirePointer in layout.c++ at
// the pinned oracle commit. Field formulas and WireValue storage are exact;
// bounded-unit wrapper types are reduced to their release-mode integers here.
struct PointerWord {
  enum Kind: uint32_t { STRUCT = 0, LIST = 1, FAR = 2, OTHER = 3 };

  capnp::_::WireValue<uint32_t> lower;
  capnp::_::WireValue<uint32_t> upper;

  Kind kind() const { return static_cast<Kind>(lower.get() & 3); }
  bool isCapability() const { return lower.get() == OTHER; }
  int32_t positionalOffset() const { return static_cast<int32_t>(lower.get()) >> 2; }
  uint16_t dataWords() const { return upper.get(); }
  uint16_t pointerCount() const { return upper.get() >> 16; }
  uint32_t elementSize() const { return upper.get() & 7; }
  uint32_t elementCount() const { return upper.get() >> 3; }
  bool isDoubleFar() const { return (lower.get() >> 2) & 1; }
  uint32_t landingPadWord() const { return lower.get() >> 3; }
  uint32_t segmentId() const { return upper.get(); }
  uint32_t capabilityIndex() const { return upper.get(); }
  uint32_t lower32() const { return lower.get(); }
  uint64_t raw() const { return lower.get() | (uint64_t{upper.get()} << 32); }

  void setRaw(uint64_t value) {
    lower.set(value);
    upper.set(value >> 32);
  }
  void setStruct(int32_t offset, uint16_t dataWords, uint16_t pointerCount) {
    lower.set(static_cast<uint32_t>(offset) << 2);
    upper.set(uint32_t{dataWords} | (uint32_t{pointerCount} << 16));
  }
  void setList(int32_t offset, uint32_t elementSize, uint32_t count) {
    lower.set((static_cast<uint32_t>(offset) << 2) | LIST);
    upper.set((count << 3) | elementSize);
  }
  void setFar(bool doubleFar, uint32_t landingPadWord, uint32_t segmentId) {
    lower.set((landingPadWord << 3) | (uint32_t{doubleFar} << 2) | FAR);
    upper.set(segmentId);
  }
  void setCapability(uint32_t index) {
    lower.set(OTHER);
    upper.set(index);
  }
};

static_assert(sizeof(PointerWord) == sizeof(uint64_t));

size_t parseSize(const char* text) {
  size_t result = 0;
  auto input = std::string_view(text);
  auto parsed = std::from_chars(input.begin(), input.end(), result);
  if (parsed.ec != std::errc() || parsed.ptr != input.end() || result == 0) {
    throw std::invalid_argument("sizes must be positive integers");
  }
  return result;
}

uint64_t xorshift(uint64_t value) {
  value ^= value << 13;
  value ^= value >> 7;
  value ^= value << 17;
  return value;
}

uint64_t checksum(const std::vector<capnp::_::WireValue<uint64_t>>& words) {
  uint64_t result = SEED;
  for (const auto& word: words) {
    result = std::rotl(result, 7) ^ word.get();
  }
  return result;
}

uint64_t checksumPointers(const std::vector<PointerWord>& pointers) {
  uint64_t result = SEED;
  for (const auto& pointer: pointers) {
    result = std::rotl(result, 7) ^ pointer.raw();
  }
  return result;
}

}  // namespace

int main(int argc, char** argv) {
  if (argc != 4) {
    std::cerr << "usage: cpp-wire-value-benchmark MODE WORDS PASSES\n";
    return 2;
  }
  auto mode = std::string_view(argv[1]);
  auto wordCount = parseSize(argv[2]);
  auto passes = parseSize(argv[3]);
  std::vector<capnp::_::WireValue<uint64_t>> words(wordCount);
  std::vector<PointerWord> pointers(wordCount);
  uint64_t state = SEED ^ (wordCount * sizeof(uint64_t));
  for (auto& word: words) {
    state = xorshift(state);
    word.set(state);
  }
  state = SEED ^ (wordCount * sizeof(uint64_t));
  for (auto& pointer: pointers) {
    state = xorshift(state);
    pointer.setRaw(state);
  }

  uint64_t result = SEED;
  auto started = std::chrono::steady_clock::now();
  if (mode == "read") {
    for (size_t pass = 0; pass < passes; ++pass) {
      for (const auto& word: words) {
        result = std::rotl(result, 7) ^ word.get();
      }
    }
  } else if (mode == "write") {
    state = SEED;
    for (size_t pass = 0; pass < passes; ++pass) {
      for (auto& word: words) {
        state = xorshift(state);
        word.set(state);
      }
    }
    result = checksum(words);
  } else if (mode == "pointer-decode") {
    for (size_t pass = 0; pass < passes; ++pass) {
      for (const auto& pointer: pointers) {
        uint64_t fingerprint;
        switch (pointer.kind()) {
          case PointerWord::STRUCT:
            fingerprint = static_cast<uint32_t>(pointer.positionalOffset()) ^
                std::rotl(uint64_t{pointer.dataWords()}, 17) ^
                std::rotl(uint64_t{pointer.pointerCount()}, 41);
            break;
          case PointerWord::LIST:
            fingerprint = static_cast<uint32_t>(pointer.positionalOffset()) ^
                std::rotl(uint64_t{pointer.elementSize()}, 13) ^
                std::rotl(uint64_t{pointer.elementCount()}, 29);
            break;
          case PointerWord::FAR:
            fingerprint = pointer.landingPadWord() ^
                std::rotl(uint64_t{pointer.isDoubleFar()}, 17) ^
                std::rotl(uint64_t{pointer.segmentId()}, 31);
            break;
          case PointerWord::OTHER:
            fingerprint = pointer.isCapability()
                ? std::rotl(uint64_t{pointer.capabilityIndex()}, 23)
                : pointer.lower32();
            break;
        }
        result = std::rotl(result, 7) ^ fingerprint;
      }
    }
  } else if (mode == "pointer-encode") {
    state = SEED;
    for (size_t pass = 0; pass < passes; ++pass) {
      size_t index = 0;
      for (auto& pointer: pointers) {
        state = xorshift(state);
        auto lower = static_cast<uint32_t>(state);
        auto upper = static_cast<uint32_t>(state >> 32);
        switch (index++ & 3) {
          case 0: pointer.setStruct(static_cast<int32_t>(lower) >> 2, upper, upper >> 16); break;
          case 1: pointer.setList(static_cast<int32_t>(lower) >> 2, upper & 7, upper >> 3); break;
          case 2: pointer.setFar(lower & 4, lower >> 3, upper); break;
          case 3: pointer.setCapability(upper); break;
        }
      }
    }
    result = checksumPointers(pointers);
  } else {
    std::cerr << "unknown benchmark mode\n";
    return 2;
  }
  auto elapsed = std::chrono::duration_cast<std::chrono::nanoseconds>(
      std::chrono::steady_clock::now() - started);
  std::cout << elapsed.count() << '\t' << result << '\n';
  return 0;
}
