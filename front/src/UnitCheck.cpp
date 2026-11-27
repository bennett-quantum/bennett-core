#include "UnitCheck.h"

#include "Claims.h"
#include "Diagnostic.h"
#include "Primitives.h"

#include <llvm/ADT/SmallVector.h>
#include <llvm/ADT/StringRef.h>
#include <llvm/IR/Attributes.h>
#include <llvm/IR/Constants.h>
#include <llvm/IR/Function.h>
#include <llvm/IR/GlobalVariable.h>
#include <llvm/IR/InstrTypes.h>
#include <llvm/IR/Instructions.h>
#include <llvm/IR/LLVMContext.h>
#include <llvm/IR/Metadata.h>
#include <llvm/IR/Module.h>
#include <llvm/Support/raw_ostream.h>
#include <llvm/Transforms/Utils/ModuleUtils.h>

#include <string>
#include <utility>
#include <vector>

using namespace llvm;

namespace bennett {

namespace {

// The global each entry of the array names, in the order of allPrimitives().
std::vector<const GlobalValue *> referenced(const Module &module) {
  const GlobalVariable *array = module.getGlobalVariable(referencesName, true);
  const auto *entries = array && array->hasInitializer() ? dyn_cast<ConstantArray>(array->getInitializer()) : nullptr;
  if (!entries || entries->getNumOperands() != allPrimitives().size()) {
    fail("the unit lacks bennett-cc's array of the primitives' addresses (bq-references.h, included by force after "
         "bq.h); a unit must not define the guard macros of either header" +
         std::string(declarationRoute));
  }
  std::vector<const GlobalValue *> globals;
  for (const Use &entry : entries->operands()) {
    const auto *global = dyn_cast<GlobalValue>(entry.get()->stripPointerCasts());
    if (!global) {
      fail("internal: an entry of bennett-cc's array of the primitives' addresses is not a global");
    }
    globals.push_back(global);
  }
  return globals;
}

// Entry k of the array checked by rule 2: the function declaration whose linker name is primitive k's.
Function *entryFunction(const GlobalValue *global, const Primitive &primitive, const Module &module) {
  std::string name = "@" + std::string(primitive.name);
  std::string linker = linkerName(*global);
  if (linker != linkerNameOf(primitive.name, module)) {
    misuse("the header's declaration of the primitive " + std::string(primitive.name) +
               " stands for the symbol `" + linker +
               "` at the end of the unit (a macro or a rename of the header's name)" + declarationRoute,
           global);
  }
  // checkRedirection has refused the other kinds of global under the primitive's linker name.
  return const_cast<Function *>(cast<Function>(global));
}

// The declaration as the LLVM 18 assembly writer prints it, with its function attributes written out instead of
// the attribute group's number, and without its metadata attachments (compared separately, sameMetadata).
std::string declarationText(Function &function) {
  AttributeList attributes = function.getAttributes();
  SmallVector<std::pair<unsigned, MDNode *>, 4> attached;
  function.getAllMetadata(attached);
  function.setAttributes(attributes.removeFnAttributes(function.getContext()));
  function.clearMetadata();
  std::string text;
  raw_string_ostream out(text);
  function.print(out);
  out.flush();
  function.setAttributes(attributes);
  for (const auto &[kind, node] : attached) {
    function.setMetadata(kind, node);
  }
  std::string line;
  SmallVector<StringRef, 4> parts;
  StringRef(text).split(parts, '\n');
  for (StringRef part : parts) {
    if (!part.empty() && !part.starts_with(";")) {
      line += part.str();
    }
  }
  return line + " { " + attributes.getFnAttrs().getAsString() + " }";
}

// The metadata attachments of the declaration other than `!dbg`, which differs from module to module and says
// nothing about behaviour. Uniqued metadata of one LLVMContext with equal contents is one object.
SmallVector<std::pair<unsigned, MDNode *>, 4> attachmentsWithoutDebug(const Function &function) {
  SmallVector<std::pair<unsigned, MDNode *>, 4> attached;
  function.getAllMetadata(attached);
  SmallVector<std::pair<unsigned, MDNode *>, 4> kept;
  for (const auto &entry : attached) {
    if (entry.first != LLVMContext::MD_dbg) {
      kept.push_back(entry);
    }
  }
  return kept;
}

std::string kindNames(const SmallVector<std::pair<unsigned, MDNode *>, 4> &attached, const LLVMContext &context) {
  SmallVector<StringRef, 64> names;
  context.getMDKindNames(names);
  std::string text;
  for (const auto &[kind, node] : attached) {
    (void)node;
    text += " !" + (kind < names.size() ? names[kind].str() : std::to_string(kind));
  }
  return text.empty() ? " none" : text;
}

// Rule 3.
void checkDeclaration(Function &declaration, Function &header, const Primitive &primitive) {
  std::string mine = declarationText(declaration);
  std::string theirs = declarationText(header);
  if (mine != theirs) {
    misuse("the declaration of the primitive @" + std::string(primitive.name) + " is `" + mine +
               "` where the header's is `" + theirs + "`" + declarationRoute,
           &declaration);
  }
  auto mineAttached = attachmentsWithoutDebug(declaration);
  auto theirsAttached = attachmentsWithoutDebug(header);
  if (mineAttached != theirsAttached) {
    misuse("the declaration of the primitive @" + std::string(primitive.name) + " has the metadata" +
               kindNames(mineAttached, declaration.getContext()) + " where the header's has" +
               kindNames(theirsAttached, header.getContext()) + declarationRoute,
           &declaration);
  }
}

// The first attribute of `one` not in `other`, as text, or empty.
std::string firstMissing(const AttributeSet &one, const AttributeSet &other) {
  for (const Attribute &attribute : one) {
    Attribute same = attribute.isStringAttribute() ? other.getAttribute(attribute.getKindAsString())
                                                   : other.getAttribute(attribute.getKindAsEnum());
    if (same != attribute) {
      return attribute.getAsString();
    }
  }
  return "";
}

const CallBase &headerCall(const Function &header, const Primitive &primitive) {
  for (const Use &use : header.uses()) {
    const auto *call = dyn_cast<CallBase>(use.getUser());
    if (call && call->isCallee(&use)) {
      return *call;
    }
  }
  fail("internal: the baseline unit has no call of the primitive @" + std::string(primitive.name));
}

// Rule 4 on one direct call.
void checkDirectCall(const CallBase &call, const Function &declaration, const CallBase &header) {
  std::string subject = "a call of the device function @" + declaration.getName().str();
  if (call.getFunctionType() != declaration.getFunctionType()) {
    misuse(subject + " through the type " + printed(call.getFunctionType()) + " where its declaration's is " +
               printed(declaration.getFunctionType()) + declarationRoute,
           &call);
  }
  if (call.getCallingConv() != declaration.getCallingConv()) {
    misuse(subject + " uses the calling convention " + conventionName(call.getCallingConv()) +
               " where its declaration's is " + conventionName(declaration.getCallingConv()) + declarationRoute,
           &call);
  }
  if (call.hasOperandBundles()) {
    misuse(subject + " has an operand bundle, which the header's calls do not have" + declarationRoute, &call);
  }
  AttributeList mine = call.getAttributes();
  AttributeList theirs = header.getAttributes();
  auto refuse = [&](const std::string &attribute, const std::string &where) {
    misuse(subject + " has attributes that let LLVM remove or move its calls (`" + attribute + "`" + where +
               " differs from the header's call)" + declarationRoute,
           &call);
  };
  if (std::string extra = firstMissing(mine.getRetAttrs(), theirs.getRetAttrs()); !extra.empty()) {
    refuse(extra, " on the result");
  }
  if (std::string lacking = firstMissing(theirs.getRetAttrs(), mine.getRetAttrs()); !lacking.empty()) {
    refuse("no " + lacking, " on the result");
  }
  for (unsigned i = 0; i < call.arg_size(); ++i) {
    std::string where = " on argument " + std::to_string(i + 1);
    if (std::string extra = firstMissing(mine.getParamAttrs(i), theirs.getParamAttrs(i)); !extra.empty()) {
      refuse(extra, where);
    }
    if (std::string lacking = firstMissing(theirs.getParamAttrs(i), mine.getParamAttrs(i)); !lacking.empty()) {
      refuse("no " + lacking, where);
    }
  }
  if (std::string lacking = firstMissing(theirs.getFnAttrs(), mine.getFnAttrs()); !lacking.empty()) {
    refuse("no " + lacking, "");
  }
  std::string extras;
  for (const Attribute &attribute : mine.getFnAttrs()) {
    bool header = (attribute.isStringAttribute() ? theirs.getFnAttrs().getAttribute(attribute.getKindAsString())
                                                 : theirs.getFnAttrs().getAttribute(attribute.getKindAsEnum())) ==
                  attribute;
    if (!header && (attribute.isStringAttribute() || !contextAttributeOnDirectCall(attribute.getKindAsEnum()))) {
      extras += (extras.empty() ? "" : " ") + attribute.getAsString();
    }
  }
  if (!extras.empty()) {
    refuse(extras, "");
  }
  SmallVector<std::pair<unsigned, MDNode *>, 4> attached;
  call.getAllMetadata(attached);
  for (const auto &[kind, node] : attached) {
    (void)node;
    if (!metadataOnDirectCall(kind)) {
      SmallVector<std::pair<unsigned, MDNode *>, 4> one{{kind, node}};
      misuse(subject + " has the metadata" + kindNames(one, call.getContext()) +
                 ", which states something about the callee" + declarationRoute,
             &call);
    }
  }
}

} // namespace

void checkUnit(Module &unit, Module &baseline, bool withReferences) {
  checkRedirection(unit);
  std::vector<const GlobalValue *> headers = referenced(baseline);
  std::vector<const GlobalValue *> entries;
  if (withReferences) {
    entries = referenced(unit);
  } else {
    for (const Primitive &primitive : allPrimitives()) {
      entries.push_back(unit.getFunction(primitive.name));
    }
  }
  for (size_t k = 0; k < allPrimitives().size(); ++k) {
    const Primitive primitive = allPrimitives()[k];
    if (!entries[k]) {
      continue; // LLVM IR that does not declare the primitive
    }
    Function *declaration = entryFunction(entries[k], primitive, unit);
    auto *header = const_cast<Function *>(cast<Function>(headers[k]));
    checkDeclaration(*declaration, *header, primitive);
    const CallBase &headerCallOf = headerCall(*header, primitive);
    for (const Use &use : declaration->uses()) {
      const auto *call = dyn_cast<CallBase>(use.getUser());
      if (call && call->isCallee(&use)) {
        checkDirectCall(*call, *declaration, headerCallOf);
      }
    }
  }
}

void removeReferences(Module &unit) {
  GlobalVariable *array = unit.getGlobalVariable(referencesName, true);
  if (!array) {
    return;
  }
  removeFromUsedLists(unit, [&](Constant *constant) { return constant == array; });
  array->eraseFromParent();
  for (const Primitive &primitive : allPrimitives()) {
    if (Function *function = unit.getFunction(primitive.name); function && function->use_empty()) {
      function->eraseFromParent();
    }
  }
}

} // namespace bennett
