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

}  // namespace

int main(int argc, char** argv) {
  if (argc != 4) {
    std::cerr << "usage: cpp-reflection schema-name|schema-index|dynamic-name|dynamic-index PASSES FIXTURE\n";
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
