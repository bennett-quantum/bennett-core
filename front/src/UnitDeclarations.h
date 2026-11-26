// The declaration rule of docs/HANDLES.md, section 4, on the parsed form of one translation unit, where the
// module alone cannot show it: after `#pragma redefine_extname bq_free other`, the module has a declaration of
// @other and nothing says that it was the header's. clang computes each declaration's linker name at the end of
// the unit, after every pragma, and writes it into its syntax tree as `mangledName`.
//
// The input is what clang 18 writes for the unit with `-fsyntax-only -Xclang -ast-dump=json`: `filtered`, the
// dumps with `-Xclang -ast-dump-filter=bq_` and then `-Xclang -ast-dump-filter=__bennett_` (every declaration whose
// name contains one of the two, wherever it is in the unit, each with a source location that names its file), and
// `full`, the unfiltered dump (every declaration of the unit, each FunctionDecl and VarDecl with linkage carrying
// its final linker name). The check refuses, by name:
//   1. a declaration with linkage of a primitive's identifier other than the header's (a redeclaration, a
//      definition, an `alias`), from the filtered dumps;
//   2. a declaration of another identifier whose linker name is a primitive's (an assembler label, a pragma), from
//      the full dump: every declaration with linkage is looked up by its linker name, not by its identifier;
//   3. a header declaration whose linker name is not its identifier (renamed by a pragma), or that carries an
//      attribute the header does not give it (`#pragma weak` makes it weak);
// and a unit in which the header does not declare every primitive (it was not included, or its guard macro was
// defined beforehand). bennett-cc (Driver.cpp) runs it on every unit before it writes the unit's bitcode.
#pragma once

#include <string>

namespace llvm {
class StringRef;
} // namespace llvm

namespace bennett {

// Checks the dumps of one unit against the header at `header`. Stops at the first violation with "error: ..." and
// status 2.
void checkUnitDeclarations(llvm::StringRef filtered, llvm::StringRef full, llvm::StringRef header);

} // namespace bennett
