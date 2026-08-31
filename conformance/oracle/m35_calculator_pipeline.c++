#include <capnp/rpc.capnp.h>
#include <capnp/serialize.h>
#include <kj/debug.h>

#include <unistd.h>

#include <array>
#include <cstring>

namespace {

void writeBootstrap() {
  capnp::MallocMessageBuilder builder;
  builder.initRoot<capnp::rpc::Message>().initBootstrap().setQuestionId(100);
  capnp::writeMessageToFd(STDOUT_FILENO, builder);
}

void writePipelineCall(uint32_t questionId, uint32_t sourceId,
                       uint16_t methodId) {
  capnp::MallocMessageBuilder builder;
  auto call = builder.initRoot<capnp::rpc::Message>().initCall();
  call.setQuestionId(questionId);
  call.setInterfaceId(0xcafecafe);
  call.setMethodId(methodId);
  auto promised = call.initTarget().initPromisedAnswer();
  promised.setQuestionId(sourceId);
  promised.initTransform(1)[0].setGetPointerField(0);
  call.getSendResultsTo().setCaller();
  call.initParams().initCapTable(0);
  capnp::writeMessageToFd(STDOUT_FILENO, builder);
}

void verify() {
  std::array<uint32_t, 3> expected = {100, 0, 1};
  for (auto answerId : expected) {
    capnp::StreamFdMessageReader reader(STDIN_FILENO);
    auto message = reader.getRoot<capnp::rpc::Message>();
    KJ_REQUIRE(message.isReturn());
    auto returned = message.getReturn();
    KJ_REQUIRE(returned.getAnswerId() == answerId, answerId,
               returned.getAnswerId());
    KJ_REQUIRE(returned.isResults());
    if (answerId != 1) {
      auto caps = returned.getResults().getCapTable();
      KJ_REQUIRE(caps.size() == 1);
      KJ_REQUIRE(caps[0].isSenderHosted());
    }
  }
}

}  // namespace

int main(int argc, char** argv) {
  KJ_REQUIRE(argc == 2, "usage: m35-calculator-pipeline generate|verify");
  if (std::strcmp(argv[1], "generate") == 0) {
    writeBootstrap();
    writePipelineCall(0, 100, 0);  // getOperator() -> (function)
    writePipelineCall(1, 0, 1);    // function.evaluate(), before getOperator returns
    return 0;
  }
  KJ_REQUIRE(std::strcmp(argv[1], "verify") == 0);
  verify();
  return 0;
}
