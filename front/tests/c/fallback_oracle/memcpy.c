#include "fixture.h"
static __attribute__((always_inline)) inline void update(_Bool q, _Bool r, _Bool *y) {
    (void)y;
    unsigned char source[4] = {7, 6, 5, 4};
    if (q) memcpy(cells, source, r ? 4 : 2);
}
#include "driver.h"
