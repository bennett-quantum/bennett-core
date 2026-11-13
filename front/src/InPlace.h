// In-place updates of qubit variables: `x = g(x, a)` where g is a bijection in x for every a.
//
// The store is replaced by three XOR stores through a fresh qubit variable n (zero before and after):
//
//     n ^= g(x, a)               n = y, the new value
//     x ^= g⁻¹(n, a) ^ n         g⁻¹(y, a) = x on every branch, so x becomes y
//     n ^= x                     n returns to 0
//
// which the control program already lowers to `apply`s. g is the stored value's chain of operations from
// a load of x to the store, each a bijection in its operand that depends on x and computed from operands
// that do not: add, sub, xor (so not), negate, multiply by an odd constant, rotation, byte swap, a select
// whose condition does not depend on x between two such chains, and the truncations and extensions
// across which the low bits of these ring operations do not change. Everything else is left alone, and the
// control program refuses it by name.
#pragma once

namespace llvm {
class Function;
} // namespace llvm

namespace bennett {

class ControlAnalysis;

// Rewrites the first store of `function` that is an in-place update as above. Returns whether it did.
bool rewriteInPlaceUpdate(llvm::Function &function, ControlAnalysis &analysis);

} // namespace bennett
