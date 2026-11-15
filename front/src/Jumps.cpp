#include "Jumps.h"

#include "Diagnostic.h"

#include <llvm/IR/IRBuilder.h>
#include <llvm/IR/InstIterator.h>
#include <llvm/IR/IntrinsicInst.h>
#include <llvm/IR/Module.h>
#include <llvm/Transforms/Utils/BasicBlockUtils.h>
#include <llvm/Transforms/Utils/Local.h>

#include <map>
#include <string>
#include <vector>

using namespace llvm;

namespace bennett {

namespace {

enum class Kind { Setjmp, Sigsetjmp, Longjmp, BuiltinSetjmp, BuiltinLongjmp, FrameAddress };

// The kind of a call to `callee`, or nothing for any other call.
std::optional<Kind> kindOf(const Function *callee) {
  if (!callee) {
    return std::nullopt;
  }
  switch (callee->getIntrinsicID()) {
  case Intrinsic::eh_sjlj_setjmp:
    return Kind::BuiltinSetjmp;
  case Intrinsic::eh_sjlj_longjmp:
    return Kind::BuiltinLongjmp;
  case Intrinsic::frameaddress:
    return Kind::FrameAddress;
  default:
    break;
  }
  if (!callee->isDeclaration() || callee->isIntrinsic()) {
    return std::nullopt;
  }
  static const std::map<std::string, Kind> library = {
      {"setjmp", Kind::Setjmp},       {"_setjmp", Kind::Setjmp},        {"sigsetjmp", Kind::Sigsetjmp},
      {"__sigsetjmp", Kind::Sigsetjmp}, {"longjmp", Kind::Longjmp},       {"_longjmp", Kind::Longjmp},
      {"siglongjmp", Kind::Longjmp},  {"__longjmp_chk", Kind::Longjmp},
  };
  auto found = library.find(callee->getName().str());
  return found == library.end() ? std::nullopt : std::optional<Kind>(found->second);
}

// Whether `type` is the prototype C gives a function of `kind`.
bool expectedType(Kind kind, const FunctionType *type) {
  auto pointerFirst = [&] { return type->getNumParams() >= 1 && type->getParamType(0)->isPointerTy(); };
  auto intParam = [&](unsigned i) { return type->getNumParams() > i && type->getParamType(i)->isIntegerTy(32); };
  if (type->isVarArg()) {
    return false;
  }
  switch (kind) {
  case Kind::Setjmp:
  case Kind::BuiltinSetjmp:
    return type->getReturnType()->isIntegerTy(32) && type->getNumParams() == 1 && pointerFirst();
  case Kind::Sigsetjmp:
    return type->getReturnType()->isIntegerTy(32) && type->getNumParams() == 2 && pointerFirst() && intParam(1);
  case Kind::Longjmp:
    return type->getReturnType()->isVoidTy() && type->getNumParams() == 2 && pointerFirst() && intParam(1);
  case Kind::BuiltinLongjmp:
    return type->getReturnType()->isVoidTy() && type->getNumParams() == 1 && pointerFirst();
  case Kind::FrameAddress:
    return true;
  }
  return false;
}

class Lowering {
public:
  explicit Lowering(Module &module) : module(module), context(module.getContext()) {}

  void run() {
    for (Function &function : module) {
      std::optional<Kind> kind = kindOf(&function);
      if (!kind || *kind == Kind::FrameAddress) {
        continue;
      }
      for (const Use &use : function.uses()) {
        const auto *call = dyn_cast<CallBase>(use.getUser());
        if (!call || !call->isCallee(&use)) {
          notHandled("the address of @" + function.getName().str() + " (it must be called directly)",
                     use.getUser());
        }
      }
    }
    std::vector<std::pair<CallBase *, Kind>> calls;
    for (Function &function : module) {
      for (Instruction &inst : instructions(function)) {
        auto *call = dyn_cast<CallBase>(&inst);
        if (!call) {
          continue;
        }
        const auto *callee = dyn_cast<Function>(call->getCalledOperand()->stripPointerCasts());
        std::optional<Kind> kind = kindOf(callee);
        if (!kind) {
          continue;
        }
        if (!isa<CallInst>(call)) {
          notHandled("an invoke of @" + callee->getName().str(), call);
        }
        if (call->getCalledFunction() != callee || !expectedType(*kind, call->getFunctionType())) {
          notHandled("a call of @" + callee->getName().str() + " through a prototype C does not give it", call);
        }
        calls.emplace_back(call, *kind);
      }
    }
    for (auto [call, kind] : calls) {
      if (kind == Kind::Sigsetjmp) {
        const auto *constant = dyn_cast<ConstantInt>(call->getArgOperand(1));
        masks = masks || !constant || !constant->isZero();
      }
    }
    for (auto [call, kind] : calls) {
      lower(*cast<CallInst>(call), kind);
    }
  }

private:
  Module &module;
  LLVMContext &context;
  std::map<Function *, Value *> frameAddresses;
  // Whether some `sigsetjmp` may save the mask. Only then does a library environment carry its flag, so a
  // program without one is lowered as before and needs no C library.
  bool masks = false;

  // The flag of the environment at `buffer`: 1 when its `sigsetjmp` saved the mask. It is the word C libraries
  // keep it in (`__fl` in musl's `jmp_buf`; glibc's `__mask_was_saved`), after the device record's 32 bytes and
  // before the saved mask, `__ss`, which `__sigsetjmp_tail` reads and writes.
  static Value *maskFlag(IRBuilder<> &b, Value *buffer) {
    return b.CreateConstInBoundsGEP1_64(b.getInt8Ty(), buffer, 64);
  }

  FunctionCallee sigsetjmpTail(IRBuilder<> &b, Value *buffer) {
    auto *type = FunctionType::get(b.getInt32Ty(), {buffer->getType(), b.getInt32Ty()}, false);
    return module.getOrInsertFunction("__sigsetjmp_tail", type);
  }

  Function *setjmpFunction() {
    auto *type = FunctionType::get(Type::getInt32Ty(context), {PointerType::get(context, 0)}, false);
    Function *function = cast<Function>(module.getOrInsertFunction(deviceSetjmp, type).getCallee());
    function->addFnAttr(Attribute::ReturnsTwice);
    function->addFnAttr(Attribute::NoUnwind);
    return function;
  }

  Function *longjmpFunction() {
    auto *type = FunctionType::get(Type::getVoidTy(context),
                                   {PointerType::get(context, 0), Type::getInt32Ty(context)}, false);
    Function *function = cast<Function>(module.getOrInsertFunction(deviceLongjmp, type).getCallee());
    function->addFnAttr(Attribute::NoReturn);
    function->addFnAttr(Attribute::NoUnwind);
    return function;
  }

  CallInst *callSetjmp(IRBuilder<> &b, Value *buffer) {
    CallInst *call = b.CreateCall(setjmpFunction(), {buffer});
    call->addFnAttr(Attribute::ReturnsTwice);
    return call;
  }

  // Replaces `call` by a jump through `buffer` with `value`, and drops what followed it in its block.
  void jump(CallInst &call, Value *buffer, Value *value) {
    IRBuilder<> b(&call);
    CallInst *jump = b.CreateCall(longjmpFunction(), {buffer, value});
    jump->setDoesNotReturn();
    jump->setDebugLoc(call.getDebugLoc());
    Instruction *next = call.getNextNode();
    call.eraseFromParent();
    if (!isa<UnreachableInst>(next)) {
      changeToUnreachable(next);
    }
  }

  void lower(CallInst &call, Kind kind) {
    IRBuilder<> b(&call);
    b.SetCurrentDebugLocation(call.getDebugLoc());
    switch (kind) {
    case Kind::BuiltinSetjmp: {
      CallInst *setjmp = callSetjmp(b, call.getArgOperand(0));
      call.replaceAllUsesWith(setjmp);
      call.eraseFromParent();
      return;
    }
    case Kind::Setjmp:
    case Kind::Sigsetjmp: {
      // Everything an environment needs is written into its buffer before the device `setjmp`, never after
      // it: after a jump the frame resumes with the slots of its latest visit of this call, which may have
      // been for another buffer or another savemask. With a nonzero savemask, C's sigsetjmp saves the mask
      // (musl's `__sigsetjmp_tail(buf, 0)`), and the flag says so; a jump restores it from its own target
      // (below, as glibc's longjmp does), so the mask travels with the buffer through copies and revisits.
      Value *buffer = call.getArgOperand(0);
      if (masks) {
        Value *saves = b.getInt64(0);
        if (kind == Kind::Sigsetjmp) {
          Value *saveMask = call.getArgOperand(1);
          const auto *constant = dyn_cast<ConstantInt>(saveMask);
          if (constant && !constant->isZero()) {
            saves = b.getInt64(1);
            b.CreateCall(sigsetjmpTail(b, buffer), {buffer, b.getInt32(0)});
          } else if (!constant) {
            Value *nonzero = b.CreateICmpNE(saveMask, b.getInt32(0));
            saves = b.CreateZExt(nonzero, b.getInt64Ty());
            Instruction *then = SplitBlockAndInsertIfThen(nonzero, &call, false);
            IRBuilder<> t(then);
            t.SetCurrentDebugLocation(call.getDebugLoc());
            t.CreateCall(sigsetjmpTail(t, buffer), {buffer, t.getInt32(0)});
            b.SetInsertPoint(&call);
          }
        }
        b.CreateStore(saves, maskFlag(b, buffer));
      }
      CallInst *setjmp = callSetjmp(b, buffer);
      call.replaceAllUsesWith(setjmp);
      call.eraseFromParent();
      return;
    }
    case Kind::Longjmp: {
      // C: a longjmp with value 0 makes setjmp return 1.
      Value *buffer = call.getArgOperand(0);
      Value *value = call.getArgOperand(1);
      Value *isZero = b.CreateICmpEQ(value, b.getInt32(0));
      Value *nonzero = b.CreateSelect(isZero, b.getInt32(1), value);
      if (masks) {
        // The target's saved mask, if it saved one, restored before the jump (glibc's order; musl restores it
        // on arrival, through the same buffer).
        Value *saved = b.CreateICmpNE(b.CreateLoad(b.getInt64Ty(), maskFlag(b, buffer)), b.getInt64(0));
        Instruction *then = SplitBlockAndInsertIfThen(saved, &call, false);
        IRBuilder<> t(then);
        t.SetCurrentDebugLocation(call.getDebugLoc());
        t.CreateCall(sigsetjmpTail(t, buffer), {buffer, t.getInt32(1)});
      }
      jump(call, buffer, nonzero);
      return;
    }
    case Kind::BuiltinLongjmp:
      jump(call, call.getArgOperand(0), b.getInt32(1));
      return;
    case Kind::FrameAddress: {
      const auto *level = dyn_cast<ConstantInt>(call.getArgOperand(0));
      if (!level || !level->isZero() || call.getType()->getPointerAddressSpace() != 0) {
        notHandled("the frame address of a caller (`__builtin_frame_address` above level 0)", &call);
      }
      Function *function = call.getFunction();
      Value *&address = frameAddresses[function];
      if (!address) {
        IRBuilder<> entry(&*function->getEntryBlock().getFirstInsertionPt());
        address = entry.CreateStackSave("frame.address");
      }
      call.replaceAllUsesWith(address);
      call.eraseFromParent();
      return;
    }
    }
  }
};

} // namespace

void lowerJumps(Module &module) { Lowering(module).run(); }

} // namespace bennett
