#include "fixture.h"
static __attribute__((always_inline)) inline void update(_Bool q, _Bool r, _Bool *y) {
    if (q) { if (r) *y ^= 1; }
}
#include "driver.h"
