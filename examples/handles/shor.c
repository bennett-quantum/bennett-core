/* shor.c: Shor's algorithm for the number N_TO_FACTOR with the base BASE, which shor15.c and shor21.c
 * define before including this file: period finding with gates and one apply, then classical
 * post-processing in plain C.
 *
 * The exponent register (EXPONENT_BITS qubits) is put into an equal superposition. Modular exponentiation
 * is the plain C function `powmod`, applied through apply: residue ^= BASE^e mod N, with the base and the
 * modulus ordinary data in the block. The residue register is then released unmeasured (the partial
 * trace). The inverse quantum Fourier transform on the exponent register is written with gates; measuring
 * it gives y close to k 2^EXPONENT_BITS / r for the period r, the smallest positive exponent with
 * BASE^r = 1 mod N. Continued fractions give a candidate, which is reduced to the period; if r is even and
 * BASE^(r/2) is neither 1 nor -1 mod N, gcd(BASE^(r/2) +- 1, N) are two proper factors. Otherwise the
 * attempt is repeated, up to eight attempts. */
#include "bq.h"
#include <math.h>

#define EXPONENT_BITS 8
#define ATTEMPTS 8

/* ---- classical subroutines: plain C ------------------------------------------------------------------ */

static uint64_t powmod(uint64_t base, uint64_t exponent, uint64_t modulus) {
    uint64_t result = 1;
    for (; exponent; exponent >>= 1, base = base * base % modulus)
        if (exponent & 1) result = result * base % modulus;
    return result;
}

static uint64_t gcd(uint64_t a, uint64_t b) {
    while (b) { uint64_t t = a % b; a = b; b = t; }
    return a;
}

/* The number of bits of the values below `limit`. */
static uint64_t bits_below(uint64_t limit) {
    uint64_t bits = 1;
    while ((limit - 1) >> bits) bits++;
    return bits;
}

/* The denominator of the last convergent of y / 2^bits whose denominator is below limit. */
static uint64_t convergent_denominator(uint64_t y, unsigned bits, uint64_t limit) {
    uint64_t num = y, den = 1ull << bits;
    uint64_t q_prev = 0, q = 1;                     /* denominators of the convergents */
    while (num != 0) {
        uint64_t a = den / num, rest = den % num;
        uint64_t q_next = a * q + q_prev;
        if (q_next >= limit) break;
        q_prev = q;
        q = q_next;
        den = num;
        num = rest;
    }
    return q;
}

/* The period from a candidate r with BASE^r = 1 mod N: the period divides r, so each factor p of r is
 * divided out while BASE^(r/p) stays 1. */
static uint64_t period_dividing(uint64_t r) {
    for (uint64_t p = 2; p <= r; p++)
        while (r % p == 0 && powmod(BASE, r / p, N_TO_FACTOR) == 1) r /= p;
    return r;
}

/* The period from a measured y: the first multiple of the convergent's denominator q (a multiple, if k/r
 * was reduced) with BASE^r = 1, reduced to the period. 0 if y says nothing (q = 1) or no multiple below N
 * works. */
static uint64_t period_from(uint64_t y) {
    uint64_t q = convergent_denominator(y, EXPONENT_BITS, N_TO_FACTOR);
    if (q < 2) return 0;
    for (uint64_t r = q; r < N_TO_FACTOR; r += q)
        if (powmod(BASE, r, N_TO_FACTOR) == 1) return period_dividing(r);
    return 0;
}

/* The block of the applied function: base and modulus are ordinary data, exponent and residue fields. */
struct modexp { uint64_t base, modulus, exponent, residue; };

static void modexp(void *block) {
    struct modexp *m = block;
    m->residue = powmod(m->base, m->exponent, m->modulus);
}

/* ---- quantum subroutines: gates ------------------------------------------------------------------------- */

static void swap(bq_reg r, uint64_t a, uint64_t b) {
    bq_cx(r, b, r, a);
    bq_cx(r, a, r, b);
    bq_cx(r, b, r, a);
}

/* The inverse of |x> to 2^{-n/2} sum_y e^{2 pi i x y / 2^n} |y>, qubit 0 least significant. */
static void inverse_fourier(bq_reg r) {
    uint64_t n = bq_width(r);
    for (uint64_t j = 0; j < n / 2; j++) swap(r, j, n - 1 - j);
    for (uint64_t j = 0; j < n; j++) {
        for (uint64_t k = 0; k < j; k++) bq_cphase(-2 * M_PI / (double)(1ull << (j - k + 1)), r, k, r, j);
        bq_h(r, j);
    }
}

/* One run of the quantum part: returns the measured y. */
static uint64_t find_frequency(void) {
    bq_reg exponent = bq_alloc(EXPONENT_BITS), residue = bq_alloc(bits_below(N_TO_FACTOR));
    for (uint64_t j = 0; j < bq_width(exponent); j++) bq_h(exponent, j);

    struct modexp image = { .base = BASE, .modulus = N_TO_FACTOR };
    bq_field fields[] = { BQ_IN_ALL(exponent, struct modexp, exponent), BQ_OUT_ALL(residue, struct modexp, residue) };
    bq_apply_scoped(modexp, BQ_BUDGET(modexp), bq_fault(), &image, sizeof image, fields, 2);  /* residue ^= a^e mod N */
    bq_faults_check("modexp");
    bq_free(residue);

    inverse_fourier(exponent);
    uint64_t y = bq_measure(exponent, 0, EXPONENT_BITS);
    bq_free(exponent);
    return y;
}

int main(void) {
    for (int attempt = 1; attempt <= ATTEMPTS; attempt++) {
        uint64_t y = find_frequency();
        uint64_t r = period_from(y);
        printf("attempt %d: y = %llu, period %llu", attempt, (unsigned long long)y, (unsigned long long)r);
        uint64_t half = r != 0 && r % 2 == 0 ? powmod(BASE, r / 2, N_TO_FACTOR) : 1;
        uint64_t p = gcd(half + N_TO_FACTOR - 1, N_TO_FACTOR), q = gcd(half + 1, N_TO_FACTOR);
        if (half == 1 || half == N_TO_FACTOR - 1 || p == 1 || q == 1 || p == N_TO_FACTOR || q == N_TO_FACTOR) {
            printf(" - no factor, retry\n");
            continue;
        }
        printf(" - %d = %llu x %llu\n", N_TO_FACTOR, (unsigned long long)p, (unsigned long long)q);
        return p * q == N_TO_FACTOR ? 0 : 1;
    }
    printf("no factor found\n");
    return 1;
}
