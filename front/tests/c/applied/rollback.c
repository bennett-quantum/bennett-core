#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

#define NOINLINE __attribute__((noinline))
enum { VALUES = 8 };

static int setup(unsigned **original) {
    *original = malloc(VALUES * sizeof **original);
    if (!*original) return -1;
    for (unsigned i = 0; i < VALUES; ++i)
        (*original)[i] = 101 + 7 * i;
    int fd = open("rollback.dat", O_CREAT | O_TRUNC | O_RDWR, 0600);
    if (fd < 0 || write(fd, "ABCDEFGH", VALUES) != VALUES || lseek(fd, 1, SEEK_SET) != 1)
        return -1;
    return fd;
}

static NOINLINE unsigned exchange(unsigned n, unsigned *original, int fd) {
    unsigned value = original[n];
    volatile unsigned *grown = realloc(original, (VALUES + n + 1) * sizeof *original);
    if (!grown) return ~0u;
    unsigned kept = grown[n];
    grown[VALUES + n] = kept * 5;
    value ^= grown[VALUES + n];
    free((void *)grown);

    unsigned char replacement = 120 + n, observed = 0;
    int copy = dup(fd);
    if (copy < 0 || write(copy, &replacement, 1) != 1 || lseek(fd, -1, SEEK_CUR) != 1
        || read(fd, &observed, 1) != 1 || close(copy) != 0 || close(fd) != 0)
        return ~0u;
    return (value << 8) + observed;
}

#ifdef NATIVE
int main(int argc, char **argv) {
    if (argc != 2) return 2;
    unsigned *original;
    int fd = setup(&original);
    if (fd < 0) return 3;
    unsigned result = exchange((unsigned)strtoul(argv[1], 0, 10), original, fd);
    if (result == ~0u) return 4;
    printf("%u\n", result);
    return 0;
}
#else
#include "bennett.h"

static NOINLINE int verify(unsigned *original, int fd) {
    unsigned char observed = 0;
    if (lseek(fd, 0, SEEK_CUR) != 1 || read(fd, &observed, 1) != 1 || observed != 'B')
        return 1;
    // Allocating again must leave the original object alive and its contents intact.
    volatile unsigned *fresh = malloc(VALUES * sizeof *fresh);
    if (!fresh || fresh == original) return 2;
    for (unsigned i = 0; i < VALUES; ++i) {
        fresh[i] = i;
        if (original[i] != 101 + 7 * i) return 3;
    }
    free((void *)fresh);
    free(original);
    return close(fd) != 0;
}

int main(void) { return 0; }
int applied_entry(void) {
    unsigned *original;
    int fd = setup(&original);
    if (fd < 0) return -1;
    unsigned char v = 0;
    unsigned r = 0;
    bennett_h(bennett_bit(&v, 0));
    bennett_h(bennett_bit(&v, 1));
    bennett_h(bennett_bit(&v, 2));
    r ^= exchange(v, original, fd);
    bennett_measure(&v);
    unsigned result = (unsigned)bennett_measure(&r);
    return verify(original, fd) ? -2 : (int)result;
}
#endif
