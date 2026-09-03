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

template <typename T>
T readData(kj::ArrayPtr<const capnp::byte> data, size_t index, T defaultValue) {
  auto offset = index * sizeof(T);
  T wire = 0;
  if (offset + sizeof(T) <= data.size()) {
    capnp::_::WireValue<T> encoded;
    std::memcpy(&encoded, data.begin() + offset, sizeof(encoded));
    wire = encoded.get();
  }
  return wire ^ defaultValue;
}

uint64_t scalarFingerprint(kj::ArrayPtr<const capnp::byte> data) {
  auto wireBool = data.size() > 0 && (data[0] & 1) != 0;
  uint64_t fingerprint = static_cast<uint64_t>(wireBool ^ true);
  fingerprint = std::rotl(fingerprint, 7)
      ^ readData<uint8_t>(data, 0, 0x5a);
  fingerprint = std::rotl(fingerprint, 11)
      ^ readData<uint16_t>(data, 0, 0xa55a);
  fingerprint = std::rotl(fingerprint, 13)
      ^ readData<uint32_t>(data, 0, 0x13579bdf);
  fingerprint = std::rotl(fingerprint, 17)
      ^ readData<uint64_t>(data, 0, 0xfedcba9876543210ull);
  fingerprint = std::rotl(fingerprint, 19)
      ^ uint64_t{readData<uint32_t>(data, 0, uint32_t(-123456))};
  fingerprint = std::rotl(fingerprint, 23)
      ^ uint64_t{readData<uint32_t>(data, 0, std::bit_cast<uint32_t>(1.25f))};
  fingerprint = std::rotl(fingerprint, 29)
      ^ readData<uint64_t>(data, 0, std::bit_cast<uint64_t>(-3.5));
  fingerprint = std::rotl(fingerprint, 31)
      ^ readData<uint16_t>(data, 0, 7);
  return fingerprint;
}

kj::ArrayPtr<const capnp::byte> blackBoxData(
    kj::ArrayPtr<const capnp::byte> data) {
  auto pointer = data.begin();
  auto size = data.size();
  asm volatile("" : "+r"(pointer), "+r"(size) : : "memory");
  return kj::arrayPtr(pointer, size);
}

uint64_t readScalarOnly(kj::ArrayPtr<const capnp::byte> data, size_t passes) {
  uint64_t checksum = SEED;
  for (size_t pass = 0; pass < passes; ++pass) {
    checksum = std::rotl(checksum, 9) ^ scalarFingerprint(blackBoxData(data));
  }
  return checksum;
}

uint64_t blobFingerprint(capnp::AnyStruct::Reader root) {
  auto pointers = root.getPointerSection();
  if (pointers.size() < 2) return 0;
  auto text = pointers[0].getAs<capnp::Text>();
  auto data = pointers[1].getAs<capnp::Data>();
  uint64_t fingerprint = std::rotl(uint64_t{text.size()}, 11)
      ^ std::rotl(uint64_t{data.size()}, 23);
  if (text.size() != 0) {
    fingerprint ^= std::rotl(uint64_t{uint8_t(text[0])}, 31)
        ^ std::rotl(uint64_t{uint8_t(text[text.size() - 1])}, 37);
  }
  if (data.size() != 0) {
    fingerprint ^= std::rotl(uint64_t{data[0]}, 43)
        ^ std::rotl(uint64_t{data[data.size() - 1]}, 47);
  }
  return fingerprint;
}

uint64_t readBlobOnly(capnp::AnyStruct::Reader root, size_t passes) {
  uint64_t checksum = SEED;
  for (size_t pass = 0; pass < passes; ++pass) {
    auto rootPointer = &root;
    asm volatile("" : "+r"(rootPointer) : : "memory");
    checksum = std::rotl(checksum, 9) ^ blobFingerprint(*rootPointer);
  }
  return checksum;
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
    setWord(segments[0], 0, uint64_t{0x00020001} << 32);
    setWord(segments[0], 1, VALUE);
    setWord(segments[0], 2, (uint64_t{0x42} << 32) | 5);
    setWord(segments[0], 3, (uint64_t{0x42} << 32) | 1);
    std::memcpy(segments[0].asBytes().begin() + 32, "capnp!!\0", 8);
  } else if (count == 2) {
    setWord(segments[0], 0, (uint64_t{1} << 32) | 2);
    setWord(segments[1], 0, uint64_t{0x00020001} << 32);
    setWord(segments[1], 1, VALUE);
    setWord(segments[1], 2, (uint64_t{0x42} << 32) | 5);
    setWord(segments[1], 3, (uint64_t{0x42} << 32) | 1);
    std::memcpy(segments[1].asBytes().begin() + 32, "capnp!!\0", 8);
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
    bool readScalars,
    bool readBlobs,
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
      if (readScalars) {
        fingerprint ^= scalarFingerprint(data);
      } else if (readBlobs) {
        fingerprint ^= blobFingerprint(root);
      } else {
        uint64_t value = 0;
        if (data.size() >= sizeof(value)) {
          capnp::_::WireValue<uint64_t> wire;
          std::memcpy(&wire, data.begin(), sizeof(wire));
          value = wire.get();
        }
        fingerprint ^= std::rotl(uint64_t{data.size()}, 17)
            ^ std::rotl(value, 37);
      }
    }
    checksum = std::rotl(checksum, 9) ^ fingerprint;
  }
  return checksum;
}

uint64_t readIsolatedRoots(
    kj::ArrayPtr<const kj::ArrayPtr<const capnp::word>> segments,
    bool readScalars,
    bool readBlobs,
    size_t passes) {
  uint64_t checksum = SEED;
  for (size_t pass = 0; pass < passes; ++pass) {
    capnp::SegmentArrayMessageReader reader(segments);
    auto root = reader.getRoot<capnp::AnyStruct>();
    auto data = root.getDataSection();
    uint64_t fingerprint;
    if (readScalars) {
      fingerprint = uint64_t{segments.size()} ^ scalarFingerprint(data);
    } else if (readBlobs) {
      fingerprint = uint64_t{segments.size()} ^ blobFingerprint(root);
    } else {
      uint64_t value = 0;
      if (data.size() >= sizeof(value)) {
        capnp::_::WireValue<uint64_t> wire;
        std::memcpy(&wire, data.begin(), sizeof(wire));
        value = wire.get();
      }
      fingerprint = uint64_t{segments.size()}
          ^ std::rotl(uint64_t{data.size()}, 17)
          ^ std::rotl(value, 37);
    }
    checksum = std::rotl(checksum, 9) ^ fingerprint;
  }
  return checksum;
}

}  // namespace

int main(int argc, char** argv) {
  if (argc != 4) {
    std::cerr << "usage: cpp-message-read framing|root|scalars|blobs|isolated-root|isolated-scalars|isolated-blobs|scalar-only|blob-only SEGMENTS PASSES\n";
    return 2;
  }
  auto mode = std::string_view(argv[1]);
  auto segmentCount = parseSize(argv[2]);
  auto passes = parseSize(argv[3]);
  if (mode != "framing" && mode != "root" && mode != "scalars"
      && mode != "blobs"
      && mode != "isolated-root" && mode != "isolated-scalars"
      && mode != "isolated-blobs" && mode != "scalar-only"
      && mode != "blob-only") {
    std::cerr << "unknown benchmark mode\n";
    return 2;
  }

  auto segments = makeSegments(segmentCount);
  auto views = segmentViews(segments);
  auto encoded = capnp::messageToFlatArray(kj::arrayPtr(views.data(), views.size()));
  if (mode == "scalar-only") {
    capnp::SegmentArrayMessageReader reader(kj::arrayPtr(views.data(), views.size()));
    auto data = reader.getRoot<capnp::AnyStruct>().getDataSection();
    auto started = std::chrono::steady_clock::now();
    auto checksum = readScalarOnly(data, passes);
    auto elapsed = std::chrono::duration_cast<std::chrono::nanoseconds>(
        std::chrono::steady_clock::now() - started);
    std::cout << elapsed.count() << '\t' << checksum << '\n';
    return 0;
  }
  if (mode == "blob-only") {
    capnp::ReaderOptions options;
    options.traversalLimitInWords = kj::maxValue;
    capnp::SegmentArrayMessageReader reader(
        kj::arrayPtr(views.data(), views.size()), options);
    auto root = reader.getRoot<capnp::AnyStruct>();
    auto started = std::chrono::steady_clock::now();
    auto checksum = readBlobOnly(root, passes);
    auto elapsed = std::chrono::duration_cast<std::chrono::nanoseconds>(
        std::chrono::steady_clock::now() - started);
    std::cout << elapsed.count() << '\t' << checksum << '\n';
    return 0;
  }
  auto started = std::chrono::steady_clock::now();
  auto isolated = mode == "isolated-root" || mode == "isolated-scalars"
      || mode == "isolated-blobs";
  auto scalars = mode == "scalars" || mode == "isolated-scalars";
  auto blobs = mode == "blobs" || mode == "isolated-blobs";
  auto checksum = isolated
      ? readIsolatedRoots(
          kj::arrayPtr(views.data(), views.size()), scalars, blobs, passes)
      : readMany(
          encoded.asPtr(), segmentCount, mode != "framing", scalars, blobs, passes);
  auto elapsed = std::chrono::duration_cast<std::chrono::nanoseconds>(
      std::chrono::steady_clock::now() - started);
  std::cout << elapsed.count() << '\t' << checksum << '\n';
  return 0;
}
