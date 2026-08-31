#include <capnp/rpc.capnp.h>
#include <capnp/serialize.h>
#include <kj/debug.h>

#include <fcntl.h>
#include <unistd.h>

#include <cstring>

namespace {

void check(capnp::rpc::Message::Reader message) {
  KJ_REQUIRE(message.isCall());
  auto call = message.getCall();
  KJ_REQUIRE(call.getQuestionId() == 10);
  KJ_REQUIRE(call.getTarget().isImportedCap());
  KJ_REQUIRE(call.getTarget().getImportedCap() == 12);
  KJ_REQUIRE(call.getInterfaceId() == 0xfeed);
  KJ_REQUIRE(call.getMethodId() == 5);
  auto caps = call.getParams().getCapTable();
  KJ_REQUIRE(caps.size() == 4);
  KJ_REQUIRE(caps[0].isSenderHosted() && caps[0].getSenderHosted() == 4);
  KJ_REQUIRE(caps[1].isSenderHosted() && caps[1].getSenderHosted() == 4);
  KJ_REQUIRE(caps[2].isReceiverHosted() && caps[2].getReceiverHosted() == 9);
  KJ_REQUIRE(caps[3].isNone());
}

}  // namespace

int main(int argc, char** argv) {
  KJ_REQUIRE(argc == 2, "usage: m34-capability-interop generate|verify");
  if (std::strcmp(argv[1], "generate") == 0) {
    capnp::MallocMessageBuilder builder;
    auto call = builder.initRoot<capnp::rpc::Message>().initCall();
    call.setQuestionId(10);
    call.getTarget().setImportedCap(12);
    call.setInterfaceId(0xfeed);
    call.setMethodId(5);
    auto caps = call.initParams().initCapTable(4);
    caps[0].setSenderHosted(4);
    caps[1].setSenderHosted(4);
    caps[2].setReceiverHosted(9);
    caps[3].setNone();
    capnp::writeMessageToFd(STDOUT_FILENO, builder);
    return 0;
  }
  KJ_REQUIRE(std::strcmp(argv[1], "verify") == 0);
  capnp::StreamFdMessageReader reader(STDIN_FILENO);
  check(reader.getRoot<capnp::rpc::Message>());
  return 0;
}
