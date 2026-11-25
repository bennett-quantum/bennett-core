// The declaration rule of docs/HANDLES.md, section 4, on the parsed form of one translation unit, where the
// module alone cannot show it: after `#pragma redefine_extname bq_free other`, the module has a declaration of
// @other and nothing says that it was the header's. clang computes each declaration's linker name at the end of
// the unit, after every pragma, and writes it into its syntax tree as `mangledName`.
//
// The input is what clang 18 writes for the unit with `-fsyntax-only -Xclang -ast-dump=json` and
// `-Xclang -ast-dump-filter=bq_`, followed by the same with `-Xclang -ast-dump-filter=__bennett_`: every
// declaration whose name contains one of the two, wherever it is in the unit, each with a source location that
// names its file. The check refuses, by name:
//   1. a declaration with linkage of a primitive's identifier other than the header's (a redeclaration, a
//      definition, an `alias`);
//   2. a declaration of another identifier whose linker name is a primitive's (an assembler label);
//   3. a header declaration whose linker name is not its identifier (renamed by a pragma);
// and a unit in which the header does not declare every primitive (it was not included by force). A
// declaration of another identifier that carries a primitive's linker name and whose name contains neither
// prefix is not in the filtered dumps; the module check (Primitives.h) sees it only when its type or
// attributes differ from the header's (docs/DESIGN.md, debts).
#pragma once

#include <string>

namespace llvm {
class StringRef;
} // namespace llvm

namespace bennett {

// Checks the dumps of one unit, `dump`, against the header at `header`. Stops at the first violation with
// "error: ..." and status 2.
void checkUnitDeclarations(llvm::StringRef dump, llvm::StringRef header);

} // namespace bennett
