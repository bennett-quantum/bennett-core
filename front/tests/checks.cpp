// Compile a real C++ translation unit against the public marker header.
#include "bennett.h"
#include <llvm/IR/Attributes.h>
#include <iostream>
#include <type_traits>

static_assert(std::is_same_v<decltype(&bennett_h), void (*)(bool *)>);
static_assert(std::is_same_v<decltype(&bennett_bit), bool *(*)(void *, unsigned)>);
static_assert(std::is_same_v<decltype(&__bennett_exit), void (*)(int)>);

enum class Policy { handled, ignored, refused };
Policy classify(llvm::Attribute::AttrKind kind) {
  // No default: -Werror=switch-enum makes every new LLVM kind demand a decision.
  switch (kind) {
  case llvm::Attribute::EmptyKey:
  case llvm::Attribute::TombstoneKey:
  case llvm::Attribute::None:
  case llvm::Attribute::EndAttrKinds:
    break;
#define BENNETT_ATTRIBUTE(kind, policy, reason) case llvm::Attribute::kind: return Policy::policy;
#include "Attributes.def"
#undef BENNETT_ATTRIBUTE
  }
  std::abort();
}

int main() {
  unsigned count = 0;
  for (unsigned k = 1; k < llvm::Attribute::EndAttrKinds; ++k) {
    (void)classify(static_cast<llvm::Attribute::AttrKind>(k));
    ++count;
  }
  std::cout << "front checks: C++ header compiled; " << count << " LLVM attribute kinds classified\n";
}
