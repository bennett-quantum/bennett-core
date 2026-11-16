#include "fixture.h"
#define update(q, r, yp) do { if (q) { if (r) *(yp) ^= 1; } } while (0)
#include "driver.h"
