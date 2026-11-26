/* grover.h: the gate routines the two Grover programs share, written with the scope wrappers of bq.h so
 * that they stay right when run under a control. */
#ifndef GROVER_H_INCLUDED
#define GROVER_H_INCLUDED
#include "bq.h"

static inline void hadamard_all(bq_reg r) { for (uint64_t i = 0; i < bq_width(r); i++) bq_h(r, i); }
static inline void not_all(bq_reg r) { for (uint64_t i = 0; i < bq_width(r); i++) bq_x(r, i); }

/* -1 times the identity: (XZ)^2 = -1 on any qubit. Alone it is a global phase; under a scope it is the
 * phase -1 where the scope's controls are 1. */
static inline void minus_one(bq_reg r) {
    bq_x(r, 0);
    bq_z(r, 0);
    bq_x(r, 0);
    bq_z(r, 0);
}

/* Inversion about the mean, 2|s><s| - 1 with |s> the uniform superposition of r. H and X on every qubit, Z
 * on the top qubit controlled by all the others, X and H again: that is 1 - 2|s><s|; then -1. */
static inline void diffusion(bq_reg r) {
    uint64_t top = bq_width(r) - 1;
    hadamard_all(r);
    not_all(r);
    for (uint64_t i = 0; i < top; i++) bq_push(r, i);
    bq_z(r, top);
    for (uint64_t i = 0; i < top; i++) bq_pop();
    not_all(r);
    hadamard_all(r);
    minus_one(r);
}
#endif
