#include <stdint.h>

int8_t r1_sat_add(int8_t a, int8_t b) {
    int sum = (int)a + b;
    return sum > 127 ? 127 : sum < -128 ? -128 : (int8_t)sum;
}
int8_t r1_popcount(int8_t x) { return __builtin_popcount((uint8_t)x); }
int8_t r1_gcd(int8_t a, int8_t b) {
    unsigned x = a < 0 ? -(int)a : a;
    unsigned y = b < 0 ? -(int)b : b;
    while (y) { unsigned r = x % y; x = y; y = r; }
    return (int8_t)x;
}
int8_t r1_sign_abs(int8_t x) {
    if (x == -128) return 127;
    if (x < 0) return -x;
    return x == 0 ? -1 : x / 2;
}
int8_t r1_lookup(int8_t x) {
    static const int8_t table[8] = {3, -7, 11, 0, 127, -128, 42, -1};
    return table[(uint8_t)x & 7];
}
