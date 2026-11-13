#include "Emit.h"

#include "Diagnostic.h"
#include "Markers.h"

#include <llvm/ADT/MapVector.h>
#include <llvm/ADT/PostOrderIterator.h>
#include <llvm/IR/Constants.h>
#include <llvm/IR/DataLayout.h>
#include <llvm/IR/Function.h>
#include <llvm/IR/InstIterator.h>
#include <llvm/IR/Instructions.h>
#include <llvm/IR/IntrinsicInst.h>
#include <llvm/IR/Module.h>
#include <llvm/IR/Operator.h>
#include <llvm/Support/raw_ostream.h>

#include <cctype>

using namespace llvm;

namespace bennett {

namespace {

// The lowest address static data may use: address 0 is the null pointer, so nothing lives near it.
constexpr uint64_t firstDataAddress = 16;

// The alignment of every `alloca` result in the device program (docs/SYNTAX.md). A wider alignment is
// made by allocating more and rounding the address up.
constexpr uint64_t deviceAllocaAlign = 8;

std::string sanitize(const std::string &hint) {
  std::string name;
  for (char c : hint) {
    name += std::isalnum(static_cast<unsigned char>(c)) || c == '_' || c == '.' ? c : '_';
  }
  if (name.empty() || !(std::isalpha(static_cast<unsigned char>(name[0])) || name[0] == '_')) {
    name = "v" + name;
  }
  return name;
}

std::string printed(const Value *value) {
  std::string text;
  raw_string_ostream out(text);
  value->printAsOperand(out, false);
  return out.str();
}

std::string printedType(const Type *type) {
  std::string text;
  raw_string_ostream out(text);
  type->print(out);
  return out.str();
}

const char *binaryMnemonic(unsigned opcode) {
  switch (opcode) {
  case Instruction::Add: return "add";
  case Instruction::Sub: return "sub";
  case Instruction::Mul: return "mul";
  case Instruction::UDiv: return "udiv";
  case Instruction::SDiv: return "sdiv";
  case Instruction::URem: return "urem";
  case Instruction::SRem: return "srem";
  case Instruction::And: return "and";
  case Instruction::Or: return "or";
  case Instruction::Xor: return "xor";
  case Instruction::Shl: return "shl";
  case Instruction::LShr: return "lshr";
  case Instruction::AShr: return "ashr";
  default: return nullptr;
  }
}

const char *predicateMnemonic(CmpInst::Predicate pred) {
  switch (pred) {
  case CmpInst::ICMP_EQ: return "eq";
  case CmpInst::ICMP_NE: return "ne";
  case CmpInst::ICMP_ULT: return "ult";
  case CmpInst::ICMP_ULE: return "ule";
  case CmpInst::ICMP_UGT: return "ugt";
  case CmpInst::ICMP_UGE: return "uge";
  case CmpInst::ICMP_SLT: return "slt";
  case CmpInst::ICMP_SLE: return "sle";
  case CmpInst::ICMP_SGT: return "sgt";
  case CmpInst::ICMP_SGE: return "sge";
  default: return nullptr;
  }
}

} // namespace

std::string Namer::fresh(const std::string &hint) {
  std::string base = sanitize(hint);
  std::string name = base;
  for (unsigned n = 1; used.count(name); ++n) {
    name = base + "." + std::to_string(n);
  }
  used.insert(name);
  return name;
}

// ---------------------------------------------------------------------------------------------------------
// The module.

namespace {

// Pointers in another address space than 0 may have another size or meaning; the device program has one
// flat memory addressed by 64-bit integers.
void checkAddressSpace(const Type *type, const Value *at) {
  if (const auto *pointer = dyn_cast<PointerType>(type->getScalarType()); pointer && pointer->getAddressSpace() != 0) {
    notHandled("a pointer in address space " + std::to_string(pointer->getAddressSpace()), at);
  }
}

} // namespace

ModuleWriter::ModuleWriter(Module &module, const Options &options, const std::set<const Function *> &control)
    : options(options), module(module), control(control) {
  const DataLayout &layout = module.getDataLayout();
  if (layout.getPointerSizeInBits() != 64 || layout.getIndexSizeInBits(0) != 64) {
    fail("the input's pointers or address offsets are not 64 bits wide");
  }
  if (layout.isBigEndian()) {
    fail("the input is for a big-endian target; the device program's memory is little-endian");
  }
  for (const GlobalVariable &global : module.globals()) {
    checkAddressSpace(global.getType(), &global);
    if (global.isExternallyInitialized()) {
      notHandled("the global @" + global.getName().str() + ", which is initialised outside the program", &global);
    }
  }
  for (const Function &function : module) {
    if (function.isDeclaration()) {
      continue;
    }
    if (function.isVarArg()) {
      notHandled("the variadic function @" + function.getName().str(), &function);
    }
    for (const Instruction &inst : instructions(function)) {
      checkAddressSpace(inst.getType(), &inst);
      for (const Use &use : inst.operands()) {
        checkAddressSpace(use->getType(), &inst);
      }
    }
    indices[&function] = definedCount++;
    names[&function] = "@" + functionNames.fresh(function.getName().str());
  }
  layoutGlobals();
}

const DataLayout &ModuleWriter::layout() const { return module.getDataLayout(); }

bool ModuleWriter::isCoherent(const Function *function) const {
  return function && !function->isDeclaration() && !isControl(function) && function->onlyReadsMemory();
}

void ModuleWriter::checkFunctionAddress(const Function *function) const {
  uint64_t align = function->getPointerAlignment(layout()).value();
  if (align > 1) {
    notHandled("the address of @" + function->getName().str() + ", which LLVM assumes is a multiple of " +
                   std::to_string(align) + " (a function's address is its index plus 1)",
               function);
  }
}

std::string ModuleWriter::functionName(const Function *function) const {
  auto found = names.find(function);
  if (found == names.end()) {
    notHandled("a call to @" + function->getName().str() + ", which has no definition (no library is linked yet)",
               function);
  }
  return found->second;
}

void ModuleWriter::layoutGlobals() {
  uint64_t address = firstDataAddress;
  for (const GlobalVariable &global : module.globals()) {
    if (!global.hasInitializer()) {
      notHandled("the global @" + global.getName().str() + ", which has no definition", &global);
    }
    TypeSize size = layout().getTypeAllocSize(global.getValueType());
    if (size.isScalable()) {
      notHandled("the global @" + global.getName().str() + ", whose size is not fixed", &global);
    }
    // At least the explicit alignment, and what LLVM may have assumed about the address.
    uint64_t align = std::max(layout().getPreferredAlign(&global), global.getPointerAlignment(layout())).value();
    address = (address + align - 1) / align * align;
    addresses[&global] = address;
    address += size.getFixedValue();
  }
  staticEnd = address;
}

std::optional<uint64_t> ModuleWriter::constantValue(const Constant *constant) const {
  if (const auto *integer = dyn_cast<ConstantInt>(constant)) {
    if (integer->getBitWidth() > 64) {
      return std::nullopt;
    }
    return integer->getZExtValue();
  }
  if (isa<ConstantPointerNull>(constant) || isa<UndefValue>(constant)) {
    return 0; // an undefined value may be any value; 0 refines it
  }
  if (const auto *global = dyn_cast<GlobalVariable>(constant)) {
    auto found = addresses.find(global);
    return found == addresses.end() ? std::nullopt : std::optional<uint64_t>(found->second);
  }
  if (const auto *function = dyn_cast<Function>(constant)) {
    checkFunctionAddress(function);
    auto found = indices.find(function);
    // A function's address is its index plus one (bennett_ir::FuncId::address).
    return found == indices.end() ? std::nullopt : std::optional<uint64_t>(found->second + 1);
  }
  if (const auto *alias = dyn_cast<GlobalAlias>(constant)) {
    return constantValue(alias->getAliasee());
  }
  if (const auto *expr = dyn_cast<ConstantExpr>(constant)) {
    if (const auto *gep = dyn_cast<GEPOperator>(expr)) {
      APInt offset(64, 0);
      std::optional<uint64_t> base = constantValue(cast<Constant>(gep->getPointerOperand()));
      if (!base || !gep->accumulateConstantOffset(layout(), offset)) {
        return std::nullopt;
      }
      return *base + offset.getZExtValue();
    }
    switch (expr->getOpcode()) {
    case Instruction::PtrToInt:
    case Instruction::IntToPtr:
    case Instruction::BitCast:
    case Instruction::AddrSpaceCast: {
      std::optional<uint64_t> value = constantValue(expr->getOperand(0));
      unsigned bits = expr->getType()->isPointerTy() ? 64 : expr->getType()->getIntegerBitWidth();
      if (!value || bits > 64) {
        return std::nullopt;
      }
      return bits == 64 ? *value : *value & ((uint64_t(1) << bits) - 1);
    }
    default:
      return std::nullopt;
    }
  }
  return std::nullopt;
}

namespace {

// The bytes of an initializer, and where function addresses go.
struct Image {
  std::vector<uint8_t> bytes;
  std::vector<std::pair<uint64_t, const Function *>> functions;
};

// Writes the bytes of `constant`, of any type and width, at `at`: an initializer is just bytes. What it
// cannot write it refuses by name.
void encode(const ModuleWriter &writer, const GlobalVariable &global, const Constant *constant, uint64_t at,
            Image &image) {
  if (!constant) {
    notHandled("an initializer of @" + global.getName().str() + " whose elements LLVM does not list", &global);
  }
  const DataLayout &layout = writer.layout();
  Type *type = constant->getType();
  auto refuse = [&](const std::string &why) {
    notHandled("the initializer " + printed(constant) + " of type " + printedType(type) + " in @" +
                   global.getName().str() + " (" + why + ")",
               &global);
  };
  auto put = [&](APInt bits) {
    uint64_t size = layout.getTypeStoreSize(type).getFixedValue();
    bits = bits.zext(unsigned(size * 8));
    for (uint64_t i = 0; i < size; ++i) {
      image.bytes[at + i] = uint8_t(bits.extractBitsAsZExtValue(8, unsigned(8 * i)));
    }
  };
  if (isa<ConstantAggregateZero>(constant) || isa<ConstantPointerNull>(constant) || isa<UndefValue>(constant)) {
    return; // already zero; an undefined value may be any value, and 0 refines it
  }
  if (const auto *function = dyn_cast<Function>(constant)) {
    writer.checkFunctionAddress(function);
    image.functions.push_back({at, function});
    return;
  }
  if (const auto *integer = dyn_cast<ConstantInt>(constant)) {
    put(integer->getValue());
    return;
  }
  if (const auto *fp = dyn_cast<ConstantFP>(constant)) {
    if (type->isPPC_FP128Ty()) {
      refuse("the byte order of ppc_fp128 is not known here");
    }
    put(fp->getValueAPF().bitcastToAPInt());
    return;
  }
  if (auto *structure = dyn_cast<StructType>(type)) {
    const StructLayout *fields = layout.getStructLayout(structure);
    for (unsigned i = 0; i < structure->getNumElements(); ++i) {
      encode(writer, global, constant->getAggregateElement(i), at + fields->getElementOffset(i), image);
    }
    return;
  }
  if (auto *array = dyn_cast<ArrayType>(type)) {
    uint64_t step = layout.getTypeAllocSize(array->getElementType()).getFixedValue();
    for (uint64_t i = 0; i < array->getNumElements(); ++i) {
      encode(writer, global, constant->getAggregateElement(unsigned(i)), at + i * step, image);
    }
    return;
  }
  if (auto *vector = dyn_cast<FixedVectorType>(type)) {
    // A vector is stored as one integer of all its elements' bits, element 0 lowest: elements of whole
    // bytes are packed without padding.
    uint64_t bits = layout.getTypeSizeInBits(vector->getElementType()).getFixedValue();
    if (bits % 8 != 0) {
      refuse("vector elements that are not a whole number of bytes");
    }
    for (unsigned i = 0; i < vector->getNumElements(); ++i) {
      encode(writer, global, constant->getAggregateElement(i), at + i * bits / 8, image);
    }
    return;
  }
  if (type->isIntOrPtrTy() && layout.getTypeStoreSize(type).getFixedValue() <= 8) {
    if (std::optional<uint64_t> value = writer.constantValue(constant)) {
      put(APInt(64, *value));
      return;
    }
  }
  refuse("not a value known before the program runs");
}

std::string quoted(const std::vector<uint8_t> &bytes, uint64_t from, uint64_t to) {
  std::string text = "\"";
  const char *hex = "0123456789abcdef";
  for (uint64_t i = from; i < to; ++i) {
    uint8_t byte = bytes[i];
    if (byte >= 0x20 && byte < 0x7f && byte != '"' && byte != '\\') {
      text += char(byte);
    } else {
      text += "\\x";
      text += hex[byte >> 4];
      text += hex[byte & 15];
    }
  }
  return text + "\"";
}

} // namespace

std::string ModuleWriter::dataSegments() const {
  std::string text;
  for (const GlobalVariable &global : module.globals()) {
    uint64_t size = layout().getTypeAllocSize(global.getValueType()).getFixedValue();
    if (size == 0) {
      continue;
    }
    Image image;
    image.bytes.assign(size, 0);
    encode(*this, global, global.getInitializer(), 0, image);
    std::string items;
    uint64_t from = 0;
    for (auto [offset, function] : image.functions) {
      if (offset > from) {
        items += (items.empty() ? "" : ", ") + quoted(image.bytes, from, offset);
      }
      items += (items.empty() ? "" : ", ") + std::string("i64 ") + functionName(function);
      from = offset + 8;
    }
    if (from < size) {
      items += (items.empty() ? "" : ", ") + quoted(image.bytes, from, size);
    }
    text += "; @" + global.getName().str() + "\n";
    text += "data " + std::to_string(addresses.at(&global)) + " = " + items + "\n";
  }
  return text;
}

std::string ModuleWriter::addFunction(const std::string &hint, const std::string &body) {
  std::string name = "@" + functionNames.fresh(hint);
  generated.push_back("func " + name + body);
  return name;
}

std::string ModuleWriter::copyFunction(unsigned bits) {
  auto found = copies.find(bits);
  if (found != copies.end()) {
    return found->second;
  }
  std::string type = BodyWriter::intType(bits);
  std::string name =
      addFunction("bennett.copy." + type, "(" + type + " %value) -> " + type + " {\nentry:\n    ret %value\n}\n");
  copies[bits] = name;
  return name;
}

std::string ModuleWriter::write() {
  std::string functions;
  for (Function &function : module) {
    if (function.isDeclaration()) {
      continue;
    }
    functions += "\n";
    functions += isControl(&function) ? writeControlFunction(*this, function) : BodyWriter(*this, &function).function();
  }
  for (const std::string &text : generated) {
    functions += "\n" + text;
  }
  std::string header = "; Written by bennett-front.\n";
  header += "memory " + std::to_string(staticEnd + options.stackBytes) + "\n";
  return header + dataSegments() + functions;
}

// ---------------------------------------------------------------------------------------------------------
// One function.

BodyWriter::BodyWriter(ModuleWriter &module, const Function *function) : module(module), current(function) {}

unsigned BodyWriter::widthOf(const Type *type, const Value *at) {
  if (type->isPointerTy()) {
    return 64;
  }
  if (type->isIntegerTy() && type->getIntegerBitWidth() <= 64) {
    return type->getIntegerBitWidth();
  }
  notHandled("a value of type " + printedType(type), at);
}

const Value *BodyWriter::resolve(const Value *value) const {
  for (auto found = aliases.find(value); found != aliases.end(); found = aliases.find(value)) {
    value = found->second;
  }
  return value;
}

std::string BodyWriter::slot(const Value *value) {
  auto found = slots.find(value);
  if (found != slots.end()) {
    return found->second;
  }
  std::string name = fresh(value->hasName() ? value->getName().str() : "v");
  slots[value] = name;
  return name;
}

std::string BodyWriter::operand(const Value *value) {
  value = resolve(value);
  if (const auto *constant = dyn_cast<Constant>(value)) {
    std::optional<uint64_t> number = module.constantValue(constant);
    if (!number) {
      notHandled("the constant " + printed(constant), current);
    }
    return std::to_string(*number);
  }
  return slot(value);
}

std::string BodyWriter::typed(const Value *value) {
  const Value *resolved = resolve(value);
  if (isa<Constant>(resolved)) {
    return intType(widthOf(resolved->getType(), resolved)) + " " + operand(resolved);
  }
  return slot(resolved);
}

std::string BodyWriter::label(const BasicBlock *block) {
  auto found = labels.find(block);
  if (found != labels.end()) {
    return found->second;
  }
  std::string name = names.fresh(block->hasName() ? block->getName().str() : "block");
  labels[block] = name;
  return name;
}

std::string BodyWriter::target(const BasicBlock *from, const BasicBlock *to) {
  std::string text = label(to);
  std::string args;
  for (const PHINode &phi : to->phis()) {
    args += (args.empty() ? "" : ", ") + typed(phi.getIncomingValueForBlock(from));
  }
  return args.empty() ? text : text + "(" + args + ")";
}

void BodyWriter::findAliases(const Function &function) {
  for (const Instruction &inst : instructions(function)) {
    switch (inst.getOpcode()) {
    case Instruction::Freeze:
    case Instruction::AddrSpaceCast:
      alias(&inst, inst.getOperand(0));
      break;
    case Instruction::BitCast:
      if (inst.getType()->isPointerTy() == inst.getOperand(0)->getType()->isPointerTy() &&
          !inst.getType()->isFloatingPointTy() && !inst.getOperand(0)->getType()->isFloatingPointTy()) {
        alias(&inst, inst.getOperand(0));
      }
      break;
    case Instruction::PtrToInt:
      if (inst.getType()->getIntegerBitWidth() == 64) {
        alias(&inst, inst.getOperand(0));
      }
      break;
    case Instruction::IntToPtr:
      if (inst.getOperand(0)->getType()->getIntegerBitWidth() == 64) {
        alias(&inst, inst.getOperand(0));
      }
      break;
    default:
      break;
    }
  }
}

std::string BodyWriter::header(const Function &function) {
  std::string params;
  for (const Argument &arg : function.args()) {
    std::string name = fresh(arg.hasName() ? arg.getName().str() : "arg");
    bind(&arg, name);
    params += (params.empty() ? "" : ", ") + intType(widthOf(arg.getType(), &function)) + " " + name;
  }
  std::string text = "func " + module.functionName(&function) + "(" + params + ")";
  if (!function.getReturnType()->isVoidTy()) {
    text += " -> " + intType(widthOf(function.getReturnType(), &function));
  }
  return text + " {\n";
}

std::string BodyWriter::function() {
  findAliases(*current);
  std::string text = header(*current);
  body(*current);
  return text + out + "}\n";
}

void BodyWriter::body(const Function &function) {
  ReversePostOrderTraversal<const Function *> order(&function);
  for (const BasicBlock *block : order) {
    std::string params;
    for (const PHINode &phi : block->phis()) {
      params += (params.empty() ? "" : ", ") + intType(widthOf(phi.getType(), &phi)) + " " + slot(&phi);
    }
    out += label(block) + (params.empty() ? "" : "(" + params + ")") + ":\n";
    if (block == &function.getEntryBlock()) {
      prologue();
    }
    for (const Instruction &inst : *block) {
      if (isa<PHINode>(inst)) {
        continue;
      }
      if (inst.isTerminator()) {
        terminator(inst);
      } else {
        instruction(inst);
      }
    }
  }
}

void BodyWriter::instruction(const Instruction &inst) {
  if (aliases.count(&inst)) {
    return;
  }
  if (const char *op = binaryMnemonic(inst.getOpcode())) {
    unsigned width = widthOf(inst.getType(), &inst);
    line(slot(&inst) + " = " + op + " " + intType(width) + " " + operand(inst.getOperand(0)) + ", " +
         operand(inst.getOperand(1)));
    return;
  }
  if (const auto *cmp = dyn_cast<ICmpInst>(&inst)) {
    unsigned width = widthOf(cmp->getOperand(0)->getType(), &inst);
    line(slot(&inst) + " = icmp " + predicateMnemonic(cmp->getPredicate()) + " " + intType(width) + " " +
         operand(cmp->getOperand(0)) + ", " + operand(cmp->getOperand(1)));
    return;
  }
  if (const auto *select = dyn_cast<SelectInst>(&inst)) {
    widthOf(select->getCondition()->getType(), &inst);
    line(slot(&inst) + " = select " + intType(widthOf(inst.getType(), &inst)) + " " +
         operand(select->getCondition()) + ", " + operand(select->getTrueValue()) + ", " +
         operand(select->getFalseValue()));
    return;
  }
  switch (inst.getOpcode()) {
  case Instruction::ZExt:
  case Instruction::SExt:
  case Instruction::Trunc:
  case Instruction::PtrToInt:
  case Instruction::IntToPtr: {
    unsigned from = widthOf(inst.getOperand(0)->getType(), &inst);
    unsigned to = widthOf(inst.getType(), &inst);
    const char *op = inst.getOpcode() == Instruction::ZExt   ? "zext"
                     : inst.getOpcode() == Instruction::SExt ? "sext"
                     : from > to                              ? "trunc"
                                                              : "zext";
    line(slot(&inst) + " = " + op + " " + intType(from) + " " + operand(inst.getOperand(0)) + " to " + intType(to));
    return;
  }
  case Instruction::GetElementPtr:
    address(inst);
    return;
  case Instruction::Load: {
    const auto &load = cast<LoadInst>(inst);
    line(slot(&inst) + " = load " + intType(widthOf(load.getType(), &inst)) + " " +
         operand(load.getPointerOperand()));
    return;
  }
  case Instruction::Store: {
    const auto &store = cast<StoreInst>(inst);
    line("store " + intType(widthOf(store.getValueOperand()->getType(), &inst)) + " " +
         operand(store.getValueOperand()) + ", " + operand(store.getPointerOperand()));
    return;
  }
  case Instruction::Alloca: {
    const auto &alloca = cast<AllocaInst>(inst);
    if (alloca.isUsedWithInAlloca()) {
      notHandled("an `inalloca` allocation", &inst);
    }
    TypeSize elementSize = module.layout().getTypeAllocSize(alloca.getAllocatedType());
    if (elementSize.isScalable()) {
      notHandled("an allocation whose size is not fixed", &inst);
    }
    uint64_t element = elementSize.getFixedValue();
    // A wider alignment than the device program's: allocate align - 1 bytes more and round up.
    uint64_t align = alloca.getAlign().value();
    uint64_t padding = align > deviceAllocaAlign ? align - 1 : 0;
    std::string result = padding ? fresh(alloca.hasName() ? alloca.getName().str() + ".unaligned" : "unaligned")
                                 : slot(&inst);
    const Value *count = alloca.getArraySize();
    if (const auto *constant = dyn_cast<ConstantInt>(count)) {
      line(result + " = alloca " + std::to_string(element * constant->getZExtValue() + padding));
    } else {
      unsigned width = widthOf(count->getType(), &inst);
      std::string wide = operand(count);
      if (width < 64) {
        wide = fresh("count");
        line(wide + " = zext " + intType(width) + " " + operand(count) + " to i64");
      }
      std::string bytes = fresh("bytes");
      line(bytes + " = mul i64 " + wide + ", " + std::to_string(element));
      if (padding) {
        std::string padded = fresh("bytes");
        line(padded + " = add i64 " + bytes + ", " + std::to_string(padding));
        bytes = padded;
      }
      line(result + " = alloca " + bytes);
    }
    if (padding) {
      std::string up = fresh("rounding");
      line(up + " = add i64 " + result + ", " + std::to_string(padding));
      line(slot(&inst) + " = and i64 " + up + ", " + std::to_string(~(align - 1)));
    }
    return;
  }
  case Instruction::Call:
    call(inst);
    return;
  default:
    notHandled("the LLVM instruction `" + std::string(inst.getOpcodeName()) + "`", &inst);
  }
}

void BodyWriter::call(const Instruction &inst) {
  const auto &call = cast<CallBase>(inst);
  if (const auto *intrinsic = dyn_cast<IntrinsicInst>(&inst)) {
    if (intrinsic->isLifetimeStartOrEnd() || isa<DbgInfoIntrinsic>(intrinsic) ||
        intrinsic->getIntrinsicID() == Intrinsic::assume ||
        intrinsic->getIntrinsicID() == Intrinsic::experimental_noalias_scope_decl) {
      return;
    }
    notHandled("the intrinsic @" + intrinsic->getCalledFunction()->getName().str(), &inst);
  }
  if (call.isInlineAsm()) {
    notHandled("inline assembly", &inst);
  }
  if (markerCall(&inst)) {
    fail("internal: a marker call in a function treated as classical");
  }
  if (call.hasOperandBundles()) {
    notHandled("a call with the operand bundle `" + call.getOperandBundleAt(0).getTagName().str() + "`", &inst);
  }
  if (call.getFunctionType()->isVarArg()) {
    notHandled("a call of a variadic function", &inst);
  }
  std::string args;
  for (const Use &arg : call.args()) {
    args += (args.empty() ? "" : ", ") + typed(arg.get());
  }
  std::string result = call.getType()->isVoidTy() || call.use_empty() ? "" : slot(&inst) + " = ";
  if (const Function *callee = call.getCalledFunction()) {
    line(result + "call " + module.functionName(callee) + "(" + args + ")");
    return;
  }
  // getCalledFunction() is null when the call's type differs from the callee's, so a direct call through a
  // wrong prototype arrives here.
  if (const auto *callee = dyn_cast<Function>(call.getCalledOperand()->stripPointerCasts())) {
    notHandled("a call through a mismatched prototype of @" + callee->getName().str(), &inst);
  }
  const FunctionType *type = call.getFunctionType();
  std::string params;
  for (const Type *param : type->params()) {
    params += (params.empty() ? "" : ", ") + intType(widthOf(param, &inst));
  }
  std::string signature = "fn(" + params + ")";
  if (!type->getReturnType()->isVoidTy()) {
    signature += " -> " + intType(widthOf(type->getReturnType(), &inst));
  }
  line(result + "call_indirect " + signature + " " + operand(call.getCalledOperand()) + "(" + args + ")");
}

void BodyWriter::address(const Instruction &inst) {
  const auto &gep = cast<GEPOperator>(inst);
  if (inst.getType()->isVectorTy()) {
    notHandled("an address computation on vectors", &inst);
  }
  MapVector<Value *, APInt> variables;
  APInt constant(64, 0);
  if (!gep.collectOffset(module.layout(), 64, variables, constant)) {
    notHandled("an address computation LLVM cannot split into offsets", &inst);
  }
  std::string sum = operand(gep.getPointerOperand());
  for (auto &[index, scale] : variables) {
    unsigned width = widthOf(index->getType(), &inst);
    std::string term = operand(index);
    if (width < 64) {
      std::string wide = fresh("index");
      line(wide + " = sext " + intType(width) + " " + term + " to i64");
      term = wide;
    }
    if (scale != 1) {
      std::string scaled = fresh("offset");
      line(scaled + " = mul i64 " + term + ", " + std::to_string(scale.getSExtValue()));
      term = scaled;
    }
    std::string next = fresh("address");
    line(next + " = add i64 " + sum + ", " + term);
    sum = next;
  }
  line(slot(&inst) + " = add i64 " + sum + ", " + std::to_string(constant.getSExtValue()));
}

void BodyWriter::terminator(const Instruction &term) {
  const BasicBlock *from = term.getParent();
  if (const auto *ret = dyn_cast<ReturnInst>(&term)) {
    line(ret->getReturnValue() ? "ret " + operand(ret->getReturnValue()) : "ret");
    return;
  }
  if (const auto *branch = dyn_cast<BranchInst>(&term)) {
    if (branch->isUnconditional()) {
      line("br " + target(from, branch->getSuccessor(0)));
    } else {
      line("br " + operand(branch->getCondition()) + ", " + target(from, branch->getSuccessor(0)) + ", " +
           target(from, branch->getSuccessor(1)));
    }
    return;
  }
  if (const auto *switchInst = dyn_cast<SwitchInst>(&term)) {
    unsigned width = widthOf(switchInst->getCondition()->getType(), &term);
    std::string text = "switch " + intType(width) + " " + operand(switchInst->getCondition());
    for (const auto &kase : switchInst->cases()) {
      text += ", " + std::to_string(kase.getCaseValue()->getZExtValue()) + ": " +
              target(from, kase.getCaseSuccessor());
    }
    line(text + ", default: " + target(from, switchInst->getDefaultDest()));
    return;
  }
  if (isa<UnreachableInst>(&term)) {
    line("fault unreachable");
    return;
  }
  notHandled("the LLVM terminator `" + std::string(term.getOpcodeName()) + "`", &term);
}

} // namespace bennett
