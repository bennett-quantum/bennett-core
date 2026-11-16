/* Separate native oracle: inputs are bytes in unsigned encoding order, last
 * argument fastest; each result is emitted as its Int8 bit pattern. */
#include <stdint.h>
#include <stdio.h>

int8_t r1_sat_add(int8_t, int8_t);
int8_t r1_gcd(int8_t, int8_t);
int8_t r1_popcount(int8_t);
int8_t r1_sign_abs(int8_t);
int8_t r1_lookup(int8_t);

int main(void) {
    int8_t (*binary[])(int8_t, int8_t) = {r1_sat_add, r1_gcd};
    int8_t (*unary[])(int8_t) = {r1_popcount, r1_sign_abs, r1_lookup};
    for (unsigned f = 0; f < 2; ++f)
        for (unsigned a = 0; a < 256; ++a)
            for (unsigned b = 0; b < 256; ++b)
                if (putchar((uint8_t)binary[f]((int8_t)a, (int8_t)b)) == EOF) return 1;
    for (unsigned f = 0; f < 3; ++f)
        for (unsigned a = 0; a < 256; ++a)
            if (putchar((uint8_t)unary[f]((int8_t)a)) == EOF) return 1;
    return 0;
}
