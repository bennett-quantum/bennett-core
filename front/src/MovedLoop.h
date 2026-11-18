// Safety of ordinary instructions executed after moving a gate loop out of qubit control.
#pragma once

namespace llvm {
class BasicBlock;
class Instruction;
}

namespace bennett {

void requireSafeMovedLoopInstruction(const llvm::Instruction &inst);
void requireFiniteMovedLoop(llvm::BasicBlock &header, const llvm::Instruction &gate);

} // namespace bennett
