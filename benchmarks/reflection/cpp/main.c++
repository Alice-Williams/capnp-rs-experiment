#include "conformance/schemas/wire-fixture.capnp.h"

#include <capnp/dynamic.h>
#include <capnp/serialize.h>

#include <array>
#include <bit>
#include <charconv>
#include <chrono>
#include <cstdint>
#include <fstream>
#include <iostream>
#include <limits>
#include <stdexcept>
#include <string_view>

namespace {

constexpr uint64_t SEED = 0x4d595df4d0f33173ull;
constexpr std::array<std::string_view, 4> SCALAR_NAMES = {
    "uint8Value", "uint16Value", "uint32Value", "uint64Value"};

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
  if (end <= 0) throw std::runtime_error("fixture is empty");
  auto byteSize = static_cast<size_t>(end);
  if (byteSize % sizeof(capnp::word) != 0) {
    throw std::runtime_error("fixture is not a word array");
  }
  auto words = kj::heapArray<capnp::word>(byteSize / sizeof(capnp::word));
  input.seekg(0);
  input.read(reinterpret_cast<char*>(words.begin()),
             static_cast<std::streamsize>(byteSize));
  if (!input) throw std::runtime_error("failed to read fixture");
  return words;
}

uint64_t dynamicScalar(capnp::DynamicValue::Reader value, size_t selector) {
  switch (selector) {
    case 0: return value.as<uint8_t>();
    case 1: return value.as<uint16_t>();
    case 2: return value.as<uint32_t>();
    case 3: return value.as<uint64_t>();
    default: throw std::logic_error("invalid scalar selector");
  }
}

uint64_t blobFingerprint(
    kj::ArrayPtr<const capnp::byte> text,
    kj::ArrayPtr<const capnp::byte> data) {
  uint64_t value = std::rotl(uint64_t{text.size()}, 11)
      ^ std::rotl(uint64_t{data.size()}, 23);
  if (text.size() != 0) {
    value ^= std::rotl(uint64_t{text.front()}, 31)
        ^ std::rotl(uint64_t{text.back()}, 37);
  }
  if (data.size() != 0) {
    value ^= std::rotl(uint64_t{data.front()}, 43)
        ^ std::rotl(uint64_t{data.back()}, 47);
  }
  return value;
}

uint32_t readU32(const capnp::byte* bytes) {
  return uint32_t{bytes[0]} | (uint32_t{bytes[1]} << 8)
      | (uint32_t{bytes[2]} << 16) | (uint32_t{bytes[3]} << 24);
}

uint64_t readU64(const capnp::byte* bytes) {
  return uint64_t{readU32(bytes)} | (uint64_t{readU32(bytes + 4)} << 32);
}

kj::Array<capnp::word> makeUnknownUnionWords(uint32_t discriminantOffset) {
  capnp::MallocMessageBuilder builder;
  builder.initRoot<WireFixture>();
  auto words = capnp::messageToFlatArray(builder);
  auto* bytes = reinterpret_cast<capnp::byte*>(words.begin());
  auto segmentCount = size_t{readU32(bytes)} + 1;
  if (segmentCount != 1) {
    throw std::runtime_error("unknown-union fixture is not one segment");
  }
  auto rootWordIndex = (segmentCount + 2) / 2;
  auto rootPointer = readU64(bytes + rootWordIndex * sizeof(capnp::word));
  if ((rootPointer & 3) != 0) {
    throw std::runtime_error("unknown-union root is not a struct pointer");
  }
  auto rawOffset = static_cast<uint32_t>(rootPointer >> 2) & 0x3fffffffu;
  auto signedOffset = (rawOffset & 0x20000000u) == 0
      ? int64_t{rawOffset}
      : int64_t{rawOffset} - (int64_t{1} << 30);
  auto contentWord = static_cast<int64_t>(rootWordIndex + 1) + signedOffset;
  auto byteOffset = contentWord * int64_t{sizeof(capnp::word)}
      + int64_t{discriminantOffset} * 2;
  if (contentWord < 0 || byteOffset < 0
      || static_cast<size_t>(byteOffset + 2) > words.size() * sizeof(capnp::word)) {
    throw std::runtime_error("unknown-union discriminant is out of bounds");
  }
  bytes[byteOffset] = 55;
  bytes[byteOffset + 1] = 0;
  return words;
}

}  // namespace

int main(int argc, char** argv) {
  if (argc != 4) {
    std::cerr << "usage: cpp-reflection schema-name|schema-index|dynamic-name|dynamic-index|dynamic-field|dynamic-blobs-borrowed|dynamic-blobs-owned|dynamic-primitive-list|dynamic-nested-struct|dynamic-struct-list|dynamic-nested-list|dynamic-enum|dynamic-default|dynamic-union-active|dynamic-union-unknown PASSES FIXTURE\n";
    return 2;
  }
  auto mode = std::string_view(argv[1]);
  auto passes = parseSize(argv[2]);
  auto words = readWords(argv[3]);
  capnp::ReaderOptions options;
  options.traversalLimitInWords = std::numeric_limits<uint64_t>::max();
  capnp::FlatArrayMessageReader message(words.asPtr(), options);
  auto root = message.getRoot<WireFixture>();
  auto dynamic = capnp::toDynamic(root);
  auto schema = capnp::Schema::from<WireFixture>();
  auto fields = schema.getFields();
  std::array<capnp::StructSchema::Field, 4> selectedFields;
  for (size_t index = 0; index < selectedFields.size(); ++index) {
    selectedFields[index] = schema.getFieldByName(
        kj::StringPtr(SCALAR_NAMES[index].data(), SCALAR_NAMES[index].size()));
  }
  auto textField = schema.getFieldByName("text");
  auto dataField = schema.getFieldByName("data");
  auto uint16sField = schema.getFieldByName("uint16s");
  auto nodeField = schema.getFieldByName("node");
  auto structsField = schema.getFieldByName("structs");
  auto nestedListsField = schema.getFieldByName("nestedLists");
  auto colorField = schema.getFieldByName("color");
  auto defaultedField = schema.getFieldByName("defaulted");
  auto choiceField = schema.getFieldByName("choice");
  auto nodeValueField = capnp::Schema::from<Node>().getFieldByName("value");
  auto choiceSchema = choiceField.getType().asStruct();
  auto unknownWords = makeUnknownUnionWords(
      choiceSchema.getProto().getStruct().getDiscriminantOffset());
  capnp::FlatArrayMessageReader unknownMessage(unknownWords.asPtr(), options);
  auto unknownRoot = unknownMessage.getRoot<WireFixture>();
  auto unknownDynamic = capnp::toDynamic(unknownRoot);

  auto started = std::chrono::steady_clock::now();
  uint64_t checksum = SEED;
  for (size_t pass = 0; pass < passes; ++pass) {
    auto selector = pass & 3;
    auto name = kj::StringPtr(
        SCALAR_NAMES[selector].data(), SCALAR_NAMES[selector].size());
    auto dynamicPointer = &dynamic;
    auto schemaPointer = &schema;
    asm volatile("" : "+r"(dynamicPointer), "+r"(schemaPointer) : : "memory");
    uint64_t observed;
    if (mode == "schema-name") {
      observed = schemaPointer->getFieldByName(name).getProto().getCodeOrder();
    } else if (mode == "schema-index") {
      observed = fields[selectedFields[selector].getIndex()].getProto().getCodeOrder();
    } else if (mode == "dynamic-name") {
      observed = dynamicScalar(dynamicPointer->get(name), selector);
    } else if (mode == "dynamic-index") {
      observed = dynamicScalar(
          dynamicPointer->get(fields[selectedFields[selector].getIndex()]), selector);
    } else if (mode == "dynamic-field") {
      observed = dynamicScalar(dynamicPointer->get(selectedFields[selector]), selector);
    } else if (mode == "dynamic-blobs-borrowed") {
      auto text = dynamicPointer->get(textField).as<capnp::Text>().asBytes();
      auto data = dynamicPointer->get(dataField).as<capnp::Data>();
      observed = blobFingerprint(text, data);
    } else if (mode == "dynamic-blobs-owned") {
      auto sourceText = dynamicPointer->get(textField).as<capnp::Text>().asBytes();
      auto sourceData = dynamicPointer->get(dataField).as<capnp::Data>();
      auto text = kj::heapArray<capnp::byte>(sourceText.size());
      auto data = kj::heapArray<capnp::byte>(sourceData.size());
      text.asPtr().copyFrom(sourceText);
      data.asPtr().copyFrom(sourceData);
      observed = blobFingerprint(text, data);
    } else if (mode == "dynamic-primitive-list") {
      auto list = dynamicPointer->get(uint16sField).as<capnp::DynamicList>();
      observed = list[2].as<uint16_t>();
    } else if (mode == "dynamic-nested-struct") {
      auto child = dynamicPointer->get(nodeField).as<capnp::DynamicStruct>();
      observed = child.get(nodeValueField).as<uint32_t>();
    } else if (mode == "dynamic-struct-list") {
      auto list = dynamicPointer->get(structsField).as<capnp::DynamicList>();
      auto child = list[1].as<capnp::DynamicStruct>();
      observed = child.get(nodeValueField).as<uint32_t>();
    } else if (mode == "dynamic-nested-list") {
      auto outer = dynamicPointer->get(nestedListsField).as<capnp::DynamicList>();
      auto inner = outer[0].as<capnp::DynamicList>();
      observed = inner[2].as<uint16_t>();
    } else if (mode == "dynamic-enum") {
      observed = dynamicPointer->get(colorField).as<capnp::DynamicEnum>().getRaw();
    } else if (mode == "dynamic-default") {
      observed = dynamicPointer->get(defaultedField).as<uint32_t>();
    } else if (mode == "dynamic-union-active") {
      auto choice = dynamicPointer->get(choiceField).as<capnp::DynamicStruct>();
      auto active = KJ_REQUIRE_NONNULL(choice.which());
      auto discriminant = active.getProto().getDiscriminantValue();
      observed = std::rotl(uint64_t{discriminant}, 17)
          ^ choice.get(active).as<uint64_t>();
    } else if (mode == "dynamic-union-unknown") {
      auto choice = unknownDynamic.get(choiceField).as<capnp::DynamicStruct>();
      if (choice.which() != kj::none) {
        throw std::runtime_error("unknown union resolved to a known field");
      }
      observed = static_cast<uint16_t>(unknownRoot.getChoice().which());
    } else {
      std::cerr << "unknown benchmark mode\n";
      return 2;
    }
    checksum = std::rotl(checksum, 9) ^ observed;
  }
  auto elapsed = std::chrono::duration_cast<std::chrono::nanoseconds>(
      std::chrono::steady_clock::now() - started);
  std::cout << elapsed.count() << '\t' << checksum << '\n';
  return 0;
}
