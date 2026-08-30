#include "ping.capnp.h"

#include <capnp/rpc-twoparty.h>
#include <kj/async-io.h>

#include <charconv>
#include <cstdint>
#include <iostream>
#include <string_view>

namespace {

class PingImpl final: public Ping::Server {
public:
  kj::Promise<void> ping(PingContext context) override {
    context.getResults().setValue(context.getParams().getValue() + 1);
    return kj::READY_NOW;
  }
};

uint64_t parseIterations(const char* text) {
  uint64_t result = 0;
  auto input = std::string_view(text);
  auto parsed = std::from_chars(input.begin(), input.end(), result);
  KJ_REQUIRE(parsed.ec == std::errc() && parsed.ptr == input.end(),
      "iterations must be an unsigned integer");
  return result;
}

}  // namespace

int main(int argc, char** argv) {
  KJ_REQUIRE(argc == 2, "usage: cpp-rpc-benchmark ITERATIONS");
  auto iterations = parseIterations(argv[1]);

  kj::EventLoop eventLoop;
  kj::WaitScope waitScope(eventLoop);
  auto pipe = kj::newTwoWayPipe();

  capnp::TwoPartyClient server(
      *pipe.ends[1], kj::heap<PingImpl>(), capnp::rpc::twoparty::Side::SERVER);
  capnp::TwoPartyClient client(*pipe.ends[0]);
  auto ping = client.bootstrap().castAs<Ping>();

  uint64_t checksum = 0;
  for (uint64_t index = 0; index < iterations; ++index) {
    auto request = ping.pingRequest();
    request.setValue(index);
    checksum ^= request.send().wait(waitScope).getValue();
  }

  std::cout << checksum << '\n';
  return 0;
}
