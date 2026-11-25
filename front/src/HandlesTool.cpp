// bennett-handles: the declaration rule of the handle interface on one translation unit's parsed form
// (docs/HANDLES.md, section 4; UnitDeclarations.h). The driver runs it on every unit; `bennett-front` checks the
// rule on the module (Primitives.h).
//
//   bennett-handles check-unit --header bq.h dump.json   status 0, or 2 with the refusal

#include "Diagnostic.h"
#include "UnitDeclarations.h"

#include <llvm/Support/InitLLVM.h>
#include <llvm/Support/MemoryBuffer.h>

#include <string>

using namespace llvm;

int main(int argc, char **argv) {
  InitLLVM init(argc, argv);
  if (argc != 5 || std::string(argv[1]) != "check-unit" || std::string(argv[2]) != "--header") {
    bennett::fail("usage: bennett-handles check-unit --header <bq.h> <dump.json>");
  }
  ErrorOr<std::unique_ptr<MemoryBuffer>> dump = MemoryBuffer::getFileOrSTDIN(argv[4]);
  if (!dump) {
    bennett::fail(std::string("cannot read ") + argv[4] + ": " + dump.getError().message());
  }
  bennett::checkUnitDeclarations((*dump)->getBuffer(), argv[3]);
  return 0;
}
