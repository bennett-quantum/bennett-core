/* thesis.c: the smallest program that lets ordinary code depend on a qubit.
 *
 * A Hadamard gate puts the qubit q into an equal superposition of 0 and 1. The ordinary C function `negate`
 * is then applied to it: the register r receives negate(q) for each value of q, which entangles r with q.
 * Measuring both gives q = 0, r = 1 or q = 1, r = 0, each with probability 1/2. Eight shots. */
#include "bq.h"

static uint64_t negate(uint64_t a) { return !a; }

int main(void) {
    for (int shot = 0; shot < 8; shot++) {
        bq_reg q = bq_alloc(1), r = bq_alloc(1);
        bq_h(q, 0);
        bq_apply_scoped_1(negate, BQ_BUDGET(negate), bq_fault(), bq_all(r), bq_all(q));   /* r ^= negate(q) */
        bq_faults_check("negate");

        uint64_t qv = bq_measure(q, 0, 1), rv = bq_measure(r, 0, 1);
        printf("shot %d: q = %llu, r = %llu%s\n", shot, (unsigned long long)qv, (unsigned long long)rv,
               rv == negate(qv) ? "" : "  WRONG");
        bq_free(q);
        bq_free(r);
    }
    return 0;
}
