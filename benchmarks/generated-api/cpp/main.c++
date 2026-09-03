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
    std::cerr << "usage: cpp-generated-api direct-scalars|generated-scalars|borrowed-direct-scalars|borrowed-scalars|direct-blobs|generated-blobs|borrowed-direct-blobs|borrowed-blobs PASSES FIXTURE\n";
    return 2;
  }
  auto mode = std::string_view(argv[1]);
  auto passes = parseSize(argv[2]);
  auto words = readWords(argv[3]);
  capnp::FlatArrayMessageReader message(words.asPtr());

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
  } else {
    std::cerr << "unknown benchmark mode\n";
    return 2;
  }
  auto elapsed = std::chrono::duration_cast<std::chrono::nanoseconds>(
      std::chrono::steady_clock::now() - started);
  std::cout << elapsed.count() << '\t' << checksum << '\n';
  return 0;
}
