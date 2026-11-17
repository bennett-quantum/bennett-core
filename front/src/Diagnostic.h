// Diagnostics. The front end stops at the first thing it does not handle, naming it and its source
// location; it never guesses and never quietly does something cheaper.
#pragma once

#include <string>

namespace llvm {
class Value;
class Function;
} // namespace llvm

namespace bennett {

// "file:line:col" of an instruction, from its debug location, or "in @function" when there is none.
std::string locationOf(const llvm::Value *at);

// Stops with "not handled yet: <what>, at <location>": ordinary code this version cannot lower yet.
[[noreturn]] void notHandled(const std::string &what, const llvm::Value *at);

// Stops with "error: <what>, at <location>": a misuse of a marker, or input that is not a program.
[[noreturn]] void misuse(const std::string &what, const llvm::Value *at);

// Records "warning: <what>, at <location>" and goes on: the program is lowered, but may not do what the C
// source would do natively (docs/DESIGN.md says what each warning means). Printed by printWarnings when the
// device program is written; a run that stops at a refusal prints only the refusal.
void warning(const std::string &what, const llvm::Value *at);

// Prints the warnings recorded so far, in order.
void printWarnings();

// Stops with "error: <what>" without a location.
[[noreturn]] void fail(const std::string &what);

} // namespace bennett
