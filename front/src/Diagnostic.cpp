#include "Diagnostic.h"

#include <llvm/IR/DebugInfoMetadata.h>
#include <llvm/IR/Function.h>
#include <llvm/IR/Instruction.h>
#include <llvm/Support/raw_ostream.h>

#include <cstdlib>

using namespace llvm;

namespace bennett {

std::string locationOf(const Value *at) {
  std::string text;
  raw_string_ostream out(text);
  if (const auto *inst = dyn_cast_or_null<Instruction>(at)) {
    if (const DILocation *loc = inst->getDebugLoc().get()) {
      out << loc->getFilename() << ":" << loc->getLine() << ":" << loc->getColumn();
      return out.str();
    }
    out << "in @" << inst->getFunction()->getName();
    return out.str();
  }
  if (const auto *function = dyn_cast_or_null<Function>(at)) {
    if (const DISubprogram *sub = function->getSubprogram()) {
      out << sub->getFilename() << ":" << sub->getLine() << " (@" << function->getName() << ")";
      return out.str();
    }
    out << "@" << function->getName();
    return out.str();
  }
  if (const auto *arg = dyn_cast_or_null<Argument>(at)) {
    return locationOf(arg->getParent());
  }
  return "an unknown location";
}

void notHandled(const std::string &what, const Value *at) {
  errs() << "not handled yet: " << what << ", at " << locationOf(at) << "\n";
  std::exit(3);
}

void misuse(const std::string &what, const Value *at) {
  errs() << "error: " << what << ", at " << locationOf(at) << "\n";
  std::exit(2);
}

void fail(const std::string &what) {
  errs() << "error: " << what << "\n";
  std::exit(2);
}

} // namespace bennett
