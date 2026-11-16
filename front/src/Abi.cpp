#include "Abi.h"

#include "Diagnostic.h"

#include <llvm/IR/Attributes.h>
#include <llvm/IR/IRBuilder.h>
#include <llvm/IR/InstIterator.h>
#include <llvm/IR/Instructions.h>
#include <llvm/IR/IntrinsicInst.h>
#include <llvm/IR/Module.h>
#include <llvm/TargetParser/Triple.h>
#include <llvm/Transforms/Utils/BasicBlockUtils.h>

#include <algorithm>
#include <array>
#include <functional>
#include <map>
#include <optional>
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
      return stack(layout.getTypeAllocSize(byval).getFixedValue(),
                   std::max<uint64_t>(slotBytes, align.valueOrOne().value()));
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
    // A vector of 16 bytes (`__m128`, `vector_size(16)`) is one eightbyte of class SSE and one of SSEUP: a whole
    // vector register, or 16 bytes of stack. A wider one (`__m256`, `__m512` under AVX) goes to the stack at its
    // own alignment when it is variadic, in LLVM's CC_X86_64_C as in clang's va_arg, which reads it from memory.
    if (vectorType) {
      uint64_t size = layout.getTypeAllocSize(vectorType).getFixedValue();
      if (size == vectorBytes) {
        if (vector < vectorRegisters) {
          return {Place::Vector, generalRegisters * generalBytes + vectorBytes * vector++};
        }
        return stack(size, vectorBytes);
      }
      if (size > vectorBytes && layout.getTypeSizeInBits(vectorType) == size * 8) {
        return stack(size, std::max<uint64_t>(slotBytes, layout.getABITypeAlign(vectorType).value()));
      }
    }
    std::string text;
    raw_string_ostream out(text);
    type->print(out);
    notHandled("a variadic argument of type " + out.str(), at);
  }

  uint64_t general = 0;
  uint64_t vector = 0;
  uint64_t stackBytes = 0;
  uint64_t align = stackAlign; // the alignment the area needs

private:
  // clang's va_arg rounds the address of an argument aligned to more than 8 bytes up to its alignment, so the
  // offset is rounded within the area, which is aligned to its widest argument (`align`). The save area is a
  // multiple of 16 bytes, so up to 16 this is the offset on the stack, as the psABI counts it.
  Place stack(uint64_t size, uint64_t alignment) {
    uint64_t at = roundUp(saveAreaBytes + stackBytes, alignment);
    stackBytes = at - saveAreaBytes + roundUp(size, slotBytes);
    align = std::max(align, alignment);
    return {Place::Stack, at};
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
  area->setAlignment(Align(assigner.align));

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

// ---- Calls through a prototype that differs from the callee's definition.
//
// Natively such a call passes what the caller's prototype says, where the x86-64 System V convention puts
// it, and the callee reads what its own definition says, from where the convention puts that. A value is
// determined wherever the reader reads bits the other side set. Each value leaves an "image" in its
// registers or 8-byte stack slots: an integer, of which a number of low bits are set, as clang 18's code
// sets them and relies on them (measured natively at -O0 and -O2). A caller extends an argument narrower
// than 32 bits that has `signext` or `zeroext` to 32 bits, and a callee relies on that extension (at -O2 it
// uses the register as it is; at -O0 it extends again): so the callee's value is determined only when the
// 32 bits are the extension of its own narrow value, which is checked when the call runs. Nothing else is
// extended: the upper half of a 32-bit argument is not set, nor anything in a result above its width
// (callees do not extend results; callers extend them again), except an `i1` result with `zeroext`, a
// byte (the psABI's _Bool).

// Bits of one value's image in one register or slot: from bit `shift`, `known` of them set.
struct Piece {
  unsigned value; // argument or parameter index, or result element
  unsigned shift;
  unsigned known;
};

// A value passed whole in memory (`byval`, `x86_fp80`): only the same value at the same place is understood.
struct WholeValue {
  unsigned value;
  Type *type;
  Type *byval;
  uint64_t bytes;
};

constexpr unsigned extendedBits = 32; // what a caller extends a narrow argument to
constexpr unsigned boolBits = 8;      // what an `i1` result with `zeroext` fills

// The image of a scalar of `type`, in bits: 0 for a type passed whole in memory, nullopt for a type not
// reasoned about here (vectors other than two floats, aggregates, integers wider than 128 bits).
std::optional<unsigned> imageBits(Type *type) {
  if (type->isIntegerTy()) {
    unsigned bits = type->getIntegerBitWidth();
    return bits <= 128 ? std::optional<unsigned>(bits) : std::nullopt;
  }
  if (type->isPointerTy() || type->isDoubleTy()) {
    return 64;
  }
  if (type->isFloatTy()) {
    return 32;
  }
  auto *vectorType = dyn_cast<FixedVectorType>(type);
  if (vectorType && vectorType->getNumElements() == 2 && vectorType->getElementType()->isFloatTy()) {
    return 64;
  }
  if (type->isX86_FP80Ty()) {
    return 0;
  }
  return std::nullopt;
}

std::string typeName(const Type *type) {
  std::string text;
  raw_string_ostream out(text);
  type->print(out);
  return out.str();
}

bool extended(const AttributeSet &attributes) {
  return attributes.hasAttribute(Attribute::SExt) || attributes.hasAttribute(Attribute::ZExt);
}

// Where the values of one side of a call go, by Place::offset.
struct Arrangement {
  std::map<uint64_t, Piece> pieces;
  std::map<uint64_t, WholeValue> whole;
  std::vector<Place> places;
  std::vector<std::array<uint64_t, 3>> used; // registers and stack used after each value

  const Piece *pieceAt(uint64_t offset) const {
    auto found = pieces.find(offset);
    return found == pieces.end() ? nullptr : &found->second;
  }
  bool insideWhole(uint64_t offset) const {
    return std::any_of(whole.begin(), whole.end(), [&](const auto &entry) {
      return offset >= entry.first && offset < entry.first + entry.second.bytes;
    });
  }
};

// Arranges values of `types` with `attributes`; a caller's images include its extensions. Returns why not.
std::string arrange(const std::vector<Type *> &types, const AttributeList &attributes, bool caller,
                    const DataLayout &layout, const Value *at, Arrangement &out) {
  Assigner assigner;
  for (unsigned i = 0; i < types.size(); ++i) {
    Type *type = types[i];
    AttributeSet set = attributes.getParamAttrs(i);
    for (Attribute::AttrKind kind : {Attribute::InReg, Attribute::InAlloca, Attribute::Preallocated, Attribute::Nest,
                                     Attribute::SwiftSelf, Attribute::SwiftError, Attribute::SwiftAsync,
                                     Attribute::ByRef}) {
      if (set.hasAttribute(kind)) {
        return "an argument with the attribute `" + Attribute::getNameFromAttrKind(kind).str() + "`";
      }
    }
    Type *byval = attributes.getParamByValType(i);
    std::optional<unsigned> bits = byval ? std::optional<unsigned>(0) : imageBits(type);
    if (!bits) {
      return "an argument of type " + typeName(type);
    }
    if (type->isIntegerTy(128) && assigner.general + 1 == generalRegisters) {
      // The psABI and LLVM's backend disagree here (see Assigner): which one each side followed is unknown.
      return "a 128-bit integer argument when one register is left";
    }
    Place place = assigner.assign(type, byval, attributes.getParamAlignment(i), layout, at);
    out.places.push_back(place);
    out.used.push_back({assigner.general, assigner.vector, assigner.stackBytes});
    if (*bits == 0) {
      uint64_t bytes = byval ? layout.getTypeAllocSize(byval).getFixedValue() : 16;
      out.whole[place.offset] = {i, type, byval, roundUp(bytes, slotBytes)};
      continue;
    }
    if (caller && *bits < extendedBits && extended(set)) {
      bits = extendedBits;
    }
    if (*bits > 64) {
      out.pieces[place.offset] = {i, 0, 64};
      out.pieces[place.offset + generalBytes] = {i, 64, *bits - 64};
    } else {
      out.pieces[place.offset] = {i, 0, *bits};
    }
  }
  return "";
}

// Whether a callee's parameter is never read: unused, or only stored to stack slots that nothing reads but
// loads whose value is unused (how clang -O0 keeps an unused parameter, and `(void)p`).
bool unread(const Argument &arg) {
  for (const User *user : arg.users()) {
    const auto *store = dyn_cast<StoreInst>(user);
    const auto *slot = store ? dyn_cast<AllocaInst>(store->getPointerOperand()) : nullptr;
    if (!slot || store->getValueOperand() != &arg) {
      return false;
    }
    for (const User *use : slot->users()) {
      const auto *other = dyn_cast<StoreInst>(use);
      const auto *load = dyn_cast<LoadInst>(use);
      if (!(other && other->getPointerOperand() == slot && other->getValueOperand() != slot) &&
          !(load && load->use_empty() && !load->isVolatile()) && !isa<LifetimeIntrinsic>(use)) {
        return false;
      }
    }
  }
  return true;
}

// How one value the reader reads is made: from pieces of the writer's images, low bits first.
struct Source {
  enum Kind { Bits, Unread, Whole } kind = Unread;
  std::vector<std::pair<Piece, unsigned>> parts; // a piece, and how many of its bits are taken
  unsigned need = 0;                             // bits read: the value's own, or what an extension fills
  enum Check { None, Sign, Zero, Bool } check = None;
  unsigned whole = 0; // the argument passed on as it is
};

// Collects `need` bits from the pieces `pieceAt` gives in turn (the next register or slot). Returns why not.
std::string collect(const std::function<const Piece *(unsigned)> &pieceAt, unsigned need, Source &out) {
  out.kind = Source::Bits;
  out.need = need;
  for (unsigned i = 0, left = need; left; ++i) {
    const Piece *piece = pieceAt(i);
    unsigned take = std::min(left, 64u);
    if (!piece) {
      return "a value that is not passed";
    }
    if (piece->known < take) {
      return "bits " + std::to_string(need - left + piece->known) + " to " + std::to_string(need - left + take - 1) +
             ", which are not set";
    }
    out.parts.push_back({*piece, take});
    left -= take;
  }
  return "";
}

// The pieces a result of `type` leaves in rax, rdx (locations 0, 1), xmm0, xmm1 (2, 3), by element. False
// when it is not reasoned about here.
bool arrangeResult(Type *type, const AttributeSet &attributes, std::map<unsigned, Piece> &out) {
  if (type->isVoidTy()) {
    return true;
  }
  std::vector<Type *> elements;
  if (auto *structure = dyn_cast<StructType>(type)) {
    elements.assign(structure->element_begin(), structure->element_end());
  } else {
    elements.push_back(type);
  }
  unsigned general = 0;
  unsigned vector = 2;
  for (unsigned e = 0; e < elements.size(); ++e) {
    std::optional<unsigned> bits = imageBits(elements[e]);
    if (!bits || *bits == 0) {
      return false;
    }
    if (elements[e]->isIntegerTy(1) && attributes.hasAttribute(Attribute::ZExt)) {
      bits = boolBits;
    }
    if (elements[e]->isFloatingPointTy() || elements[e]->isVectorTy()) {
      if (vector == 4) {
        return false;
      }
      out[vector++] = {e, 0, *bits};
      continue;
    }
    for (unsigned shift = 0; shift < *bits; shift += 64) {
      if (general == 2) {
        return false;
      }
      out[general++] = {e, shift, std::min(*bits - shift, 64u)};
    }
  }
  return true;
}

std::vector<Type *> elementsOf(Type *type) {
  if (auto *structure = dyn_cast<StructType>(type)) {
    return {structure->element_begin(), structure->element_end()};
  }
  return {type};
}

// How a call through a prototype other than the callee's is made with the callee's own.
struct Plan {
  std::vector<Source> params;
  std::vector<unsigned> variadic; // arguments passed on as the callee's variadic ones
  bool resultWhole = false;       // the caller's result is the callee's, as it is
  std::vector<Source> result;     // otherwise, per element of the caller's result (empty: none used)
};

// Plans a call of `callee` by a call of `type` with `attributes` and the argument types `args` (more than
// `type` names, when it is variadic). Returns why the convention does not determine what is read.
std::string plan(FunctionType *type, const AttributeList &attributes, const std::vector<Type *> &args,
                 bool resultUsed, const Function &callee, const DataLayout &layout, const Value *at, Plan &out) {
  FunctionType *own = callee.getFunctionType();
  std::string name = "@" + callee.getName().str();
  bool callerSret = !args.empty() && attributes.hasParamAttr(0, Attribute::StructRet);
  bool calleeSret = own->getNumParams() > 0 && callee.hasParamAttribute(0, Attribute::StructRet);
  if (callerSret != calleeSret) {
    return "a result " + name + " returns in memory on one side only";
  }
  Arrangement caller;
  if (std::string why = arrange(args, attributes, true, layout, at, caller); !why.empty()) {
    return why;
  }
  std::vector<Type *> params(own->param_begin(), own->param_end());
  Arrangement reader;
  if (std::string why = arrange(params, callee.getAttributes(), false, layout, at, reader); !why.empty()) {
    return why + " in " + name;
  }
  for (unsigned j = 0; j < params.size(); ++j) {
    Place place = reader.places[j];
    std::string what = "parameter " + std::to_string(j + 1) + " of " + name;
    Source source;
    if (reader.whole.count(place.offset)) {
      auto passed = caller.whole.find(place.offset);
      if (passed != caller.whole.end() && passed->second.type == params[j] &&
          passed->second.byval == reader.whole[place.offset].byval) {
        source.kind = Source::Whole;
        source.whole = passed->second.value;
      } else if (caller.insideWhole(place.offset) || caller.pieceAt(place.offset) || !unread(*callee.getArg(j))) {
        return what + ", passed in memory, where the caller passes something else";
      }
      out.params.push_back(source);
      continue;
    }
    if (caller.insideWhole(place.offset)) {
      return what + ", where the caller passes part of a value in memory";
    }
    if (!caller.pieceAt(place.offset)) {
      if (!unread(*callee.getArg(j))) {
        return what + ", which is read and not passed";
      }
      out.params.push_back(source);
      continue;
    }
    unsigned bits = *imageBits(params[j]);
    bool narrow = params[j]->isIntegerTy() && bits < extendedBits && extended(callee.getAttributes().getParamAttrs(j));
    std::string why = collect([&](unsigned i) { return caller.pieceAt(place.offset + i * generalBytes); },
                              narrow ? extendedBits : bits, source);
    if (!why.empty()) {
      return what + " reads " + why;
    }
    if (narrow) {
      source.check = callee.hasParamAttribute(j, Attribute::ZExt) ? Source::Zero : Source::Sign;
    }
    out.params.push_back(source);
  }
  // Arguments past the callee's parameters: never read, or its variadic arguments.
  if (own->isVarArg()) {
    std::array<uint64_t, 3> named = reader.used.empty() ? std::array<uint64_t, 3>{} : reader.used.back();
    std::array<uint64_t, 3> used{};
    unsigned first = 0;
    while (first < args.size() && used != named) {
      used = caller.used[first++];
    }
    bool fewer = used[0] <= named[0] && used[1] <= named[1] && used[2] <= named[2];
    if (used != named && !(first == args.size() && fewer)) {
      return "variadic arguments that do not start where the parameters of " + name + " end";
    }
    for (unsigned i = first; i < args.size(); ++i) {
      // A call without a variadic prototype does not set %al, the number of vector registers used, which the
      // callee's prologue tests before saving them.
      if (!type->isVarArg() && caller.places[i].kind == Place::Vector) {
        return "a variadic floating-point argument of " + name + " from a call without a variadic prototype";
      }
      out.variadic.push_back(i);
    }
  }
  Type *wanted = type->getReturnType();
  if (!resultUsed || wanted->isVoidTy()) {
    return "";
  }
  AttributeSet wantedAttributes = attributes.getRetAttrs();
  if (wanted == own->getReturnType() && (!wanted->isIntegerTy(1) || wantedAttributes.hasAttribute(Attribute::ZExt) ==
                                                                         callee.hasRetAttribute(Attribute::ZExt))) {
    out.resultWhole = true;
    return "";
  }
  std::map<unsigned, Piece> given;
  std::map<unsigned, Piece> read;
  if (!arrangeResult(own->getReturnType(), callee.getAttributes().getRetAttrs(), given)) {
    return "a result of type " + typeName(own->getReturnType()) + " from " + name;
  }
  if (!arrangeResult(wanted, AttributeSet(), read)) {
    return "a result of type " + typeName(wanted) + " from a call of " + name;
  }
  std::vector<Type *> elements = elementsOf(wanted);
  for (unsigned e = 0; e < elements.size(); ++e) {
    std::vector<unsigned> locations;
    for (const auto &[location, piece] : read) {
      if (piece.value == e) {
        locations.push_back(location);
      }
    }
    bool isBool = elements[e]->isIntegerTy(1) && wantedAttributes.hasAttribute(Attribute::ZExt);
    Source source;
    std::string why = collect(
        [&](unsigned i) -> const Piece * {
          if (i >= locations.size()) {
            return nullptr;
          }
          auto found = given.find(locations[i]);
          return found == given.end() ? nullptr : &found->second;
        },
        isBool ? boolBits : *imageBits(elements[e]), source);
    if (!why.empty()) {
      return "the result of " + name + ": " + why;
    }
    if (isBool) {
      source.check = Source::Bool;
    }
    out.result.push_back(source);
  }
  return "";
}

// The image of `value` as an integer.
Value *imageOf(IRBuilder<> &b, Value *value, bool extend, bool sign) {
  Type *type = value->getType();
  if (type->isPointerTy()) {
    return b.CreatePtrToInt(value, b.getInt64Ty());
  }
  if (!type->isIntegerTy()) {
    return b.CreateBitCast(value, b.getIntNTy(unsigned(type->getPrimitiveSizeInBits().getFixedValue())));
  }
  if (extend && type->getIntegerBitWidth() < extendedBits) {
    return sign ? b.CreateSExt(value, b.getInt32Ty()) : b.CreateZExt(value, b.getInt32Ty());
  }
  return value;
}

// Stops the run with the named fault `unreachable` unless `ok`, before `at`.
void trapUnless(Value *ok, Instruction *at) {
  Instruction *then = SplitBlockAndInsertIfThen(BinaryOperator::CreateNot(ok, "", at), at, /*Unreachable=*/true);
  IRBuilder<> b(then);
  b.CreateCall(Intrinsic::getDeclaration(at->getModule(), Intrinsic::trap));
}

// The value `source` makes, of `type`, from the images `images` gives (by piece value), at `b`. A check splits
// the block; `b` is left after it.
Value *build(IRBuilder<> &b, const Source &source, Type *type, const std::function<Value *(unsigned)> &images) {
  IntegerType *wide = b.getIntNTy(source.need);
  Value *bits = ConstantInt::get(wide, 0);
  unsigned at = 0;
  for (const auto &[piece, take] : source.parts) {
    Value *image = images(piece.value);
    if (image->getType()->getIntegerBitWidth() < piece.shift + take) {
      image = b.CreateZExt(image, b.getIntNTy(piece.shift + take)); // a `zeroext` `i1` result fills its byte
    }
    Value *part = b.CreateLShr(image, piece.shift);
    part = b.CreateZExtOrTrunc(b.CreateTrunc(part, b.getIntNTy(take)), wide);
    bits = b.CreateOr(bits, at ? b.CreateShl(part, at) : part);
    at += take;
  }
  unsigned width = type->isIntegerTy() ? type->getIntegerBitWidth() : unsigned(type->getPrimitiveSizeInBits());
  if (type->isPointerTy()) {
    width = 64;
  }
  Value *value = b.CreateTrunc(bits, b.getIntNTy(width));
  if (source.check == Source::Sign || source.check == Source::Zero) {
    Value *again = source.check == Source::Sign ? b.CreateSExt(value, wide) : b.CreateZExt(value, wide);
    trapUnless(b.CreateICmpEQ(again, bits), &*b.GetInsertPoint());
    b.SetInsertPoint(&*b.GetInsertPoint()); // now in the block after the check
  } else if (source.check == Source::Bool) {
    trapUnless(b.CreateICmpULT(bits, ConstantInt::get(wide, 2)), &*b.GetInsertPoint());
    b.SetInsertPoint(&*b.GetInsertPoint());
  }
  if (type->isPointerTy()) {
    return b.CreateIntToPtr(value, type);
  }
  return type->isIntegerTy() ? value : b.CreateBitCast(value, type);
}

// Replaces `call` of type `type` by a call of `callee` with its own prototype, as `plan` says. Inserts
// before `at` and returns the value of the caller's result type (null when void or unused).
Value *emitPlanned(CallInst *call, FunctionType *type, Function *callee, const Plan &plan, Instruction *at) {
  IRBuilder<> b(at);
  b.SetCurrentDebugLocation(call->getDebugLoc());
  AttributeList attributes = call->getAttributes();
  std::map<unsigned, Value *> images;
  auto argImage = [&](unsigned i) {
    if (!images.count(i)) {
      images[i] = imageOf(b, call->getArgOperand(i), extended(attributes.getParamAttrs(i)),
                          attributes.hasParamAttr(i, Attribute::SExt));
    }
    return images[i];
  };
  FunctionType *own = callee->getFunctionType();
  std::vector<Value *> args;
  std::vector<AttributeSet> params;
  for (unsigned j = 0; j < own->getNumParams(); ++j) {
    const Source &source = plan.params[j];
    Type *param = own->getParamType(j);
    if (source.kind == Source::Unread) {
      args.push_back(Constant::getNullValue(param));
    } else if (source.kind == Source::Whole) {
      args.push_back(call->getArgOperand(source.whole));
    } else {
      args.push_back(build(b, source, param, argImage));
    }
    params.push_back(callee->getAttributes().getParamAttrs(j));
  }
  for (unsigned i : plan.variadic) {
    args.push_back(call->getArgOperand(i));
    params.push_back(attributes.getParamAttrs(i));
  }
  CallInst *direct = b.CreateCall(own, callee, args);
  direct->setAttributes(
      AttributeList::get(call->getContext(), AttributeSet(), callee->getAttributes().getRetAttrs(), params));
  direct->setDebugLoc(call->getDebugLoc());
  if (call->getType()->isVoidTy() || call->use_empty()) {
    return nullptr;
  }
  if (plan.resultWhole) {
    return direct;
  }
  std::vector<Value *> elements;
  std::map<unsigned, Value *> results;
  auto resultImage = [&](unsigned e) {
    if (!results.count(e)) {
      Value *element = isa<StructType>(own->getReturnType()) ? b.CreateExtractValue(direct, e) : direct;
      results[e] = imageOf(b, element, false, false);
    }
    return results[e];
  };
  std::vector<Type *> wanted = elementsOf(type->getReturnType());
  for (unsigned e = 0; e < wanted.size(); ++e) {
    elements.push_back(build(b, plan.result[e], wanted[e], resultImage));
  }
  if (!isa<StructType>(type->getReturnType())) {
    return elements.front();
  }
  Value *result = PoisonValue::get(type->getReturnType());
  for (unsigned e = 0; e < elements.size(); ++e) {
    result = b.CreateInsertValue(result, elements[e], e);
  }
  return result;
}

std::vector<Type *> argTypes(const CallInst *call) {
  std::vector<Type *> types;
  for (const Value *arg : call->args()) {
    types.push_back(arg->getType());
  }
  return types;
}

// Whether a call of `type` with `attributes` reaches `callee` natively just as a device call does: the same
// prototype and the same extension of narrow values.
bool sameShape(FunctionType *type, const AttributeList &attributes, const Function &callee) {
  if (type != callee.getFunctionType()) {
    return false;
  }
  auto narrow = [](Type *t) { return t->isIntegerTy() && t->getIntegerBitWidth() < extendedBits; };
  for (unsigned i = 0; i < type->getNumParams(); ++i) {
    if (narrow(type->getParamType(i)) &&
        (attributes.hasParamAttr(i, Attribute::SExt) != callee.hasParamAttribute(i, Attribute::SExt) ||
         attributes.hasParamAttr(i, Attribute::ZExt) != callee.hasParamAttribute(i, Attribute::ZExt))) {
      return false;
    }
    if (attributes.getParamByValType(i) != callee.getParamByValType(i) ||
        attributes.hasParamAttr(i, Attribute::StructRet) != callee.hasParamAttribute(i, Attribute::StructRet)) {
      return false;
    }
  }
  return !type->getReturnType()->isIntegerTy(1) ||
         attributes.hasRetAttr(Attribute::ZExt) == callee.hasRetAttribute(Attribute::ZExt);
}

// A plain call with the argument types `call` has, of `target`, before `at`.
CallInst *plainCall(CallInst *call, Value *target, Instruction *at) {
  IRBuilder<> b(at);
  FunctionType *type = FunctionType::get(call->getType(), argTypes(call), false);
  std::vector<Value *> args(call->arg_begin(), call->arg_end());
  CallInst *plain = b.CreateCall(type, target, args);
  plain->setAttributes(call->getAttributes());
  plain->setCallingConv(call->getCallingConv());
  plain->setDebugLoc(call->getDebugLoc());
  return plain;
}

// Whether a device call of `type` may reach `callee` without the named fault of a mismatched indirect call:
// the device compares the widths of parameters and result, as the later lowerings leave them. Generous
// where those lowerings change the prototype (aggregates and vectors, `x86_fp80`).
bool mayRunAlike(FunctionType *type, const Function &callee) {
  auto width = [](Type *t) -> int {
    if (t->isIntegerTy()) {
      return int(t->getIntegerBitWidth());
    }
    if (t->isPointerTy() || t->isDoubleTy()) {
      return 64;
    }
    if (t->isFloatTy()) {
      return 32;
    }
    if (t->isVoidTy()) {
      return 0;
    }
    return -1; // any
  };
  auto exotic = [](FunctionType *t) {
    return t->getReturnType()->isAggregateType() || t->getReturnType()->isVectorTy() ||
           std::any_of(t->param_begin(), t->param_end(), [](Type *p) { return p->isVectorTy(); });
  };
  FunctionType *own = callee.getFunctionType();
  if (exotic(type) || exotic(own)) {
    return true;
  }
  std::vector<int> want;
  std::vector<int> have;
  for (Type *param : type->params()) {
    want.push_back(width(param));
  }
  for (Type *param : own->params()) {
    have.push_back(width(param));
  }
  if (own->isVarArg()) {
    have.push_back(64); // the area (lowerVariadics)
  }
  auto same = [](int a, int b) { return a == b || a < 0 || b < 0; };
  if (want.size() != have.size() || !same(width(type->getReturnType()), width(own->getReturnType()))) {
    return false;
  }
  for (size_t i = 0; i < want.size(); ++i) {
    if (!same(want[i], have[i])) {
      return false;
    }
  }
  return true;
}

// A call through a pointer. The target is known only when the call runs, so the call compares the pointer
// with each function whose address is taken and that a device call would not reach as the native call
// does: on a match it calls that function with its own prototype, as planned, or, where the convention
// does not determine what the native call reads and the device call would run, stops with the named fault
// `unreachable`. Otherwise it is a plain call (for a call of variadic type, through an unprototyped
// pointer: with the argument types it has), and a target it does not fit ends in the named fault of a
// mismatched indirect call. The comparisons cost every such call, also in programs that never call
// through another prototype (measured on the musl tests: see experiments/c-corpus/README.md).
void lowerCallThroughPointer(CallInst *call, const std::vector<Function *> &taken, const DataLayout &layout) {
  FunctionType *type = call->getFunctionType();
  FunctionType *plainType = type->isVarArg() ? FunctionType::get(call->getType(), argTypes(call), false) : type;
  bool used = !call->getType()->isVoidTy() && !call->use_empty();
  std::vector<std::pair<Function *, std::optional<Plan>>> candidates;
  for (Function *function : taken) {
    if (sameShape(plainType, call->getAttributes(), *function)) {
      continue;
    }
    Plan planned;
    bool adaptable = plan(type, call->getAttributes(), argTypes(call), used, *function, layout, call, planned).empty();
    bool alike = mayRunAlike(plainType, *function);
    if (adaptable) {
      candidates.emplace_back(function, std::move(planned));
    } else if (alike) {
      candidates.emplace_back(function, std::nullopt);
    }
  }
  if (candidates.empty() && !type->isVarArg()) {
    return;
  }
  if (call->isMustTailCall()) {
    notHandled("a `musttail` call through a pointer whose targets need other prototypes", call);
  }
  Value *target = call->getCalledOperand();
  BasicBlock *head = call->getParent();
  Function &caller = *head->getParent();
  BasicBlock *tail = head->splitBasicBlock(call, "call.join");
  head->getTerminator()->eraseFromParent();
  PHINode *phi = nullptr;
  if (used) {
    phi = PHINode::Create(call->getType(), unsigned(candidates.size() + 1), "", &*tail->begin());
  }
  BasicBlock *test = head;
  for (const auto &[function, planned] : candidates) {
    BasicBlock *match = BasicBlock::Create(call->getContext(), "call.adapted", &caller, tail);
    BasicBlock *next = BasicBlock::Create(call->getContext(), "call.next", &caller, tail);
    IRBuilder<> t(test);
    t.SetCurrentDebugLocation(call->getDebugLoc());
    t.CreateCondBr(t.CreateICmpEQ(target, function), match, next);
    test = next;
    if (!planned) {
      IRBuilder<> m(match);
      m.SetCurrentDebugLocation(call->getDebugLoc());
      m.CreateCall(Intrinsic::getDeclaration(call->getModule(), Intrinsic::trap));
      m.CreateUnreachable();
      continue;
    }
    BranchInst *join = BranchInst::Create(tail, match);
    join->setDebugLoc(call->getDebugLoc());
    Value *value = emitPlanned(call, type, function, *planned, join);
    if (phi) {
      phi->addIncoming(value, join->getParent());
    }
  }
  BranchInst *join = BranchInst::Create(tail, test);
  join->setDebugLoc(call->getDebugLoc());
  CallInst *fallback = type->isVarArg() ? plainCall(call, target, join) : cast<CallInst>(call->clone());
  if (!type->isVarArg()) {
    fallback->insertBefore(join);
  }
  if (phi) {
    phi->addIncoming(fallback, test);
    phi->takeName(call);
    call->replaceAllUsesWith(phi);
  }
  call->eraseFromParent();
}

} // namespace

void lowerMismatchedCalls(Module &module) {
  const DataLayout &layout = module.getDataLayout();
  std::vector<Function *> taken; // functions a pointer may hold
  std::vector<CallInst *> direct;
  std::vector<CallInst *> indirect;
  for (Function &function : module) {
    if (!function.isDeclaration() && function.hasAddressTaken()) {
      taken.push_back(&function);
    }
    for (Instruction &inst : instructions(function)) {
      auto *call = dyn_cast<CallInst>(&inst);
      if (!call || call->isInlineAsm() || isa<IntrinsicInst>(call)) {
        continue;
      }
      auto *callee = dyn_cast<Function>(call->getCalledOperand());
      if (!callee) {
        indirect.push_back(call); // through a pointer, or an alias

      } else if (callee && !callee->isDeclaration() &&
                 !sameShape(call->getFunctionType(), call->getAttributes(), *callee)) {
        direct.push_back(call);
      }
    }
  }
  for (CallInst *call : direct) {
    auto *callee = cast<Function>(call->getCalledOperand());
    // LLVM's InstCombine (so native clang -O2) passes an argument of the same size as the parameter as a cast
    // of its value, where the convention (and -O0) passes it in a register of another class: native builds
    // disagree, and nothing determines the result.
    FunctionType *own = callee->getFunctionType();
    for (unsigned i = 0; i < std::min<unsigned>(call->arg_size(), own->getNumParams()); ++i) {
      Type *arg = call->getArgOperand(i)->getType();
      Type *param = own->getParamType(i);
      auto floating = [](Type *t) { return t->isFloatingPointTy() || t->isVectorTy(); };
      if (floating(arg) != floating(param) && arg->isSized() && param->isSized() &&
          layout.getTypeSizeInBits(arg) == layout.getTypeSizeInBits(param)) {
        notHandled("a call of @" + callee->getName().str() + " through another prototype whose argument " +
                       std::to_string(i + 1) + " is " + typeName(arg) + " for a parameter " + typeName(param) +
                       ": native clang -O2 passes it as a cast and -O0 in a register of another class",
                   call);
      }
    }
    Plan planned;
    bool used = !call->getType()->isVoidTy() && !call->use_empty();
    if (std::string why = plan(call->getFunctionType(), call->getAttributes(), argTypes(call), used, *callee, layout,
                               call, planned);
        !why.empty()) {
      notHandled("a call of @" + callee->getName().str() +
                     " through another prototype, where x86-64 does not determine what is read: " + why,
                 call);
    }
    if (call->isMustTailCall()) {
      notHandled("a `musttail` call of @" + callee->getName().str() + " through another prototype", call);
    }
    Value *value = emitPlanned(call, call->getFunctionType(), callee, planned, call);
    if (value) {
      value->takeName(call);
      call->replaceAllUsesWith(value);
    }
    call->eraseFromParent();
  }
  for (CallInst *call : indirect) {
    lowerCallThroughPointer(call, taken, layout);
  }
}

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

  for (CallInst *call : calls) {
    auto *callee = dyn_cast<Function>(call->getCalledOperand());
    if (callee && callee->isDeclaration()) {
      continue; // refused later: no library is linked yet
    }
    if (!callee || !callee->isVarArg() || callee->getFunctionType() != call->getFunctionType()) {
      notHandled("internal: a variadic call that lowerMismatchedCalls left through another prototype", call);
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
