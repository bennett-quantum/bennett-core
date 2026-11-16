#include "fixture.h"
__attribute__((noinline)) void write_through(unsigned char *p, unsigned value) {
    p[0] = (unsigned char)value;
    p[1] += 3;
}
static __attribute__((always_inline)) inline void update(_Bool q, _Bool r, _Bool *y) {
    (void)y;
    if (q) write_through(cells, r + 6);
}
#include "driver.h"
