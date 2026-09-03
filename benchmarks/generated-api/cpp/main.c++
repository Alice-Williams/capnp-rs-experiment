#include "conformance/schemas/wire-fixture.capnp.h"
#include "conformance/schemas/evolution-v1.capnp.h"

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
#include <type_traits>

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

kj::Array<capnp::word> emptyMessageWords() {
  auto words = kj::heapArray<capnp::word>(2);
  std::memset(words.begin(), 0, words.asBytes().size());
  const uint32_t segmentWords = 1;
  std::memcpy(words.asBytes().begin() + sizeof(uint32_t),
              &segmentWords, sizeof(segmentWords));
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

template <typename T>
void writeData(
    kj::ArrayPtr<capnp::byte> data, size_t index, T value,
    T defaultValue = 0) {
  auto offset = index * sizeof(T);
  if (offset + sizeof(T) > data.size()) {
    throw std::runtime_error("direct scalar write exceeds the data section");
  }
  capnp::_::WireValue<T> encoded;
  if constexpr (std::is_floating_point_v<T>) {
    if (defaultValue != 0) {
      throw std::runtime_error("non-zero floating-point default is unsupported here");
    }
    encoded.set(value);
  } else {
    encoded.set(value ^ defaultValue);
  }
  std::memcpy(data.begin() + offset, &encoded, sizeof(encoded));
}

struct ScalarBuilderValues {
  uint64_t raw;
  bool boolValue;
  int8_t int8Value;
  int16_t int16Value;
  int32_t int32Value;
  int64_t int64Value;
  uint8_t uint8Value;
  uint16_t uint16Value;
  uint32_t uint32Value;
  uint64_t uint64Value;
  float float32Value;
  double float64Value;
  Color color;
  uint32_t defaulted;
};

ScalarBuilderValues scalarBuilderValues(size_t pass) {
  auto raw = SEED + static_cast<uint64_t>(pass) * 0x9e3779b97f4a7c15ull;
  auto color = raw % 3 == 0 ? Color::RED
      : raw % 3 == 1 ? Color::GREEN : Color::BLUE;
  return {
    raw,
    (raw & 1) != 0,
    static_cast<int8_t>(raw & 0x7f),
    static_cast<int16_t>(raw & 0x7fff),
    static_cast<int32_t>(raw & 0x7fffffffull),
    static_cast<int64_t>(raw & 0x7fffffffffffffffull),
    static_cast<uint8_t>(raw),
    static_cast<uint16_t>(raw),
    static_cast<uint32_t>(raw),
    raw,
    std::bit_cast<float>(0x3f800000u | (static_cast<uint32_t>(raw) & 0x007fffffu)),
    std::bit_cast<double>(0x3ff0000000000000ull | (raw & 0x000fffffffffffffull)),
    color,
    static_cast<uint32_t>(raw >> 17),
  };
}

uint64_t scalarBuilderFingerprint(size_t pass) {
  auto values = scalarBuilderValues(pass);
  uint64_t value = values.boolValue;
  value = std::rotl(value, 5) ^ uint64_t{static_cast<uint8_t>(values.int8Value)};
  value = std::rotl(value, 7) ^ uint64_t{static_cast<uint16_t>(values.int16Value)};
  value = std::rotl(value, 11) ^ uint64_t{static_cast<uint32_t>(values.int32Value)};
  value = std::rotl(value, 13) ^ static_cast<uint64_t>(values.int64Value);
  value = std::rotl(value, 17) ^ values.uint8Value;
  value = std::rotl(value, 19) ^ values.uint16Value;
  value = std::rotl(value, 23) ^ values.uint32Value;
  value = std::rotl(value, 29) ^ values.uint64Value;
  value = std::rotl(value, 31) ^ std::bit_cast<uint32_t>(values.float32Value);
  value = std::rotl(value, 37) ^ std::bit_cast<uint64_t>(values.float64Value);
  value = std::rotl(value, 41) ^ static_cast<uint16_t>(values.color);
  return std::rotl(value, 43) ^ values.defaulted ^ std::rotl(values.raw, 47);
}

void writeDirectScalars(capnp::AnyStruct::Builder& root, size_t pass) {
  auto values = scalarBuilderValues(pass);
  auto data = root.getDataSection();
  if (data.size() < 72) {
    throw std::runtime_error("WireFixture direct builder has a short data section");
  }
  data[0] = static_cast<capnp::byte>((data[0] & ~1u) | values.boolValue);
  writeData<int8_t>(data, 1, values.int8Value);
  writeData<int16_t>(data, 1, values.int16Value);
  writeData<int32_t>(data, 1, values.int32Value);
  writeData<int64_t>(data, 1, values.int64Value);
  writeData<uint8_t>(data, 16, values.uint8Value);
  writeData<uint16_t>(data, 9, values.uint16Value);
  writeData<uint32_t>(data, 5, values.uint32Value);
  writeData<uint64_t>(data, 3, values.uint64Value);
  writeData<float>(data, 8, values.float32Value);
  writeData<double>(data, 5, values.float64Value);
  writeData<uint16_t>(data, 18, static_cast<uint16_t>(values.color));
  writeData<uint32_t>(data, 16, values.defaulted, 123456u);
}

void writeGeneratedScalars(WireFixture::Builder& root, size_t pass) {
  auto values = scalarBuilderValues(pass);
  root.setBoolValue(values.boolValue);
  root.setInt8Value(values.int8Value);
  root.setInt16Value(values.int16Value);
  root.setInt32Value(values.int32Value);
  root.setInt64Value(values.int64Value);
  root.setUint8Value(values.uint8Value);
  root.setUint16Value(values.uint16Value);
  root.setUint32Value(values.uint32Value);
  root.setUint64Value(values.uint64Value);
  root.setFloat32Value(values.float32Value);
  root.setFloat64Value(values.float64Value);
  root.setColor(values.color);
  root.setDefaulted(values.defaulted);
}

constexpr std::string_view BUILDER_TEXT[] = {"capnp-a", "capnp-b"};
constexpr capnp::byte BUILDER_DATA[][8] = {
  {'d', 'a', 't', 'a', '-', '-', '-', 'a'},
  {'d', 'a', 't', 'a', '-', '-', '-', 'b'},
};

uint64_t blobBuilderFingerprint(size_t pass) {
  auto selected = pass & 1;
  auto text = BUILDER_TEXT[selected];
  auto data = kj::arrayPtr(BUILDER_DATA[selected], size_t{8});
  uint64_t value = std::rotl(uint64_t{text.size()}, 11)
      ^ std::rotl(uint64_t{data.size()}, 23);
  value ^= std::rotl(uint64_t{static_cast<uint8_t>(text.front())}, 31)
      ^ std::rotl(uint64_t{static_cast<uint8_t>(text.back())}, 37);
  return value ^ std::rotl(uint64_t{data.front()}, 43)
      ^ std::rotl(uint64_t{data.back()}, 47);
}

void writeDirectBlobs(capnp::AnyStruct::Builder& root, size_t pass) {
  auto selected = pass & 1;
  auto text = BUILDER_TEXT[selected];
  auto data = kj::arrayPtr(BUILDER_DATA[selected], size_t{8});
  auto pointers = root.getPointerSection();
  pointers[0].setAs<capnp::Text>(kj::StringPtr(text.data(), text.size()));
  pointers[1].setAs<capnp::Data>(data);
}

void writeGeneratedBlobs(WireFixture::Builder& root, size_t pass) {
  auto selected = pass & 1;
  auto text = BUILDER_TEXT[selected];
  root.setText(kj::StringPtr(text.data(), text.size()));
  root.setData(kj::arrayPtr(BUILDER_DATA[selected], size_t{8}));
}

uint64_t structBuilderFingerprint(size_t pass) {
  return std::rotl(static_cast<uint64_t>(pass), 29) ^ 0xae37c0cc5acf02c6ull;
}

void writeDirectStruct(capnp::AnyStruct::Builder& root, size_t pass) {
  auto node = root.getPointerSection()[22].initAsAnyStruct(1, 1);
  writeData<uint32_t>(node.getDataSection(), 0, static_cast<uint32_t>(pass));
}

void writeGeneratedStruct(WireFixture::Builder& root, size_t pass) {
  root.initNode().setValue(static_cast<uint32_t>(pass));
}

void listBuilderValues(size_t pass, uint16_t (&values)[4]) {
  auto value = static_cast<uint16_t>(pass);
  values[0] = value;
  values[1] = value ^ 0x55aa;
  values[2] = std::rotl(value, 3);
  values[3] = static_cast<uint16_t>(value + 7);
}

uint64_t listBuilderFingerprint(size_t pass) {
  uint16_t values[4];
  listBuilderValues(pass, values);
  uint64_t fingerprint = values[0];
  fingerprint = std::rotl(fingerprint, 11) ^ values[1];
  fingerprint = std::rotl(fingerprint, 17) ^ values[2];
  return std::rotl(fingerprint, 23) ^ values[3];
}

void writeDirectList(capnp::AnyStruct::Builder& root, size_t pass) {
  uint16_t values[4];
  listBuilderValues(pass, values);
  auto list = root.getPointerSection()[9].initAs<capnp::List<uint16_t>>(4);
  for (uint32_t index = 0; index < 4; ++index) list.set(index, values[index]);
}

void writeGeneratedList(WireFixture::Builder& root, size_t pass) {
  uint16_t values[4];
  listBuilderValues(pass, values);
  auto list = root.initUint16s(4);
  for (uint32_t index = 0; index < 4; ++index) list.set(index, values[index]);
}

template <typename Root, typename Write, typename Fingerprint>
uint64_t measureBuilder(
    Root& root, Write write, Fingerprint fingerprint, size_t passes) {
  uint64_t checksum = SEED;
  for (size_t pass = 0; pass < passes; ++pass) {
    auto rootPointer = &root;
    asm volatile("" : "+r"(rootPointer) : : "memory");
    write(*rootPointer, pass);
    checksum = std::rotl(checksum, 9) ^ fingerprint(pass);
  }
  return checksum;
}

void benchmarkDirectBuilder(size_t passes) {
  capnp::MallocMessageBuilder message;
  auto root = message.initRoot<capnp::AnyPointer>().initAsAnyStruct(9, 28);
  auto started = std::chrono::steady_clock::now();
  auto checksum = measureBuilder(
      root, writeDirectScalars, scalarBuilderFingerprint, passes);
  auto elapsed = std::chrono::duration_cast<std::chrono::nanoseconds>(
      std::chrono::steady_clock::now() - started);
  std::cout << elapsed.count() << '\t' << checksum << '\n';
}

void benchmarkGeneratedBuilder(size_t passes) {
  capnp::MallocMessageBuilder message;
  auto root = message.initRoot<WireFixture>();
  auto started = std::chrono::steady_clock::now();
  auto checksum = measureBuilder(
      root, writeGeneratedScalars, scalarBuilderFingerprint, passes);
  auto elapsed = std::chrono::duration_cast<std::chrono::nanoseconds>(
      std::chrono::steady_clock::now() - started);
  std::cout << elapsed.count() << '\t' << checksum << '\n';
}

void benchmarkDirectBlobBuilder(size_t passes) {
  auto words = passes * 2 + 64;
  auto scratch = kj::heapArray<capnp::word>(words);
  capnp::MallocMessageBuilder message(
      scratch.asPtr(), capnp::AllocationStrategy::FIXED_SIZE);
  auto root = message.initRoot<capnp::AnyPointer>().initAsAnyStruct(9, 28);
  auto started = std::chrono::steady_clock::now();
  auto checksum = measureBuilder(
      root, writeDirectBlobs, blobBuilderFingerprint, passes);
  auto elapsed = std::chrono::duration_cast<std::chrono::nanoseconds>(
      std::chrono::steady_clock::now() - started);
  std::cout << elapsed.count() << '\t' << checksum << '\n';
}

void benchmarkGeneratedBlobBuilder(size_t passes) {
  auto words = passes * 2 + 64;
  auto scratch = kj::heapArray<capnp::word>(words);
  capnp::MallocMessageBuilder message(
      scratch.asPtr(), capnp::AllocationStrategy::FIXED_SIZE);
  auto root = message.initRoot<WireFixture>();
  auto started = std::chrono::steady_clock::now();
  auto checksum = measureBuilder(
      root, writeGeneratedBlobs, blobBuilderFingerprint, passes);
  auto elapsed = std::chrono::duration_cast<std::chrono::nanoseconds>(
      std::chrono::steady_clock::now() - started);
  std::cout << elapsed.count() << '\t' << checksum << '\n';
}

void benchmarkDirectStructBuilder(size_t passes) {
  auto words = passes * 2 + 64;
  auto scratch = kj::heapArray<capnp::word>(words);
  capnp::MallocMessageBuilder message(
      scratch.asPtr(), capnp::AllocationStrategy::FIXED_SIZE);
  auto root = message.initRoot<capnp::AnyPointer>().initAsAnyStruct(9, 28);
  auto started = std::chrono::steady_clock::now();
  auto checksum = measureBuilder(
      root, writeDirectStruct, structBuilderFingerprint, passes);
  auto elapsed = std::chrono::duration_cast<std::chrono::nanoseconds>(
      std::chrono::steady_clock::now() - started);
  std::cout << elapsed.count() << '\t' << checksum << '\n';
}

void benchmarkGeneratedStructBuilder(size_t passes) {
  auto words = passes * 2 + 64;
  auto scratch = kj::heapArray<capnp::word>(words);
  capnp::MallocMessageBuilder message(
      scratch.asPtr(), capnp::AllocationStrategy::FIXED_SIZE);
  auto root = message.initRoot<WireFixture>();
  auto started = std::chrono::steady_clock::now();
  auto checksum = measureBuilder(
      root, writeGeneratedStruct, structBuilderFingerprint, passes);
  auto elapsed = std::chrono::duration_cast<std::chrono::nanoseconds>(
      std::chrono::steady_clock::now() - started);
  std::cout << elapsed.count() << '\t' << checksum << '\n';
}

void benchmarkDirectListBuilder(size_t passes) {
  auto words = passes * 2 + 64;
  auto scratch = kj::heapArray<capnp::word>(words);
  capnp::MallocMessageBuilder message(
      scratch.asPtr(), capnp::AllocationStrategy::FIXED_SIZE);
  auto root = message.initRoot<capnp::AnyPointer>().initAsAnyStruct(9, 28);
  auto started = std::chrono::steady_clock::now();
  auto checksum = measureBuilder(
      root, writeDirectList, listBuilderFingerprint, passes);
  auto elapsed = std::chrono::duration_cast<std::chrono::nanoseconds>(
      std::chrono::steady_clock::now() - started);
  std::cout << elapsed.count() << '\t' << checksum << '\n';
}

void benchmarkGeneratedListBuilder(size_t passes) {
  auto words = passes * 2 + 64;
  auto scratch = kj::heapArray<capnp::word>(words);
  capnp::MallocMessageBuilder message(
      scratch.asPtr(), capnp::AllocationStrategy::FIXED_SIZE);
  auto root = message.initRoot<WireFixture>();
  auto started = std::chrono::steady_clock::now();
  auto checksum = measureBuilder(
      root, writeGeneratedList, listBuilderFingerprint, passes);
  auto elapsed = std::chrono::duration_cast<std::chrono::nanoseconds>(
      std::chrono::steady_clock::now() - started);
  std::cout << elapsed.count() << '\t' << checksum << '\n';
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

uint64_t directNestedFingerprint(capnp::AnyStruct::Reader root) {
  auto nested = root.getPointerSection()[22].getAs<capnp::AnyStruct>();
  return readData<uint32_t>(nested.getDataSection(), 0);
}

uint64_t generatedNestedFingerprint(WireFixture::Reader root) {
  return root.getNode().getValue();
}

uint64_t directStructListFingerprint(capnp::AnyStruct::Reader root) {
  auto nodes = root.getPointerSection()[17]
      .getAs<capnp::AnyList>()
      .as<capnp::List<capnp::AnyStruct>>();
  return readData<uint32_t>(nodes[1].getDataSection(), 0);
}

uint64_t generatedStructListFingerprint(WireFixture::Reader root) {
  return root.getStructs()[1].getValue();
}

uint64_t evolutionFingerprint(
    uint32_t id, uint16_t state, kj::ArrayPtr<const capnp::byte> name,
    uint32_t secondValue) {
  uint64_t value = id;
  value = std::rotl(value, 13) ^ state;
  value = std::rotl(value, 19) ^ uint64_t{name.size()};
  value = std::rotl(value, 23)
      ^ static_cast<uint64_t>(name.size() == 0 ? 0 : name[name.size() - 1]);
  return std::rotl(value, 29) ^ secondValue;
}

uint64_t directEvolutionFingerprint(capnp::AnyStruct::Reader root) {
  auto data = root.getDataSection();
  auto pointers = root.getPointerSection();
  return evolutionFingerprint(
      readData<uint32_t>(data, 0), readData<uint16_t>(data, 2),
      pointers[0].getAs<capnp::Text>().asBytes(),
      pointers[1].getAs<capnp::List<uint32_t>>()[1]);
}

uint64_t generatedEvolutionFingerprint(Record::Reader root) {
  return evolutionFingerprint(
      root.getId(), static_cast<uint16_t>(root.getState()),
      root.getName().asBytes(), root.getValues()[1]);
}

uint64_t textFingerprint(kj::ArrayPtr<const capnp::byte> text) {
  auto value = std::rotl(uint64_t{text.size()}, 17);
  return value ^ std::rotl(
      static_cast<uint64_t>(text.size() == 0 ? 0 : text[text.size() - 1]), 31);
}

uint64_t directDefaultFingerprint(capnp::AnyStruct::Reader root) {
  auto pointers = root.getPointerSection();
  if (pointers.size() > 25 && !pointers[25].isNull()) {
    return textFingerprint(pointers[25].getAs<capnp::Text>().asBytes());
  }
  constexpr auto defaultText = std::string_view("default text");
  return textFingerprint(kj::arrayPtr(
      reinterpret_cast<const capnp::byte*>(defaultText.data()),
      defaultText.size()));
}

uint64_t generatedDefaultFingerprint(WireFixture::Reader root) {
  return textFingerprint(root.getDefaultText().asBytes());
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
    std::cerr << "usage: cpp-generated-api direct-scalars|generated-scalars|borrowed-direct-scalars|borrowed-scalars|direct-blobs|generated-blobs|borrowed-direct-blobs|borrowed-blobs|borrowed-direct-groups|borrowed-groups|borrowed-direct-lists|borrowed-lists|borrowed-direct-nested|borrowed-nested|borrowed-direct-struct-lists|borrowed-struct-lists|borrowed-direct-evolution|borrowed-evolution|borrowed-direct-defaults|borrowed-defaults|direct-builder-scalars|generated-builder-scalars|direct-builder-blobs|generated-builder-blobs|direct-builder-struct|generated-builder-struct|direct-builder-list|generated-builder-list PASSES FIXTURE\n";
    return 2;
  }
  auto mode = std::string_view(argv[1]);
  auto passes = parseSize(argv[2]);
  if (mode == "direct-builder-scalars") {
    benchmarkDirectBuilder(passes);
    return 0;
  }
  if (mode == "generated-builder-scalars") {
    benchmarkGeneratedBuilder(passes);
    return 0;
  }
  if (mode == "direct-builder-blobs") {
    benchmarkDirectBlobBuilder(passes);
    return 0;
  }
  if (mode == "generated-builder-blobs") {
    benchmarkGeneratedBlobBuilder(passes);
    return 0;
  }
  if (mode == "direct-builder-struct") {
    benchmarkDirectStructBuilder(passes);
    return 0;
  }
  if (mode == "generated-builder-struct") {
    benchmarkGeneratedStructBuilder(passes);
    return 0;
  }
  if (mode == "direct-builder-list") {
    benchmarkDirectListBuilder(passes);
    return 0;
  }
  if (mode == "generated-builder-list") {
    benchmarkGeneratedListBuilder(passes);
    return 0;
  }
  auto words = mode == "borrowed-direct-defaults" || mode == "borrowed-defaults"
      ? emptyMessageWords()
      : readWords(argv[3]);
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
  } else if (mode == "borrowed-direct-nested") {
    checksum = measure(message.getRoot<capnp::AnyStruct>(), directNestedFingerprint, passes);
  } else if (mode == "borrowed-nested") {
    checksum = measure(message.getRoot<WireFixture>(), generatedNestedFingerprint, passes);
  } else if (mode == "borrowed-direct-struct-lists") {
    checksum = measure(message.getRoot<capnp::AnyStruct>(), directStructListFingerprint, passes);
  } else if (mode == "borrowed-struct-lists") {
    checksum = measure(message.getRoot<WireFixture>(), generatedStructListFingerprint, passes);
  } else if (mode == "borrowed-direct-evolution") {
    checksum = measure(message.getRoot<capnp::AnyStruct>(), directEvolutionFingerprint, passes);
  } else if (mode == "borrowed-evolution") {
    checksum = measure(message.getRoot<Record>(), generatedEvolutionFingerprint, passes);
  } else if (mode == "borrowed-direct-defaults") {
    checksum = measure(message.getRoot<capnp::AnyStruct>(), directDefaultFingerprint, passes);
  } else if (mode == "borrowed-defaults") {
    checksum = measure(message.getRoot<WireFixture>(), generatedDefaultFingerprint, passes);
  } else {
    std::cerr << "unknown benchmark mode\n";
    return 2;
  }
  auto elapsed = std::chrono::duration_cast<std::chrono::nanoseconds>(
      std::chrono::steady_clock::now() - started);
  std::cout << elapsed.count() << '\t' << checksum << '\n';
  return 0;
}
