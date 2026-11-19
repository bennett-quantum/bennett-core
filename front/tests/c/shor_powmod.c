#include "bennett.h"
#include <stdint.h>

static uint64_t powmod(uint64_t base, uint64_t exponent, uint64_t modulus) {
    uint64_t result = 1;
    for (; exponent; exponent >>= 1, base = base * base % modulus) {
        if (exponent & 1)
            result = result * base % modulus;
    }
    return result;
}

unsigned long long entry(void) {
    unsigned char exponent = 0, residue = 0;
    for (unsigned bit = 0; bit < 8; ++bit)
        bennett_h(bennett_bit(&exponent, bit));
    residue ^= powmod(BASE, exponent, MODULUS);
    unsigned long long input = bennett_measure(&exponent);
    return input | bennett_measure(&residue) << 8;
}
