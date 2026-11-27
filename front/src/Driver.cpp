// bennett-cc: compiles one unit of a program in the handle interface to LLVM bitcode with the declaration check of
// docs/HANDLES.md, section 4 (docs/DESIGN.md, "The declaration check"):
//
//   bennett-cc [clang options] -o unit.bc unit.c        (or unit.ll, unit.bc)
//
// 1. A C unit is compiled with the user's options at the requested level with every LLVM pass off, with two
//    includes ahead of anything the user passes: bq.h, then bq-references.h (UnitCheck.h). The baseline
//    bq-baseline.c is compiled the same way, at the same time. A unit given as LLVM IR is read as it is.
// 2. The unit is checked against the baseline (UnitCheck.h, rules 2 to 4); calls through a pointer lose their
//    claims (Claims.h, rule 5); the reference array is deleted.
// 3. clang optimises that module with the same options (`-x ir`), so the passes are those clang chooses for them.
//    The result is checked for redirection and call shape (Primitives.h, rule 6) and written with the driver's mark,
//    as bitcode, or as LLVM text when the output's name ends in `.ll`.
// A refused unit writes no output and stops with status 2; a unit clang refuses stops with clang's status. clang is
// $CLANG if set (a bare name is looked up on PATH), else the clang 18 the build found. Temporary files are removed
// on every exit.

#include "Claims.h"
#include "Diagnostic.h"
#include "Primitives.h"
#include "UnitCheck.h"

#include <llvm/ADT/SmallString.h>
#include <llvm/ADT/StringRef.h>
#include <llvm/Bitcode/BitcodeWriter.h>
#include <llvm/IR/Function.h>
#include <llvm/IR/LLVMContext.h>
#include <llvm/IR/Metadata.h>
#include <llvm/IR/Module.h>
#include <llvm/IR/Verifier.h>
#include <llvm/IRReader/IRReader.h>
#include <llvm/Support/FileSystem.h>
#include <llvm/Support/InitLLVM.h>
#include <llvm/Support/MemoryBuffer.h>
#include <llvm/Support/Path.h>
#include <llvm/Support/Program.h>
#include <llvm/Support/SourceMgr.h>
#include <llvm/Support/raw_ostream.h>

#include <cstdlib>
#include <memory>
#include <optional>
#include <string>
#include <vector>

using namespace llvm;

namespace {

// Temporary files, removed by an exit handler: the refusals stop with std::exit, which runs no destructor.
std::vector<std::string> &temporaries() {
  static std::vector<std::string> paths;
  return paths;
}

// The baseline's clang, while it runs: waited for before the temporaries go, so that it writes none after.
std::optional<sys::ProcessInfo> &running() {
  static std::optional<sys::ProcessInfo> process;
  return process;
}

void removeTemporaries() {
  if (running()) {
    sys::Wait(*running(), std::nullopt);
    running().reset();
  }
  for (const std::string &path : temporaries()) {
    sys::fs::remove(path);
  }
}

std::string temporary(StringRef suffix) {
  SmallString<256> path;
  if (std::error_code code = sys::fs::createTemporaryFile("bennett-cc", suffix, path)) {
    bennett::fail("cannot create a temporary file: " + code.message());
  }
  temporaries().push_back(path.str().str());
  return path.str().str();
}

// clang is $CLANG if set, else the clang 18 the build found; a name without a folder is looked up on PATH, as a
// shell would.
std::string clangPath() {
  const char *chosen = std::getenv("CLANG");
  std::string clang = chosen && *chosen ? chosen : BENNETT_CLANG;
  if (sys::path::has_parent_path(clang)) {
    return clang;
  }
  ErrorOr<std::string> found = sys::findProgramByName(clang);
  if (!found) {
    bennett::fail("cannot find " + clang + " on PATH: " + found.getError().message());
  }
  return *found;
}

std::vector<StringRef> commandLine(const std::string &clang, const std::vector<std::string> &args) {
  std::vector<StringRef> all = {clang};
  all.insert(all.end(), args.begin(), args.end());
  return all;
}

// Runs clang with `args` and stops with clang's status if it fails (clang has said why on standard error).
void runClang(const std::string &clang, const std::vector<std::string> &args) {
  std::string error;
  int status = sys::ExecuteAndWait(clang, commandLine(clang, args), std::nullopt, {}, 0, 0, &error);
  if (status < 0) {
    bennett::fail("cannot run " + clang + ": " + error);
  }
  if (status != 0) {
    std::exit(status);
  }
}

std::unique_ptr<Module> read(StringRef path, LLVMContext &context, StringRef what) {
  SMDiagnostic error;
  std::unique_ptr<Module> module = parseIRFile(path, error, context);
  if (!module) {
    std::string text;
    raw_string_ostream out(text);
    error.print("bennett-cc", out);
    bennett::fail("cannot read " + what.str() + ":\n" + out.str());
  }
  return module;
}

void write(const Module &module, StringRef path, bool text) {
  std::error_code code;
  raw_fd_ostream out(path, code, sys::fs::OF_None);
  if (code) {
    bennett::fail("cannot write " + path.str() + ": " + code.message());
  }
  if (text) {
    module.print(out, nullptr);
  } else {
    WriteBitcodeToFile(module, out);
  }
}

// What rule 6 compares after optimisation: each primitive declaration's type, linkage and calling convention.
struct Shape {
  std::string name;
  std::string type; // printed: the optimised module is read into a context of its own
  GlobalValue::LinkageTypes linkage;
  CallingConv::ID convention;
};

std::vector<Shape> shapes(const Module &module) {
  std::vector<Shape> found;
  for (const bennett::Primitive &primitive : bennett::allPrimitives()) {
    if (const Function *function = module.getFunction(primitive.name)) {
      found.push_back({primitive.name, bennett::printed(function->getFunctionType()), function->getLinkage(),
                       function->getCallingConv()});
    }
  }
  return found;
}

void checkShapesKept(const Module &module, const std::vector<Shape> &before) {
  for (const Shape &shape : before) {
    const Function *function = module.getFunction(shape.name);
    if (function && (bennett::printed(function->getFunctionType()) != shape.type || function->getLinkage() != shape.linkage ||
                     function->getCallingConv() != shape.convention)) {
      bennett::misuse("the optimiser changed the declaration of the primitive @" + shape.name +
                          "; its type, linkage and calling convention must stay the header's",
                      function);
    }
  }
}

[[noreturn]] void usage() {
  bennett::fail("usage: bennett-cc [clang options] -o OUTPUT UNIT (a C unit, .c, or LLVM IR, .ll or .bc; the output "
                "is LLVM text if its name ends in .ll, else bitcode)");
}

} // namespace

int main(int argc, char **argv) {
  InitLLVM init(argc, argv);
  // The two lists exist before the handler is registered, so that they outlive it (statics made later are
  // destroyed before it runs).
  temporaries();
  running();
  std::atexit(removeTemporaries);
  std::vector<std::string> options;
  std::optional<std::string> output;
  for (int i = 1; i + 1 < argc; ++i) {
    if (StringRef(argv[i]) == "-o") {
      if (output || i + 2 >= argc) {
        usage();
      }
      output = argv[++i];
      continue;
    }
    options.emplace_back(argv[i]);
  }
  if (argc < 2 || !output || StringRef(argv[argc - 1]).starts_with("-")) {
    usage();
  }
  StringRef unit = argv[argc - 1];
  StringRef extension = sys::path::extension(unit);
  bool fromC = extension == ".c";
  if (!fromC && extension != ".ll" && extension != ".bc") {
    usage();
  }
  // A refused unit leaves no output, not even an earlier one.
  sys::fs::remove(*output);
  std::string clang = clangPath();
  // The header first, then the array of references; the header's folder on the include path, so that
  // `#include "bq.h"` finds the same file (its guard then makes that include empty).
  StringRef folder = sys::path::parent_path(BENNETT_HANDLE_HEADER);
  std::vector<std::string> passFree = {"-include", BENNETT_HANDLE_HEADER, "-include",
                                       (folder + "/bq-references.h").str(), "-I", folder.str()};
  passFree.insert(passFree.end(), options.begin(), options.end());
  for (const char *arg : {"-emit-llvm", "-c", "-Xclang", "-disable-llvm-passes"}) {
    passFree.emplace_back(arg);
  }

  // The baseline, in parallel with the unit; its messages are shown only if it alone fails.
  std::string baselinePath = temporary("bc");
  std::string baselineErrors = temporary("txt");
  std::vector<std::string> baselineArgs = passFree;
  for (std::string arg : {(folder + "/bq-baseline.c").str(), std::string("-o"), baselinePath}) {
    baselineArgs.push_back(arg);
  }
  std::optional<StringRef> redirects[] = {std::nullopt, std::nullopt, StringRef(baselineErrors)};
  std::string error;
  bool failedToStart = false;
  sys::ProcessInfo baseline =
      sys::ExecuteNoWait(clang, commandLine(clang, baselineArgs), std::nullopt, redirects, 0, &error, &failedToStart);
  if (failedToStart) {
    bennett::fail("cannot run " + clang + ": " + error);
  }
  running() = baseline;

  LLVMContext context;
  std::unique_ptr<Module> module;
  if (fromC) {
    std::string unitPath = temporary("bc");
    std::vector<std::string> unitArgs = passFree;
    for (std::string arg : {unit.str(), std::string("-o"), unitPath}) {
      unitArgs.push_back(arg);
    }
    runClang(clang, unitArgs);
    module = read(unitPath, context, "the bitcode clang wrote for " + unit.str());
  } else {
    module = read(unit, context, unit);
    std::string problems;
    raw_string_ostream out(problems);
    if (verifyModule(*module, &out)) {
      bennett::fail("the unit " + unit.str() + " is not valid LLVM IR:\n" + out.str());
    }
  }
  sys::ProcessInfo ended = sys::Wait(baseline, std::nullopt, &error);
  running().reset();
  if (ended.ReturnCode != 0) {
    if (ErrorOr<std::unique_ptr<MemoryBuffer>> text = MemoryBuffer::getFile(baselineErrors)) {
      errs() << (*text)->getBuffer();
    }
    bennett::fail("the baseline of the declaration check (bq-baseline.c) does not compile with these options" +
                  (error.empty() ? std::string() : ": " + error));
  }
  std::unique_ptr<Module> header = read(baselinePath, context, "the baseline's bitcode");

  bennett::checkUnit(*module, *header, fromC);
  bennett::removePointerCallClaims(*module);
  bennett::removeReferences(*module);
  std::vector<Shape> before = shapes(*module);

  std::string checked = temporary("bc");
  write(*module, checked, false);
  std::string optimised = temporary("bc");
  std::vector<std::string> optimise = options;
  for (std::string arg : {std::string("-Wno-unused-command-line-argument"), std::string("-emit-llvm"),
                          std::string("-c"), std::string("-x"), std::string("ir"), checked, std::string("-o"),
                          optimised}) {
    optimise.push_back(arg);
  }
  runClang(clang, optimise);
  // A context of its own, so that its type names are not renamed against the baseline's.
  LLVMContext resultContext;
  std::unique_ptr<Module> result = read(optimised, resultContext, "the bitcode clang optimised");
  bennett::checkRedirection(*result);
  bennett::checkCallShape(*result);
  checkShapesKept(*result, before);
  result->getOrInsertNamedMetadata(bennett::driverMark)
      ->addOperand(MDNode::get(resultContext, MDString::get(resultContext, "passed the declaration check of bennett-cc")));
  write(*result, *output, StringRef(*output).ends_with(".ll"));
  return 0;
}
