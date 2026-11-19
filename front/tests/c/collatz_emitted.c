#include "bennett.h"

static unsigned char collatz(unsigned char n) {
    unsigned value = n;
    unsigned char steps = 0;
    while (value > 1) {
        value = (value & 1) ? 3 * value + 1 : value >> 1;
        ++steps;
    }
    return steps;
}

unsigned long long entry(void) {
    unsigned char x = 0, y = 0;
    for (unsigned bit = 0; bit < 8; ++bit)
        bennett_h(bennett_bit(&x, bit));
    y ^= collatz(x);
    unsigned long long input = bennett_measure(&x);
    return input | bennett_measure(&y) << 8;
}
