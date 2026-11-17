/* printf inside an applied call. musl writes stdout out at a newline: that console write is the named fault
 * effect_in_region. Output without a newline stays in musl's buffer, which is memory and is undone with the
 * rest of the call's writes. Two entries, neither called by `main` (see work.c). */
#include <stdio.h>
#include "bennett.h"
__attribute__((noinline)) unsigned line(unsigned v) { printf("v=%u\n", v); return v * 5; }
__attribute__((noinline)) unsigned partial(unsigned v) { printf("v=%u", v); return v * 5; }
int main(void) { return 0; }
int line_entry(void) {
    unsigned char v = 0;
    unsigned r = 0;
    bennett_h(bennett_bit(&v, 0));
    bennett_h(bennett_bit(&v, 1));
    bennett_h(bennett_bit(&v, 2));
    r ^= line(v);
    return (int)(bennett_measure(&v) + bennett_measure(&r));
}
int partial_entry(void) {
    unsigned char v = 0;
    unsigned r = 0;
    bennett_h(bennett_bit(&v, 0));
    bennett_h(bennett_bit(&v, 1));
    bennett_h(bennett_bit(&v, 2));
    r ^= partial(v);
    return (int)(bennett_measure(&v) + bennett_measure(&r));
}
