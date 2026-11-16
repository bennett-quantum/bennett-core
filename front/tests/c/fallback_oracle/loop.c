#include "fixture.h"
static __attribute__((always_inline)) inline void update(_Bool q, _Bool r, _Bool *y) {
    (void)y;
    for (unsigned i = 0; i < (unsigned)q + 2 * (unsigned)r; ++i) {
        cells[i] += (unsigned char)(i + 1);
    }
}
#include "driver.h"
