#include "conformance/schemas/wire-fixture.capnp.h"

#include <capnp/any.h>
#include <capnp/endian.h>
#include <capnp/serialize.h>

#include <bit>
#include <charconv>
#include <chrono>
#include <cstdint>
#include <cstring>
#include <fstream>
#include <iostream>
#include <limits>
#include <stdexcept>
#include <string_view>

namespace {

constexpr uint64_t SEED = 0x4d595df4d0f33173ull;

size_t parseSize(const char* text) {
  size_t result = 0;
  auto input = std::string_view(text);
  auto parsed = std::from_chars(input.begin(), input.end(), result);
  if (parsed.ec != std::errc() || parsed.ptr != input.end() || result == 0) {
    throw std::invalid_argument("passes must be a positive integer");
  }
  return result;
}

kj::Array<capnp::word> readWords(const char* path) {
  std::ifstream input(path, std::ios::binary | std::ios::ate);
  if (!input) throw std::runtime_error("failed to open fixture");
  auto end = input.tellg();
  if (end <= 0) {
    throw std::runtime_error("fixture is not a non-empty word array");
  }
  auto byteSize = static_cast<size_t>(end);
  if (byteSize % sizeof(capnp::word) != 0) {
    throw std::runtime_error("fixture is not a word array");
  }
  auto words = kj::heapArray<capnp::word>(
      static_cast<size_t>(byteSize) / sizeof(capnp::word));
  input.seekg(0);
  input.read(reinterpret_cast<char*>(words.begin()), static_cast<std::streamsize>(byteSize));
  if (!input) throw std::runtime_error("failed to read fixture");
  return words;
}

template <typename T>
T readData(kj::ArrayPtr<const capnp::byte> data, size_t index, T defaultValue = 0) {
  auto offset = index * sizeof(T);
  T wire = 0;
  if (offset + sizeof(T) <= data.size()) {
    capnp::_::WireValue<T> encoded;
    std::memcpy(&encoded, data.begin() + offset, sizeof(encoded));
    wire = encoded.get();
  }
  return wire ^ defaultValue;
}

uint64_t directScalarFingerprint(capnp::AnyStruct::Reader root) {
  auto data = root.getDataSection();
  uint64_t value = data.size() > 0 && (data[0] & 1) != 0;
  value = std::rotl(value, 5) ^ uint64_t{uint8_t(readData<int8_t>(data, 1))};
  value = std::rotl(value, 7) ^ uint64_t{uint16_t(readData<int16_t>(data, 1))};
  value = std::rotl(value, 11) ^ uint64_t{uint32_t(readData<int32_t>(data, 1))};
  value = std::rotl(value, 13) ^ static_cast<uint64_t>(readData<int64_t>(data, 1));
  value = std::rotl(value, 17) ^ readData<uint8_t>(data, 16);
  value = std::rotl(value, 19) ^ readData<uint16_t>(data, 9);
  value = std::rotl(value, 23) ^ readData<uint32_t>(data, 5);
  value = std::rotl(value, 29) ^ readData<uint64_t>(data, 3);
  value = std::rotl(value, 31)
      ^ readData<uint32_t>(data, 8);
  value = std::rotl(value, 37)
      ^ readData<uint64_t>(data, 5);
  value = std::rotl(value, 41) ^ readData<uint16_t>(data, 18);
  value = std::rotl(value, 43) ^ readData<uint32_t>(data, 16, 123456u);
  return value;
}

uint64_t generatedScalarFingerprint(WireFixture::Reader root) {
  uint64_t value = root.getBoolValue();
  value = std::rotl(value, 5) ^ uint64_t{uint8_t(root.getInt8Value())};
  value = std::rotl(value, 7) ^ uint64_t{uint16_t(root.getInt16Value())};
  value = std::rotl(value, 11) ^ uint64_t{uint32_t(root.getInt32Value())};
  value = std::rotl(value, 13) ^ static_cast<uint64_t>(root.getInt64Value());
  value = std::rotl(value, 17) ^ root.getUint8Value();
  value = std::rotl(value, 19) ^ root.getUint16Value();
  value = std::rotl(value, 23) ^ root.getUint32Value();
  value = std::rotl(value, 29) ^ root.getUint64Value();
  value = std::rotl(value, 31) ^ std::bit_cast<uint32_t>(root.getFloat32Value());
  value = std::rotl(value, 37) ^ std::bit_cast<uint64_t>(root.getFloat64Value());
  value = std::rotl(value, 41) ^ uint16_t(root.getColor());
  value = std::rotl(value, 43) ^ root.getDefaulted();
  return value;
}

uint64_t blobFingerprint(
    kj::ArrayPtr<const capnp::byte> text,
    kj::ArrayPtr<const capnp::byte> data) {
  uint64_t value = std::rotl(uint64_t{text.size()}, 11)
      ^ std::rotl(uint64_t{data.size()}, 23);
  if (text.size() != 0) {
    value ^= std::rotl(uint64_t{text[0]}, 31)
        ^ std::rotl(uint64_t{text[text.size() - 1]}, 37);
  }
  if (data.size() != 0) {
    value ^= std::rotl(uint64_t{data[0]}, 43)
        ^ std::rotl(uint64_t{data[data.size() - 1]}, 47);
  }
  return value;
}

uint64_t directBlobFingerprint(capnp::AnyStruct::Reader root) {
  auto pointers = root.getPointerSection();
  return blobFingerprint(
      pointers[0].getAs<capnp::Text>().asBytes(),
      pointers[1].getAs<capnp::Data>());
}

uint64_t generatedBlobFingerprint(WireFixture::Reader root) {
  return blobFingerprint(root.getText().asBytes(), root.getData());
}

uint64_t groupFingerprint(
    uint16_t tag, uint64_t number, uint64_t created, bool valid) {
  uint64_t value = tag;
  value = std::rotl(value, 17) ^ number;
  value = std::rotl(value, 23) ^ created;
  return std::rotl(value, 29) ^ uint64_t{valid};
}

uint64_t directGroupFingerprint(capnp::AnyStruct::Reader root) {
  auto data = root.getDataSection();
  return groupFingerprint(
      readData<uint16_t>(data, 19),
      readData<uint64_t>(data, 6),
      readData<uint64_t>(data, 7),
      data.size() > 0 && (data[0] & 2) != 0);
}

uint64_t generatedGroupFingerprint(WireFixture::Reader root) {
  auto choice = root.getChoice();
  auto metadata = root.getMetadata();
  return groupFingerprint(
      static_cast<uint16_t>(choice.which()), choice.getNumber(),
      metadata.getCreated(), metadata.getValid());
}

uint64_t listFingerprint(
    uint16_t integer, uint16_t color,
    kj::ArrayPtr<const capnp::byte> text,
    kj::ArrayPtr<const capnp::byte> data,
    uint16_t nested) {
  uint64_t value = integer;
  value = std::rotl(value, 11) ^ color;
  value = std::rotl(value, 17) ^ uint64_t{text.size()};
  value = std::rotl(value, 23)
      ^ static_cast<uint64_t>(text.size() == 0 ? 0 : text[text.size() - 1]);
  value = std::rotl(value, 29) ^ uint64_t{data.size()};
  value = std::rotl(value, 31)
      ^ static_cast<uint64_t>(data.size() == 0 ? 0 : data[data.size() - 1]);
  return std::rotl(value, 37) ^ nested;
}

uint64_t directListFingerprint(capnp::AnyStruct::Reader root) {
  auto pointers = root.getPointerSection();
  auto integers = pointers[9].getAs<capnp::List<uint16_t>>();
  auto colors = pointers[14].getAs<capnp::List<uint16_t>>();
  auto texts = pointers[15].getAs<capnp::List<capnp::Text>>();
  auto blobs = pointers[16].getAs<capnp::List<capnp::Data>>();
  auto nested = pointers[18].getAs<capnp::List<capnp::List<uint16_t>>>();
  return listFingerprint(
      integers[2], colors[2], texts[2].asBytes(), blobs[1], nested[0][2]);
}

uint64_t generatedListFingerprint(WireFixture::Reader root) {
  auto text = root.getTexts()[2];
  auto data = root.getDataBlobs()[1];
  return listFingerprint(
      root.getUint16s()[2], static_cast<uint16_t>(root.getColors()[2]),
      text.asBytes(), data, root.getNestedLists()[0][2]);
}

template <typename Root, typename Fingerprint>
uint64_t measure(Root root, Fingerprint fingerprint, size_t passes) {
  uint64_t checksum = SEED;
  for (size_t pass = 0; pass < passes; ++pass) {
    auto rootPointer = &root;
    asm volatile("" : "+r"(rootPointer) : : "memory");
    checksum = std::rotl(checksum, 9) ^ fingerprint(*rootPointer);
  }
  return checksum;
}

}  // namespace

int main(int argc, char** argv) {
  if (argc != 4) {
    std::cerr << "usage: cpp-generated-api direct-scalars|generated-scalars|borrowed-direct-scalars|borrowed-scalars|direct-blobs|generated-blobs|borrowed-direct-blobs|borrowed-blobs|borrowed-direct-groups|borrowed-groups|borrowed-direct-lists|borrowed-lists PASSES FIXTURE\n";
    return 2;
  }
  auto mode = std::string_view(argv[1]);
  auto passes = parseSize(argv[2]);
  auto words = readWords(argv[3]);
  capnp::ReaderOptions options;
  options.traversalLimitInWords = std::numeric_limits<uint64_t>::max();
  capnp::FlatArrayMessageReader message(words.asPtr(), options);

  auto started = std::chrono::steady_clock::now();
  uint64_t checksum;
  if (mode == "direct-scalars" || mode == "borrowed-direct-scalars") {
    checksum = measure(message.getRoot<capnp::AnyStruct>(), directScalarFingerprint, passes);
  } else if (mode == "generated-scalars" || mode == "borrowed-scalars") {
    checksum = measure(message.getRoot<WireFixture>(), generatedScalarFingerprint, passes);
  } else if (mode == "direct-blobs" || mode == "borrowed-direct-blobs") {
    checksum = measure(message.getRoot<capnp::AnyStruct>(), directBlobFingerprint, passes);
  } else if (mode == "generated-blobs" || mode == "borrowed-blobs") {
    checksum = measure(message.getRoot<WireFixture>(), generatedBlobFingerprint, passes);
  } else if (mode == "borrowed-direct-groups") {
    checksum = measure(message.getRoot<capnp::AnyStruct>(), directGroupFingerprint, passes);
  } else if (mode == "borrowed-groups") {
    checksum = measure(message.getRoot<WireFixture>(), generatedGroupFingerprint, passes);
  } else if (mode == "borrowed-direct-lists") {
    checksum = measure(message.getRoot<capnp::AnyStruct>(), directListFingerprint, passes);
  } else if (mode == "borrowed-lists") {
    checksum = measure(message.getRoot<WireFixture>(), generatedListFingerprint, passes);
  } else {
    std::cerr << "unknown benchmark mode\n";
    return 2;
  }
  auto elapsed = std::chrono::duration_cast<std::chrono::nanoseconds>(
      std::chrono::steady_clock::now() - started);
  std::cout << elapsed.count() << '\t' << checksum << '\n';
  return 0;
}
