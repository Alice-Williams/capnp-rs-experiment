#include <capnp/any.h>
#include <capnp/endian.h>
#include <capnp/message.h>

#include <array>
#include <bit>
#include <charconv>
#include <chrono>
#include <cstdint>
#include <cstring>
#include <iostream>
#include <stdexcept>
#include <string_view>

namespace {

constexpr uint64_t SEED = 0x4d595df4d0f33173ull;
constexpr uint64_t VALUE = 0x0123456789abcdefull;

size_t parseSize(const char* text) {
  size_t result = 0;
  auto input = std::string_view(text);
  auto parsed = std::from_chars(input.begin(), input.end(), result);
  if (parsed.ec != std::errc() || parsed.ptr != input.end() || result == 0) {
    throw std::invalid_argument("passes must be a positive integer");
  }
  return result;
}

void setWord(capnp::byte* bytes, size_t index, uint64_t value) {
  capnp::_::WireValue<uint64_t> encoded;
  encoded.set(value);
  std::memcpy(bytes + index * sizeof(uint64_t), &encoded, sizeof(encoded));
}

__attribute__((always_inline)) inline uint64_t readWord(
    const capnp::byte* bytes, size_t index) {
  capnp::_::WireValue<uint64_t> encoded;
  auto pointer = bytes;
  asm volatile("" : "+r"(pointer) : : "memory");
  std::memcpy(&encoded, pointer + index * sizeof(uint64_t), sizeof(encoded));
  return encoded.get();
}

__attribute__((always_inline)) inline uint64_t hashPrepared(
    const std::array<capnp::word, 4>& words, size_t wordCount,
    size_t segmentCount) {
  uint64_t hash = SEED ^ std::rotl(uint64_t{segmentCount}, 17)
      ^ std::rotl(uint64_t{wordCount}, 31);
  auto bytes = reinterpret_cast<const capnp::byte*>(words.data());
  for (size_t index = 0; index < wordCount; ++index) {
    hash = std::rotl(hash, 7) ^ readWord(bytes, index);
  }
  return hash;
}

__attribute__((always_inline)) inline uint64_t hashSegments(
    kj::ArrayPtr<const kj::ArrayPtr<const capnp::word>> segments) {
  uint64_t hash = SEED ^ std::rotl(uint64_t{segments.size()}, 17);
  for (size_t index = 0; index < segments.size(); ++index) {
    hash ^= std::rotl(uint64_t{index}, 7)
        ^ std::rotl(uint64_t{segments[index].size()}, 31);
    auto bytes = segments[index].asBytes();
    for (size_t word = 0; word < segments[index].size(); ++word) {
      hash = std::rotl(hash, 7) ^ readWord(bytes.begin(), word);
    }
  }
  return hash;
}

__attribute__((noinline)) uint64_t preparedIteration(
    bool far, uint64_t first, uint64_t second) {
  std::array<capnp::word, 4> words{};
  auto bytes = reinterpret_cast<capnp::byte*>(words.data());
  if (far) {
    // A matched prepared-storage lower case: four observable wire words split
    // conceptually as [1, 3], without arena allocation or object placement.
    setWord(bytes, 0, (uint64_t{1} << 32) | 2);
    setWord(bytes, 1, uint64_t{2} << 32);
    setWord(bytes, 2, first);
    setWord(bytes, 3, second);
    return hashPrepared(words, 4, 2);
  }
  setWord(bytes, 0, uint64_t{2} << 32);
  setWord(bytes, 1, first);
  setWord(bytes, 2, second);
  return hashPrepared(words, 3, 1);
}

__attribute__((noinline)) uint64_t freshIteration(
    bool far, uint64_t first, uint64_t second) {
  capnp::MallocMessageBuilder message(
      far ? 1 : 3, capnp::AllocationStrategy::FIXED_SIZE);
  auto root = message.getRoot<capnp::AnyPointer>().initAsAnyStruct(2, 0);
  auto data = root.getDataSection();
  setWord(data.begin(), 0, first);
  setWord(data.begin(), 1, second);
  return hashSegments(message.getSegmentsForOutput());
}

}  // namespace

int main(int argc, char** argv) {
  if (argc != 4) {
    std::cerr << "usage: cpp-message-build prepared|fresh direct|far PASSES\n";
    return 2;
  }
  auto mode = std::string_view(argv[1]);
  auto shape = std::string_view(argv[2]);
  auto passes = parseSize(argv[3]);
  if ((mode != "prepared" && mode != "fresh")
      || (shape != "direct" && shape != "far")) {
    std::cerr << "unknown benchmark mode or shape\n";
    return 2;
  }

  auto far = shape == "far";
  uint64_t semantic = SEED;
  uint64_t wire = SEED;
  auto started = std::chrono::steady_clock::now();
  for (size_t pass = 0; pass < passes; ++pass) {
    auto first = VALUE ^ uint64_t{pass};
    auto second = std::rotl(first, 23);
    auto fingerprint = mode == "prepared"
        ? preparedIteration(far, first, second)
        : freshIteration(far, first, second);
    semantic = std::rotl(semantic, 9) ^ first ^ std::rotl(second, 13);
    wire = std::rotl(wire, 11) ^ fingerprint;
  }
  auto elapsed = std::chrono::duration_cast<std::chrono::nanoseconds>(
      std::chrono::steady_clock::now() - started);
  asm volatile("" : "+r"(semantic), "+r"(wire) : : "memory");
  std::cout << elapsed.count() << '\t' << semantic << '\t' << wire << '\n';
  return 0;
}
