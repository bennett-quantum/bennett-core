// bennett-cc: compiles one C translation unit of a program in the handle interface to LLVM bitcode, the way
// docs/HANDLES.md, section 4, says the driver does: the header bq.h is included by force, and the unit's parsed form
// is checked against the declaration rule (UnitDeclarations.h) before any code is generated. A unit that breaks the
// rule is refused by name (status 2) and no bitcode is written. The bitcode then goes to bennett-front, which checks
// the rule again on the module (Primitives.h).
//
//   bennett-cc [clang options] -o unit.bc unit.c
//
// The last argument is the unit; `-o` names the bitcode; every other argument goes to clang unchanged, for the
// check and for the compilation alike. clang is $CLANG if set (a bare name is looked up on PATH), else the clang 18
// the build found. The bitcode carries the driver's mark (Primitives.h), which tells bennett-front that the unit came
// this way.

#include "Diagnostic.h"
#include "Primitives.h"
#include "UnitDeclarations.h"

#include <llvm/Bitcode/BitcodeWriter.h>
#include <llvm/IR/LLVMContext.h>
#include <llvm/IR/Metadata.h>
#include <llvm/IR/Module.h>
#include <llvm/IRReader/IRReader.h>
#include <llvm/Support/SourceMgr.h>
#include <llvm/Support/raw_ostream.h>
#include <llvm/ADT/SmallString.h>
#include <llvm/ADT/StringRef.h>
#include <llvm/Support/FileSystem.h>
#include <llvm/Support/FileUtilities.h>
#include <llvm/Support/InitLLVM.h>
#include <llvm/Support/MemoryBuffer.h>
#include <llvm/Support/Path.h>
#include <llvm/Support/Program.h>

#include <cstdlib>
#include <memory>
#include <optional>
#include <string>
#include <vector>

using namespace llvm;

namespace {

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

// Runs clang with `args`, its standard output to `out` if given; stops with clang's status if it fails (clang has
// said why on standard error), after removing `out`: std::exit runs no destructor, so a FileRemover would not.
void runClang(const std::vector<std::string> &args, std::optional<StringRef> out) {
  std::string clang = clangPath();
  std::vector<StringRef> argv = {clang};
  argv.insert(argv.end(), args.begin(), args.end());
  std::optional<StringRef> redirects[] = {std::nullopt, out, std::nullopt};
  std::string error;
  int status = sys::ExecuteAndWait(clang, argv, std::nullopt, redirects, 0, 0, &error);
  if (status < 0) {
    if (out) {
      sys::fs::remove(*out);
    }
    bennett::fail("cannot run " + clang + ": " + error);
  }
  if (status != 0) {
    if (out) {
      sys::fs::remove(*out);
    }
    std::exit(status);
  }
}

// One dump of the unit: clang's syntax tree as JSON, through a temporary file (removed after; the buffer maps it,
// so that the unfiltered dump of a large unit is not copied).
std::unique_ptr<MemoryBuffer> dump(const std::vector<std::string> &front, StringRef unit, std::optional<StringRef> filter) {
  SmallString<256> path;
  if (std::error_code code = sys::fs::createTemporaryFile("bennett-cc", "json", path)) {
    bennett::fail("cannot create a temporary file: " + code.message());
  }
  FileRemover remover(path);
  std::vector<std::string> args = front;
  for (const char *arg : {"-fsyntax-only", "-Xclang", "-ast-dump=json"}) {
    args.emplace_back(arg);
  }
  if (filter) {
    args.emplace_back("-Xclang");
    args.push_back(("-ast-dump-filter=" + *filter).str());
  }
  args.push_back(unit.str());
  runClang(args, StringRef(path));
  ErrorOr<std::unique_ptr<MemoryBuffer>> text = MemoryBuffer::getFile(path);
  if (!text) {
    bennett::fail("cannot read the declaration dump: " + text.getError().message());
  }
  return std::move(*text);
}

// Puts the driver's mark (Primitives.h, driverMark) into the unit's bitcode: the unit passed the check.
void mark(StringRef path) {
  LLVMContext context;
  SMDiagnostic error;
  std::unique_ptr<Module> module = parseIRFile(path, error, context);
  if (!module) {
    bennett::fail("cannot read the bitcode clang wrote, " + path.str() + ": " + error.getMessage().str());
  }
  module->getOrInsertNamedMetadata(bennett::driverMark)
      ->addOperand(MDNode::get(context, MDString::get(context, "passed the declaration rule of bq.h")));
  std::error_code code;
  raw_fd_ostream out(path, code, sys::fs::OF_None);
  if (code) {
    bennett::fail("cannot write " + path.str() + ": " + code.message());
  }
  WriteBitcodeToFile(*module, out);
}

} // namespace

int main(int argc, char **argv) {
  InitLLVM init(argc, argv);
  std::vector<std::string> options;
  std::optional<std::string> output;
  for (int i = 1; i + 1 < argc; ++i) {
    if (StringRef(argv[i]) == "-o") {
      if (output || i + 2 >= argc) {
        bennett::fail("usage: bennett-cc [clang options] -o unit.bc unit.c (one -o, before the unit)");
      }
      output = argv[++i];
      continue;
    }
    options.emplace_back(argv[i]);
  }
  if (argc < 2 || !output || StringRef(argv[argc - 1]).starts_with("-")) {
    bennett::fail("usage: bennett-cc [clang options] -o unit.bc unit.c");
  }
  StringRef unit = argv[argc - 1];
  // The header first, so that it is the first thing the unit declares; its folder on the include path, so that
  // `#include "bq.h"` finds the same file (its guard then makes that include empty).
  std::vector<std::string> front = {"-include", BENNETT_HANDLE_HEADER, "-I",
                                    sys::path::parent_path(BENNETT_HANDLE_HEADER).str()};
  front.insert(front.end(), options.begin(), options.end());
  std::string filtered = dump(front, unit, StringRef("bq_"))->getBuffer().str() +
                         dump(front, unit, StringRef("__bennett_"))->getBuffer().str();
  std::unique_ptr<MemoryBuffer> full = dump(front, unit, std::nullopt);
  bennett::checkUnitDeclarations(filtered, full->getBuffer(), BENNETT_HANDLE_HEADER);
  std::vector<std::string> compile = front;
  for (const char *arg : {"-emit-llvm", "-c"}) {
    compile.emplace_back(arg);
  }
  compile.push_back(unit.str());
  compile.emplace_back("-o");
  compile.push_back(*output);
  runClang(compile, std::nullopt);
  mark(*output);
  return 0;
}
