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

// Stops with "error: <what>" without a location.
[[noreturn]] void fail(const std::string &what);

} // namespace bennett
