#include <capnp/endian.h>
#include <capnp/serialize.h>
#include <kj/io.h>

#include <bit>
#include <array>
#include <charconv>
#include <chrono>
#include <cstring>
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

std::vector<kj::Array<capnp::word>> makeSegments(size_t count) {
  std::vector<size_t> sizes;
  if (count == 1) sizes = {8};
  else if (count == 2) sizes = {3, 5};
  else if (count == 64) sizes.assign(64, 1);
  else throw std::invalid_argument("segment count must be 1, 2, or 64");

  uint64_t state = SEED ^ count;
  std::vector<kj::Array<capnp::word>> result;
  result.reserve(count);
  for (auto size: sizes) {
    auto segment = kj::heapArray<capnp::word>(size);
    auto wire = reinterpret_cast<capnp::_::WireValue<uint64_t>*>(segment.begin());
    for (size_t i = 0; i < size; ++i) {
      state = xorshift(state);
      wire[i].set(state);
    }
    result.push_back(kj::mv(segment));
  }
  return result;
}

std::vector<kj::ArrayPtr<const capnp::word>> segmentViews(
    const std::vector<kj::Array<capnp::word>>& segments) {
  std::vector<kj::ArrayPtr<const capnp::word>> result;
  result.reserve(segments.size());
  for (const auto& segment: segments) result.push_back(segment.asPtr());
  return result;
}

uint64_t parseMany(kj::ArrayPtr<const capnp::word> encoded, size_t segmentCount, size_t passes) {
  uint64_t checksum = SEED;
  for (size_t pass = 0; pass < passes; ++pass) {
    capnp::FlatArrayMessageReader reader(encoded);
    auto encodedBytes = static_cast<size_t>(reader.getEnd() - encoded.begin()) * 8;
    auto fingerprint = uint64_t{segmentCount}
        ^ std::rotl(uint64_t{segmentCount / 2 + 1} * 8, 11)
        ^ std::rotl(uint64_t{encodedBytes}, 23);
    for (size_t index = 0; index < segmentCount; ++index) {
      auto segment = reader.getSegment(index);
      auto bytes = segment.asBytes();
      fingerprint = std::rotl(fingerprint, 9)
          ^ uint64_t{index}
          ^ std::rotl(uint64_t{segment.size()}, 13)
          ^ std::rotl(uint64_t{bytes[0]}, 29)
          ^ std::rotl(uint64_t{bytes[bytes.size() - 1]}, 47);
    }
    checksum = std::rotl(checksum, 9) ^ fingerprint;
  }
  return checksum;
}

uint64_t parseNoAllocMany(
    kj::ArrayPtr<const capnp::word> encoded, size_t expectedCount, size_t passes) {
  uint64_t checksum = SEED;
  std::array<kj::ArrayPtr<const capnp::word>, 64> segments;
  for (size_t pass = 0; pass < passes; ++pass) {
    auto table = reinterpret_cast<const capnp::_::WireValue<uint32_t>*>(encoded.begin());
    auto segmentCount = size_t{table[0].get()} + 1;
    if (segmentCount != expectedCount || segmentCount > segments.size()) {
      throw std::invalid_argument("unexpected segment count");
    }
    size_t offset = segmentCount / 2 + 1;
    if (offset > encoded.size()) throw std::invalid_argument("truncated table");
    uint64_t totalWords = 0;
    for (size_t index = 0; index < segmentCount; ++index) {
      auto words = uint64_t{table[index + 1].get()};
      if (words > UINT64_MAX - totalWords) throw std::overflow_error("word count overflow");
      totalWords += words;
      if (totalWords > 8 * 1024 * 1024) throw std::invalid_argument("message too large");
    }
    if (totalWords > encoded.size() - offset) throw std::invalid_argument("truncated body");
    for (size_t index = 0; index < segmentCount; ++index) {
      auto words = size_t{table[index + 1].get()};
      segments[index] = encoded.slice(offset, offset + words);
      offset += words;
    }
    // Match Rust's black_box barrier: caller-storage descriptor writes must
    // materialize before the consuming checksum loop.
    asm volatile("" : : "r"(segments.data()) : "memory");
    auto fingerprint = uint64_t{segmentCount}
        ^ std::rotl(uint64_t{segmentCount / 2 + 1} * 8, 11)
        ^ std::rotl(uint64_t{offset * 8}, 23);
    for (size_t index = 0; index < segmentCount; ++index) {
      auto segment = segments[index];
      auto bytes = segment.asBytes();
      fingerprint = std::rotl(fingerprint, 9)
          ^ uint64_t{index}
          ^ std::rotl(uint64_t{segment.size()}, 13)
          ^ std::rotl(uint64_t{bytes[0]}, 29)
          ^ std::rotl(uint64_t{bytes[bytes.size() - 1]}, 47);
    }
    checksum = std::rotl(checksum, 9) ^ fingerprint;
  }
  return checksum;
}

uint64_t encodeMany(
    kj::ArrayPtr<const kj::ArrayPtr<const capnp::word>> segments, size_t passes) {
  uint64_t checksum = SEED;
  for (size_t pass = 0; pass < passes; ++pass) {
    auto encoded = capnp::messageToFlatArray(segments);
    auto bytes = encoded.asBytes();
    auto fingerprint = uint64_t{bytes.size()}
        ^ std::rotl(uint64_t{bytes[0]}, 7)
        ^ std::rotl(uint64_t{bytes[bytes.size() - 1]}, 19);
    checksum = std::rotl(checksum, 9) ^ fingerprint;
  }
  return checksum;
}

uint64_t streamReadMany(kj::ArrayPtr<const capnp::word> encoded, size_t passes) {
  uint64_t checksum = SEED;
  auto sourceBytes = encoded.asBytes();
  for (size_t pass = 0; pass < passes; ++pass) {
    kj::ArrayInputStream input(sourceBytes);
    std::array<kj::byte, 264> table;
    input.read(kj::arrayPtr(table.data(), size_t{8}));
    auto wireTable = reinterpret_cast<const capnp::_::WireValue<uint32_t>*>(table.data());
    auto segmentCount = size_t{wireTable[0].get()} + 1;
    if (segmentCount == 0 || segmentCount > 512) {
      throw std::invalid_argument("invalid segment count");
    }
    auto tableBytes = (segmentCount / 2 + 1) * 8;
    if (tableBytes > 8) {
      input.read(kj::arrayPtr(table.data() + 8, tableBytes - 8));
    }
    size_t bodyBytes = 0;
    for (size_t index = 0; index < segmentCount; ++index) {
      auto words = size_t{wireTable[index + 1].get()};
      if (words > (SIZE_MAX - bodyBytes) / sizeof(capnp::word)) {
        throw std::overflow_error("message size overflow");
      }
      bodyBytes += words * sizeof(capnp::word);
    }
    auto frame = kj::heapArray<kj::byte>(tableBytes + bodyBytes);
    std::memcpy(frame.begin(), table.data(), tableBytes);
    input.read(frame.slice(tableBytes, frame.size()));
    auto fingerprint = uint64_t{frame.size()}
        ^ std::rotl(uint64_t{frame[0]}, 7)
        ^ std::rotl(uint64_t{frame[frame.size() - 1]}, 19)
        ^ std::rotl(uint64_t{wireTable[1].get()}, 31);
    checksum = std::rotl(checksum, 9) ^ fingerprint;
  }
  return checksum;
}

uint64_t streamReadReuseMany(kj::ArrayPtr<const capnp::word> encoded, size_t passes) {
  uint64_t checksum = SEED;
  auto sourceBytes = encoded.asBytes();
  auto frame = kj::heapArray<kj::byte>(sourceBytes.size());
  for (size_t pass = 0; pass < passes; ++pass) {
    kj::ArrayInputStream input(sourceBytes);
    std::array<kj::byte, 264> table;
    input.read(kj::arrayPtr(table.data(), size_t{8}));
    auto wireTable = reinterpret_cast<const capnp::_::WireValue<uint32_t>*>(table.data());
    auto segmentCount = size_t{wireTable[0].get()} + 1;
    if (segmentCount == 0 || segmentCount > 512) {
      throw std::invalid_argument("invalid segment count");
    }
    auto tableBytes = (segmentCount / 2 + 1) * 8;
    if (tableBytes > 8) {
      input.read(kj::arrayPtr(table.data() + 8, tableBytes - 8));
    }
    size_t bodyBytes = 0;
    for (size_t index = 0; index < segmentCount; ++index) {
      auto words = size_t{wireTable[index + 1].get()};
      if (words > (SIZE_MAX - bodyBytes) / sizeof(capnp::word)) {
        throw std::overflow_error("message size overflow");
      }
      bodyBytes += words * sizeof(capnp::word);
    }
    if (frame.size() != tableBytes + bodyBytes) {
      throw std::invalid_argument("unexpected frame size");
    }
    std::memcpy(frame.begin(), table.data(), tableBytes);
    input.read(frame.slice(tableBytes, frame.size()));
    auto fingerprint = uint64_t{frame.size()}
        ^ std::rotl(uint64_t{frame[0]}, 7)
        ^ std::rotl(uint64_t{frame[frame.size() - 1]}, 19)
        ^ std::rotl(uint64_t{wireTable[1].get()}, 31);
    checksum = std::rotl(checksum, 9) ^ fingerprint;
  }
  return checksum;
}

uint64_t streamWriteMany(
    kj::ArrayPtr<const kj::ArrayPtr<const capnp::word>> segments,
    size_t encodedBytes,
    size_t passes) {
  uint64_t checksum = SEED;
  for (size_t pass = 0; pass < passes; ++pass) {
    kj::VectorOutputStream output(encodedBytes);
    capnp::writeMessage(output, segments);
    auto bytes = output.getArray();
    auto fingerprint = uint64_t{bytes.size()}
        ^ std::rotl(uint64_t{bytes[0]}, 7)
        ^ std::rotl(uint64_t{bytes[bytes.size() - 1]}, 19);
    checksum = std::rotl(checksum, 9) ^ fingerprint;
  }
  return checksum;
}

}  // namespace

int main(int argc, char** argv) {
  if (argc != 4) {
    std::cerr << "usage: cpp-framing-benchmark MODE SEGMENTS PASSES\n";
    return 2;
  }
  auto mode = std::string_view(argv[1]);
  auto segmentCount = parseSize(argv[2]);
  auto passes = parseSize(argv[3]);
  auto segments = makeSegments(segmentCount);
  auto views = segmentViews(segments);
  auto encoded = capnp::messageToFlatArray(kj::arrayPtr(views.data(), views.size()));

  auto started = std::chrono::steady_clock::now();
  uint64_t checksum;
  if (mode == "parse") {
    checksum = parseMany(encoded.asPtr(), segmentCount, passes);
  } else if (mode == "parse-noalloc") {
    checksum = parseNoAllocMany(encoded.asPtr(), segmentCount, passes);
  } else if (mode == "encode") {
    checksum = encodeMany(kj::arrayPtr(views.data(), views.size()), passes);
  } else if (mode == "stream-read") {
    checksum = streamReadMany(encoded.asPtr(), passes);
  } else if (mode == "stream-read-reuse") {
    checksum = streamReadReuseMany(encoded.asPtr(), passes);
  } else if (mode == "stream-write") {
    checksum = streamWriteMany(
        kj::arrayPtr(views.data(), views.size()), encoded.size() * 8, passes);
  } else {
    std::cerr << "unknown benchmark mode\n";
    return 2;
  }
  auto elapsed = std::chrono::duration_cast<std::chrono::nanoseconds>(
      std::chrono::steady_clock::now() - started);
  std::cout << elapsed.count() << '\t' << checksum << '\n';
  return 0;
}
