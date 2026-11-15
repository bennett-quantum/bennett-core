#include "Abi.h"

#include "Diagnostic.h"

#include <llvm/IR/Attributes.h>
#include <llvm/IR/IRBuilder.h>
#include <llvm/IR/InstIterator.h>
#include <llvm/IR/Instructions.h>
#include <llvm/IR/IntrinsicInst.h>
#include <llvm/IR/Module.h>
#include <llvm/TargetParser/Triple.h>

#include <algorithm>
#include <functional>
#include <set>
#include <string>
#include <vector>

using namespace llvm;

namespace bennett {

namespace {

// The x86-64 System V register save area: 6 general registers of 8 bytes, then 8 vector registers of 16.
constexpr unsigned generalRegisters = 6;
constexpr unsigned vectorRegisters = 8;
constexpr uint64_t generalBytes = 8;
constexpr uint64_t vectorBytes = 16;
constexpr uint64_t saveAreaBytes = generalRegisters * generalBytes + vectorRegisters * vectorBytes;
// A stack slot, and the alignment of the stack at a call.
constexpr uint64_t slotBytes = 8;
constexpr uint64_t stackAlign = 16;
// The fields of `__va_list_tag` { i32 gp_offset, i32 fp_offset, ptr overflow_arg_area, ptr reg_save_area }.
constexpr uint64_t fpOffsetField = 4;
constexpr uint64_t overflowField = 8;
constexpr uint64_t saveAreaField = 16;
constexpr uint64_t vaListBytes = 24;

// A function or call that now also reads or writes (`access`) memory through a pointer argument.
template <typename T> void allowArgumentMemory(T *at, ModRefInfo access) {
  at->setMemoryEffects(at->getMemoryEffects() | MemoryEffects::argMemOnly(access));
}

uint64_t roundUp(uint64_t value, uint64_t align) { return (value + align - 1) / align * align; }

// Where one argument goes: a general or vector register, or the stack.
struct Place {
  enum Kind { General, Vector, Stack } kind;
  uint64_t offset; // in the area: a register's slot in the save area, or the stack slot after it
};

// The registers and stack used so far, as LLVM's x86-64 backend assigns them (CC_X86_64_C).
class Assigner {
public:
  // Assigns the argument `arg` of `call` (or the parameter of type `type` with `byval` type `byval`).
  Place assign(Type *type, Type *byval, MaybeAlign align, const DataLayout &layout, const Value *at) {
    if (byval) {
      uint64_t a = std::max<uint64_t>(slotBytes, align.valueOrOne().value());
      if (a > stackAlign) {
        notHandled("a variadic argument aligned to more than " + std::to_string(stackAlign) + " bytes", at);
      }
      return stack(layout.getTypeAllocSize(byval).getFixedValue(), a);
    }
    if (type->isPointerTy() || (type->isIntegerTy() && type->getIntegerBitWidth() <= 64)) {
      if (general < generalRegisters) {
        return {Place::General, generalBytes * general++};
      }
      return stack(slotBytes, slotBytes);
    }
    if (type->isIntegerTy(128)) {
      // Two consecutive registers, or the stack, never split. When only one register is left, the
      // psABI leaves it for the next argument, and so does clang's inline va_arg, which is what reads
      // it. (LLVM's backend skips it instead, so a native call disagrees with its own callee there.)
      if (general + 2 <= generalRegisters) {
        Place place{Place::General, generalBytes * general};
        general += 2;
        return place;
      }
      return stack(16, 16);
    }
    // An eightbyte of class SSE: a float, a double, or two floats of a struct as `<2 x float>`, which is how
    // clang passes `struct { float a[3]; }` (with a second, plain float).
    auto *vectorType = dyn_cast<FixedVectorType>(type);
    bool twoFloats = vectorType && vectorType->getNumElements() == 2 && vectorType->getElementType()->isFloatTy();
    if (type->isFloatTy() || type->isDoubleTy() || twoFloats) {
      if (vector < vectorRegisters) {
        return {Place::Vector, generalRegisters * generalBytes + vectorBytes * vector++};
      }
      return stack(slotBytes, slotBytes);
    }
    if (type->isX86_FP80Ty()) {
      return stack(16, 16);
    }
    std::string text;
    raw_string_ostream out(text);
    type->print(out);
    notHandled("a variadic argument of type " + out.str(), at);
  }

  uint64_t general = 0;
  uint64_t vector = 0;
  uint64_t stackBytes = 0;

private:
  Place stack(uint64_t size, uint64_t align) {
    uint64_t at = roundUp(stackBytes, align);
    stackBytes = at + roundUp(size, slotBytes);
    return {Place::Stack, saveAreaBytes + at};
  }
};

// The registers the named parameters of `type` use.
Assigner namedRegisters(FunctionType *type, const AttributeList &attributes, const DataLayout &layout,
                        const Value *at) {
  Assigner assigner;
  for (unsigned i = 0; i < type->getNumParams(); ++i) {
    assigner.assign(type->getParamType(i), attributes.getParamByValType(i), attributes.getParamAlignment(i), layout,
                    at);
  }
  return assigner;
}

// Parameter attributes a variadic argument may carry: they say nothing about where it goes.
bool harmless(Attribute attribute) {
  if (attribute.isStringAttribute()) {
    return true;
  }
  switch (attribute.getKindAsEnum()) {
  case Attribute::NoUndef:
  case Attribute::NonNull:
  case Attribute::Dereferenceable:
  case Attribute::DereferenceableOrNull:
  case Attribute::Alignment:
  case Attribute::ZExt:
  case Attribute::SExt:
  case Attribute::ByVal:
  case Attribute::NoCapture:
  case Attribute::ReadOnly:
  case Attribute::NoAlias:
  case Attribute::NoFree:
    return true;
  default:
    return false;
  }
}

// The type of a variadic function once it takes the area instead.
FunctionType *withArea(FunctionType *type) {
  std::vector<Type *> params(type->param_begin(), type->param_end());
  params.push_back(PointerType::get(type->getContext(), 0));
  return FunctionType::get(type->getReturnType(), params, false);
}

// Rewrites a call of variadic type into a call of `withArea(type)` that passes the area it fills.
void lowerCall(CallInst *call, const DataLayout &layout) {
  FunctionType *type = call->getFunctionType();
  unsigned named = type->getNumParams();
  if (call->isMustTailCall()) {
    notHandled("a `musttail` call of a variadic function", call);
  }
  AttributeList attributes = call->getAttributes();
  Assigner assigner = namedRegisters(type, attributes, layout, call);
  std::vector<Place> places;
  for (unsigned i = named; i < call->arg_size(); ++i) {
    for (Attribute attribute : attributes.getParamAttrs(i)) {
      if (!harmless(attribute)) {
        notHandled("a variadic argument with the attribute `" + attribute.getAsString() + "`", call);
      }
    }
    places.push_back(assigner.assign(call->getArgOperand(i)->getType(), attributes.getParamByValType(i),
                                     attributes.getParamAlignment(i), layout, call));
  }

  // The area lives in the caller's frame, allocated once in its entry block.
  Function &caller = *call->getFunction();
  IRBuilder<> entry(&caller.getEntryBlock(), caller.getEntryBlock().getFirstInsertionPt());
  entry.SetCurrentDebugLocation(call->getDebugLoc());
  uint64_t bytes = saveAreaBytes + roundUp(assigner.stackBytes, stackAlign);
  AllocaInst *area = entry.CreateAlloca(ArrayType::get(entry.getInt8Ty(), bytes), nullptr, "va.area");
  area->setAlignment(Align(stackAlign));

  IRBuilder<> b(call);
  for (unsigned i = named; i < call->arg_size(); ++i) {
    Value *arg = call->getArgOperand(i);
    Place place = places[i - named];
    Value *slot = b.CreateConstInBoundsGEP1_64(b.getInt8Ty(), area, place.offset);
    if (Type *byval = attributes.getParamByValType(i)) {
      b.CreateMemCpy(slot, Align(slotBytes), arg, attributes.getParamAlignment(i),
                     layout.getTypeAllocSize(byval).getFixedValue());
      continue;
    }
    // A narrow integer fills its whole register or slot, extended as its attributes say: the callee
    // reads it at the width of its C type.
    if (arg->getType()->isIntegerTy() && arg->getType()->getIntegerBitWidth() < 64) {
      arg = attributes.hasParamAttr(i, Attribute::SExt) ? b.CreateSExt(arg, b.getInt64Ty())
                                                         : b.CreateZExt(arg, b.getInt64Ty());
    }
    b.CreateStore(arg, slot);
  }

  std::vector<Value *> args(call->arg_begin(), call->arg_begin() + named);
  args.push_back(area);
  std::vector<AttributeSet> params;
  for (unsigned i = 0; i < named; ++i) {
    params.push_back(attributes.getParamAttrs(i));
  }
  params.push_back(AttributeSet());
  CallInst *lowered = b.CreateCall(withArea(type), call->getCalledOperand(), args);
  lowered->setAttributes(AttributeList::get(call->getContext(), attributes.getFnAttrs(), attributes.getRetAttrs(),
                                            params));
  lowered->setCallingConv(call->getCallingConv());
  lowered->setTailCallKind(CallInst::TCK_None); // the area is in the caller's frame
  lowered->setDebugLoc(call->getDebugLoc());
  allowArgumentMemory(lowered, ModRefInfo::Ref);
  lowered->takeName(call);
  call->replaceAllUsesWith(lowered);
  call->eraseFromParent();
}

// A call with a variadic type of a function that is not variadic: a plain call of the function if the
// arguments it takes have its parameters' types and the result is the same or unused.
void lowerCallOfFixed(CallInst *call, Function *callee) {
  FunctionType *type = callee->getFunctionType();
  bool fits = call->arg_size() >= type->getNumParams() &&
              (call->getType() == type->getReturnType() || call->use_empty() || call->getType()->isVoidTy());
  for (unsigned i = 0; fits && i < type->getNumParams(); ++i) {
    fits = call->getArgOperand(i)->getType() == type->getParamType(i) && !call->isByValArgument(i) &&
           !callee->hasParamAttribute(i, Attribute::ByVal);
  }
  if (!fits) {
    notHandled("a call through a mismatched prototype of @" + callee->getName().str(), call);
  }
  IRBuilder<> b(call);
  std::vector<Value *> args(call->arg_begin(), call->arg_begin() + type->getNumParams());
  CallInst *lowered = b.CreateCall(callee, args);
  lowered->setDebugLoc(call->getDebugLoc());
  if (!call->use_empty()) {
    lowered->takeName(call);
    call->replaceAllUsesWith(lowered);
  }
  call->eraseFromParent();
}

// A call of variadic type through a pointer. clang gives every call through an unprototyped pointer
// (`void (*)()`) a variadic type, and on x86-64 System V such a call reaches a variadic and a fixed function
// alike. The target is known only when the call runs, so the call compares the pointer with each variadic
// function whose address is taken: on a match it passes the area as a call of that function would, with
// that function's named parameters; otherwise it is a plain call with the argument types it has. A target
// whose parameters differ from the arguments still ends in the named fault of a mismatched indirect call.
void lowerCallThroughPointer(CallInst *call, const std::vector<Function *> &reachable, const DataLayout &layout) {
  IRBuilder<> b(call);
  Value *target = call->getCalledOperand();
  auto plainCall = [&](IRBuilder<> &at) {
    std::vector<Type *> params;
    for (Value *arg : call->args()) {
      params.push_back(arg->getType());
    }
    FunctionType *type = FunctionType::get(call->getType(), params, false);
    std::vector<Value *> args(call->arg_begin(), call->arg_end());
    CallInst *plain = at.CreateCall(type, target, args);
    plain->setAttributes(call->getAttributes());
    plain->setCallingConv(call->getCallingConv());
    plain->setDebugLoc(call->getDebugLoc());
    return plain;
  };
  std::vector<Function *> candidates;
  for (Function *function : reachable) {
    FunctionType *type = function->getFunctionType();
    bool fits = call->arg_size() >= type->getNumParams() &&
                (call->getType() == type->getReturnType() || call->use_empty() || call->getType()->isVoidTy());
    for (unsigned i = 0; fits && i < type->getNumParams(); ++i) {
      fits = call->getArgOperand(i)->getType() == type->getParamType(i) &&
             call->isByValArgument(i) == function->hasParamAttribute(i, Attribute::ByVal);
    }
    if (fits) {
      candidates.push_back(function);
    }
  }
  if (candidates.empty()) {
    CallInst *plain = plainCall(b);
    plain->takeName(call);
    call->replaceAllUsesWith(plain);
    call->eraseFromParent();
    return;
  }
  BasicBlock *head = call->getParent();
  Function &caller = *head->getParent();
  BasicBlock *tail = head->splitBasicBlock(call, "call.join");
  head->getTerminator()->eraseFromParent();
  bool result = !call->getType()->isVoidTy() && !call->use_empty();
  PHINode *phi = nullptr;
  if (result) {
    phi = PHINode::Create(call->getType(), unsigned(candidates.size() + 1), "", &*tail->begin());
  }
  BasicBlock *test = head;
  std::vector<CallInst *> toLower;
  for (Function *function : candidates) {
    BasicBlock *match = BasicBlock::Create(call->getContext(), "call.variadic", &caller, tail);
    BasicBlock *next = BasicBlock::Create(call->getContext(), "call.next", &caller, tail);
    IRBuilder<> t(test);
    t.SetCurrentDebugLocation(call->getDebugLoc());
    t.CreateCondBr(t.CreateICmpEQ(target, function), match, next);
    IRBuilder<> m(match);
    std::vector<Value *> args(call->arg_begin(), call->arg_end());
    CallInst *direct = m.CreateCall(function->getFunctionType(), function, args);
    direct->setAttributes(call->getAttributes());
    direct->setCallingConv(call->getCallingConv());
    direct->setDebugLoc(call->getDebugLoc());
    m.CreateBr(tail);
    if (phi) {
      phi->addIncoming(direct, match);
    }
    toLower.push_back(direct);
    test = next;
  }
  IRBuilder<> e(test);
  CallInst *plain = plainCall(e);
  e.CreateBr(tail);
  if (phi) {
    phi->addIncoming(plain, test);
    phi->takeName(call);
    call->replaceAllUsesWith(phi);
  }
  call->eraseFromParent();
  for (CallInst *direct : toLower) {
    lowerCall(direct, layout); // replaces the call, and its use in the phi
  }
}

// Fills the `va_list` at `list` for a function whose named parameters use `named`, reading `area`.
void startList(VAStartInst *start, Value *area, const Assigner &named) {
  IRBuilder<> b(start);
  Value *list = start->getArgList();
  b.CreateStore(b.getInt32(uint32_t(generalBytes * named.general)), list);
  b.CreateStore(b.getInt32(uint32_t(generalRegisters * generalBytes + vectorBytes * named.vector)),
                b.CreateConstInBoundsGEP1_64(b.getInt8Ty(), list, fpOffsetField));
  // The stack arguments start after those of the named parameters, as the caller placed them.
  b.CreateStore(b.CreateConstInBoundsGEP1_64(b.getInt8Ty(), area, saveAreaBytes + named.stackBytes),
                b.CreateConstInBoundsGEP1_64(b.getInt8Ty(), list, overflowField));
  b.CreateStore(area, b.CreateConstInBoundsGEP1_64(b.getInt8Ty(), list, saveAreaField));
  start->eraseFromParent();
}

// Gives the variadic function `function` its area parameter. Returns the new function.
Function *lowerDefinition(Function *function, const DataLayout &layout) {
  FunctionType *type = function->getFunctionType();
  Assigner named = namedRegisters(type, function->getAttributes(), layout, function);
  Function *lowered = Function::Create(withArea(type), function->getLinkage(), function->getAddressSpace(), "",
                                       function->getParent());
  lowered->copyAttributesFrom(function);
  allowArgumentMemory(lowered, ModRefInfo::Ref);
  lowered->setSubprogram(function->getSubprogram());
  function->setSubprogram(nullptr);
  lowered->takeName(function);
  lowered->splice(lowered->begin(), function);
  auto arg = lowered->arg_begin();
  for (Argument &old : function->args()) {
    old.replaceAllUsesWith(&*arg);
    arg->takeName(&old);
    ++arg;
  }
  arg->setName("va.area");
  std::vector<VAStartInst *> starts;
  for (Instruction &inst : instructions(*lowered)) {
    if (auto *start = dyn_cast<VAStartInst>(&inst)) {
      starts.push_back(start);
    }
    if (auto *call = dyn_cast<CallInst>(&inst); call && call->isMustTailCall()) {
      notHandled("a `musttail` call in a variadic function", call);
    }
  }
  for (VAStartInst *start : starts) {
    startList(start, &*arg, named);
  }
  function->replaceAllUsesWith(lowered);
  function->eraseFromParent();
  return lowered;
}

bool isSystemVx86_64(const Module &module) {
  Triple triple(module.getTargetTriple());
  return triple.getArch() == Triple::x86_64 && !triple.isOSWindows() && !triple.isUEFI();
}

// Calls `leaf(indices, offset, type)` for each scalar inside a value of aggregate type `type`.
void forEachLeaf(Type *type, const DataLayout &layout, std::vector<unsigned> &indices, uint64_t offset,
                 const std::function<void(const std::vector<unsigned> &, uint64_t, Type *)> &leaf) {
  if (auto *structure = dyn_cast<StructType>(type)) {
    const StructLayout *fields = layout.getStructLayout(structure);
    for (unsigned i = 0; i < structure->getNumElements(); ++i) {
      indices.push_back(i);
      forEachLeaf(structure->getElementType(i), layout, indices, offset + fields->getElementOffset(i), leaf);
      indices.pop_back();
    }
    return;
  }
  if (auto *array = dyn_cast<ArrayType>(type)) {
    uint64_t step = layout.getTypeAllocSize(array->getElementType()).getFixedValue();
    for (unsigned i = 0; i < array->getNumElements(); ++i) {
      indices.push_back(i);
      forEachLeaf(array->getElementType(), layout, indices, offset + i * step, leaf);
      indices.pop_back();
    }
    return;
  }
  leaf(indices, offset, type);
}

void forEachLeaf(Type *type, const DataLayout &layout,
                 const std::function<void(const std::vector<unsigned> &, uint64_t, Type *)> &leaf) {
  std::vector<unsigned> indices;
  forEachLeaf(type, layout, indices, 0, leaf);
}

bool isAggregate(Type *type) { return type->isStructTy() || type->isArrayTy(); }

// The widest integer a device value has.
constexpr uint64_t widestValueBits = 128;

// The integer that carries an aggregate result whose fields are integers and floating-point values of
// at most 128 bits in all (SoftFloat's extFloat80_t, a 128-bit integer or a small struct as clang returns
// it on x86-64), the fields side by side from bit 0 in order; null for any other type. Pointers stay out:
// an address turned into an integer hides from LLVM what memory a call may touch.
IntegerType *packedResult(Type *type, const DataLayout &layout) {
  if (!isAggregate(type)) {
    return nullptr;
  }
  uint64_t bits = 0;
  bool scalars = true;
  forEachLeaf(type, layout, [&](const std::vector<unsigned> &, uint64_t, Type *leaf) {
    scalars = scalars && (leaf->isIntegerTy() || leaf->isFloatingPointTy());
    bits += layout.getTypeSizeInBits(leaf).getFixedValue();
  });
  if (!scalars || bits == 0 || bits > widestValueBits) {
    return nullptr;
  }
  return IntegerType::get(type->getContext(), unsigned(bits));
}

// A result that goes through memory: a vector, or an aggregate that is not carried as one integer.
bool resultInMemory(Type *type, const DataLayout &layout) {
  return (isAggregate(type) && !packedResult(type, layout)) || type->isVectorTy();
}

// Whether a function of `type` passes something through memory: a result, or a vector parameter.
bool usesMemory(FunctionType *type, const DataLayout &layout) {
  return resultInMemory(type->getReturnType(), layout) ||
         std::any_of(type->param_begin(), type->param_end(), [](Type *param) { return param->isVectorTy(); });
}

// Whether a function of `type` is rewritten: it passes something through memory or packs its result.
bool passesInMemory(FunctionType *type, const DataLayout &layout) {
  return usesMemory(type, layout) || isAggregate(type->getReturnType());
}

// The type once results are packed or go through memory and vector parameters go through memory: a
// result address first, if any, and an address in place of each vector.
FunctionType *inMemory(FunctionType *type, const DataLayout &layout) {
  Type *pointer = PointerType::get(type->getContext(), 0);
  std::vector<Type *> params;
  bool result = resultInMemory(type->getReturnType(), layout);
  if (result) {
    params.push_back(pointer);
  }
  for (Type *param : type->params()) {
    params.push_back(param->isVectorTy() ? pointer : param);
  }
  Type *returned = type->getReturnType();
  if (result) {
    returned = Type::getVoidTy(type->getContext());
  } else if (IntegerType *packed = packedResult(returned, layout)) {
    returned = packed;
  }
  return FunctionType::get(returned, params, type->isVarArg());
}

// `value`, an aggregate that packedResult carries, as that integer. Each field is frozen: a poison field
// (one never set) must not make the others poison, as it does not when the fields are stored one by one.
Value *pack(IRBuilder<> &b, Value *value, IntegerType *packed, const DataLayout &layout) {
  Value *result = ConstantInt::get(packed, 0);
  uint64_t at = 0;
  forEachLeaf(value->getType(), layout, [&](const std::vector<unsigned> &indices, uint64_t, Type *leaf) {
    unsigned bits = unsigned(layout.getTypeSizeInBits(leaf).getFixedValue());
    Value *field = b.CreateBitCast(b.CreateFreeze(b.CreateExtractValue(value, indices)), b.getIntNTy(bits));
    result = b.CreateOr(result, b.CreateShl(b.CreateZExt(field, packed), at));
    at += bits;
  });
  return result;
}

// The aggregate of `type` that the integer `packed` carries, as pack made it.
Value *unpack(IRBuilder<> &b, Value *packed, Type *type, const DataLayout &layout) {
  Value *value = PoisonValue::get(type);
  uint64_t at = 0;
  forEachLeaf(type, layout, [&](const std::vector<unsigned> &indices, uint64_t, Type *leaf) {
    unsigned bits = unsigned(layout.getTypeSizeInBits(leaf).getFixedValue());
    Value *field = b.CreateBitCast(b.CreateTrunc(b.CreateLShr(packed, at), b.getIntNTy(bits)), leaf);
    value = b.CreateInsertValue(value, field, indices);
    at += bits;
  });
  return value;
}

// Stores `value` at `address`: an aggregate field by field (its padding untouched, as in registers), a
// vector whole (the Scalarizer splits it later).
void storeValue(IRBuilder<> &b, Value *value, Value *address, const DataLayout &layout) {
  if (!isAggregate(value->getType())) {
    b.CreateStore(value, address);
    return;
  }
  forEachLeaf(value->getType(), layout, [&](const std::vector<unsigned> &indices, uint64_t offset, Type *) {
    b.CreateStore(b.CreateExtractValue(value, indices), b.CreateConstInBoundsGEP1_64(b.getInt8Ty(), address, offset));
  });
}

// The value of `type` at `address`, read as storeValue wrote it.
Value *loadValue(IRBuilder<> &b, Type *type, Value *address, const DataLayout &layout) {
  if (!isAggregate(type)) {
    return b.CreateLoad(type, address);
  }
  Value *value = PoisonValue::get(type);
  forEachLeaf(type, layout, [&](const std::vector<unsigned> &indices, uint64_t offset, Type *leaf) {
    Value *field = b.CreateLoad(leaf, b.CreateConstInBoundsGEP1_64(b.getInt8Ty(), address, offset));
    value = b.CreateInsertValue(value, field, indices);
  });
  return value;
}

// The attributes of a call or function of `type` once inMemory: those of the parameters that stay, and
// those of the result if it is unchanged.
AttributeList inMemoryAttributes(LLVMContext &context, FunctionType *type, const AttributeList &attributes,
                                 const DataLayout &layout) {
  bool result = resultInMemory(type->getReturnType(), layout);
  bool changed = isAggregate(type->getReturnType()) || result;
  std::vector<AttributeSet> params;
  if (result) {
    params.push_back(AttributeSet());
  }
  for (unsigned i = 0; i < type->getNumParams(); ++i) {
    params.push_back(type->getParamType(i)->isVectorTy() ? AttributeSet() : attributes.getParamAttrs(i));
  }
  return AttributeList::get(context, attributes.getFnAttrs(), changed ? AttributeSet() : attributes.getRetAttrs(),
                            params);
}

void lowerCallInMemory(CallInst *call, const DataLayout &layout) {
  FunctionType *type = call->getFunctionType();
  Function &caller = *call->getFunction();
  IRBuilder<> entry(&caller.getEntryBlock(), caller.getEntryBlock().getFirstInsertionPt());
  entry.SetCurrentDebugLocation(call->getDebugLoc());
  IRBuilder<> b(call);
  std::vector<Value *> args;
  AllocaInst *result = nullptr;
  if (resultInMemory(type->getReturnType(), layout)) {
    result = entry.CreateAlloca(type->getReturnType(), nullptr, "result");
    args.push_back(result);
  }
  // Each vector argument is copied into the caller's frame, as a struct passed by value would be.
  for (Value *arg : call->args()) {
    if (!arg->getType()->isVectorTy()) {
      args.push_back(arg);
      continue;
    }
    AllocaInst *copy = entry.CreateAlloca(arg->getType(), nullptr, "argument");
    storeValue(b, arg, copy, layout);
    args.push_back(copy);
  }
  CallInst *lowered = b.CreateCall(inMemory(type, layout), call->getCalledOperand(), args);
  lowered->setAttributes(inMemoryAttributes(call->getContext(), type, call->getAttributes(), layout));
  lowered->setCallingConv(call->getCallingConv());
  lowered->setDebugLoc(call->getDebugLoc());
  if (usesMemory(type, layout)) {
    lowered->setTailCallKind(CallInst::TCK_None); // the copies are in the caller's frame
    allowArgumentMemory(lowered, ModRefInfo::ModRef);
  } else {
    lowered->setTailCallKind(call->getTailCallKind());
  }
  if (result) {
    Value *value = loadValue(b, type->getReturnType(), result, layout);
    value->takeName(call);
    call->replaceAllUsesWith(value);
  } else if (isAggregate(type->getReturnType())) {
    Value *value = unpack(b, lowered, type->getReturnType(), layout);
    value->takeName(call);
    call->replaceAllUsesWith(value);
  } else if (!call->getType()->isVoidTy()) {
    lowered->takeName(call);
    call->replaceAllUsesWith(lowered);
  }
  call->eraseFromParent();
}

void lowerDefinitionInMemory(Function *function, const DataLayout &layout) {
  FunctionType *type = function->getFunctionType();
  Function *lowered =
      Function::Create(inMemory(type, layout), function->getLinkage(), function->getAddressSpace(), "",
                       function->getParent());
  lowered->copyAttributesFrom(function);
  lowered->setAttributes(inMemoryAttributes(function->getContext(), type, function->getAttributes(), layout));
  if (usesMemory(type, layout)) {
    allowArgumentMemory(lowered, ModRefInfo::ModRef);
  }
  lowered->setSubprogram(function->getSubprogram());
  function->setSubprogram(nullptr);
  lowered->takeName(function);
  lowered->splice(lowered->begin(), function);
  bool inMemoryResult = resultInMemory(type->getReturnType(), layout);
  Argument *result = inMemoryResult ? lowered->getArg(0) : nullptr;
  if (result) {
    result->setName("result");
  }
  BasicBlock &entryBlock = lowered->getEntryBlock();
  IRBuilder<> entry(&entryBlock, entryBlock.getFirstInsertionPt());
  if (Instruction *first = entryBlock.getFirstNonPHIOrDbg()) {
    entry.SetCurrentDebugLocation(first->getDebugLoc());
  }
  auto arg = lowered->arg_begin() + (inMemoryResult ? 1 : 0);
  for (Argument &old : function->args()) {
    arg->takeName(&old);
    if (old.getType()->isVectorTy()) {
      old.replaceAllUsesWith(loadValue(entry, old.getType(), &*arg, layout));
    } else {
      old.replaceAllUsesWith(&*arg);
    }
    ++arg;
  }
  IntegerType *packed = packedResult(type->getReturnType(), layout);
  if (result || packed) {
    std::vector<ReturnInst *> returns;
    for (BasicBlock &block : *lowered) {
      if (auto *ret = dyn_cast<ReturnInst>(block.getTerminator())) {
        returns.push_back(ret);
      }
    }
    for (ReturnInst *ret : returns) {
      IRBuilder<> b(ret);
      b.SetCurrentDebugLocation(ret->getDebugLoc());
      if (packed) {
        b.CreateRet(pack(b, ret->getReturnValue(), packed, layout));
      } else {
        storeValue(b, ret->getReturnValue(), result, layout);
        b.CreateRetVoid();
      }
      ret->eraseFromParent();
    }
  }
  function->replaceAllUsesWith(lowered);
  function->eraseFromParent();
}

} // namespace

void lowerValuesInMemory(Module &module) {
  const DataLayout &layout = module.getDataLayout();
  std::vector<Function *> functions;
  std::vector<CallInst *> calls;
  for (Function &function : module) {
    if (!function.isDeclaration() && passesInMemory(function.getFunctionType(), layout)) {
      functions.push_back(&function);
    }
    for (Instruction &inst : instructions(function)) {
      auto *call = dyn_cast<CallInst>(&inst);
      if (!call || call->isInlineAsm() || isa<IntrinsicInst>(call) ||
          !passesInMemory(call->getFunctionType(), layout)) {
        continue;
      }
      auto *callee = dyn_cast<Function>(call->getCalledOperand());
      // A declaration is refused later; a call through another prototype too.
      if (callee && (callee->isDeclaration() || callee->getFunctionType() != call->getFunctionType())) {
        continue;
      }
      if (call->isMustTailCall()) {
        notHandled("a `musttail` call with a result or argument passed in memory", call);
      }
      calls.push_back(call);
    }
  }
  for (CallInst *call : calls) {
    lowerCallInMemory(call, layout);
  }
  for (Function *function : functions) {
    lowerDefinitionInMemory(function, layout);
  }
}

void lowerVariadics(Module &module) {
  const DataLayout &layout = module.getDataLayout();
  std::vector<Function *> variadic;
  for (Function &function : module) {
    if (function.isVarArg() && !function.isDeclaration()) {
      variadic.push_back(&function);
    }
  }
  std::vector<CallInst *> calls;
  std::vector<Instruction *> lists; // va_copy and va_end; va_start is rewritten with its function
  const Instruction *start = nullptr;
  for (Function &function : module) {
    for (Instruction &inst : instructions(function)) {
      if (isa<VAStartInst>(inst)) {
        start = &inst;
      } else if (isa<VACopyInst>(inst) || isa<VAEndInst>(inst)) {
        lists.push_back(&inst);
      } else if (isa<VAArgInst>(inst)) {
        notHandled("the LLVM instruction `va_arg`", &inst);
      } else if (auto *call = dyn_cast<CallBase>(&inst); call && call->getFunctionType()->isVarArg()) {
        if (!isa<CallInst>(call)) {
          notHandled("a variadic call that is not a plain call", &inst);
        }
        if (!call->isInlineAsm()) {
          calls.push_back(cast<CallInst>(call));
        }
      }
    }
  }
  if ((!variadic.empty() || start || !lists.empty()) && !isSystemVx86_64(module)) {
    const Value *at = !variadic.empty() ? static_cast<const Value *>(variadic.front())
                      : start           ? start
                                        : lists.front();
    notHandled("a variadic function for a target other than x86-64 System V", at);
  }

  std::vector<Function *> reachable; // variadic functions a pointer may hold
  for (Function *function : variadic) {
    if (function->hasAddressTaken()) {
      reachable.push_back(function);
    }
  }
  for (CallInst *call : calls) {
    auto *callee = dyn_cast<Function>(call->getCalledOperand());
    if (callee && callee->isDeclaration()) {
      continue; // refused later: no library is linked yet
    }
    if (callee && !callee->isVarArg()) {
      lowerCallOfFixed(call, callee);
      continue;
    }
    if (!callee) {
      lowerCallThroughPointer(call, reachable, layout);
      continue;
    }
    lowerCall(call, layout);
  }
  for (Function *function : variadic) {
    lowerDefinition(function, layout);
  }
  for (Instruction *inst : lists) {
    if (auto *copy = dyn_cast<VACopyInst>(inst)) {
      IRBuilder<> b(copy);
      b.CreateMemCpy(copy->getDest(), Align(slotBytes), copy->getSrc(), Align(slotBytes), vaListBytes);
      copy->eraseFromParent();
    } else {
      inst->eraseFromParent();
    }
  }
}

} // namespace bennett
