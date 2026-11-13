#include "Markers.h"

#include "Diagnostic.h"

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
  return std::nullopt;
}

std::optional<Marker> markerCall(const Value *value) {
  const auto *call = dyn_cast_or_null<CallBase>(value);
  if (!call) {
    return std::nullopt;
  }
  return markerOf(call->getCalledFunction());
}

void checkMarkers(const Module &module) {
  for (const Function &function : module) {
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
              type->getParamType(1)->isDoubleTy();
      break;
    case MarkerKind::Measure:
      shape = type->getReturnType()->isIntegerTy(64) && type->getNumParams() == 1 && pointerFirst;
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
