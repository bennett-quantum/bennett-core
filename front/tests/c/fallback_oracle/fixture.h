/* Native and device builds execute exactly the same update() body. The native
 * build has no marker stubs and never links any Bennett implementation.
 *
 * The four cells the bodies write lie inside a twelve-byte image, between four
 * guard bytes on each side, and both sides compare the whole image: a stray
 * write just before or just past the cells is a different image, not unseen.
 * `cells` is a pointer into the image, so `cells[-1]` and `cells[4]` are
 * defined C (bytes of `image`), which the sanitized native build relies on. */
#include "bennett.h"
#include <string.h>
unsigned char image[12] = {0xa5, 0x5a, 0xc3, 0x3c, 1, 2, 3, 0, 0x96, 0x69, 0xf0, 0x0f};
#define cells (image + 4)
