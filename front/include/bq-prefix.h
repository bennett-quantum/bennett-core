/* bq-prefix.h: the declarations of the handle interface's eleven primitives, the one file bennett-cc includes by
 * force, first, in every unit it compiles, the C library's included (docs/HANDLES.md, section 4; docs/DESIGN.md,
 * "The declaration check"). bq.h includes it, so a program that includes bq.h has it without the driver too.
 *
 * It includes nothing, so no search path, overlay or header of the user's or the host's is read before these
 * declarations. It is written only with keywords, names reserved to the implementation (the struct tags
 * `__bennett_*`, the compiler's `__UINT64_TYPE__`) and the primitives' own names, each in parentheses, so that a
 * function-like macro of that name does not expand here (an object-like one renames the declaration, which the
 * driver refuses by name). The one ordinary name, the member `id` of a handle, is kept from the user's macros by
 * push_macro and pop_macro.
 *
 * After the declarations comes the array of the primitives' addresses: clang emits a declaration only where
 * something uses it and fixes the symbol it stands for at that first use, so the array makes that first use come
 * here, before any text of the unit's own. bennett-cc checks the entries and deletes the array before any optimiser
 * pass. In a program compiled without the driver it stays (it is `used`), so every unit that includes this file,
 * through bq.h or by itself, references all eleven primitives, and a native link needs a definition of each (the
 * native stand-in, front/tests/handles/standin.c, defines them all). The order is that of docs/HANDLES.md, sections
 * 5.3 to 5.6. */
#ifndef BENNETT_BQ_PREFIX_H_INCLUDED
#define BENNETT_BQ_PREFIX_H_INCLUDED
#pragma GCC system_header

#ifdef __cplusplus
extern "C" {
#endif

/* A handle (bq_reg in bq.h). The control entry and the field entry (bq_bit, bq_field) are only pointed to here;
 * bq.h completes them. */
#pragma push_macro("id")
#undef id
struct __bennett_reg { __UINT64_TYPE__ id; };
#pragma pop_macro("id")
struct __bennett_bit;
struct __bennett_field;

struct __bennett_reg (bq_alloc)(__UINT64_TYPE__);
void (bq_free)(struct __bennett_reg);
__UINT64_TYPE__ (bq_width)(struct __bennett_reg);
__UINT64_TYPE__ (bq_measure)(struct __bennett_reg, __UINT64_TYPE__, __UINT64_TYPE__);
void (bq_gate)(__UINT64_TYPE__, struct __bennett_reg, __UINT64_TYPE__, const struct __bennett_bit *, __UINT64_TYPE__);
void (bq_rotate)(__UINT64_TYPE__, double, struct __bennett_reg, __UINT64_TYPE__, const struct __bennett_bit *,
                 __UINT64_TYPE__);
void (bq_apply)(void (*)(void *), __UINT64_TYPE__, struct __bennett_reg, const void *, __UINT64_TYPE__,
                const struct __bennett_field *, __UINT64_TYPE__, const struct __bennett_bit *, __UINT64_TYPE__);
void (__bennett_effect)(char);
__attribute__((__noreturn__)) void (__bennett_exit)(int);
__attribute__((__noreturn__)) void (__bennett_out_of_memory)(void);
__attribute__((__noreturn__)) void (__bennett_bad_argument)(void);

__attribute__((__used__)) static void (*const __bennett_references_[])(void) = {
    (void (*)(void))bq_alloc,         (void (*)(void))bq_free,
    (void (*)(void))bq_width,         (void (*)(void))bq_measure,
    (void (*)(void))bq_gate,          (void (*)(void))bq_rotate,
    (void (*)(void))bq_apply,         (void (*)(void))__bennett_effect,
    (void (*)(void))__bennett_exit,   (void (*)(void))__bennett_out_of_memory,
    (void (*)(void))__bennett_bad_argument,
};

#ifdef __cplusplus
}
#endif
#endif
