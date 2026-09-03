#include <capnp/any.h>
#include <capnp/endian.h>
#include <capnp/serialize.h>

#include <bit>
#include <charconv>
#include <chrono>
#include <cstdint>
#include <cstring>
#include <iostream>
#include <string_view>
#include <vector>

namespace {

constexpr uint64_t SEED = 0x4d595df4d0f33173ull;
constexpr uint64_t VALUE = 0x0123456789abcdefull;

size_t parseSize(const char* text) {
  size_t result = 0;
  auto input = std::string_view(text);
  auto parsed = std::from_chars(input.begin(), input.end(), result);
  if (parsed.ec != std::errc() || parsed.ptr != input.end() || result == 0) {
    throw std::invalid_argument("sizes must be positive integers");
  }
  return result;
}

void setWord(kj::Array<capnp::word>& segment, size_t index, uint64_t value) {
  auto wire = reinterpret_cast<capnp::_::WireValue<uint64_t>*>(segment.begin());
  wire[index].set(value);
}

std::vector<kj::Array<capnp::word>> makeSegments(size_t count) {
  std::vector<size_t> sizes;
  if (count == 1) sizes = {8};
  else if (count == 2) sizes = {3, 5};
  else if (count == 64) sizes.assign(64, 1);
  else throw std::invalid_argument("segment count must be 1, 2, or 64");

  std::vector<kj::Array<capnp::word>> segments;
  segments.reserve(count);
  for (auto size: sizes) {
    auto segment = kj::heapArray<capnp::word>(size);
    std::memset(segment.begin(), 0, segment.asBytes().size());
    segments.push_back(kj::mv(segment));
  }

  if (count == 1) {
    setWord(segments[0], 0, uint64_t{1} << 32);
    setWord(segments[0], 1, VALUE);
  } else if (count == 2) {
    setWord(segments[0], 0, (uint64_t{1} << 32) | 2);
    setWord(segments[1], 0, uint64_t{1} << 32);
    setWord(segments[1], 1, VALUE);
  } else {
    setWord(segments[0], 0, (uint64_t{63} << 32) | 2);
  }
  return segments;
}

std::vector<kj::ArrayPtr<const capnp::word>> segmentViews(
    const std::vector<kj::Array<capnp::word>>& segments) {
  std::vector<kj::ArrayPtr<const capnp::word>> result;
  result.reserve(segments.size());
  for (const auto& segment: segments) result.push_back(segment.asPtr());
  return result;
}

uint64_t readMany(
    kj::ArrayPtr<const capnp::word> encoded,
    size_t segmentCount,
    bool readRoot,
    size_t passes) {
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
    if (readRoot) {
      auto root = reader.getRoot<capnp::AnyStruct>();
      auto data = root.getDataSection();
      uint64_t value = 0;
      if (data.size() >= sizeof(value)) {
        capnp::_::WireValue<uint64_t> wire;
        std::memcpy(&wire, data.begin(), sizeof(wire));
        value = wire.get();
      }
      fingerprint ^= std::rotl(uint64_t{data.size()}, 17)
          ^ std::rotl(value, 37);
    }
    checksum = std::rotl(checksum, 9) ^ fingerprint;
  }
  return checksum;
}

}  // namespace

int main(int argc, char** argv) {
  if (argc != 4) {
    std::cerr << "usage: cpp-message-read framing|root SEGMENTS PASSES\n";
    return 2;
  }
  auto mode = std::string_view(argv[1]);
  auto segmentCount = parseSize(argv[2]);
  auto passes = parseSize(argv[3]);
  if (mode != "framing" && mode != "root") {
    std::cerr << "unknown benchmark mode\n";
    return 2;
  }

  auto segments = makeSegments(segmentCount);
  auto views = segmentViews(segments);
  auto encoded = capnp::messageToFlatArray(kj::arrayPtr(views.data(), views.size()));
  auto started = std::chrono::steady_clock::now();
  auto checksum = readMany(encoded.asPtr(), segmentCount, mode == "root", passes);
  auto elapsed = std::chrono::duration_cast<std::chrono::nanoseconds>(
      std::chrono::steady_clock::now() - started);
  std::cout << elapsed.count() << '\t' << checksum << '\n';
  return 0;
}
