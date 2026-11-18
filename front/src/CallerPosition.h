// Return addresses as observable inputs to an applied computation.
#pragma once

namespace llvm {
class CallBase;
class Function;
class Instruction;
}

namespace bennett {

bool canReadReturnAddress(const llvm::Function &function);
void requireCallerIndependent(const llvm::Function &function, const llvm::Instruction &at);
void requireCallerIndependent(const llvm::CallBase &call, const llvm::Instruction &at);

} // namespace bennett
