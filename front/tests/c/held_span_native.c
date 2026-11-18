#ifdef NATIVE
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

uint64_t bennett_measure_bytes(const void *address, unsigned count) {
    const unsigned char *bytes = address;
    uint64_t value = 0;
    for (unsigned i = 0; i < count; ++i)
        value |= (uint64_t)bytes[i] << (8 * i);
    return value;
}
#else
#include "bennett.h"
#endif

unsigned char image[7] = {34, 17, 5, 9, 3, 6, 66};
static const unsigned char pattern[2] = {47, 181};

#ifndef NATIVE
#define PREPARE(q, prep) do {                         \
    if ((prep) == 2) { bennett_h(&(q)); bennett_s(&(q)); } \
    else if ((prep) == 1) bennett_x(&(q));              \
} while (0)
#else
#define PREPARE(q, prep) ((q) = (prep) != 0)
#endif

__attribute__((noinline)) static void put_pointer(unsigned char *p, unsigned prep) {
    _Bool q = 0;
    PREPARE(q, prep);
    if (q) {
        p[1] ^= 0x37;
        __builtin_memset(p + 2, 0x85, 2);
        __builtin_memcpy(p + 4, pattern, sizeof pattern);
    }
}

__attribute__((noinline)) static void put_global(unsigned prep) {
    _Bool q = 0;
    PREPARE(q, prep);
    if (q) {
        image[1] ^= 0x37;
        __builtin_memset(image + 2, 0x85, 2);
        __builtin_memcpy(image + 4, pattern, sizeof pattern);
    }
}

unsigned long long entry(unsigned prep, unsigned pointer) {
    if (pointer) put_pointer(image, prep);
    else put_global(prep);
    return bennett_measure_bytes(image, sizeof image);
}

#ifdef NATIVE
int main(int argc, char **argv) {
    if (argc != 3) return 1;
    printf("%llu\n", entry(atoi(argv[1]), atoi(argv[2])));
    return 0;
}
#endif
