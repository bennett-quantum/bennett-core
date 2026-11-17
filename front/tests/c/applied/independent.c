#include <setjmp.h>

#define NOINLINE __attribute__((noinline))

static volatile unsigned counter = 11;

static NOINLINE unsigned descend(unsigned n, volatile unsigned *cell) {
    volatile unsigned scratch[n + 1];
    scratch[n] = n + *cell;
    counter += n + 1;
    *cell += counter;
    unsigned child = n ? descend(n - 1, cell) : 7;
    return child * 5 + scratch[n] + *cell;
}

static NOINLINE unsigned recursive_work(unsigned n, volatile unsigned *cell) {
    return descend(n, cell) ^ (counter << 11);
}

static NOINLINE void jump_from(jmp_buf record, unsigned depth, unsigned value) {
    volatile unsigned saved = value;
    if (depth == 99) return;
    if (!depth) longjmp(record, (int)value);
    jump_from(record, depth - 1, value * 2 + 1);
    if (saved == 0) longjmp(record, 1);
}

static NOINLINE unsigned jump_work(unsigned n) {
    jmp_buf record;
    volatile unsigned scratch = 19 + n;
    switch (setjmp(record)) {
    case 0:
        scratch *= 3;
        jump_from(record, n + 1, n + 1);
        return 0;
    case 3:
        return scratch ^ 0x131;
    case 11:
        return scratch ^ 0x2a3;
    default:
        return scratch + 0x173;
    }
}

NOINLINE unsigned nested_work(unsigned n) {
    volatile unsigned acc = 29;
    for (unsigned i = 0; i <= n; ++i)
        for (unsigned j = 0; j <= i + n; ++j)
            acc = acc * 3u + (i ^ j);
    return acc;
}

#ifdef NATIVE
#include <stdio.h>
#include <stdlib.h>

int main(int argc, char **argv) {
    if (argc != 2) return 2;
    unsigned n = (unsigned)strtoul(argv[1], 0, 10);
    volatile unsigned cell = 23;
    printf("%u %u %u\n", recursive_work(n, &cell), jump_work(n), nested_work(n));
    return 0;
}
#else
#include "bennett.h"

#define PREPARE(v)                         \
    unsigned char v = 0;                   \
    bennett_h(bennett_bit(&v, 0));          \
    bennett_h(bennett_bit(&v, 1));          \
    bennett_h(bennett_bit(&v, 2))

unsigned recursive_entry(void) {
    volatile unsigned cell = 23;
    PREPARE(v);
    unsigned r = 0x172;
    bennett_x(bennett_bit(&r, 0));
    r ^= recursive_work(v, &cell);
    bennett_measure(&v);
    return (unsigned)bennett_measure(&r) ^ cell ^ counter;
}

unsigned jump_entry(void) {
    PREPARE(v);
    unsigned r = 0;
    r ^= jump_work(v);
    bennett_measure(&v);
    return (unsigned)bennett_measure(&r);
}

unsigned nested_entry(void) {
    PREPARE(v);
    unsigned r = 0;
    r ^= nested_work(v);
    bennett_measure(&v);
    return (unsigned)bennett_measure(&r);
}
#endif
