#include "RelativeLoads.h"

#include <llvm/ADT/STLExtras.h>
#include <llvm/IR/Function.h>
#include <llvm/IR/IRBuilder.h>
#include <llvm/IR/IntrinsicInst.h>
#include <llvm/IR/Module.h>

#include <vector>

using namespace llvm;

namespace bennett {

void lowerRelativeLoads(Module &module) {
  std::vector<Function *> declarations;
  for (Function &function : module) {
    if (function.getIntrinsicID() == Intrinsic::load_relative) {
      declarations.push_back(&function);
    }
  }
  for (Function *declaration : declarations) {
    for (User *user : make_early_inc_range(declaration->users())) {
      auto *call = cast<CallInst>(user);
      IRBuilder<> builder(call);
      Value *base = call->getArgOperand(0);
      Value *entry = builder.CreateGEP(builder.getInt8Ty(), base, call->getArgOperand(1), "relative.entry");
      Value *offset = builder.CreateAlignedLoad(builder.getInt32Ty(), entry, Align(4), "relative.offset");
      Value *wide = builder.CreateSExt(offset, builder.getInt64Ty(), "relative.wide");
      Value *target = builder.CreateGEP(builder.getInt8Ty(), base, wide, "relative.target");
      call->replaceAllUsesWith(target);
      call->eraseFromParent();
    }
    declaration->eraseFromParent();
  }
}

} // namespace bennett
