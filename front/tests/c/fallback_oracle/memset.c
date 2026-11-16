#include "fixture.h"
static __attribute__((always_inline)) inline void update(_Bool q, _Bool r, _Bool *y) {
    (void)y;
    if (q) memset(cells, r ? 5 : 6, 4);
}
#include "driver.h"
