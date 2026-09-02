#include <capnp/endian.h>

#include <bit>
#include <charconv>
#include <cstdint>
#include <iostream>
#include <string_view>
#include <vector>

namespace {

constexpr uint64_t SEED = 0x4d595df4d0f33173ull;

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
  uint64_t state = SEED ^ (wordCount * sizeof(uint64_t));
  for (auto& word: words) {
    state = xorshift(state);
    word.set(state);
  }

  uint64_t result = SEED;
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
  } else {
    std::cerr << "unknown benchmark mode\n";
    return 2;
  }
  std::cout << result << '\n';
  return 0;
}
