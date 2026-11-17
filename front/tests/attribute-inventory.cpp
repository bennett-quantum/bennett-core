// Standalone measurement tool; build with llvm-config-18's core/irreader flags.
// Count actual AttributeSets, including declarations and call sites, before front lowering.
#include <llvm/IR/Attributes.h>
#include <llvm/IR/InstIterator.h>
#include <llvm/IR/Instructions.h>
#include <llvm/IR/Module.h>
#include <llvm/IRReader/IRReader.h>
#include <llvm/Support/SourceMgr.h>
#include <iostream>
#include <map>
#include <tuple>

using Key = std::tuple<std::string, std::string, std::string, std::string>;
std::map<Key, unsigned long long> counts;

void count(llvm::AttributeList list, unsigned params, const char *site) {
  auto record = [&](llvm::AttributeSet set, const char *placement) {
    for (llvm::Attribute attr : set) {
      // "enum" includes LLVM's integer- and type-valued enum kinds.
      std::string kind = attr.isStringAttribute() ? "string" : "enum";
      ++counts[{site, placement, kind, attr.getAsString()}];
    }
  };
  record(list.getFnAttrs(), "function");
  record(list.getRetAttrs(), "return");
  for (unsigned i = 0; i < params; ++i)
    record(list.getParamAttrs(i), "parameter");
}

int main(int argc, char **argv) {
  llvm::LLVMContext context;
  for (int i = 1; i < argc; ++i) {
    llvm::SMDiagnostic diagnostic;
    auto module = llvm::parseIRFile(argv[i], diagnostic, context);
    if (!module) {
      diagnostic.print(argv[0], llvm::errs());
      return 1;
    }
    for (llvm::Function &function : *module) {
      count(function.getAttributes(), function.arg_size(), function.isDeclaration() ? "declaration" : "definition");
      for (llvm::Instruction &inst : llvm::instructions(function))
        if (auto *call = llvm::dyn_cast<llvm::CallBase>(&inst))
          count(call->getAttributes(), call->arg_size(), "call");
    }
  }
  for (auto &[key, value] : counts) {
    auto &[site, placement, kind, spelling] = key;
    std::cout << site << '\t' << placement << '\t' << kind << '\t' << spelling << '\t' << value << '\n';
  }
}
