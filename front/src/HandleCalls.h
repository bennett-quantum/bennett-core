// The translation of the handle interface's calls into device instructions (docs/HANDLES.md, sections 5 and 6;
// docs/DESIGN.md, "Translating the handle interface").
//
// Rule R: each call is translated from its callee's name and its run-time operands alone. Nothing here asks where
// an operand came from, what a value depends on, or what any other statement does; every check of a call is the
// device's, made when the instruction runs.
#pragma once

namespace llvm {
class CallBase;
class Function;
class Module;
} // namespace llvm

namespace bennett {

class BodyWriter;

// Marks the module's apply targets (docs/HANDLES.md, 6.2 check 1; section 14, D11): every defined function whose
// type is one pointer parameter and no result, primitives excluded. Run on the module as read, before any lowering,
// so that the set is the compiled program's: `void a(void *)` and `void b(int *)` are both targets, `void
// c(uint64_t)` is not although its device signature is the same. The mark is the function metadata
// `bennett.apply_target`; the writer prints a marked function as `applied func`.
void markApplyTargets(llvm::Module &module);

// Whether the writer prints `function` as an apply target. Stops with an internal error for a marked function
// whose address is taken and whose type the lowering changed (LLVM changes the type of a function only where it
// sees every call).
bool isApplyTarget(const llvm::Function &function);

// If `call` is a direct call of one of the seven interface functions or of the named stop `__bennett_bad_argument`,
// writes its one device instruction into `body` and returns true; otherwise returns false and writes nothing:
//
//   bq_alloc(bits)                          %h = qalloc i64 bits
//   bq_free(r)                              qfree r
//   bq_width(r)                             %w = qwidth r
//   bq_measure(r, low, bits)                %v = measure i64 r[low, bits]
//   bq_gate(kind, r, bit, c, n)             gate(kind) r[bit] ctrl list(c, n)
//   bq_rotate(kind, theta, r, bit, c, n)    rotate(kind, theta) r[bit] ctrl list(c, n)
//   bq_apply(f, B, e, p, s, q, m, c, n)     apply f budget B, fault e, image(p, s), fields(q, m), ctrl list(c, n)
//   __bennett_bad_argument()                fault bad_argument
//
// Each operand is the call's operand as the writer names any value: a slot, or a constant (a handle, a code, an
// angle's binary64 bits, an address). The other three effect primitives keep the writer's own lowering.
bool translateInterfaceCall(BodyWriter &body, const llvm::CallBase &call);

// Whether `call` ends its block on the device: a call of `__bennett_bad_argument`, whose `fault` is a terminator.
bool endsBlock(const llvm::CallBase &call);

} // namespace bennett
