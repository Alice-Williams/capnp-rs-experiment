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

#include "message_build.capnp.h"

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

template <size_t N>
__attribute__((always_inline)) inline uint64_t hashPrepared(
    const std::array<capnp::word, N>& words, size_t wordCount,
    size_t segmentCount) {
  uint64_t hash = SEED ^ std::rotl(uint64_t{segmentCount}, 17)
      ^ std::rotl(uint64_t{wordCount}, 31);
  auto bytes = reinterpret_cast<const capnp::byte*>(words.data());
  for (size_t index = 0; index < wordCount; ++index) {
    hash = std::rotl(hash, 7) ^ readWord(bytes, index);
  }
  return hash;
}

kj::Array<capnp::word> makeGraph() {
  auto words = kj::heapArray<capnp::word>(11);
  std::memset(words.begin(), 0, words.asBytes().size());
  auto bytes = reinterpret_cast<capnp::byte*>(words.begin());
  setWord(bytes, 0, uint64_t{0x00010001} << 32);
  setWord(bytes, 1, VALUE);
  setWord(bytes, 2, (uint64_t{514} << 32) | 1);
  for (size_t index = 0; index < 64; ++index) {
    bytes[24 + index] = static_cast<capnp::byte>(index ^ 0xa5);
  }
  return words;
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
    unsigned int shape, uint64_t first, uint64_t second) {
  std::array<capnp::word, 5> words{};
  auto bytes = reinterpret_cast<capnp::byte*>(words.data());
  if (shape == 2) {
    // Three conceptual segments [1, 2, 2] with a double-far root.
    setWord(bytes, 0, (uint64_t{2} << 32) | 6);
    setWord(bytes, 1, first);
    setWord(bytes, 2, second);
    setWord(bytes, 3, (uint64_t{1} << 32) | 2);
    setWord(bytes, 4, uint64_t{2} << 32);
    return hashPrepared(words, 5, 3);
  }
  if (shape == 1) {
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
    unsigned int shape, uint64_t first, uint64_t second) {
  capnp::MallocMessageBuilder message(
      shape == 0 ? 3 : 1, capnp::AllocationStrategy::FIXED_SIZE);
  if (shape == 2) {
    auto orphan = message.getOrphanage().newOrphan<BuildRoot>();
    auto root = orphan.get();
    root.setFirst(first);
    root.setSecond(second);
    message.adoptRoot(kj::mv(orphan));
    return hashSegments(message.getSegmentsForOutput());
  }
  auto root = message.getRoot<capnp::AnyPointer>().initAsAnyStruct(2, 0);
  auto data = root.getDataSection();
  setWord(data.begin(), 0, first);
  setWord(data.begin(), 1, second);
  return hashSegments(message.getSegmentsForOutput());
}

__attribute__((noinline)) uint64_t reuseIteration(
    std::array<capnp::word, 3>& scratch, uint64_t first, uint64_t second) {
  uint64_t fingerprint;
  {
    capnp::MallocMessageBuilder message(
        kj::arrayPtr(scratch.data(), scratch.size()),
        capnp::AllocationStrategy::FIXED_SIZE);
    auto root = message.getRoot<capnp::AnyPointer>().initAsAnyStruct(2, 0);
    auto data = root.getDataSection();
    setWord(data.begin(), 0, first);
    setWord(data.begin(), 1, second);
    fingerprint = hashSegments(message.getSegmentsForOutput());
  }
  return fingerprint;
}

__attribute__((noinline)) uint64_t copyPreparedIteration(
    kj::ArrayPtr<const capnp::word> source) {
  alignas(capnp::word) std::array<capnp::byte, 88> output{};
  std::memcpy(output.data(), source.begin(), output.size());
  uint64_t hash = SEED ^ std::rotl(uint64_t{1}, 17)
      ^ std::rotl(uint64_t{11}, 31);
  for (size_t index = 0; index < 11; ++index) {
    hash = std::rotl(hash, 7) ^ readWord(output.data(), index);
  }
  return hash;
}

__attribute__((noinline)) uint64_t copyIteration(capnp::AnyPointer::Reader source) {
  capnp::MallocMessageBuilder message(
      11, capnp::AllocationStrategy::FIXED_SIZE);
  message.setRoot(source);
  return hashSegments(message.getSegmentsForOutput());
}

__attribute__((noinline)) uint64_t copyReuseIteration(
    std::array<capnp::word, 11>& scratch, capnp::AnyPointer::Reader source) {
  uint64_t fingerprint;
  {
    capnp::MallocMessageBuilder message(
        kj::arrayPtr(scratch.data(), scratch.size()),
        capnp::AllocationStrategy::FIXED_SIZE);
    message.setRoot(source);
    fingerprint = hashSegments(message.getSegmentsForOutput());
  }
  return fingerprint;
}

}  // namespace

int main(int argc, char** argv) {
  if (argc != 4) {
    std::cerr << "usage: cpp-message-build prepared|fresh|reuse|copy-prepared|copy|copy-reuse direct|far|double-far|graph PASSES\n";
    return 2;
  }
  auto mode = std::string_view(argv[1]);
  auto shape = std::string_view(argv[2]);
  auto passes = parseSize(argv[3]);
  auto copyMode = mode == "copy-prepared" || mode == "copy" || mode == "copy-reuse";
  if ((mode != "prepared" && mode != "fresh" && mode != "reuse" && !copyMode)
      || (shape != "direct" && shape != "far" && shape != "double-far"
          && shape != "graph")
      || (mode == "reuse" && shape != "direct")
      || (copyMode != (shape == "graph"))) {
    std::cerr << "unknown benchmark mode or shape\n";
    return 2;
  }

  auto shapeId = shape == "direct" ? 0u : shape == "far" ? 1u : shape == "double-far" ? 2u : 3u;
  uint64_t semantic = SEED;
  uint64_t wire = SEED;
  std::array<capnp::word, 3> scratch{};
  std::array<capnp::word, 11> copyScratch{};
  auto graph = makeGraph();
  auto graphView = kj::arrayPtr(
      static_cast<const capnp::word*>(graph.begin()), graph.size());
  capnp::ReaderOptions options;
  options.traversalLimitInWords = kj::maxValue;
  capnp::SegmentArrayMessageReader graphReader(kj::arrayPtr(&graphView, 1), options);
  auto graphRoot = graphReader.getRoot<capnp::AnyPointer>();
  auto started = std::chrono::steady_clock::now();
  for (size_t pass = 0; pass < passes; ++pass) {
    auto first = VALUE ^ uint64_t{pass};
    auto second = std::rotl(first, 23);
    uint64_t fingerprint;
    if (mode == "prepared") {
      fingerprint = preparedIteration(shapeId, first, second);
    } else if (mode == "reuse") {
      fingerprint = reuseIteration(scratch, first, second);
    } else if (mode == "copy-prepared") {
      fingerprint = copyPreparedIteration(graphView);
    } else if (mode == "copy") {
      fingerprint = copyIteration(graphRoot);
    } else if (mode == "copy-reuse") {
      fingerprint = copyReuseIteration(copyScratch, graphRoot);
    } else {
      fingerprint = freshIteration(shapeId, first, second);
    }
    semantic = std::rotl(semantic, 9) ^ first ^ std::rotl(second, 13);
    wire = std::rotl(wire, 11) ^ fingerprint;
  }
  auto elapsed = std::chrono::duration_cast<std::chrono::nanoseconds>(
      std::chrono::steady_clock::now() - started);
  if (mode == "reuse") {
    auto bytes = reinterpret_cast<const capnp::byte*>(scratch.data());
    if (readWord(bytes, 0) != 0 || readWord(bytes, 1) != 0
        || readWord(bytes, 2) != 0) {
      std::cerr << "scratch storage was not cleared\n";
      return 1;
    }
  }
  if (mode == "copy-reuse") {
    auto bytes = reinterpret_cast<const capnp::byte*>(copyScratch.data());
    for (size_t index = 0; index < copyScratch.size(); ++index) {
      if (readWord(bytes, index) != 0) {
        std::cerr << "copy scratch storage was not cleared\n";
        return 1;
      }
    }
  }
  asm volatile("" : "+r"(semantic), "+r"(wire) : : "memory");
  std::cout << elapsed.count() << '\t' << semantic << '\t' << wire << '\n';
  return 0;
}
