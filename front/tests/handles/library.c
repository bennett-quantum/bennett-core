/* library.c: a check of the header's libraries, not an example. Each form below runs on every basis input
 * of four qubits a, b, t, c and is compared with its truth table; then the width query, and the diffusion
 * step of the Grover programs under a control.
 *
 * The phase forms are read through interference: c is put into (|0> + (-1)^c |1>)/sqrt 2, the form
 * multiplies its 1 part by -1 where the form's condition holds, and a Hadamard on c turns that sign into
 * the value of c. A rotation Rz in place of the phase rotation would leave c in an equal mixture and fail. */
#include "grover.h"
#include <math.h>

static uint64_t id(uint64_t a) { return a; }
static uint64_t both(uint64_t a, uint64_t b) { return a && b; }

enum { CNOT_APPLY, TOFFOLI_APPLY, CNOT_GATE, CNOT_SCOPE, TOFFOLI_SCOPED_APPLY, SCOPED_SCALAR_2, REPEATED_CONTROLS,
       PHASE_SCOPE, PHASE_TWO_CONTROLS, PHASE_EXPLICIT, FORMS };
static const char *const names[FORMS] = {
    "apply_1 CNOT", "apply_2 Toffoli", "gate CNOT", "cx CNOT", "scoped apply_1 Toffoli",
    "scoped apply_2 under scope", "repeated controls", "phase under scope", "phase under two controls",
    "phase explicit list",
};

/* A control list of more than 2^16 entries, all the same qubit: the same as one entry. */
enum { REPEATS = (1 << 16) + 1 };
static bq_bit repeated[REPEATS];

/* Prepares |a b t c>, runs one form, returns the measured qubit that carries the form's result. */
static uint64_t run(int form, uint64_t a0, uint64_t b0, uint64_t t0, uint64_t c0) {
    bq_reg a = bq_alloc(1), b = bq_alloc(1), t = bq_alloc(1), c = bq_alloc(1);
    if (a0) bq_x(a, 0);
    if (b0) bq_x(b, 0);
    if (t0) bq_x(t, 0);
    if (c0) bq_x(c, 0);
    bq_reg result = t;
    switch (form) {
    case CNOT_APPLY: bq_apply_1(id, BQ_BUDGET(library), bq_fault(), bq_all(t), bq_all(a), 0, 0); break;
    case TOFFOLI_APPLY: bq_apply_2(both, BQ_BUDGET(library), bq_fault(), bq_all(t), bq_all(a), bq_all(b), 0, 0); break;
    case CNOT_GATE: { bq_bit control = bq_bit_of(a, 0); bq_gate(BQ_GATE_X, t, 0, &control, 1); } break;
    case CNOT_SCOPE: bq_cx(a, 0, t, 0); break;
    case TOFFOLI_SCOPED_APPLY:
        bq_push(b, 0);
        bq_apply_scoped_1(id, BQ_BUDGET(library), bq_fault(), bq_all(t), bq_all(a));
        bq_pop();
        break;
    case SCOPED_SCALAR_2:                   /* t ^= a && b under the control c */
        bq_push(c, 0);
        bq_apply_scoped_2(both, BQ_BUDGET(library), bq_fault(), bq_all(t), bq_all(a), bq_all(b));
        bq_pop();
        break;
    case REPEATED_CONTROLS:                 /* t ^= a */
        for (int k = 0; k < REPEATS; k++) repeated[k] = bq_bit_of(a, 0);
        bq_gate(BQ_GATE_X, t, 0, repeated, REPEATS);
        break;
    case PHASE_SCOPE:                       /* c picks up -1 where t is 1: c reads c ^ t */
        bq_h(c, 0);
        bq_push(c, 0);
        bq_phase(M_PI, t, 0);
        bq_pop();
        bq_h(c, 0);
        result = c;
        break;
    case PHASE_TWO_CONTROLS:                /* c reads c ^ (a && t) */
        bq_h(c, 0);
        bq_push(c, 0);
        bq_cphase(M_PI, a, 0, t, 0);
        bq_pop();
        bq_h(c, 0);
        result = c;
        break;
    case PHASE_EXPLICIT: {                  /* c reads c ^ (a && t), with an explicit two-entry list */
        bq_bit controls[2] = { bq_bit_of(c, 0), bq_bit_of(a, 0) };
        bq_h(c, 0);
        bq_rotate(BQ_ROT_PHASE, M_PI, t, 0, controls, 2);
        bq_h(c, 0);
        result = c;
    } break;
    }
    bq_faults_check(names[form]);
    uint64_t v = bq_measure(result, 0, 1);
    bq_free(a);
    bq_free(b);
    bq_free(t);
    bq_free(c);
    return v;
}

static uint64_t want(int form, uint64_t a, uint64_t b, uint64_t t, uint64_t c) {
    switch (form) {
    case TOFFOLI_APPLY: case TOFFOLI_SCOPED_APPLY: return t ^ (a & b);
    case SCOPED_SCALAR_2: return t ^ (a & b & c);
    case PHASE_SCOPE: return c ^ t;
    case PHASE_TWO_CONTROLS: case PHASE_EXPLICIT: return c ^ (a & t);
    default: return t ^ a;
    }
}

/* The diffusion step under a control in (|0> + |1>)/sqrt 2, on the uniform superposition of a 4-qubit
 * register, which inversion about the mean leaves unchanged: the control must read 0 after a Hadamard,
 * and the register 0 after Hadamards. A routine that is right only up to a phase leaves the control 1. */
static int diffusion_under_control_wrong(void) {
    bq_reg c = bq_alloc(1), r = bq_alloc(4);
    bq_h(c, 0);
    hadamard_all(r);
    bq_push(c, 0);
    diffusion(r);
    bq_pop();
    hadamard_all(r);
    bq_h(c, 0);
    uint64_t cv = bq_measure(c, 0, 1), rv = bq_measure(r, 0, 4);
    bq_free(c);
    bq_free(r);
    return cv != 0 || rv != 0;
}

int main(void) {
    int status = 0;
    for (int form = 0; form < FORMS; form++) {
        int wrong = 0;
        for (uint64_t in = 0; in < 16; in++) {
            uint64_t a = in & 1, b = in >> 1 & 1, t = in >> 2 & 1, c = in >> 3;
            wrong += run(form, a, b, t, c) != want(form, a, b, t, c);
        }
        printf("%-27s %s\n", names[form], wrong ? "WRONG" : "matches its truth table");
        status |= wrong != 0;
    }
    int widths_wrong = 0;
    for (uint64_t w = 1; w <= 8; w++) {
        bq_reg r = bq_alloc(w);
        bq_slice all = bq_all(r);
        widths_wrong += bq_width(r) != w || all.low != 0 || all.bits != w;
        bq_free(r);
    }
    printf("%-27s %s\n", "width query", widths_wrong ? "WRONG" : "matches the allocated widths");
    int diffusion_wrong = diffusion_under_control_wrong();
    printf("%-27s %s\n", "diffusion under control", diffusion_wrong ? "WRONG" : "leaves the mean unchanged");
    return status | (widths_wrong != 0) | diffusion_wrong;
}
