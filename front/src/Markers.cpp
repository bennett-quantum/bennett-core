#include "Markers.h"

#include "Diagnostic.h"

#include <llvm/IR/Constants.h>
#include <llvm/IR/Function.h>
#include <llvm/IR/InstrTypes.h>
#include <llvm/IR/Module.h>

using namespace llvm;

namespace bennett {

namespace {

constexpr const char *prefix = "bennett_";

const char *const gates[] = {"x", "h", "z", "s", "sdg", "t", "tdg"};
const char *const rotations[] = {"rx", "ry", "rz"};

} // namespace

std::optional<Marker> markerOf(const Function *callee) {
  if (!callee || !callee->getName().starts_with(prefix)) {
    return std::nullopt;
  }
  StringRef name = callee->getName().drop_front(StringRef(prefix).size());
  for (const char *gate : gates) {
    if (name == gate) {
      return Marker{MarkerKind::Gate, gate};
    }
  }
  for (const char *axis : rotations) {
    if (name == axis) {
      return Marker{MarkerKind::Rotation, axis};
    }
  }
  if (name == "measure") {
    return Marker{MarkerKind::Measure, "measure"};
  }
  if (name == "measure_bytes") {
    return Marker{MarkerKind::MeasureBytes, "measure"};
  }
  if (name == "bit") {
    return Marker{MarkerKind::Bit, "bit"};
  }
  return std::nullopt;
}

bool isGateCall(const Value *value) {
  std::optional<Marker> marker = markerCall(value);
  return marker && (marker->kind == MarkerKind::Gate || marker->kind == MarkerKind::Rotation);
}

std::optional<double> rotationAngle(const CallBase &call) {
  const auto *bits = dyn_cast<ConstantInt>(call.getArgOperand(1));
  if (!bits || bits->getBitWidth() != 64) {
    return std::nullopt;
  }
  return bits->getValue().bitsToDouble();
}

std::optional<Marker> markerCall(const Value *value) {
  const auto *call = dyn_cast_or_null<CallBase>(value);
  if (!call) {
    return std::nullopt;
  }
  return markerOf(call->getCalledFunction());
}

void checkMarkers(const Module &module, bool floatsAreBits) {
  // The name must be a plain declaration: an alias or ifunc of that name would define it outside the function
  // list checked below, and a call would reach the program's own code instead of the fault.
  if (const GlobalValue *named = module.getNamedValue("__bennett_out_of_memory"); named && !isa<Function>(named)) {
    misuse("the device function @__bennett_out_of_memory is defined by an alias or as data; it is only declared, "
           "_Noreturn void __bennett_out_of_memory(void)",
           named);
  }
  for (const Function &function : module) {
    if (function.getName() == "__bennett_out_of_memory") {
      // The C runtime's allocator calls it when its arena cannot hold a request: `fault out_of_memory`.
      FunctionType *type = function.getFunctionType();
      if (!function.isDeclaration() || !type->getReturnType()->isVoidTy() || type->getNumParams() != 0 ||
          type->isVarArg() || !function.doesNotReturn()) {
        misuse("the device function @__bennett_out_of_memory has the wrong signature or is defined; it is "
               "_Noreturn void __bennett_out_of_memory(void)",
               &function);
      }
      if (function.onlyReadsMemory() || function.willReturn() || function.isSpeculatable()) {
        misuse("the device function @__bennett_out_of_memory has attributes that let LLVM remove or move its "
               "calls",
               &function);
      }
      // Attributes on one call license removing that call just as the declaration's would.
      for (const User *user : function.users()) {
        const auto *call = dyn_cast<CallBase>(user);
        if (call && call->getCalledOperand() == &function &&
            (call->onlyReadsMemory() || call->hasFnAttr(Attribute::WillReturn))) {
          misuse("a call of the device function @__bennett_out_of_memory has attributes that let LLVM remove or "
                 "move it",
                 call);
        }
      }
      continue;
    }
    if (function.getName() == "__bennett_effect" || function.getName() == "__bennett_exit") {
      bool exits = function.getName() == "__bennett_exit";
      std::string name = "@" + function.getName().str();
      FunctionType *type = function.getFunctionType();
      if (!function.isDeclaration() || !type->getReturnType()->isVoidTy() || type->getNumParams() != 1 ||
          !type->getParamType(0)->isIntegerTy(exits ? 32 : 8) || type->isVarArg() ||
          (exits && !function.doesNotReturn())) {
        misuse("the device function " + name + " has the wrong signature or is defined; include bennett.h",
               &function);
      }
      if (function.onlyReadsMemory() || function.willReturn() || function.isSpeculatable()) {
        misuse("the device function " + name + " has attributes that let LLVM remove or move its calls",
               &function);
      }
      continue; // Device effects are ordinary calls, never quantum markers.
    }
    if (!function.getName().starts_with(prefix)) {
      continue;
    }
    std::optional<Marker> marker = markerOf(&function);
    std::string name = "@" + function.getName().str();
    if (!marker) {
      misuse(name + " is not a Bennett marker (see docs/MARKERS.md)", &function);
    }
    if (!function.isDeclaration()) {
      misuse("the marker " + name + " is defined; markers are only declared", &function);
    }
    FunctionType *type = function.getFunctionType();
    bool pointerFirst = type->getNumParams() >= 1 && type->getParamType(0)->isPointerTy();
    bool shape = false;
    switch (marker->kind) {
    case MarkerKind::Gate:
      shape = type->getReturnType()->isVoidTy() && type->getNumParams() == 1 && pointerFirst;
      break;
    case MarkerKind::Rotation:
      shape = type->getReturnType()->isVoidTy() && type->getNumParams() == 2 && pointerFirst &&
              (floatsAreBits ? type->getParamType(1)->isIntegerTy(64) : type->getParamType(1)->isDoubleTy());
      break;
    case MarkerKind::Measure:
      shape = type->getReturnType()->isIntegerTy(64) && type->getNumParams() == 1 && pointerFirst;
      break;
    case MarkerKind::MeasureBytes:
      shape = type->getReturnType()->isIntegerTy(64) && type->getNumParams() == 2 && pointerFirst &&
              type->getParamType(1)->isIntegerTy(32);
      break;
    case MarkerKind::Bit:
      shape = type->getReturnType()->isPointerTy() && type->getNumParams() == 2 && pointerFirst &&
              type->getParamType(1)->isIntegerTy();
      break;
    }
    if (!shape || type->isVarArg()) {
      misuse("the marker " + name + " has the wrong signature; include bennett.h", &function);
    }
    // A marker may read and write memory and may not return: then stock passes keep the dynamic sequence of
    // its calls. Any of these attributes would license deleting, merging or speculating a call.
    if (function.onlyReadsMemory() || function.willReturn() || function.isSpeculatable()) {
      misuse("the marker " + name + " is declared with attributes that let LLVM remove or move its calls",
             &function);
    }
  }
}

} // namespace bennett
