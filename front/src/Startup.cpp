#include "Startup.h"
#include "Diagnostic.h"

#include <llvm/IR/Constants.h>
#include <llvm/IR/GlobalAlias.h>
#include <llvm/IR/IRBuilder.h>
#include <llvm/IR/Metadata.h>
#include <llvm/IR/Module.h>
#include <llvm/Transforms/Utils/ModuleUtils.h>

#include <algorithm>
#include <cstdint>
#include <set>
#include <string>
#include <vector>

using namespace llvm;

namespace bennett {
namespace {
const StringRef bounds[] = {"__init_array_start", "__init_array_end", "__fini_array_start", "__fini_array_end"};
const char *const pending = "bennett.startup.pending";

// One input section of the native object file. LLVM's AsmPrinter emits globals in module order: an explicit
// section object shares the section of that name with the special array's entries, unless `llvm.used` retains
// it, which gives it a section of its own (SHF_GNU_RETAIN). The special array's entries are emitted where that
// array sits, stable-sorted by priority clamped to 65535 (65535 is the plain section).
struct Section {
  std::string name;
  bool unique = false;
  bool numbered = false;
  uint32_t priority = 0;
  std::vector<Constant *> functions;
};
struct Array {
  std::vector<Section> sections;         // in creation order, then in link order
  std::vector<Constant *> functions;     // in link order
  std::vector<GlobalVariable *> objects; // explicit section objects
  GlobalVariable *special = nullptr;
  GlobalVariable *storage = nullptr;
};

void checkFunction(Constant *value, const Value *at) {
  auto *function = dyn_cast<Function>(value->stripPointerCasts());
  if (!value->getType()->isPointerTy() || value->getType()->getPointerAddressSpace() != 0 ||
      !function || function->isDeclaration() || !function->getReturnType()->isVoidTy() ||
      function->arg_size() != 0 || function->isVarArg() || function->getCallingConv() != CallingConv::C) {
    notHandled("a constructor or destructor other than a defined void(void) C function", at);
  }
}

Section &sectionNamed(Array &array, const std::string &name, StringRef prefix, bool unique, const Value *at) {
  if (!unique) {
    for (Section &section : array.sections) {
      if (!section.unique && section.name == name) {
        return section;
      }
    }
  }
  Section section;
  section.name = name;
  section.unique = unique;
  StringRef suffix = StringRef(name).drop_front(prefix.size());
  if (!suffix.empty()) {
    section.numbered = true;
    if (!suffix.consume_front(".") || suffix.empty() || suffix.getAsInteger(10, section.priority) ||
        section.priority > 65535) {
      notHandled("the constructor/destructor section " + name + " (no standard numeric priority)", at);
    }
  }
  array.sections.push_back(std::move(section));
  return array.sections.back();
}

void collectSpecial(Array &array, GlobalVariable &global, StringRef prefix) {
  StringRef special = global.getName();
  if (!global.hasInitializer() || !global.getValueType()->isArrayTy() || !global.use_empty()) {
    notHandled("a malformed or address-taken @" + special.str(), &global);
  }
  std::vector<std::pair<uint32_t, Constant *>> entries;
  Constant *values = global.getInitializer();
  for (unsigned i = 0; i < cast<ArrayType>(global.getValueType())->getNumElements(); ++i) {
    Constant *item = values->getAggregateElement(i);
    if (!item || !item->getType()->isStructTy() || cast<StructType>(item->getType())->getNumElements() != 3) {
      notHandled("a malformed @" + special.str(), &global);
    }
    auto *priority = dyn_cast_or_null<ConstantInt>(item->getAggregateElement(0u));
    Constant *function = item->getAggregateElement(1u);
    Constant *associated = item->getAggregateElement(2u);
    if (!priority || priority->getBitWidth() != 32 || !function || !associated || !associated->isNullValue()) {
      notHandled("a constructor/destructor with associated data or malformed priority", &global);
    }
    if (function->isNullValue()) {
      continue;
    }
    checkFunction(function, &global);
    entries.push_back({static_cast<uint32_t>(priority->getLimitedValue(65535)), function});
  }
  std::stable_sort(entries.begin(), entries.end(), [](const auto &a, const auto &b) { return a.first < b.first; });
  for (const auto &[priority, function] : entries) {
    std::string name = prefix.str() + (priority == 65535 ? "" : "." + std::to_string(priority));
    sectionNamed(array, name, prefix, false, &global).functions.push_back(function);
  }
}

void collectObject(Array &array, GlobalVariable &global, StringRef prefix, bool retained, Module &module) {
  if (!global.hasInitializer() || global.hasComdat() || global.isThreadLocal() || global.isExternallyInitialized() ||
      global.getPointerAlignment(module.getDataLayout()) > Align(8)) {
    notHandled("a constructor/destructor section without a plain static initializer and pointer alignment", &global);
  }
  if (global.hasMetadata(LLVMContext::MD_associated)) {
    // SHF_LINK_ORDER: the linker places the section by its associated symbol's section.
    notHandled("a constructor/destructor section object with !associated metadata", &global);
  }
  Section &section = sectionNamed(array, global.getSection().str(), prefix, retained, &global);
  Constant *init = global.getInitializer();
  auto append = [&](Constant *function) {
    if (!function || !function->getType()->isPointerTy()) {
      notHandled("a constructor/destructor section containing something other than function pointers", &global);
    }
    checkFunction(function, &global);
    section.functions.push_back(function);
  };
  if (auto *type = dyn_cast<ArrayType>(global.getValueType())) {
    for (unsigned i = 0; i < type->getNumElements(); ++i) {
      append(init->getAggregateElement(i));
    }
  } else {
    append(init);
  }
  array.objects.push_back(&global);
}

// The array as the native static link lays it out: the object's sections are placed as GNU ld's default script
// places them, numbered sections sorted by priority (ties by name, then input order) before the plain sections in
// input order.
Array collect(Module &module, StringRef special, StringRef prefix) {
  std::set<const GlobalValue *> retained;
  if (auto *used = module.getGlobalVariable("llvm.used"); used && used->hasInitializer()) {
    for (const Use &item : cast<Constant>(used->getInitializer())->operands()) {
      retained.insert(dyn_cast<GlobalValue>(item->stripPointerCasts()));
    }
  }
  Array array;
  for (GlobalVariable &global : module.globals()) {
    if (global.getName() == special) {
      array.special = &global;
      collectSpecial(array, global, prefix);
    } else if (global.getSection().starts_with(prefix)) {
      collectObject(array, global, prefix, retained.count(&global) != 0, module);
    }
  }
  std::stable_sort(array.sections.begin(), array.sections.end(), [](const Section &a, const Section &b) {
    if (a.numbered != b.numbered) {
      return a.numbered;
    }
    return a.numbered && (a.priority != b.priority ? a.priority < b.priority : a.name < b.name);
  });
  for (const Section &section : array.sections) {
    array.functions.insert(array.functions.end(), section.functions.begin(), section.functions.end());
  }
  return array;
}

void checkBounds(Module &module) {
  for (StringRef name : bounds) {
    GlobalValue *old = module.getNamedValue(name);
    if (old && (!old->isDeclaration() || !isa<GlobalVariable>(old) || old->getAddressSpace() != 0 ||
                old->getPointerAlignment(module.getDataLayout()) > Align(8))) {
      notHandled("the linker array bound @" + name.str() + " is already defined or has unsupported type/alignment",
                 old);
    }
  }
}

void defineBound(Module &module, StringRef name, Constant *address) {
  GlobalValue *old = module.getNamedValue(name);
  if (!old) {
    return;
  }
  old->setName("");
  auto *alias = GlobalAlias::create(Type::getInt8Ty(module.getContext()), 0, GlobalValue::ExternalLinkage,
                                  name, address, &module);
  old->replaceAllUsesWith(alias);
  old->eraseFromParent();
}

void materialize(Module &module, Array &array, StringRef kind) {
  auto *ptr = PointerType::getUnqual(module.getContext());
  auto *type = ArrayType::get(ptr, array.functions.size());
  array.storage = new GlobalVariable(module, type, true, GlobalValue::InternalLinkage,
                                    ConstantArray::get(type, array.functions), "__bennett." + kind + "_array");
  array.storage->setAlignment(Align(8));
  // Section entries are linker contributions. Their original addresses cannot in general survive sorting.
  for (GlobalVariable *global : array.objects) {
    global->removeDeadConstantUsers();
    if (!global->use_empty()) {
      notHandled("an address-taken constructor/destructor section object", global);
    }
    global->eraseFromParent();
  }
  if (array.special) {
    array.special->eraseFromParent();
  }
}

// What the static linker does: the bounds become the start and the end of the merged array.
void defineBounds(Module &module, GlobalVariable *storage, StringRef kind) {
  auto *i64 = Type::getInt64Ty(module.getContext());
  auto *type = cast<ArrayType>(storage->getValueType());
  Constant *startIndices[] = {ConstantInt::get(i64, 0), ConstantInt::get(i64, 0)};
  Constant *endIndices[] = {ConstantInt::get(i64, 0), ConstantInt::get(i64, type->getNumElements())};
  defineBound(module, ("__" + kind + "_array_start").str(),
              ConstantExpr::getGetElementPtr(type, storage, startIndices));
  defineBound(module, ("__" + kind + "_array_end").str(), ConstantExpr::getGetElementPtr(type, storage, endIndices));
}

bool boundsReferenced(Module &module) {
  bool referenced = false;
  for (StringRef name : bounds) {
    if (GlobalValue *value = module.getNamedValue(name)) {
      value->removeDeadConstantUsers();
      referenced |= !value->use_empty();
    }
  }
  return referenced;
}

// Renames `entryName` to an internal body and defines `entryName` as: constructors, the body, destructors in reverse.
void wrapEntry(Module &module, StringRef entryName, ArrayRef<Constant *> ctors, ArrayRef<Constant *> dtors) {
  Function *entry = module.getFunction(entryName);
  if (!entry || entry->isDeclaration() || entry->isVarArg() || entry->hasComdat()) {
    notHandled("constructors/destructors without a defined entry @" + entryName.str() + " (use --entry)", entry);
  }
  std::string name = entry->getName().str();
  entry->setName("__bennett.entry.body");
  auto linkage = entry->getLinkage();
  entry->setLinkage(GlobalValue::InternalLinkage);
  Function *wrapper = Function::Create(entry->getFunctionType(), linkage, name, module);
  wrapper->setCallingConv(entry->getCallingConv());
  IRBuilder<> builder(BasicBlock::Create(module.getContext(), "entry", wrapper));
  auto *signature = FunctionType::get(builder.getVoidTy(), false);
  for (Constant *ctor : ctors) {
    builder.CreateCall(signature, ctor);
  }
  std::vector<Value *> args;
  for (Argument &arg : wrapper->args()) {
    wrapper->addParamAttrs(arg.getArgNo(),
                          AttrBuilder(module.getContext(), entry->getAttributes().getParamAttrs(arg.getArgNo())));
    args.push_back(&arg);
  }
  CallInst *result = builder.CreateCall(entry, args);
  result->setCallingConv(entry->getCallingConv());
  result->setAttributes(entry->getAttributes());
  for (auto it = dtors.rbegin(); it != dtors.rend(); ++it) {
    builder.CreateCall(signature, *it);
  }
  if (entry->getReturnType()->isVoidTy()) {
    builder.CreateRetVoid();
  } else {
    builder.CreateRet(result);
  }
}
} // namespace

void lowerStartup(Module &module, StringRef entryName) {
  bool referenced = boundsReferenced(module);
  // These sections need ABI-specific startup or linker scripts; never emit them as inert data.
  for (const GlobalVariable &global : module.globals()) {
    StringRef section = global.getSection();
    if (section.starts_with(".preinit_array") || section.starts_with(".ctors") || section.starts_with(".dtors") ||
        section == ".init" || section == ".fini") {
      notHandled("the startup section " + section.str(), &global);
    }
  }
  checkBounds(module);
  Array ctors = collect(module, "llvm.global_ctors", ".init_array");
  Array dtors = collect(module, "llvm.global_dtors", ".fini_array");
  // llvm.used is an LLVM retention list, not a program read of these section objects. Remove only these
  // objects from retention before replacing them by the merged arrays, which remain referenced by startup.
  removeFromUsedLists(module, [&](Constant *value) {
    auto *global = dyn_cast<GlobalVariable>(value->stripPointerCasts());
    return llvm::is_contained(ctors.objects, global) || llvm::is_contained(dtors.objects, global);
  });
  materialize(module, ctors, "init");
  materialize(module, dtors, "fini");
  bool none = ctors.functions.empty() && dtors.functions.empty();
  if (none || !referenced) {
    // Nothing to run, or no C startup to run it: ownership is already decided.
    defineBounds(module, ctors.storage, "init");
    defineBounds(module, dtors.storage, "fini");
    if (!none) {
      wrapEntry(module, entryName, ctors.functions, dtors.functions);
    }
    return;
  }
  // A bound reference may sit in code the front's optimiser deletes; ownership is decided by finishStartup on the
  // references that survive it. Until then the bounds stay the undefined linker symbols a native compile sees, so
  // the optimiser cannot fold a read of the arrays into a direct call and erase a live reference that way: a
  // reference disappears only with dead code or an unused value. The arrays are kept so their functions survive.
  appendToCompilerUsed(module, {ctors.storage, dtors.storage});
  module.getOrInsertNamedMetadata(pending)->addOperand(MDNode::get(
      module.getContext(), {ConstantAsMetadata::get(ctors.storage), ConstantAsMetadata::get(dtors.storage)}));
}

void finishStartup(Module &module, StringRef entryName) {
  NamedMDNode *marker = module.getNamedMetadata(pending);
  if (!marker) {
    return;
  }
  GlobalVariable *storage[2] = {};
  for (int i = 0; i < 2 && marker->getNumOperands() == 1; ++i) {
    storage[i] = mdconst::dyn_extract_or_null<GlobalVariable>(marker->getOperand(0)->getOperand(i));
  }
  module.eraseNamedMetadata(marker);
  removeFromUsedLists(module, [&](Constant *value) { return llvm::is_contained(storage, value->stripPointerCasts()); });
  if (!storage[0] || !storage[1] || !storage[0]->hasInitializer() || !storage[1]->hasInitializer()) {
    fail("internal: the constructor/destructor arrays did not survive the optimiser");
  }
  if (boundsReferenced(module)) {
    defineBounds(module, storage[0], "init");
    defineBounds(module, storage[1], "fini");
    return;
  }
  std::vector<Constant *> functions[2];
  for (int i = 0; i < 2; ++i) {
    Constant *init = storage[i]->getInitializer();
    for (unsigned j = 0; j < cast<ArrayType>(storage[i]->getValueType())->getNumElements(); ++j) {
      functions[i].push_back(init->getAggregateElement(j));
    }
  }
  wrapEntry(module, entryName, functions[0], functions[1]);
  // Nothing refers to the arrays or their bounds now.
  for (StringRef name : bounds) {
    if (GlobalValue *value = module.getNamedValue(name)) {
      value->eraseFromParent();
    }
  }
  for (GlobalVariable *global : storage) {
    global->removeDeadConstantUsers();
    if (global->use_empty()) {
      global->eraseFromParent();
    }
  }
}
} // namespace bennett
