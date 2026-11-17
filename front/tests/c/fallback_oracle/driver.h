#ifdef NATIVE
#include <stdio.h>
#include <stdlib.h>
int main(int argc, char **argv) {
    if (argc != 2) return 2;
    unsigned basis = (unsigned)strtoul(argv[1], 0, 10);
    if (basis >= 8) return 2;
    _Bool q = basis & 1, r = (basis >> 1) & 1, y = (basis >> 2) & 1;
    update(q, r, &y);
#ifdef STRAY_WRITE
    /* A seeded stray write just past the cells: the native image must show it. */
    cells[4] ^= 0x10;
#endif
    printf("%u %u %u", q, r, y);
    for (unsigned i = 0; i < sizeof image; ++i) printf(" %u", image[i]);
    printf("\n");
    return 0;
}
#elif defined(CLASSICAL_REGION)
/* The whole twelve-byte image, guard bytes included, little-endian, so every
 * write the region makes is compared, also one just outside the cells. */
unsigned __int128 result(unsigned _BitInt(2) basis) {
    _Bool y = 0;
    update(basis & 1, (basis >> 1) & 1, &y);
    unsigned __int128 whole = 0;
    for (unsigned i = 0; i < sizeof image; ++i) whole |= (unsigned __int128)image[i] << (8 * i);
    return whole;
}
/* One bit of it, bit 1 of cells[0], for the phase-conjugation envelope. */
_Bool bit(unsigned _BitInt(2) basis) { return (result(basis) >> 33) & 1; }
/* Four bits of it, small enough for the dense interpreter's qubit cap. */
unsigned _BitInt(4) nibble(unsigned _BitInt(2) basis, unsigned _BitInt(5) which) {
    return (unsigned _BitInt(4))(result(basis) >> (4 * which));
}
#else
/* basis 0..7 prepares a column; 8 prepares a complex product state; 9 a real
 * product state whose eight Born weights all differ, so a measured check can
 * tell every basis state from every other.
 * All measurements precede reading the ordinary memory into the result. The
 * result holds the four cells; the harness reads the whole image, guard bytes
 * included, from the machine's memory. */
unsigned long long experiment(unsigned basis) {
    _Bool q = 0, r = 0, y = 0;
    if (basis < 8) {
        if (basis & 1) bennett_x(&q);
        if (basis & 2) bennett_x(&r);
        if (basis & 4) bennett_x(&y);
    } else if (basis == 8) {
        bennett_ry(&q, 0.7);
        bennett_h(&r); bennett_s(&r);
        bennett_h(&y); bennett_t(&y);
    } else {
        bennett_ry(&q, 0.7);
        bennett_ry(&r, 1.3);
        bennett_ry(&y, 2.2);
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
