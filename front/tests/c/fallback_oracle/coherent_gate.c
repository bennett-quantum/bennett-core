#include "fixture.h"
/* X has this ordinary C action on a basis input. Native runs never execute
 * preparation/phase gates; the Rust reference assembles those amplitudes. */
#ifdef NATIVE
#define flip(p) (*(p) = !*(p))
#else
#define flip(p) bennett_x(p)
#endif
#define update(q, r, yp) do { if ((q) && !(r)) flip(yp); } while (0)
#include "driver.h"
