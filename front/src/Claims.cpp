#include "Claims.h"

#include <llvm/ADT/SmallVector.h>
#include <llvm/IR/Attributes.h>
#include <llvm/IR/Function.h>
#include <llvm/IR/InstrTypes.h>
#include <llvm/IR/Instructions.h>
#include <llvm/IR/LLVMContext.h>
#include <llvm/IR/Metadata.h>
#include <llvm/IR/Module.h>

#include <utility>

using namespace llvm;

namespace bennett {

namespace {

// What happens to an attribute of a call through a pointer (Claims.h).
enum class OnPointerCall {
  Abi,           // how an argument or the result is passed
  Limit,         // a limit on the optimiser, or `noreturn`/`nounwind`
  KeepBesideAbi, // `align`: kept on a parameter that also carries an ABI attribute (the alignment of a `byval`)
  Remove,        // a permission
};

struct Meaning {
  OnPointerCall pointer;
  bool contextOnDirectCall; // Claims.h, contextAttributeOnDirectCall
};

constexpr Meaning abi{OnPointerCall::Abi, false};
constexpr Meaning limit{OnPointerCall::Limit, false};
constexpr Meaning permission{OnPointerCall::Remove, false};

// The one classification of LLVM 18's attribute kinds. ABI: how an argument or result is passed. A limit: forbids an
// optimisation (`strictfp`, `nomerge`, `noinline`, `nobuiltin`, `convergent`, `noduplicate`, `returns_twice`) or
// is a placement hint that licenses nothing (`cold`, `hot`). `noreturn` stays because it never removes a call (a
// `noreturn` pointer that reaches a primitive which returns is a call through a mismatched prototype and faults
// `unreachable` where it runs); `nounwind` is true of every C function. Everything else lets the optimiser assume
// something about the callee, an argument or the result, and is a permission.
Meaning meaningOf(Attribute::AttrKind kind) {
  switch (kind) {
  case Attribute::ByRef:
  case Attribute::ByVal:
  case Attribute::ElementType:
  case Attribute::InAlloca:
  case Attribute::InReg:
  case Attribute::Nest:
  case Attribute::Preallocated:
  case Attribute::SExt:
  case Attribute::StackAlignment:
  case Attribute::StructRet:
  case Attribute::SwiftAsync:
  case Attribute::SwiftError:
  case Attribute::SwiftSelf:
  case Attribute::ZExt:
    return abi;
  case Attribute::Alignment:
    return {OnPointerCall::KeepBesideAbi, false};
  case Attribute::StrictFP:
  case Attribute::NoMerge:
  case Attribute::NoInline:
    return {OnPointerCall::Limit, true};
  case Attribute::NoBuiltin:
  case Attribute::Convergent:
  case Attribute::NoDuplicate:
  case Attribute::ReturnsTwice:
  case Attribute::Cold:
  case Attribute::Hot:
  case Attribute::NoReturn:
  case Attribute::NoUnwind:
    return limit;
  case Attribute::AlwaysInline:
    return {OnPointerCall::Remove, true};
  case Attribute::AllocAlign:
  case Attribute::AllocKind:
  case Attribute::AllocSize:
  case Attribute::AllocatedPointer:
  case Attribute::Builtin:
  case Attribute::CoroDestroyOnlyWhenComplete:
  case Attribute::DeadOnUnwind:
  case Attribute::Dereferenceable:
  case Attribute::DereferenceableOrNull:
  case Attribute::DisableSanitizerInstrumentation:
  case Attribute::FnRetThunkExtern:
  case Attribute::ImmArg:
  case Attribute::InlineHint:
  case Attribute::JumpTable:
  case Attribute::Memory:
  case Attribute::MinSize:
  case Attribute::MustProgress:
  case Attribute::Naked:
  case Attribute::NoAlias:
  case Attribute::NoCallback:
  case Attribute::NoCapture:
  case Attribute::NoCfCheck:
  case Attribute::NoFPClass:
  case Attribute::NoFree:
  case Attribute::NoImplicitFloat:
  case Attribute::NoProfile:
  case Attribute::NoRecurse:
  case Attribute::NoRedZone:
  case Attribute::NoSanitizeBounds:
  case Attribute::NoSanitizeCoverage:
  case Attribute::NoSync:
  case Attribute::NoUndef:
  case Attribute::NonLazyBind:
  case Attribute::NonNull:
  case Attribute::NullPointerIsValid:
  case Attribute::OptForFuzzing:
  case Attribute::OptimizeForDebugging:
  case Attribute::OptimizeForSize:
  case Attribute::OptimizeNone:
  case Attribute::PresplitCoroutine:
  case Attribute::ReadNone:
  case Attribute::ReadOnly:
  case Attribute::Returned:
  case Attribute::SafeStack:
  case Attribute::SanitizeAddress:
  case Attribute::SanitizeHWAddress:
  case Attribute::SanitizeMemTag:
  case Attribute::SanitizeMemory:
  case Attribute::SanitizeThread:
  case Attribute::ShadowCallStack:
  case Attribute::SkipProfile:
  case Attribute::Speculatable:
  case Attribute::SpeculativeLoadHardening:
  case Attribute::StackProtect:
  case Attribute::StackProtectReq:
  case Attribute::StackProtectStrong:
  case Attribute::UWTable:
  case Attribute::VScaleRange:
  case Attribute::WillReturn:
  case Attribute::Writable:
  case Attribute::WriteOnly:
    return permission;
  case Attribute::None: // the markers of the enumeration, never an attribute of a call
  case Attribute::EndAttrKinds:
  case Attribute::EmptyKey:
  case Attribute::TombstoneKey:
    return permission;
  }
  return permission;
}

enum class MetadataMeaning {
  Debug,       // `!dbg`
  ProgramLoop, // `!llvm.access.group`: the program's own claim about its loop (a loop pragma)
  Permission,  // everything else: a fact for the optimiser
};

// The one classification of LLVM 18's fixed metadata kinds. A kind registered by name (beyond the fixed ones) is a
// permission.
MetadataMeaning metadataMeaning(unsigned kind) {
  switch (static_cast<decltype(LLVMContext::MD_dbg)>(kind)) {
  case LLVMContext::MD_dbg:
    return MetadataMeaning::Debug;
  case LLVMContext::MD_access_group:
    return MetadataMeaning::ProgramLoop;
  case LLVMContext::MD_tbaa:
  case LLVMContext::MD_prof:
  case LLVMContext::MD_fpmath:
  case LLVMContext::MD_range:
  case LLVMContext::MD_tbaa_struct:
  case LLVMContext::MD_invariant_load:
  case LLVMContext::MD_alias_scope:
  case LLVMContext::MD_noalias:
  case LLVMContext::MD_nontemporal:
  case LLVMContext::MD_mem_parallel_loop_access:
  case LLVMContext::MD_nonnull:
  case LLVMContext::MD_dereferenceable:
  case LLVMContext::MD_dereferenceable_or_null:
  case LLVMContext::MD_make_implicit:
  case LLVMContext::MD_unpredictable:
  case LLVMContext::MD_invariant_group:
  case LLVMContext::MD_align:
  case LLVMContext::MD_loop:
  case LLVMContext::MD_type:
  case LLVMContext::MD_section_prefix:
  case LLVMContext::MD_absolute_symbol:
  case LLVMContext::MD_associated:
  case LLVMContext::MD_callees:
  case LLVMContext::MD_irr_loop:
  case LLVMContext::MD_callback:
  case LLVMContext::MD_preserve_access_index:
  case LLVMContext::MD_vcall_visibility:
  case LLVMContext::MD_noundef:
  case LLVMContext::MD_annotation:
  case LLVMContext::MD_nosanitize:
  case LLVMContext::MD_func_sanitize:
  case LLVMContext::MD_exclude:
  case LLVMContext::MD_memprof:
  case LLVMContext::MD_callsite:
  case LLVMContext::MD_kcfi_type:
  case LLVMContext::MD_pcsections:
  case LLVMContext::MD_DIAssignID:
  case LLVMContext::MD_coro_outside_frame:
    return MetadataMeaning::Permission;
  }
  return MetadataMeaning::Permission; // a kind registered by name
}

bool keptOnPointerCall(const Attribute &attribute, bool besideAbi) {
  if (attribute.isStringAttribute()) {
    return false;
  }
  switch (meaningOf(attribute.getKindAsEnum()).pointer) {
  case OnPointerCall::Abi:
  case OnPointerCall::Limit:
    return true;
  case OnPointerCall::KeepBesideAbi:
    return besideAbi;
  case OnPointerCall::Remove:
    return false;
  }
  return false;
}

// Whether the set has an ABI attribute, so that its `align` is the alignment that ABI attribute
// passes.
bool hasAbiAttribute(const AttributeSet &set) {
  for (const Attribute &attribute : set) {
    if (!attribute.isStringAttribute() && meaningOf(attribute.getKindAsEnum()).pointer == OnPointerCall::Abi) {
      return true;
    }
  }
  return false;
}

AttributeSet kept(LLVMContext &context, const AttributeSet &set) {
  bool besideAbi = hasAbiAttribute(set);
  AttrBuilder builder(context);
  for (const Attribute &attribute : set) {
    if (keptOnPointerCall(attribute, besideAbi)) {
      builder.addAttribute(attribute);
    }
  }
  return AttributeSet::get(context, builder);
}

bool removeClaims(CallBase &call) {
  LLVMContext &context = call.getContext();
  AttributeList list = call.getAttributes();
  SmallVector<AttributeSet, 8> params;
  for (unsigned i = 0; i < call.arg_size(); ++i) {
    params.push_back(kept(context, list.getParamAttrs(i)));
  }
  AttributeList stripped =
      AttributeList::get(context, kept(context, list.getFnAttrs()), kept(context, list.getRetAttrs()), params);
  bool changed = stripped != list;
  call.setAttributes(stripped);
  SmallVector<std::pair<unsigned, MDNode *>, 4> attached;
  call.getAllMetadata(attached);
  for (const auto &[kind, node] : attached) {
    (void)node;
    switch (metadataMeaning(kind)) {
    case MetadataMeaning::Debug:
      break;
    case MetadataMeaning::ProgramLoop:
    case MetadataMeaning::Permission:
      call.setMetadata(kind, nullptr);
      changed = true;
      break;
    }
  }
  return changed;
}

} // namespace

unsigned removePointerCallClaims(Module &module) {
  unsigned changed = 0;
  for (Function &function : module) {
    for (BasicBlock &block : function) {
      for (Instruction &inst : block) {
        auto *call = dyn_cast<CallBase>(&inst);
        if (call && !isa<Function>(call->getCalledOperand()) && removeClaims(*call)) {
          ++changed;
        }
      }
    }
  }
  return changed;
}

bool contextAttributeOnDirectCall(unsigned kind) {
  return meaningOf(static_cast<Attribute::AttrKind>(kind)).contextOnDirectCall;
}

bool metadataOnDirectCall(unsigned kind) {
  switch (metadataMeaning(kind)) {
  case MetadataMeaning::Debug:
  case MetadataMeaning::ProgramLoop:
    return true;
  case MetadataMeaning::Permission:
    return false;
  }
  return false;
}

} // namespace bennett
