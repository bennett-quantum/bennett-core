// bennett-front: reads LLVM bitcode (or text IR) and writes a Bennett device program.
//
//   bennett-front input.bc -o output.bn --region-budget N --stack-bytes N [--taint-all] [--emit-llvm file.ll]

#include "Diagnostic.h"
#include "Emit.h"
#include "Markers.h"
#include "Normalise.h"
#include "Outline.h"

#include <llvm/Analysis/AliasAnalysis.h>
#include <llvm/IR/LLVMContext.h>
#include <llvm/IR/Module.h>
#include <llvm/IR/Verifier.h>
#include <llvm/IRReader/IRReader.h>
#include <llvm/Passes/PassBuilder.h>
#include <llvm/Support/CommandLine.h>
#include <llvm/Support/FileSystem.h>
#include <llvm/Support/InitLLVM.h>
#include <llvm/Support/SourceMgr.h>
#include <llvm/Support/raw_ostream.h>

using namespace llvm;

namespace {

cl::OptionCategory category("bennett-front options");
cl::opt<std::string> input(cl::Positional, cl::desc("<input bitcode or IR>"), cl::Required, cl::cat(category));
cl::opt<std::string> output("o", cl::desc("Output device program (default: standard output)"), cl::init("-"),
                            cl::cat(category));
cl::opt<uint64_t> regionBudget("region-budget", cl::desc("Step budget of each run of an applied function"),
                               cl::Required, cl::cat(category));
cl::opt<uint64_t> stackBytes("stack-bytes", cl::desc("Bytes of memory above the static data, for the stack"),
                             cl::Required, cl::cat(category));
cl::opt<bool> taintAll("taint-all",
                       cl::desc("Treat every value that can run coherently as if it depended on a qubit (a check: "
                                "results must not change)"),
                       cl::cat(category));
cl::opt<std::string> emitLLVM("emit-llvm", cl::desc("Also write the LLVM IR as lowered, for inspection"),
                              cl::cat(category));

// Runs the module pipeline `pipeline` with fresh analyses.
void runPipeline(Module &module, const std::string &pipeline, bool optimise) {
  LoopAnalysisManager loops;
  FunctionAnalysisManager functions;
  CGSCCAnalysisManager sccs;
  ModuleAnalysisManager modules;
  PipelineTuningOptions tuning;
  tuning.LoopVectorization = false; // the device program has no vector values
  tuning.SLPVectorization = false;
  PassBuilder builder(nullptr, tuning);
  functions.registerPass([&] { return builder.buildDefaultAAPipeline(); });
  builder.registerModuleAnalyses(modules);
  builder.registerCGSCCAnalyses(sccs);
  builder.registerFunctionAnalyses(functions);
  builder.registerLoopAnalyses(loops);
  builder.crossRegisterProxies(loops, functions, sccs, modules);
  if (optimise) {
    bennett::optimise(module, builder, modules);
    return;
  }
  ModulePassManager passes;
  if (Error error = builder.parsePassPipeline(passes, pipeline)) {
    bennett::fail(toString(std::move(error)));
  }
  passes.run(module, modules);
}

void verify(const Module &module, const char *stage) {
  std::string problems;
  raw_string_ostream out(problems);
  if (verifyModule(module, &out)) {
    bennett::fail(std::string("internal: the LLVM module is broken ") + stage + ":\n" + out.str());
  }
}

} // namespace

int main(int argc, char **argv) {
  InitLLVM init(argc, argv);
  cl::HideUnrelatedOptions(category);
  cl::ParseCommandLineOptions(argc, argv, "Bennett front end: LLVM bitcode to a device program\n");

  LLVMContext context;
  SMDiagnostic error;
  std::unique_ptr<Module> module = parseIRFile(input, error, context);
  if (!module) {
    error.print(argv[0], errs());
    return 2;
  }
  verify(*module, "on input");
  bennett::checkMarkers(*module);
  runPipeline(*module, "", true);
  bennett::checkMarkers(*module);
  bennett::lowerIntrinsics(*module);

  std::set<const Function *> control = bennett::controlFunctions(*module);
  for (Function &function : *module) {
    if (control.count(&function)) {
      bennett::outlineRegions(function, taintAll, control);
    }
  }
  // Memory effects of the outlined functions, which decide what may run coherently.
  runPipeline(*module, "cgscc(function-attrs)", false);
  verify(*module, "after outlining");

  if (!emitLLVM.empty()) {
    std::error_code code;
    raw_fd_ostream file(emitLLVM, code, sys::fs::OF_Text);
    if (code) {
      bennett::fail("cannot write " + emitLLVM + ": " + code.message());
    }
    module->print(file, nullptr);
  }

  bennett::Options options;
  options.regionBudget = regionBudget;
  options.stackBytes = stackBytes;
  options.taintAll = taintAll;
  bennett::ModuleWriter writer(*module, options, control);
  std::string text = writer.write();

  std::error_code code;
  raw_fd_ostream out(output, code, sys::fs::OF_Text);
  if (code) {
    bennett::fail("cannot write " + output + ": " + code.message());
  }
  out << text;
  return 0;
}
