// The options bennett-cc gives the baseline of the declaration check (docs/DESIGN.md, "The declaration check",
// rule 1): of the user's command, only those of code-generation kinds that one table names. They are the options
// for which clang puts attributes or metadata on the declarations and calls of the primitives (`-O`, the target,
// `-funsigned-char`, `-fno-builtin`, `-fsanitize=kcfi`, ...), so that the unit and the baseline agree on them. Every
// entry is an option that reads no file and defines no macro: nothing the user controls reaches the baseline's text
// through it.
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
