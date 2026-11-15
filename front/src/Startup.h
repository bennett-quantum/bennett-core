// Static-link constructor/destructor arrays and the fallback entry wrapper.
#pragma once
#include <llvm/ADT/StringRef.h>
namespace llvm { class Module; }
namespace bennett {
// Standard array-bound references hand startup ownership to C. Otherwise wrap `entry`. Ownership is decided on the
// references that survive the front's optimiser: call lowerStartup before it and finishStartup after it. In between
// the bounds are undefined, as in a native compile, so a live read of an array cannot be folded away.
void lowerStartup(llvm::Module &module, llvm::StringRef entry);
void finishStartup(llvm::Module &module, llvm::StringRef entry);
}
