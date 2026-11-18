// Safety of ordinary instructions executed after moving a gate loop out of qubit control.
#pragma once

namespace llvm {
class DominatorTree;
class Instruction;
class Loop;
}

namespace bennett {

void requireSafeMovedLoopInstruction(const llvm::Instruction &inst);
void requireFiniteMovedLoop(const llvm::Loop &loop, const llvm::DominatorTree &dominators,
                            const llvm::Instruction &gate);

} // namespace bennett
