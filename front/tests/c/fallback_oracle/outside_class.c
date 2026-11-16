#include "fixture.h"
static __attribute__((always_inline)) inline void update(_Bool q, _Bool r, _Bool *y) {
    (void)q; (void)r;
    /* Deliberately erases a qubit. Only immediate measurement probabilities
     * are promised; no assertion about phases/coherence is legitimate. */
    *y = 0;
}
#include "driver.h"
