#include "fixture.h"
/* A macro keeps the update at its C call site in both builds, so the front
 * sees the original Boolean loads, rather than an inlined pointer parameter. */
#define update(q, r, yp) (*(yp) ^= (q) && !(r))
#include "driver.h"
