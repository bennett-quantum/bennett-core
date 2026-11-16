#ifdef NATIVE
#include <stdio.h>
#include <stdlib.h>
int main(int argc, char **argv) {
    if (argc != 2) return 2;
    unsigned basis = (unsigned)strtoul(argv[1], 0, 10);
    if (basis >= 8) return 2;
    _Bool q = basis & 1, r = (basis >> 1) & 1, y = (basis >> 2) & 1;
    update(q, r, &y);
    printf("%u %u %u %u %u %u %u\n", q, r, y,
           cells[0], cells[1], cells[2], cells[3]);
    return 0;
}
#elif defined(CLASSICAL_REGION)
_Bool result(unsigned _BitInt(2) basis) {
    _Bool y = 0;
    update(basis & 1, (basis >> 1) & 1, &y);
    return (cells[0] >> 1) & 1;
}
#else
/* basis 0..7 prepares a column; 8 prepares a complex product state.
 * All measurements precede reading the ordinary memory into the result. */
unsigned long long experiment(unsigned basis) {
    _Bool q = 0, r = 0, y = 0;
    if (basis < 8) {
        if (basis & 1) bennett_x(&q);
        if (basis & 2) bennett_x(&r);
        if (basis & 4) bennett_x(&y);
    } else {
        bennett_ry(&q, 0.7);
        bennett_h(&r); bennett_s(&r);
        bennett_h(&y); bennett_t(&y);
    }
    update(q, r, &y);
#ifdef ROUND_TRIP
    bennett_z(&y);
    update(q, r, &y);
    bennett_h(&q);
#endif
    unsigned long long result = bennett_measure(&q);
    result |= bennett_measure(&r) << 1;
    result |= bennett_measure(&y) << 2;
    for (unsigned i = 0; i < 4; ++i)
        result |= (unsigned long long)cells[i] << (3 + 8 * i);
    return result;
}
#endif
