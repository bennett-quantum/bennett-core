#include "fixture.h"
static __attribute__((always_inline)) inline void update(_Bool q, _Bool r, _Bool *y) {
    *y ^= q && !r;
}
#include "driver.h"
