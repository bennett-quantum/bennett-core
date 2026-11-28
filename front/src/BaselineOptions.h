// The options bennett-cc gives the baseline of the declaration check (docs/DESIGN.md, "The declaration check",
// rule 1): of the user's command, only those of code-generation kinds that one table names. They are the options
// for which clang puts attributes or metadata on the declarations and calls of the primitives (`-O`, the target,
// `-funsigned-char`, `-fno-builtin`, `-fsanitize=kcfi`, ...), so that the unit and the baseline agree on them. Every
// entry is an option that reads no file and defines no macro of its own choosing: nothing the user controls reaches
// the baseline's text through it.
//
// No string of the user's reaches the baseline's command line in a form clang reads further. An entry is a fixed
// spelling, or a fixed spelling joined to a value of a closed form (letters, digits and `_ . , + -`; for the target,
// a triple whose architecture LLVM knows), and it is forwarded in that joined spelling (`-target T` as
// `--target=T`). The form matters, not the spelling: clang's driver hands a value on to its front end as an argument
// of its own, where an `@file` value is expanded as a response file (measured: `--target=@file`). A value of the
// closed form has no `@`, `/`, `=`, space or quote, so it is no response file and names no file; a value of another
// form is not forwarded. The values clang's driver reads after an option (SeparateOptions.def, from clang's own
// option table) are withheld with it and never read as options here, so `-Xclang -O2` forwards nothing.
//
// An option outside the table is not given to the baseline. If the unit then differs from the baseline in a
// declaration or a call of a primitive, the unit is refused by name, and the refusal names the options that were
// withheld. So the check's soundness does not depend on the table being complete; the table only decides how much
// is accepted, and it grows from measured programs.
#pragma once

#include <string>
#include <vector>

namespace bennett {

struct BaselineOptions {
  std::vector<std::string> forwarded; // given to the baseline, in the user's order
  std::vector<std::string> withheld;  // the user's other arguments, in order
};

// Splits the user's options (the command without `-o OUTPUT` and the unit) by the table.
BaselineOptions baselineOptions(const std::vector<std::string> &options);

// The part of a refusal of rules 2 to 4 that says where the difference may come from.
std::string baselineNote(const BaselineOptions &split);

} // namespace bennett
