#include <capnp/endian.h>
#include <capnp/serialize.h>

#include <bit>
#include <charconv>
#include <chrono>
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
    auto first = reader.getSegment(0);
    auto last = reader.getSegment(segmentCount - 1);
    auto firstBytes = first.asBytes();
    auto lastBytes = last.asBytes();
    auto encodedBytes = static_cast<size_t>(reader.getEnd() - encoded.begin()) * 8;
    auto fingerprint = uint64_t{segmentCount}
        ^ std::rotl(uint64_t{segmentCount / 2 + 1} * 8, 11)
        ^ std::rotl(uint64_t{encodedBytes}, 23)
        ^ std::rotl(uint64_t{first.size()}, 37)
        ^ std::rotl(uint64_t{last.size()}, 49)
        ^ std::rotl(uint64_t{firstBytes[0]}, 7)
        ^ std::rotl(uint64_t{lastBytes[lastBytes.size() - 1]}, 19);
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
  } else if (mode == "encode") {
    checksum = encodeMany(kj::arrayPtr(views.data(), views.size()), passes);
  } else {
    std::cerr << "unknown benchmark mode\n";
    return 2;
  }
  auto elapsed = std::chrono::duration_cast<std::chrono::nanoseconds>(
      std::chrono::steady_clock::now() - started);
  std::cout << elapsed.count() << '\t' << checksum << '\n';
  return 0;
}
