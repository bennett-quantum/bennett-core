// Static-link constructor/destructor arrays and the fallback entry wrapper.
#pragma once
#include <llvm/ADT/StringRef.h>
namespace llvm { class Module; }
namespace bennett {
// Standard array-bound references hand startup ownership to C. Otherwise wrap `entry`.
void lowerStartup(llvm::Module &module, llvm::StringRef entry);
}
