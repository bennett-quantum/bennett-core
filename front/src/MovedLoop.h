// Safety of ordinary instructions executed after moving a gate loop out of qubit control.
#pragma once

#include <set>

namespace llvm {
class BasicBlock;
class Instruction;
class Value;
}

namespace bennett {

void requireSafeMovedLoopInstruction(const llvm::Instruction &inst);
void requireFiniteMovedLoop(llvm::BasicBlock &header, const llvm::Instruction &gate,
                            const std::set<const llvm::Value *> &available);

} // namespace bennett
