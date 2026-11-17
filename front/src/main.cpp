// bennett-front: reads LLVM bitcode (or text IR) and writes a Bennett device program.
//
//   bennett-front input.bc -o output.bn --region-budget N --stack-bytes N [--taint-all] [--emit-llvm file.ll]
//                 [--float-runtime file.bc] [--complex-runtime file.bc]

#include "Abi.h"
#include "Complex.h"
#include "Diagnostic.h"
#include "Emit.h"
#include "Jumps.h"
#include "Markers.h"
#include "Normalise.h"
#include "Outline.h"
#include "SoftFloat.h"
#include "Startup.h"

#include <llvm/Analysis/AliasAnalysis.h>
#include <llvm/IR/LLVMContext.h>
#include <llvm/IR/Module.h>
#include <llvm/IR/Verifier.h>
#include <llvm/IRReader/IRReader.h>
#include <llvm/Passes/PassBuilder.h>
#include <llvm/Support/CommandLine.h>
#include <llvm/Support/CrashRecoveryContext.h>
#include <llvm/Support/ErrorHandling.h>
#include <llvm/Support/FileSystem.h>
#include <llvm/Support/InitLLVM.h>
#include <llvm/Support/MemoryBuffer.h>
#include <llvm/Support/Path.h>
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
cl::opt<std::string> entryName("entry", cl::desc("Entry wrapped by automatic constructors/destructors"),
                               cl::init("main"), cl::cat(category));
cl::opt<bool> taintAll("taint-all",
                       cl::desc("Treat every value that can run coherently as if it depended on a qubit (a check: "
                                "results must not change)"),
                       cl::cat(category));
cl::opt<std::string> emitLLVM("emit-llvm", cl::desc("Also write the LLVM IR as lowered, for inspection"),
                              cl::cat(category));
cl::opt<std::string> floatRuntime("float-runtime",
                                  cl::desc("Bitcode defining the floating-point runtime-call interface (default: "
                                           "float-runtime.bc next to bennett-front, Berkeley SoftFloat 3e)"),
                                  cl::cat(category));

cl::opt<std::string> complexRuntime("complex-runtime",
                                    cl::desc("Bitcode defining the complex multiply and divide helpers (default: "
                                             "complex-runtime.bc next to bennett-front, libgcc's)"),
                                    cl::cat(category));

// `name` in the directory of this executable.
std::string besideFront(const char *argv0, const char *name) {
  static int anchor;
  SmallString<256> path(sys::fs::getMainExecutable(argv0, &anchor));
  sys::path::remove_filename(path);
  sys::path::append(path, name);
  return std::string(path);
}

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

// What a module that is not valid LLVM IR is called, wherever it is found.
constexpr const char *invalidInput = "the input is not valid LLVM IR";

// Reads the input. Three ways LLVM's reader can find it is not LLVM IR, each named, with status 2 and no
// device program written:
// - It cannot parse it (text that is not LLVM's assembly language, bitcode that is cut short): LLVM's
//   diagnostic, then the name.
// - It stops with a fatal error. It verifies a module that carries debug info while upgrading it
//   (`UpgradeDebugInfo`) and, when the module is broken, calls `report_fatal_error` (clang 18 writes such a
//   module for an argument aligned past LLVM's limit of 2^14 bytes, a `byval` with `align 1048576`); the
//   verifier has printed its findings above the name.
// - It crashes: damaged metadata (a subprogram whose type refers to metadata that is not there) makes its
//   metadata loader stop with SIGSEGV. A signal while reading is caught (CrashRecoveryContext) and named as
//   well; whether the input or LLVM is at fault cannot be told apart.
// An empty file is LLVM's empty module, which is valid: it becomes an empty device program. A file that
// cannot be read at all is not called invalid IR.
std::unique_ptr<Module> readInput(LLVMContext &context, const char *argv0) {
  ErrorOr<std::unique_ptr<MemoryBuffer>> bytes = MemoryBuffer::getFileOrSTDIN(input);
  if (!bytes) {
    errs() << "error: cannot read " << input << ": " << bytes.getError().message() << "\n";
    std::exit(2);
  }
  ScopedFatalErrorHandler handler([](void *, const char *reason, bool) {
    errs() << "error: " << invalidInput << " (LLVM's reader stopped: " << reason
           << "); no device program was written\n";
    std::exit(2);
  });
  SMDiagnostic error;
  std::unique_ptr<Module> module;
  CrashRecoveryContext::Enable();
  CrashRecoveryContext recovery;
  bool finished = recovery.RunSafely([&] { module = parseIR((*bytes)->getMemBufferRef(), error, context); });
  CrashRecoveryContext::Disable();
  if (!finished) {
    errs() << "error: " << invalidInput
           << " (LLVM's reader crashed on it, which damaged metadata does; if the input is valid, this is a bug in "
              "LLVM); no device program was written\n";
    std::exit(2);
  }
  if (!module) {
    error.print(argv0, errs());
    errs() << "error: " << invalidInput << " (LLVM's reader could not parse it); no device program was written\n";
    std::exit(2);
  }
  std::string problems;
  raw_string_ostream out(problems);
  if (verifyModule(*module, &out)) {
    bennett::fail(std::string(invalidInput) + ":\n" + out.str());
  }
  return module;
}

} // namespace

int main(int argc, char **argv) {
  InitLLVM init(argc, argv);
  cl::HideUnrelatedOptions(category);
  cl::ParseCommandLineOptions(argc, argv, "Bennett front end: LLVM bitcode to a device program\n");

  LLVMContext context;
  std::unique_ptr<Module> module = readInput(context, argv[0]);
  bennett::checkMarkers(*module);
  bennett::lowerStartup(*module, entryName);
  verify(*module, "after constructor/destructor startup");
  bennett::linkComplexHelpers(*module,
                              complexRuntime.empty() ? besideFront(argv[0], "complex-runtime.bc") : complexRuntime);
  verify(*module, "after linking the complex multiply and divide helpers");
  bool softFloat = bennett::lowerFloatOperations(
      *module, floatRuntime.empty() ? besideFront(argv[0], "float-runtime.bc") : floatRuntime);
  verify(*module, "after lowering floating-point operations");
  bennett::lowerMismatchedCalls(*module);
  verify(*module, "after lowering calls through another prototype");
  bennett::lowerVariadics(*module);
  bennett::lowerValuesInMemory(*module);
  bennett::lowerIndirectBranches(*module);
  verify(*module, "after lowering calls and computed goto");
  bennett::floatsToIntegers(*module);
  verify(*module, "after rewriting floating-point values as integers");
  bennett::checkNoFloatingPoint(*module, "after rewriting floating-point values as integers");
  bennett::lowerParameterAttributes(*module);
  // LLVM's optimiser has bugs of its own. A crash inside it is reported by name; no device program is written.
  CrashRecoveryContext::Enable();
  CrashRecoveryContext recovery;
  if (!recovery.RunSafely([&] { runPipeline(*module, "", true); })) {
    bennett::fail("LLVM's optimiser crashed on this module (a bug in LLVM); no device program was written");
  }
  CrashRecoveryContext::Disable();
  bennett::checkNoFloatingPoint(*module, "after the optimiser");
  // The optimiser deletes the runtime's exception flags, which nothing reads, after it has inferred that
  // the runtime writes them: the memory effects are inferred again, or no runtime call could run
  // coherently. Then runtime calls on constants are evaluated (a rotation's angle must be a constant).
  if (softFloat) {
    runPipeline(*module, "cgscc(function-attrs)", false);
    if (bennett::foldConstantCalls(*module)) {
      runPipeline(*module, "globaldce,cgscc(function-attrs)", false);
    }
  }
  bennett::finishStartup(*module, entryName);
  verify(*module, "after deciding constructor/destructor startup");
  bennett::checkMarkers(*module, /*floatsAreBits=*/true);
  bennett::lowerParameterAttributes(*module);
  bennett::scalarise(*module);
  bennett::lowerEmptyAssembly(*module);
  bennett::lowerJumps(*module);
  verify(*module, "after lowering setjmp and longjmp");
  bennett::lowerIntrinsics(*module, /*keepBijections=*/true);

  std::set<const Function *> control = bennett::controlFunctions(*module);
  std::vector<Function *> controlled;
  for (Function &function : *module) {
    if (control.count(&function)) {
      controlled.push_back(&function);
    }
  }
  for (Function *function : controlled) {
    bennett::outlineRegions(*function, taintAll, control);
  }
  bennett::lowerIntrinsics(*module);
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
  bennett::printWarnings();
  return 0;
}
