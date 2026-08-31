#include <capnp/rpc.capnp.h>
#include <capnp/serialize.h>
#include <kj/debug.h>

#include <unistd.h>

#include <array>
#include <cstring>

namespace {

void writeCall() {
  capnp::MallocMessageBuilder builder;
  auto call = builder.initRoot<capnp::rpc::Message>().initCall();
  call.setQuestionId(10);
  call.initTarget().setImportedCap(3);
  call.setInterfaceId(0xfeedbeef);
  call.setMethodId(6);
  auto caps = call.initParams().initCapTable(1);
  caps[0].setSenderPromise(4);
  capnp::writeMessageToFd(STDOUT_FILENO, builder);
}

void writeResolve() {
  capnp::MallocMessageBuilder builder;
  auto resolve = builder.initRoot<capnp::rpc::Message>().initResolve();
  resolve.setPromiseId(4);
  resolve.initCap().setSenderHosted(5);
  capnp::writeMessageToFd(STDOUT_FILENO, builder);
}

void writeDisembargo(bool sender) {
  capnp::MallocMessageBuilder builder;
  auto disembargo = builder.initRoot<capnp::rpc::Message>().initDisembargo();
  disembargo.initTarget().setImportedCap(4);
  if (sender) {
    disembargo.initContext().setSenderLoopback(77);
  } else {
    disembargo.initContext().setReceiverLoopback(77);
  }
  capnp::writeMessageToFd(STDOUT_FILENO, builder);
}

void verify() {
  {
    capnp::StreamFdMessageReader reader(STDIN_FILENO);
    auto message = reader.getRoot<capnp::rpc::Message>();
    KJ_REQUIRE(message.isCall());
    auto call = message.getCall();
    KJ_REQUIRE(call.getQuestionId() == 10);
    KJ_REQUIRE(call.getTarget().isImportedCap() &&
               call.getTarget().getImportedCap() == 3);
    KJ_REQUIRE(call.getInterfaceId() == 0xfeedbeef);
    KJ_REQUIRE(call.getMethodId() == 6);
    auto caps = call.getParams().getCapTable();
    KJ_REQUIRE(caps.size() == 1 && caps[0].isSenderPromise() &&
               caps[0].getSenderPromise() == 4);
  }
  {
    capnp::StreamFdMessageReader reader(STDIN_FILENO);
    auto resolve = reader.getRoot<capnp::rpc::Message>().getResolve();
    KJ_REQUIRE(resolve.getPromiseId() == 4);
    KJ_REQUIRE(resolve.isCap());
    KJ_REQUIRE(resolve.getCap().isSenderHosted() &&
               resolve.getCap().getSenderHosted() == 5);
  }
  for (bool sender : std::array<bool, 2>{true, false}) {
    capnp::StreamFdMessageReader reader(STDIN_FILENO);
    auto disembargo = reader.getRoot<capnp::rpc::Message>().getDisembargo();
    KJ_REQUIRE(disembargo.getTarget().isImportedCap() &&
               disembargo.getTarget().getImportedCap() == 4);
    auto context = disembargo.getContext();
    if (sender) {
      KJ_REQUIRE(context.isSenderLoopback() && context.getSenderLoopback() == 77);
    } else {
      KJ_REQUIRE(context.isReceiverLoopback() && context.getReceiverLoopback() == 77);
    }
  }
}

}  // namespace

int main(int argc, char** argv) {
  KJ_REQUIRE(argc == 2, "usage: m36-promise-resolution generate|verify");
  if (std::strcmp(argv[1], "generate") == 0) {
    writeCall();
    writeResolve();
    writeDisembargo(true);
    writeDisembargo(false);
    return 0;
  }
  KJ_REQUIRE(std::strcmp(argv[1], "verify") == 0);
  verify();
  return 0;
}
