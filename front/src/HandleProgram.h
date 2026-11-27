// bennett-front's part of the declaration check of the handle interface (docs/HANDLES.md, section 4; docs/DESIGN.md,
// "The declaration check", rule 7), run on every module before anything else.
#pragma once

namespace llvm {
class Module;
} // namespace llvm

namespace bennett {

// In order:
// - removes the claims from every call through a pointer (Claims.h), which is sound for every module;
// - refuses by name a global that redirects a primitive and a call of a primitive's declaration of another type or
//   calling convention (Primitives.h, checkRedirection and checkCallShape);
// - refuses by name a module that uses one of the seven interface functions and lacks the mark of bennett-cc
//   ("compile each unit with bennett-cc"), and a program that uses both the marker interface (docs/MARKERS.md) and
//   the handle interface;
// - refuses a program that uses the handle interface with "not handled yet" (status 3) at its first interface
//   call: the translation of the calls into device instructions waits on the device forms of docs/HANDLES.md,
//   section 14;
// - in a module with the mark, gives each primitive whose address the program takes a callable device function,
//   `<name>.address`, internal, whose body is the direct call; every use other than a direct call names it, so that
//   a call through a pointer reaches the primitive as a direct call does. A module without the mark keeps the old
//   layer's behaviour for the effect primitives (docs/MARKERS.md).
void checkHandleProgram(llvm::Module &module);

} // namespace bennett
