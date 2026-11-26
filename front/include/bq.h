/* bq.h: the handle interface of quantum C, and its libraries in ordinary C. See docs/HANDLES.md.
 *
 * Qubits live in the device; C holds only handles to registers of them. Every quantum operation is a call
 * of one of the primitives declared below, and the one bridge from qubits to ordinary code is bq_apply,
 * which applies a plain C function `void f(void *block)` to fields of a private block of bytes. Qubit 0
 * of a register is its least significant bit.
 *
 * Everything after the primitives is ordinary C that calls them: slices and fields, fault registers,
 * budgets, the scalar forms and the control scopes. Each is defined by what its C body does.
 */
#ifndef BENNETT_BQ_H_INCLUDED
#define BENNETT_BQ_H_INCLUDED

#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

#ifdef __cplusplus
extern "C" {
#define BQ_NORETURN [[noreturn]]
#define BQ_STATIC_ASSERT(condition, message) static_assert(condition, message)
#define BQ_ALIGNOF(type) alignof(type)
#else
#define BQ_NORETURN _Noreturn
#define BQ_STATIC_ASSERT(condition, message) _Static_assert(condition, message)
#define BQ_ALIGNOF(type) _Alignof(type)
#endif

/* ---- Types ---------------------------------------------------------------------------------------- */

/* A handle: the k-th successful bq_alloc of a run returns the handle holding k, from 1. A handle is never
 * reused; after bq_free its number names nothing. */
typedef struct { uint64_t id; } bq_reg;
/* One qubit of a register: an entry of a control list. */
typedef struct { bq_reg reg; uint64_t bit; } bq_bit;
/* The `bits` qubits of a register from qubit `low` up, read as an integer with qubit `low` lowest. */
typedef struct { bq_reg reg; uint64_t low, bits; } bq_slice;
/* A field of an apply: a slice tied to the ceil(bits / 8) bytes of the block from `offset`. */
typedef struct { bq_slice slice; uint64_t offset, direction; } bq_field;
/* The one type of an applied function. */
typedef void bq_fn(void *block);

/* The device reads control lists and field lists from memory in exactly these layouts (little-endian
 * unsigned 64-bit integers, no padding, alignment 8). */
BQ_STATIC_ASSERT(sizeof(bq_reg) == 8 && BQ_ALIGNOF(bq_reg) == 8, "a handle is 8 bytes, aligned to 8");
BQ_STATIC_ASSERT(BQ_ALIGNOF(bq_bit) == 8 && BQ_ALIGNOF(bq_slice) == 8 && BQ_ALIGNOF(bq_field) == 8,
                 "control entries, slices and field entries are aligned to 8 bytes");
BQ_STATIC_ASSERT(sizeof(bq_bit) == 16 && offsetof(bq_bit, reg) == 0 && offsetof(bq_bit, bit) == 8,
                 "control entry: handle at 0, qubit at 8, 16 bytes");
BQ_STATIC_ASSERT(sizeof(bq_slice) == 24 && offsetof(bq_slice, reg) == 0 && offsetof(bq_slice, low) == 8 &&
                     offsetof(bq_slice, bits) == 16,
                 "slice: handle at 0, low at 8, bits at 16, 24 bytes");
BQ_STATIC_ASSERT(sizeof(bq_field) == 40 && offsetof(bq_field, slice) == 0 && offsetof(bq_field, offset) == 24 &&
                     offsetof(bq_field, direction) == 32,
                 "field entry: slice at 0, offset at 24, direction at 32, 40 bytes");

/* ---- Codes ---------------------------------------------------------------------------------------- */

/* Gate kinds for bq_gate and rotation kinds for bq_rotate: distinct numbers, so that a gate kind given to
 * bq_rotate (or a rotation kind to bq_gate) is bad_gate. */
enum {
    BQ_GATE_X = 1, BQ_GATE_Y = 2, BQ_GATE_Z = 3, BQ_GATE_H = 4,
    BQ_GATE_S = 5, BQ_GATE_SDG = 6, BQ_GATE_T = 7, BQ_GATE_TDG = 8,
};
enum { BQ_ROT_X = 9, BQ_ROT_Y = 10, BQ_ROT_Z = 11, BQ_ROT_PHASE = 12 };   /* PHASE: diag(1, e^{i theta}) */
enum { BQ_DIR_IN = 1, BQ_DIR_OUT = 2 };

/* Fault codes, numbered as the device language numbers its faults. The first group is what bq_apply XORs
 * into the low BQ_FAULT_BITS qubits of a fault register for an input on which the applied function does
 * not return within its budget. The second group ends the run at a failed check of an interface call. */
enum { BQ_FAULT_BITS = 5 };   /* the low qubits of a fault register that receive the code */
enum {
    BQ_FAULT_BUDGET = 1, BQ_FAULT_DIVIDE_BY_ZERO = 2, BQ_FAULT_OUT_OF_BOUNDS = 3, BQ_FAULT_BAD_INDIRECT = 4,
    BQ_FAULT_UNREACHABLE = 5, BQ_FAULT_STACK_OVERFLOW = 6, BQ_FAULT_EFFECT_IN_APPLY = 7,
    BQ_FAULT_QUANTUM_IN_APPLY = 8, BQ_FAULT_BAD_STACK_RESTORE = 12, BQ_FAULT_EXIT_IN_APPLY = 13,
    BQ_FAULT_BAD_LONGJMP = 16, BQ_FAULT_QUBIT_MEMORY_ACCESS = 17, BQ_FAULT_OUT_OF_MEMORY = 19,

    BQ_FAULT_BAD_FUNCTION = 4, BQ_FAULT_BAD_HANDLE = 9, BQ_FAULT_CONTROL_IN_USE = 10, BQ_FAULT_BAD_ANGLE = 18,
    BQ_FAULT_BAD_WIDTH = 20, BQ_FAULT_OUT_OF_HANDLES = 21, BQ_FAULT_BAD_RANGE = 22, BQ_FAULT_BAD_GATE = 23,
    BQ_FAULT_BAD_ARGUMENT = 24,
};

/* ---- The primitives ------------------------------------------------------------------------------- */

/* Registers. */
bq_reg bq_alloc(uint64_t bits);                       /* a fresh register of `bits` qubits, all 0       */
void bq_free(bq_reg r);                               /* the partial trace: r is dropped unexamined     */
uint64_t bq_width(bq_reg r);                          /* the width r was allocated with                 */
uint64_t bq_measure(bq_reg r, uint64_t low, uint64_t bits);   /* measures a slice; r stays allocated    */

/* Gates: the gate on qubit `bit` of `r`, where every qubit of the control list is 1. */
void bq_gate(uint64_t kind, bq_reg r, uint64_t bit, const bq_bit *controls, uint64_t n);
void bq_rotate(uint64_t kind, double theta, bq_reg r, uint64_t bit, const bq_bit *controls, uint64_t n);

/* Apply: for each definite value of the in fields, run f on a private copy of memory, with the image copied
 * into a fresh block and the in fields written into it, for at most `budget` steps; XOR the out fields it
 * leaves, or the fault code, into the out qubits or the fault register. */
void bq_apply(bq_fn *f, uint64_t budget, bq_reg fault, const void *image, uint64_t size, const bq_field *fields,
              uint64_t m, const bq_bit *controls, uint64_t n);

/* The reserved effect primitives: console output of one byte, the end of the run with a status, and two
 * named stops, out_of_memory (the C library's allocator, when its storage cannot hold a request) and
 * bad_argument (the libraries below, at a check of their own that fails). */
void __bennett_effect(char byte);
BQ_NORETURN void __bennett_exit(int status);
BQ_NORETURN void __bennett_out_of_memory(void);
BQ_NORETURN void __bennett_bad_argument(void);

/* ---- Slices and fields ---------------------------------------------------------------------------- */

static inline bq_bit bq_bit_of(bq_reg r, uint64_t bit) {
    bq_bit b = { r, bit };
    return b;
}
static inline bq_slice bq_slice_of(bq_reg r, uint64_t low, uint64_t bits) {
    bq_slice s = { r, low, bits };
    return s;
}
/* Every qubit of r. */
static inline bq_slice bq_all(bq_reg r) { return bq_slice_of(r, 0, bq_width(r)); }

/* A field whose bytes must lie inside a member of `member_size` bytes: a slice wider than the member would
 * write into the next one, so it stops with bad_argument. The other bytes of the member keep the image's
 * value (for an in field) and an out field is read back as its `bits` low bits. */
static inline bq_field bq_field_of(bq_slice slice, uint64_t offset, uint64_t member_size, uint64_t direction) {
    if (slice.bits / 8 + (slice.bits % 8 != 0) > member_size) __bennett_bad_argument();
    bq_field f = { slice, offset, direction };
    return f;
}
#define BQ_MEMBER_SIZE(type, member) sizeof(((type *)0)->member)
/* A field for `member` of the block's struct `type`, from a slice or from every qubit of a register. */
#define BQ_IN(slice, type, member) \
    bq_field_of((slice), offsetof(type, member), BQ_MEMBER_SIZE(type, member), BQ_DIR_IN)
#define BQ_OUT(slice, type, member) \
    bq_field_of((slice), offsetof(type, member), BQ_MEMBER_SIZE(type, member), BQ_DIR_OUT)
#define BQ_IN_ALL(reg, type, member) BQ_IN(bq_all(reg), type, member)
#define BQ_OUT_ALL(reg, type, member) BQ_OUT(bq_all(reg), type, member)

/* ---- Budgets -------------------------------------------------------------------------------------- */

/* The step budget named `name`: the macro BQ_BUDGET_<name>, which the build defines. Steps are counted by
 * the device, never guessed: a measuring build defines every budget as BQ_UNBOUNDED, the run reports the
 * most steps each apply used, and the program is built again with those numbers (docs/HANDLES.md). */
#define BQ_UNBOUNDED UINT64_MAX
#define BQ_BUDGET(name) BQ_BUDGET_##name

/* ---- Fault registers ------------------------------------------------------------------------------ */

static inline const char *bq_fault_name(uint64_t code) {
    static const char *const names[] = {
        "none", "budget", "divide_by_zero", "out_of_bounds", "bad_indirect", "unreachable", "stack_overflow",
        "effect_in_apply", "quantum_in_apply", "bad_handle", "control_in_use", "unused", "bad_stack_restore",
        "exit_in_apply", "unused", "unused", "bad_longjmp", "qubit_memory_access", "bad_angle", "out_of_memory",
        "bad_width", "out_of_handles", "bad_range", "bad_gate", "bad_argument",
    };
    return code < sizeof names / sizeof names[0] ? names[code] : "unused";
}

/* A fresh fault register for one apply. */
static inline bq_reg bq_fault_alloc(void) { return bq_alloc(BQ_FAULT_BITS); }

/* Measures the fault code, releases the register, returns the code (0: no fault). */
static inline uint64_t bq_fault_take(bq_reg fault) {
    uint64_t code = bq_measure(fault, 0, BQ_FAULT_BITS);
    bq_free(fault);
    return code;
}

/* Fault registers waiting to be read: bq_fault() allocates one and adds it to the set, so that an apply is
 * written `bq_apply...(..., bq_fault(), ...)`; bq_faults_check reads them all at a point the program
 * chooses. One set per program (a weak definition, merged by the linker). */
struct bq_fault_set { bq_reg *list; uint64_t count, capacity; };
__attribute__((weak)) struct bq_fault_set bq_fault_set_;

/* Room for one more entry of `size` bytes in a list that grows on demand (no fixed capacity), keeping the
 * old entries. The new capacity and its byte count are computed with checked arithmetic. A count that does
 * not fit the allocator's argument, or a null result for this positive request (on the device the
 * allocator stops the run itself when its arena cannot hold a request, and returns null only for one it
 * rejects from its arguments alone), is out_of_memory, before any entry is written. */
static inline void *bq_grow_(void *list, uint64_t *capacity, uint64_t count, uint64_t size) {
    if (count < *capacity) return list;
    uint64_t wanted = *capacity ? 2 * *capacity : 1;
    if (wanted < *capacity || size == 0 || wanted > SIZE_MAX / size) __bennett_out_of_memory();
    void *grown = realloc(list, (size_t)(wanted * size));
    if (!grown) __bennett_out_of_memory();
    *capacity = wanted;
    return grown;
}

static inline bq_reg bq_fault(void) {
    struct bq_fault_set *set = &bq_fault_set_;
    set->list = (bq_reg *)bq_grow_(set->list, &set->capacity, set->count, sizeof *set->list);
    bq_reg fault = bq_fault_alloc();
    set->list[set->count++] = fault;
    return fault;
}

/* Measures and releases every register bq_fault() gave out since the last call, oldest first; returns the
 * first nonzero code, or 0. A code that is 0 with certainty is read without disturbing anything. */
static inline uint64_t bq_faults_take(void) {
    struct bq_fault_set *set = &bq_fault_set_;
    uint64_t first = 0;
    for (uint64_t k = 0; k < set->count; k++) {
        uint64_t code = bq_fault_take(set->list[k]);
        if (first == 0) first = code;
    }
    set->count = 0;
    return first;
}

/* The status of a run that bq_faults_check ends. */
#define BQ_FAULT_EXIT_STATUS 4

/* bq_faults_take; on a fault, says which and where on standard output and ends the run. Standard output,
 * because the device's runtime has a console only: a write to standard error stops the run there. */
static inline void bq_faults_check(const char *where) {
    uint64_t code = bq_faults_take();
    if (code == 0) return;
    printf("fault %s in %s\n", bq_fault_name(code), where);
    exit(BQ_FAULT_EXIT_STATUS);
}

/* ---- Scalar forms: out ^= g(in0, ...) for a C function of k integers, k from 0 to 4 --------------- */

/* The image of a scalar apply: g, then k inputs, then the result, all 8 bytes wide. */
struct bq_w0 { uint64_t (*g)(void); uint64_t r; };
struct bq_w1 { uint64_t (*g)(uint64_t); uint64_t a0, r; };
struct bq_w2 { uint64_t (*g)(uint64_t, uint64_t); uint64_t a0, a1, r; };
struct bq_w3 { uint64_t (*g)(uint64_t, uint64_t, uint64_t); uint64_t a0, a1, a2, r; };
struct bq_w4 { uint64_t (*g)(uint64_t, uint64_t, uint64_t, uint64_t); uint64_t a0, a1, a2, a3, r; };

/* The applied functions of the scalar forms; their steps count against the budget with those of g. */
static inline void bq_w0(void *p) { struct bq_w0 *w = (struct bq_w0 *)p; w->r = w->g(); }
static inline void bq_w1(void *p) { struct bq_w1 *w = (struct bq_w1 *)p; w->r = w->g(w->a0); }
static inline void bq_w2(void *p) { struct bq_w2 *w = (struct bq_w2 *)p; w->r = w->g(w->a0, w->a1); }
static inline void bq_w3(void *p) { struct bq_w3 *w = (struct bq_w3 *)p; w->r = w->g(w->a0, w->a1, w->a2); }
static inline void bq_w4(void *p) {
    struct bq_w4 *w = (struct bq_w4 *)p;
    w->r = w->g(w->a0, w->a1, w->a2, w->a3);
}

/* A slice of a scalar form holds at most 64 qubits, the width of its member (bad_argument otherwise). */
static inline bq_slice bq_scalar_(bq_slice s) {
    if (s.bits < 1 || s.bits > 64) __bennett_bad_argument();
    return s;
}

static inline void bq_apply_0(uint64_t (*g)(void), uint64_t budget, bq_reg fault, bq_slice out,
                              const bq_bit *controls, uint64_t n) {
    struct bq_w0 image = { g, 0 };
    bq_field f[1] = { BQ_OUT(bq_scalar_(out), struct bq_w0, r) };
    bq_apply(bq_w0, budget, fault, &image, sizeof image, f, 1, controls, n);
}
static inline void bq_apply_1(uint64_t (*g)(uint64_t), uint64_t budget, bq_reg fault, bq_slice out, bq_slice in0,
                              const bq_bit *controls, uint64_t n) {
    struct bq_w1 image = { g, 0, 0 };
    bq_field f[2] = { BQ_IN(bq_scalar_(in0), struct bq_w1, a0), BQ_OUT(bq_scalar_(out), struct bq_w1, r) };
    bq_apply(bq_w1, budget, fault, &image, sizeof image, f, 2, controls, n);
}
static inline void bq_apply_2(uint64_t (*g)(uint64_t, uint64_t), uint64_t budget, bq_reg fault, bq_slice out,
                              bq_slice in0, bq_slice in1, const bq_bit *controls, uint64_t n) {
    struct bq_w2 image = { g, 0, 0, 0 };
    bq_field f[3] = { BQ_IN(bq_scalar_(in0), struct bq_w2, a0), BQ_IN(bq_scalar_(in1), struct bq_w2, a1),
                      BQ_OUT(bq_scalar_(out), struct bq_w2, r) };
    bq_apply(bq_w2, budget, fault, &image, sizeof image, f, 3, controls, n);
}
static inline void bq_apply_3(uint64_t (*g)(uint64_t, uint64_t, uint64_t), uint64_t budget, bq_reg fault,
                              bq_slice out, bq_slice in0, bq_slice in1, bq_slice in2, const bq_bit *controls,
                              uint64_t n) {
    struct bq_w3 image = { g, 0, 0, 0, 0 };
    bq_field f[4] = { BQ_IN(bq_scalar_(in0), struct bq_w3, a0), BQ_IN(bq_scalar_(in1), struct bq_w3, a1),
                      BQ_IN(bq_scalar_(in2), struct bq_w3, a2), BQ_OUT(bq_scalar_(out), struct bq_w3, r) };
    bq_apply(bq_w3, budget, fault, &image, sizeof image, f, 4, controls, n);
}
static inline void bq_apply_4(uint64_t (*g)(uint64_t, uint64_t, uint64_t, uint64_t), uint64_t budget,
                              bq_reg fault, bq_slice out, bq_slice in0, bq_slice in1, bq_slice in2, bq_slice in3,
                              const bq_bit *controls, uint64_t n) {
    struct bq_w4 image = { g, 0, 0, 0, 0, 0 };
    bq_field f[5] = { BQ_IN(bq_scalar_(in0), struct bq_w4, a0), BQ_IN(bq_scalar_(in1), struct bq_w4, a1),
                      BQ_IN(bq_scalar_(in2), struct bq_w4, a2), BQ_IN(bq_scalar_(in3), struct bq_w4, a3),
                      BQ_OUT(bq_scalar_(out), struct bq_w4, r) };
    bq_apply(bq_w4, budget, fault, &image, sizeof image, f, 5, controls, n);
}

/* ---- Control scopes: every gate, rotation and apply issued through the forms below, between a push and
 * its pop, is controlled by the pushed qubit. ---------------------------------------------------------- */

/* The current control list, last in first out. One per program (a weak definition); it grows on demand. */
struct bq_scope { bq_bit *list; uint64_t depth, capacity; };
__attribute__((weak)) struct bq_scope bq_scope_;

static inline void bq_push(bq_reg r, uint64_t bit) {
    struct bq_scope *s = &bq_scope_;
    s->list = (bq_bit *)bq_grow_(s->list, &s->capacity, s->depth, sizeof *s->list);
    s->list[s->depth++] = bq_bit_of(r, bit);
}
/* Removes the most recent push; a pop without one is bad_argument. */
static inline void bq_pop(void) {
    if (bq_scope_.depth == 0) __bennett_bad_argument();
    bq_scope_.depth--;
}
static inline uint64_t bq_scope_depth(void) { return bq_scope_.depth; }
static inline const bq_bit *bq_scope_list(void) { return bq_scope_.list; }

/* Gates and rotations under the current scope (no push: no control). */
static inline void bq_x(bq_reg r, uint64_t bit) { bq_gate(BQ_GATE_X, r, bit, bq_scope_.list, bq_scope_.depth); }
static inline void bq_y(bq_reg r, uint64_t bit) { bq_gate(BQ_GATE_Y, r, bit, bq_scope_.list, bq_scope_.depth); }
static inline void bq_z(bq_reg r, uint64_t bit) { bq_gate(BQ_GATE_Z, r, bit, bq_scope_.list, bq_scope_.depth); }
static inline void bq_h(bq_reg r, uint64_t bit) { bq_gate(BQ_GATE_H, r, bit, bq_scope_.list, bq_scope_.depth); }
static inline void bq_s(bq_reg r, uint64_t bit) { bq_gate(BQ_GATE_S, r, bit, bq_scope_.list, bq_scope_.depth); }
static inline void bq_sdg(bq_reg r, uint64_t bit) {
    bq_gate(BQ_GATE_SDG, r, bit, bq_scope_.list, bq_scope_.depth);
}
static inline void bq_t(bq_reg r, uint64_t bit) { bq_gate(BQ_GATE_T, r, bit, bq_scope_.list, bq_scope_.depth); }
static inline void bq_tdg(bq_reg r, uint64_t bit) {
    bq_gate(BQ_GATE_TDG, r, bit, bq_scope_.list, bq_scope_.depth);
}
static inline void bq_rx(double theta, bq_reg r, uint64_t bit) {
    bq_rotate(BQ_ROT_X, theta, r, bit, bq_scope_.list, bq_scope_.depth);
}
static inline void bq_ry(double theta, bq_reg r, uint64_t bit) {
    bq_rotate(BQ_ROT_Y, theta, r, bit, bq_scope_.list, bq_scope_.depth);
}
static inline void bq_rz(double theta, bq_reg r, uint64_t bit) {
    bq_rotate(BQ_ROT_Z, theta, r, bit, bq_scope_.list, bq_scope_.depth);
}
/* diag(1, e^{i theta}). Unlike Rz, which is this times the phase e^{-i theta / 2}, it stays exact under a
 * control, so a routine written with it is right when run under a scope. */
static inline void bq_phase(double theta, bq_reg r, uint64_t bit) {
    bq_rotate(BQ_ROT_PHASE, theta, r, bit, bq_scope_.list, bq_scope_.depth);
}

/* One extra control for one gate: a push, the gate, the pop. */
static inline void bq_cx(bq_reg control, uint64_t control_bit, bq_reg r, uint64_t bit) {
    bq_push(control, control_bit);
    bq_x(r, bit);
    bq_pop();
}
static inline void bq_cphase(double theta, bq_reg control, uint64_t control_bit, bq_reg r, uint64_t bit) {
    bq_push(control, control_bit);
    bq_phase(theta, r, bit);
    bq_pop();
}

/* Apply and the scalar forms under the current scope. */
static inline void bq_apply_scoped(bq_fn *f, uint64_t budget, bq_reg fault, const void *image, uint64_t size,
                                   const bq_field *fields, uint64_t m) {
    bq_apply(f, budget, fault, image, size, fields, m, bq_scope_.list, bq_scope_.depth);
}
static inline void bq_apply_scoped_0(uint64_t (*g)(void), uint64_t budget, bq_reg fault, bq_slice out) {
    bq_apply_0(g, budget, fault, out, bq_scope_.list, bq_scope_.depth);
}
static inline void bq_apply_scoped_1(uint64_t (*g)(uint64_t), uint64_t budget, bq_reg fault, bq_slice out,
                                     bq_slice in0) {
    bq_apply_1(g, budget, fault, out, in0, bq_scope_.list, bq_scope_.depth);
}
static inline void bq_apply_scoped_2(uint64_t (*g)(uint64_t, uint64_t), uint64_t budget, bq_reg fault,
                                     bq_slice out, bq_slice in0, bq_slice in1) {
    bq_apply_2(g, budget, fault, out, in0, in1, bq_scope_.list, bq_scope_.depth);
}
static inline void bq_apply_scoped_3(uint64_t (*g)(uint64_t, uint64_t, uint64_t), uint64_t budget, bq_reg fault,
                                     bq_slice out, bq_slice in0, bq_slice in1, bq_slice in2) {
    bq_apply_3(g, budget, fault, out, in0, in1, in2, bq_scope_.list, bq_scope_.depth);
}
static inline void bq_apply_scoped_4(uint64_t (*g)(uint64_t, uint64_t, uint64_t, uint64_t), uint64_t budget,
                                     bq_reg fault, bq_slice out, bq_slice in0, bq_slice in1, bq_slice in2,
                                     bq_slice in3) {
    bq_apply_4(g, budget, fault, out, in0, in1, in2, in3, bq_scope_.list, bq_scope_.depth);
}

#ifdef __cplusplus
}
#endif

#undef BQ_NORETURN
#undef BQ_STATIC_ASSERT
#endif
