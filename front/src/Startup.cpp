#include "Startup.h"
#include "Diagnostic.h"

#include <llvm/IR/Constants.h>
#include <llvm/IR/GlobalAlias.h>
#include <llvm/IR/IRBuilder.h>
#include <llvm/IR/Module.h>

#include <algorithm>
#include <cstdint>
#include <vector>

using namespace llvm;

namespace bennett {
namespace {
struct Entry {
  uint32_t priority;
  Constant *function;
};
struct Array {
  std::vector<Entry> entries;
  std::vector<GlobalVariable *> sections;
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

Array collect(Module &module, StringRef special, StringRef section) {
  Array array;
  array.special = module.getGlobalVariable(special);
  if (array.special) {
    auto *global = array.special;
    if (!global->hasInitializer() || !global->getValueType()->isArrayTy() || !global->use_empty()) {
      notHandled("a malformed or address-taken @" + special.str(), global);
    }
    Constant *values = global->getInitializer();
    for (unsigned i = 0; i < cast<ArrayType>(global->getValueType())->getNumElements(); ++i) {
      Constant *item = values->getAggregateElement(i);
      if (!item || !item->getType()->isStructTy() || cast<StructType>(item->getType())->getNumElements() != 3) {
        notHandled("a malformed @" + special.str(), global);
      }
      auto *priority = dyn_cast_or_null<ConstantInt>(item->getAggregateElement(0u));
      Constant *function = item->getAggregateElement(1u);
      Constant *associated = item->getAggregateElement(2u);
      if (!priority || priority->getBitWidth() != 32 || !function || !associated || !associated->isNullValue()) {
        notHandled("a constructor/destructor with associated data or malformed priority", global);
      }
      if (function->isNullValue()) {
        continue;
      }
      checkFunction(function, global);
      array.entries.push_back({static_cast<uint32_t>(priority->getZExtValue()), function});
    }
  }
  for (GlobalVariable &global : module.globals()) {
    StringRef name = global.getSection();
    if (!name.starts_with(section)) {
      continue;
    }
    uint32_t priority = 65535;
    StringRef suffix = name.drop_front(section.size());
    if (!suffix.empty() && (!suffix.consume_front(".") || suffix.empty() || suffix.getAsInteger(10, priority) ||
                            priority > 65535)) {
      notHandled("the constructor/destructor section " + name.str() + " (no standard numeric priority)", &global);
    }
    if (!global.hasInitializer() || global.hasComdat() || global.isThreadLocal() || global.isExternallyInitialized() ||
        global.getPointerAlignment(module.getDataLayout()) > Align(8)) {
      notHandled("a constructor/destructor section without a plain static initializer and pointer alignment", &global);
    }
    Constant *init = global.getInitializer();
    auto append = [&](Constant *function) {
      if (!function || !function->getType()->isPointerTy()) {
        notHandled("a constructor/destructor section containing something other than function pointers", &global);
      }
      checkFunction(function, &global);
      array.entries.push_back({priority, function});
    };
    if (auto *type = dyn_cast<ArrayType>(global.getValueType())) {
      for (unsigned i = 0; i < type->getNumElements(); ++i) {
        append(init->getAggregateElement(i));
      }
    } else {
      append(init);
    }
    array.sections.push_back(&global);
  }
  std::stable_sort(array.entries.begin(), array.entries.end(), [](const Entry &a, const Entry &b) {
    return a.priority < b.priority;
  });
  return array;
}

void defineBound(Module &module, StringRef name, Constant *address) {
  GlobalValue *old = module.getNamedValue(name);
  if (!old) {
    return;
  }
  if (!old->isDeclaration() || !isa<GlobalVariable>(old) ||
      old->getAddressSpace() != 0 || old->getPointerAlignment(module.getDataLayout()) > Align(8)) {
    notHandled("the linker array bound @" + name.str() + " is already defined or has unsupported type/alignment", old);
  }
  old->setName("");
  auto *alias = GlobalAlias::create(Type::getInt8Ty(module.getContext()), 0, GlobalValue::ExternalLinkage,
                                  name, address, &module);
  old->replaceAllUsesWith(alias);
  old->eraseFromParent();
}

void materialize(Module &module, Array &array, StringRef kind) {
  auto *ptr = PointerType::getUnqual(module.getContext());
  auto *type = ArrayType::get(ptr, array.entries.size());
  std::vector<Constant *> functions;
  for (const Entry &entry : array.entries) {
    functions.push_back(entry.function);
  }
  array.storage = new GlobalVariable(module, type, true, GlobalValue::InternalLinkage,
                                    ConstantArray::get(type, functions), "__bennett." + kind + "_array");
  array.storage->setAlignment(Align(8));
  // Section entries are linker contributions. Their original addresses cannot in general survive sorting.
  for (GlobalVariable *global : array.sections) {
    global->removeDeadConstantUsers();
    if (!global->use_empty()) {
      notHandled("an address-taken constructor/destructor section object", global);
    }
    global->eraseFromParent();
  }
  auto *i64 = Type::getInt64Ty(module.getContext());
  Constant *startIndices[] = {ConstantInt::get(i64, 0), ConstantInt::get(i64, 0)};
  Constant *endIndices[] = {ConstantInt::get(i64, 0), ConstantInt::get(i64, functions.size())};
  defineBound(module, ("__" + kind + "_array_start").str(),
              ConstantExpr::getGetElementPtr(type, array.storage, startIndices));
  defineBound(module, ("__" + kind + "_array_end").str(),
              ConstantExpr::getGetElementPtr(type, array.storage, endIndices));
  if (array.special) {
    array.special->eraseFromParent();
  }
}
} // namespace

void lowerStartup(Module &module, StringRef entryName) {
  bool owned = false;
  for (StringRef name : {"__init_array_start", "__init_array_end", "__fini_array_start", "__fini_array_end"}) {
    if (GlobalValue *value = module.getNamedValue(name)) {
      owned |= !value->use_empty();
    }
  }
  // These sections need ABI-specific startup or linker scripts; never emit them as inert data.
  for (const GlobalVariable &global : module.globals()) {
    StringRef section = global.getSection();
    if (section.starts_with(".preinit_array") || section.starts_with(".ctors") || section.starts_with(".dtors") ||
        section == ".init" || section == ".fini") {
      notHandled("the startup section " + section.str(), &global);
    }
  }
  Array ctors = collect(module, "llvm.global_ctors", ".init_array");
  Array dtors = collect(module, "llvm.global_dtors", ".fini_array");
  // llvm.used is an LLVM retention list, not a program read of these section objects. Remove only these
  // objects from retention before replacing them by the merged arrays, which remain referenced by startup.
  for (StringRef name : {"llvm.used", "llvm.compiler.used"}) {
    auto *used = module.getGlobalVariable(name);
    if (!used || !used->hasInitializer()) {
      continue;
    }
    std::vector<Constant *> kept;
    auto *init = used->getInitializer();
    for (unsigned i = 0; i < cast<ArrayType>(used->getValueType())->getNumElements(); ++i) {
      Constant *value = init->getAggregateElement(i);
      auto *global = dyn_cast<GlobalVariable>(value->stripPointerCasts());
      bool section = std::find(ctors.sections.begin(), ctors.sections.end(), global) != ctors.sections.end() ||
                     std::find(dtors.sections.begin(), dtors.sections.end(), global) != dtors.sections.end();
      if (!section) {
        kept.push_back(value);
      }
    }
    if (kept.size() == cast<ArrayType>(used->getValueType())->getNumElements()) {
      continue;
    }
    used->eraseFromParent();
    if (!kept.empty()) {
      auto *type = ArrayType::get(PointerType::getUnqual(module.getContext()), kept.size());
      auto *replacement = new GlobalVariable(module, type, false, GlobalValue::AppendingLinkage,
                                           ConstantArray::get(type, kept), name);
      replacement->setSection("llvm.metadata");
    }
  }
  materialize(module, ctors, "init");
  materialize(module, dtors, "fini");
  if (owned || (ctors.entries.empty() && dtors.entries.empty())) {
    return;
  }
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
  for (const Entry &ctor : ctors.entries) {
    builder.CreateCall(signature, ctor.function);
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
  for (auto it = dtors.entries.rbegin(); it != dtors.entries.rend(); ++it) {
    builder.CreateCall(signature, it->function);
  }
  if (entry->getReturnType()->isVoidTy()) {
    builder.CreateRetVoid();
  } else {
    builder.CreateRet(result);
  }
}
} // namespace bennett
