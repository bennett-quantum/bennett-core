/* grover_predicate.c: Grover search over a plain C predicate on 8-bit values.
 *
 * The predicate is an ordinary C function. The oracle applies it to the search register with the flag qubit
 * in the state (|0> - |1>)/sqrt 2, so that flag ^= is_marked(x) multiplies the amplitude of x by -1 where
 * x is marked. The diffusion step is written with gates (grover.h). With one marked value among N = 256
 * values the best number of iterations is round(pi / (4 theta) - 1/2) with sin(theta) = 1/16, which is
 * 12; the marked value is then measured with probability above 0.9999. */
#include "grover.h"
#include <math.h>

#define BITS 8

/* A value is marked if its scrambled form is 0x5A. Multiplying by the odd number 167 is a bijection on
 * bytes, so exactly one value is marked. */
static uint64_t is_marked(uint64_t x) { return (uint8_t)(167 * x + 13) == 0x5A; }

/* |x>|-> to (-1)^is_marked(x) |x>|->. */
static void oracle(bq_reg x, bq_reg flag) {
    bq_apply_scoped_1(is_marked, BQ_BUDGET(is_marked), bq_fault(), bq_all(flag), bq_all(x));
    bq_faults_check("is_marked");
}

int main(void) {
    const double marked = 1, values = 1 << BITS;
    const int iterations = (int)round(M_PI / (4 * asin(sqrt(marked / values))) - 0.5);

    bq_reg x = bq_alloc(BITS), flag = bq_alloc(1);
    bq_x(flag, 0);
    bq_h(flag, 0);                                  /* flag = (|0> - |1>)/sqrt 2 */
    hadamard_all(x);                                /* every value with the same amplitude */
    for (int k = 0; k < iterations; k++) {
        oracle(x, flag);
        diffusion(x);
    }
    uint64_t found = bq_measure(x, 0, BITS);
    printf("iterations %d, measured %llu, is_marked = %llu\n", iterations, (unsigned long long)found,
           (unsigned long long)is_marked(found));
    bq_free(x);
    bq_free(flag);
    return is_marked(found) ? 0 : 1;
}
