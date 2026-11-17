#include "fixture.h"
static __attribute__((always_inline)) inline void update(_Bool q, _Bool r, _Bool *y) {
    (void)y;
    if (q) {
        cells[0] += 2;
        if (r) cells[1] ^= 7;
        else cells[2] = cells[0] + 1;
    }
}
#define MEASURED_CELLS 1
#include "driver.h"
