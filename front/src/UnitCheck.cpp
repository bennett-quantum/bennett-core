#include "UnitCheck.h"

#include "Claims.h"
#include "Diagnostic.h"
#include "Primitives.h"

#include <llvm/ADT/DenseMap.h>
#include <llvm/ADT/STLExtras.h>
#include <llvm/ADT/SmallVector.h>
#include <llvm/ADT/StringRef.h>
#include <llvm/IR/Attributes.h>
#include <llvm/IR/Constants.h>
#include <llvm/IR/DerivedTypes.h>
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
    fail("the unit lacks bennett-cc's array of the primitives' addresses, which the declarations file bq-prefix.h "
         "holds; a unit must not define that file's guard macro BENNETT_BQ_PREFIX_H_INCLUDED, nor replace the file" +
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
Function *entryFunction(const GlobalValue *global, const Primitive &primitive, const Module &module,
                        const std::string &note) {
  std::string name = "@" + std::string(primitive.name);
  std::string linker = linkerName(*global);
  if (linker != linkerNameOf(primitive.name, module)) {
    misuse("the header's declaration of the primitive " + std::string(primitive.name) +
               " stands for the symbol `" + linker +
               "` at the end of the unit (a macro or a rename of the header's name)" + note + declarationRoute,
           global);
  }
  // checkRedirection has refused the other kinds of global under the primitive's linker name.
  return const_cast<Function *>(cast<Function>(global));
}

// The named structure types `type` reaches, each once, in the order met.
void namedStructs(Type *type, SmallVectorImpl<StructType *> &found) {
  if (auto *structure = dyn_cast<StructType>(type); structure && !structure->isLiteral()) {
    if (is_contained(found, structure)) {
      return;
    }
    found.push_back(structure);
  }
  for (Type *contained : type->subtypes()) {
    namedStructs(contained, found);
  }
}

void attributeStructs(const AttributeList &attributes, SmallVectorImpl<StructType *> &found) {
  for (const AttributeSet &set : attributes) {
    for (const Attribute &attribute : set) {
      if (attribute.isTypeAttribute() && attribute.getValueAsType()) {
        namedStructs(attribute.getValueAsType(), found);
      }
    }
  }
}

// The bodies of named structure types, as the assembly writer prints them (`%struct.x = type { i64 }`), after
// `where`; empty if there are none. The unit and the baseline are read into contexts of their own, so a type's name
// is the one clang gave it in each, and the body says what it is.
std::string bodies(const SmallVectorImpl<StructType *> &found) {
  std::string text;
  raw_string_ostream out(text);
  for (StructType *structure : found) {
    out << (text.empty() ? " where " : "; ");
    structure->print(out);
  }
  return out.str();
}

// An attribute as text, with the bodies of the named structure types it names (`sret(%struct.__bennett_reg)`).
std::string attributeText(const Attribute &attribute) {
  SmallVector<StructType *, 2> found;
  if (attribute.isTypeAttribute() && attribute.getValueAsType()) {
    namedStructs(attribute.getValueAsType(), found);
  }
  return attribute.getAsString() + bodies(found);
}

// The declaration as the LLVM 18 assembly writer prints it, with its function attributes written out instead of
// the attribute group's number, without its metadata attachments (checkDeclaration compares them apart), and with the
// bodies of the named structure types its type and attributes name.
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
  SmallVector<StructType *, 4> found;
  namedStructs(function.getFunctionType(), found);
  attributeStructs(attributes, found);
  return line + " { " + attributes.getFnAttrs().getAsString() + " }" + bodies(found);
}

// Metadata as text, through every node it reaches: a node met again is written as its number.
std::string metadataText(const Metadata *metadata, const Module &module, DenseMap<const MDNode *, unsigned> &numbers) {
  if (!metadata) {
    return "null";
  }
  if (const auto *node = dyn_cast<MDNode>(metadata)) {
    auto [entry, first] = numbers.try_emplace(node, numbers.size());
    if (!first) {
      return "!" + std::to_string(entry->second);
    }
    std::string text = std::string(node->isDistinct() ? "distinct " : "") + "!" +
                       std::to_string(node->getMetadataID()) + "{";
    for (const MDOperand &operand : node->operands()) {
      text += metadataText(operand.get(), module, numbers) + ", ";
    }
    return text + "}";
  }
  std::string text;
  raw_string_ostream out(text);
  metadata->print(out, &module);
  return out.str();
}

// The metadata attachments of the declaration other than `!dbg`, which differs from module to module and says
// nothing about behaviour: each kind's name and its node as text, ordered by name (the unit and the baseline have
// contexts of their own, which number custom kinds in the order they meet them).
std::vector<std::pair<std::string, std::string>> attachmentsWithoutDebug(const Function &function) {
  SmallVector<std::pair<unsigned, MDNode *>, 4> attached;
  function.getAllMetadata(attached);
  SmallVector<StringRef, 64> names;
  function.getContext().getMDKindNames(names);
  std::vector<std::pair<std::string, std::string>> kept;
  for (const auto &[kind, node] : attached) {
    if (kind != LLVMContext::MD_dbg) {
      DenseMap<const MDNode *, unsigned> numbers;
      kept.emplace_back(kind < names.size() ? names[kind].str() : std::to_string(kind),
                        metadataText(node, *function.getParent(), numbers));
    }
  }
  llvm::sort(kept);
  return kept;
}

std::string kindNames(const std::vector<std::pair<std::string, std::string>> &attached) {
  std::string text;
  for (const auto &[kind, node] : attached) {
    (void)node;
    text += " !" + kind;
  }
  return text.empty() ? " none" : text;
}

std::string kindName(unsigned kind, const LLVMContext &context) {
  SmallVector<StringRef, 64> names;
  context.getMDKindNames(names);
  return kind < names.size() ? names[kind].str() : std::to_string(kind);
}

// Rule 3.
void checkDeclaration(Function &declaration, Function &header, const Primitive &primitive, const std::string &note) {
  std::string mine = declarationText(declaration);
  std::string theirs = declarationText(header);
  if (mine != theirs) {
    misuse("the declaration of the primitive @" + std::string(primitive.name) + " is `" + mine +
               "` where the header's is `" + theirs + "`" + note + declarationRoute,
           &declaration);
  }
  auto mineAttached = attachmentsWithoutDebug(declaration);
  auto theirsAttached = attachmentsWithoutDebug(header);
  if (mineAttached != theirsAttached) {
    misuse("the declaration of the primitive @" + std::string(primitive.name) + " has the metadata" +
               kindNames(mineAttached) + " where the header's has" + kindNames(theirsAttached) + note +
               declarationRoute,
           &declaration);
  }
}

// Whether `set` has `attribute`: one of its kind with the same text (the sets are of two contexts).
bool hasSame(const AttributeSet &set, const Attribute &attribute) {
  Attribute same = attribute.isStringAttribute() ? set.getAttribute(attribute.getKindAsString())
                                                 : set.getAttribute(attribute.getKindAsEnum());
  return same.isValid() && attributeText(same) == attributeText(attribute);
}

// The first attribute of `one` not in `other`, as text, or empty.
std::string firstMissing(const AttributeSet &one, const AttributeSet &other) {
  for (const Attribute &attribute : one) {
    if (!hasSame(other, attribute)) {
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
void checkDirectCall(const CallBase &call, const Function &declaration, const CallBase &header,
                     const std::string &note) {
  std::string subject = "a call of the device function @" + declaration.getName().str();
  if (call.getFunctionType() != declaration.getFunctionType()) {
    misuse(subject + " through the type " + printed(call.getFunctionType()) + " where its declaration's is " +
               printed(declaration.getFunctionType()) + note + declarationRoute,
           &call);
  }
  if (call.getCallingConv() != declaration.getCallingConv()) {
    misuse(subject + " uses the calling convention " + conventionName(call.getCallingConv()) +
               " where its declaration's is " + conventionName(declaration.getCallingConv()) + note +
                   declarationRoute,
           &call);
  }
  if (call.hasOperandBundles()) {
    misuse(subject + " has an operand bundle, which the header's calls do not have" + note + declarationRoute, &call);
  }
  AttributeList mine = call.getAttributes();
  AttributeList theirs = header.getAttributes();
  auto refuse = [&](const std::string &attribute, const std::string &where) {
    misuse(subject + " has attributes that let LLVM remove or move its calls (`" + attribute + "`" + where +
               " differs from the header's call)" + note + declarationRoute,
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
    bool context = !attribute.isStringAttribute() && contextAttributeOnDirectCall(attribute.getKindAsEnum());
    if (!hasSame(theirs.getFnAttrs(), attribute) && !context) {
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
      misuse(subject + " has the metadata !" + kindName(kind, call.getContext()) +
                 ", which states something about the callee" + note + declarationRoute,
             &call);
    }
  }
}

} // namespace

void checkUnit(Module &unit, Module &baseline, bool withReferences, const std::string &note) {
  checkRedirection(unit, note);
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
    Function *declaration = entryFunction(entries[k], primitive, unit, note);
    auto *header = const_cast<Function *>(cast<Function>(headers[k]));
    checkDeclaration(*declaration, *header, primitive, note);
    const CallBase &headerCallOf = headerCall(*header, primitive);
    for (const Use &use : declaration->uses()) {
      const auto *call = dyn_cast<CallBase>(use.getUser());
      if (call && call->isCallee(&use)) {
        checkDirectCall(*call, *declaration, headerCallOf, note);
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
