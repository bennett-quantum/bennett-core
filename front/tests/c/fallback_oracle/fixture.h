/* Native and device builds execute exactly the same update() body. The native
 * build has no marker stubs and never links any Bennett implementation. */
#include "bennett.h"
#include <string.h>
unsigned char cells[4] = {1, 2, 3, 0};
