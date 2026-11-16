#include "fixture.h"
static __attribute__((always_inline)) inline void update(_Bool q, _Bool r, _Bool *y) {
    (void)r; (void)y;
    if (q) cells[0] = 7;
}
#include "driver.h"
