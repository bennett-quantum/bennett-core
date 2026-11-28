#include "HandleCalls.h"

#include "Diagnostic.h"
#include "Emit.h"
#include "Primitives.h"

#include <llvm/IR/DerivedTypes.h>
#include <llvm/IR/Function.h>
#include <llvm/IR/InstrTypes.h>
#include <llvm/IR/Metadata.h>
#include <llvm/IR/Module.h>

#include <optional>
#include <string>

using namespace llvm;

namespace bennett {

namespace {

constexpr const char *applyTargetMark = "bennett.apply_target";

// One pointer parameter and no result, not variadic: the compiled signature of `void f(void *)`.
bool hasApplySignature(const Function &function) {
  const FunctionType *type = function.getFunctionType();
  return type->getReturnType()->isVoidTy() && !type->isVarArg() && type->getNumParams() == 1 &&
         type->getParamType(0)->isPointerTy();
}

bool addressTaken(const Function &function) {
  for (const Use &use : function.uses()) {
    const auto *call = dyn_cast<CallBase>(use.getUser());
    if (!call || !call->isCallee(&use)) {
      return true;
    }
  }
  return false;
}

// The name of the primitive `call` calls directly, if it does.
std::optional<Primitive> directPrimitive(const CallBase &call) {
  const Function *callee = call.getCalledFunction();
  if (!callee || !callee->isDeclaration()) {
    return std::nullopt;
  }
  return primitiveNamed(callee->getName());
}

} // namespace

void markApplyTargets(Module &module) {
  for (Function &function : module) {
    if (function.isDeclaration() || primitiveNamed(function.getName()) || !hasApplySignature(function)) {
      continue;
    }
    function.setMetadata(applyTargetMark, MDNode::get(module.getContext(), {}));
  }
}

bool isApplyTarget(const Function &function) {
  if (!function.getMetadata(applyTargetMark)) {
    return false;
  }
  if (hasApplySignature(function)) {
    return true;
  }
  if (addressTaken(function)) {
    fail("internal: the apply target @" + function.getName().str() +
         " has its address taken and lost the type one pointer parameter and no result");
  }
  return false;
}

bool translateInterfaceCall(BodyWriter &body, const CallBase &call) {
  std::optional<Primitive> primitive = directPrimitive(call);
  if (!primitive) {
    return false;
  }
  std::string name = primitive->name;
  // The call's operands as the writer names them; checkCallShape (Primitives.h) has made sure the call has the
  // declaration's type, so the count is the declaration's.
  auto arg = [&](unsigned i) {
    if (i >= call.arg_size()) {
      fail("internal: the call of @" + name + " has " + std::to_string(call.arg_size()) + " operands");
    }
    return body.operand(call.getArgOperand(i));
  };
  if (name == "bq_alloc") {
    body.line(body.slot(&call) + " = qalloc i64 " + arg(0));
  } else if (name == "bq_free") {
    body.line("qfree " + arg(0));
  } else if (name == "bq_width") {
    body.line(body.slot(&call) + " = qwidth " + arg(0));
  } else if (name == "bq_measure") {
    body.line(body.slot(&call) + " = measure i64 " + arg(0) + "[" + arg(1) + ", " + arg(2) + "]");
  } else if (name == "bq_gate") {
    body.line("gate(" + arg(0) + ") " + arg(1) + "[" + arg(2) + "] ctrl list(" + arg(3) + ", " + arg(4) + ")");
  } else if (name == "bq_rotate") {
    body.line("rotate(" + arg(0) + ", " + arg(1) + ") " + arg(2) + "[" + arg(3) + "] ctrl list(" + arg(4) + ", " +
              arg(5) + ")");
  } else if (name == "bq_apply") {
    body.line("apply " + arg(0) + " budget " + arg(1) + ", fault " + arg(2) + ", image(" + arg(3) + ", " + arg(4) +
              "), fields(" + arg(5) + ", " + arg(6) + "), ctrl list(" + arg(7) + ", " + arg(8) + ")");
  } else if (name == "__bennett_bad_argument") {
    body.line("fault bad_argument");
  } else {
    return false; // the output, exit and out-of-memory primitives: the writer's own lowering
  }
  return true;
}

bool endsBlock(const CallBase &call) {
  std::optional<Primitive> primitive = directPrimitive(call);
  return primitive && std::string(primitive->name) == "__bennett_bad_argument";
}

} // namespace bennett
