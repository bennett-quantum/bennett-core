#include "Complex.h"

#include "Diagnostic.h"

#include <llvm/ADT/StringSet.h>
#include <llvm/IR/Function.h>
#include <llvm/IR/Module.h>
#include <llvm/IRReader/IRReader.h>
#include <llvm/Linker/Linker.h>
#include <llvm/Support/SourceMgr.h>
#include <llvm/Support/raw_ostream.h>
#include <llvm/Transforms/IPO/Internalize.h>

#include <memory>
#include <string>

using namespace llvm;

namespace bennett {

namespace {

constexpr const char *helpers[] = {"__mulsc3", "__divsc3", "__muldc3", "__divdc3", "__mulxc3", "__divxc3"};

std::string printed(const Type *type) {
  std::string text;
  raw_string_ostream out(text);
  type->print(out);
  return out.str();
}

} // namespace

bool linkComplexHelpers(Module &module, const std::string &path) {
  std::vector<Function *> needed;
  for (const char *name : helpers) {
    Function *function = module.getFunction(name);
    if (function && function->isDeclaration() && !function->use_empty()) {
      needed.push_back(function);
    }
  }
  if (needed.empty()) {
    return false;
  }
  SMDiagnostic error;
  std::unique_ptr<Module> source = parseIRFile(path, error, module.getContext());
  if (!source) {
    std::string text;
    raw_string_ostream out(text);
    error.print("bennett-front", out);
    fail("cannot read the complex-arithmetic runtime " + path + ":\n" + out.str());
  }
  if (source->getDataLayout() != module.getDataLayout()) {
    fail("the complex-arithmetic runtime " + path + " was compiled for the data layout `" +
         source->getDataLayoutStr() + "`, the program for `" + module.getDataLayoutStr() + "`");
  }
  source->setTargetTriple(module.getTargetTriple());
  for (Function *function : needed) {
    const Function *defined = source->getFunction(function->getName());
    const Value *at = function->user_back();
    if (!defined || defined->isDeclaration()) {
      notHandled("a call to @" + function->getName().str() + ", which the complex-arithmetic runtime " + path +
                     " does not define",
                 at);
    }
    if (defined->getFunctionType() != function->getFunctionType()) {
      notHandled("a call to @" + function->getName().str() + " declared with the type " +
                     printed(function->getFunctionType()) + ", which the complex-arithmetic runtime " + path +
                     " defines with the type " + printed(defined->getFunctionType()),
                 at);
    }
  }
  bool failed = Linker::linkModules(module, std::move(source), Linker::Flags::LinkOnlyNeeded,
                                    [](Module &linked, const StringSet<> &names) {
                                      internalizeModule(linked, [&names](const GlobalValue &value) {
                                        return !value.hasName() || !names.count(value.getName());
                                      });
                                    });
  if (failed) {
    fail("cannot link the complex-arithmetic runtime " + path);
  }
  return true;
}

} // namespace bennett
