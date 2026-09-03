#include <capnp/serialize-packed.h>
#include <kj/io.h>

#include <algorithm>
#include <bit>
#include <array>
#include <charconv>
#include <chrono>
#include <cstdint>
#include <fstream>
#include <iostream>
#include <iterator>
#include <string_view>
#include <vector>

namespace {

constexpr uint64_t SEED = 0x4d595df4d0f33173ull;
constexpr size_t STREAM_DECODE_CHUNK_BYTES = 1025;

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
  return value ^ (value << 17);
}

std::vector<uint8_t> readFixture(const char* path) {
  std::ifstream input(path, std::ios::binary);
  if (!input) throw std::invalid_argument("could not open realistic fixture");
  return std::vector<uint8_t>(
      std::istreambuf_iterator<char>(input), std::istreambuf_iterator<char>());
}

std::vector<uint8_t> makeInput(
    std::string_view shape, size_t words, const char* fixturePath) {
  if (words > SIZE_MAX / 8) throw std::overflow_error("input size overflow");
  std::vector<uint8_t> result(words * 8, 0);
  if (shape == "realistic") {
    auto fixture = readFixture(fixturePath);
    if (fixture.empty()) throw std::invalid_argument("realistic fixture is empty");
    for (size_t i = 0; i < result.size(); ++i) result[i] = fixture[i % fixture.size()];
    return result;
  }

  uint64_t state = SEED ^ words;
  for (size_t index = 0; index < result.size(); ++index) {
    state = xorshift(state);
    auto word = index / 8;
    auto lane = index % 8;
    bool nonzero;
    if (shape == "zero") {
      nonzero = false;
    } else if (shape == "raw") {
      nonzero = true;
    } else if (shape == "mixed") {
      switch (word % 8) {
        case 0: nonzero = false; break;
        case 1: nonzero = lane == 0 || lane == 3; break;
        case 2: nonzero = true; break;
        case 3: nonzero = lane != 4; break;
        case 4: nonzero = lane >= 2; break;
        case 5: nonzero = lane % 2 == 0; break;
        case 6: nonzero = true; break;
        default: nonzero = lane == 7; break;
      }
    } else {
      throw std::invalid_argument("unknown shape");
    }
    if (nonzero) result[index] = static_cast<uint8_t>(state) | 1;
  }
  return result;
}

std::vector<uint8_t> packOnce(const std::vector<uint8_t>& input) {
  kj::VectorOutputStream output(8);
  capnp::_::PackedOutputStream packed(output);
  packed.write(kj::arrayPtr(
      reinterpret_cast<const kj::byte*>(input.data()), input.size()));
  auto bytes = output.getArray();
  return std::vector<uint8_t>(bytes.begin(), bytes.end());
}

std::vector<uint8_t> unpackOnce(
    const std::vector<uint8_t>& packed, size_t outputSize) {
  kj::ArrayInputStream input(kj::arrayPtr(
      reinterpret_cast<const kj::byte*>(packed.data()), packed.size()));
  capnp::_::PackedInputStream unpacked(input);
  std::vector<uint8_t> output(outputSize);
  unpacked.read(kj::arrayPtr(
      reinterpret_cast<kj::byte*>(output.data()), output.size()));
  return output;
}

size_t streamChunkWords(std::string_view shape) {
  if (shape == "zero" || shape == "raw") return 256;
  if (shape == "mixed") return 8;
  if (shape == "realistic") return 100;
  throw std::invalid_argument("unknown shape");
}

std::vector<uint8_t> packStreaming(
    const std::vector<uint8_t>& input, std::string_view shape) {
  kj::VectorOutputStream output(8);
  capnp::_::PackedOutputStream packed(output);
  auto chunkBytes = streamChunkWords(shape) * 8;
  for (size_t offset = 0; offset < input.size(); offset += chunkBytes) {
    auto size = std::min(chunkBytes, input.size() - offset);
    packed.write(kj::arrayPtr(
        reinterpret_cast<const kj::byte*>(input.data() + offset), size));
  }
  auto bytes = output.getArray();
  return std::vector<uint8_t>(bytes.begin(), bytes.end());
}

std::vector<uint8_t> unpackStreaming(
    const std::vector<uint8_t>& packed, size_t outputSize) {
  kj::ArrayInputStream input(kj::arrayPtr(
      reinterpret_cast<const kj::byte*>(packed.data()), packed.size()));
  std::array<kj::byte, STREAM_DECODE_CHUNK_BYTES> inputBuffer;
  kj::BufferedInputStreamWrapper buffered(
      input, kj::arrayPtr(inputBuffer.data(), inputBuffer.size()));
  capnp::_::PackedInputStream unpacked(buffered);
  std::vector<uint8_t> output(outputSize);
  unpacked.read(kj::arrayPtr(
      reinterpret_cast<kj::byte*>(output.data()), output.size()));
  return output;
}

void blackBox(const void* value) {
  asm volatile("" : : "r"(value) : "memory");
}

uint64_t observe(uint64_t checksum, const uint8_t* bytes, size_t size) {
  checksum = std::rotl(checksum, 9) ^ size;
  if (size != 0) {
    checksum ^= std::rotl(uint64_t{bytes[0]}, 7);
    checksum ^= std::rotl(uint64_t{bytes[size - 1]}, 19);
  }
  return checksum;
}

uint64_t observe(uint64_t checksum, const std::vector<uint8_t>& bytes) {
  return observe(checksum, bytes.data(), bytes.size());
}

uint64_t fnv1a(const std::vector<uint8_t>& bytes) {
  uint64_t hash = 0xcbf29ce484222325ull;
  for (auto byte: bytes) hash = (hash ^ byte) * 0x00000100000001b3ull;
  return hash;
}

}  // namespace

int main(int argc, char** argv) {
  if (argc != 6) {
    std::cerr << "usage: cpp-packing-benchmark MODE SHAPE WORDS PASSES FIXTURE\n";
    return 2;
  }
  auto mode = std::string_view(argv[1]);
  auto shape = std::string_view(argv[2]);
  auto words = parseSize(argv[3]);
  auto passes = parseSize(argv[4]);
  auto input = makeInput(shape, words, argv[5]);
  auto packed = packOnce(input);
  if (unpackOnce(packed, input.size()) != input) {
    throw std::invalid_argument("packing fixture did not round trip");
  }
  if (packStreaming(input, shape) != packed) {
    throw std::invalid_argument("stream chunks changed the canonical packed bytes");
  }
  if (unpackStreaming(packed, input.size()) != input) {
    throw std::invalid_argument("streaming packing fixture did not round trip");
  }

  uint64_t checksum = SEED;
  auto started = std::chrono::steady_clock::now();
  if (mode == "copy-unpacked") {
    for (size_t pass = 0; pass < passes; ++pass) {
      auto output = input;
      blackBox(output.data());
      checksum = observe(checksum, output);
    }
  } else if (mode == "copy-packed") {
    for (size_t pass = 0; pass < passes; ++pass) {
      auto output = packed;
      blackBox(output.data());
      checksum = observe(checksum, output);
    }
  } else if (mode == "pack") {
    for (size_t pass = 0; pass < passes; ++pass) {
      kj::VectorOutputStream output(8);
      capnp::_::PackedOutputStream encoder(output);
      encoder.write(kj::arrayPtr(
          reinterpret_cast<const kj::byte*>(input.data()), input.size()));
      auto bytes = output.getArray();
      blackBox(bytes.begin());
      checksum = observe(
          checksum, reinterpret_cast<const uint8_t*>(bytes.begin()), bytes.size());
    }
  } else if (mode == "unpack") {
    for (size_t pass = 0; pass < passes; ++pass) {
      auto output = unpackOnce(packed, input.size());
      blackBox(output.data());
      checksum = observe(checksum, output);
    }
  } else if (mode == "pack-stream") {
    auto chunkBytes = streamChunkWords(shape) * 8;
    for (size_t pass = 0; pass < passes; ++pass) {
      kj::VectorOutputStream output(8);
      capnp::_::PackedOutputStream encoder(output);
      for (size_t offset = 0; offset < input.size(); offset += chunkBytes) {
        auto size = std::min(chunkBytes, input.size() - offset);
        encoder.write(kj::arrayPtr(
            reinterpret_cast<const kj::byte*>(input.data() + offset), size));
      }
      auto bytes = output.getArray();
      blackBox(bytes.begin());
      checksum = observe(
          checksum, reinterpret_cast<const uint8_t*>(bytes.begin()), bytes.size());
    }
  } else if (mode == "unpack-stream") {
    for (size_t pass = 0; pass < passes; ++pass) {
      auto output = unpackStreaming(packed, input.size());
      blackBox(output.data());
      checksum = observe(checksum, output);
    }
  } else {
    std::cerr << "unknown benchmark mode\n";
    return 2;
  }
  auto elapsed = std::chrono::duration_cast<std::chrono::nanoseconds>(
      std::chrono::steady_clock::now() - started);
  const auto& canonical =
      mode == "copy-packed" || mode == "pack" || mode == "pack-stream"
      ? packed : input;
  checksum ^= fnv1a(canonical);
  std::cout << elapsed.count() << '\t' << checksum << '\n';
  return 0;
}
