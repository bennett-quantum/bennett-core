/* An ordinary function applied whole under a qubit-dependent argument (docs/DESIGN.md, "Applied calls"):
 * it allocates and frees on the heap, keeps a static buffer, writes and reads back a file of the file
 * model, and runs two nested loops whose trip counts depend on its argument. The native build runs it once
 * per argument value in a fresh process; the device build applies it to all eight values at once. */
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

__attribute__((noinline)) unsigned work(unsigned v) {
    static unsigned char buffer[32];
    unsigned n = v + 1;
    unsigned *heap = malloc(n * sizeof *heap);
    if (!heap) return 0xffffffffu;
    unsigned acc = 7;
    for (unsigned i = 0; i < n; ++i) {
        heap[i] = 0;
        for (unsigned j = 0; j <= i * v; ++j) {
            heap[i] += j ^ i;
            acc = acc * 31 + j + i;
        }
    }
    for (unsigned i = 0; i < sizeof buffer; ++i) buffer[i] += (unsigned char)(acc >> (8 * (i % 4))) ^ (unsigned char)i;
    int fd = open("applied.dat", O_CREAT | O_RDWR | O_TRUNC, 0600);
    if (fd < 0) return 0xfffffffeu;
    if (write(fd, buffer, sizeof buffer) != (ssize_t)sizeof buffer) return 0xfffffffdu;
    unsigned char back[2];
    if (lseek(fd, (off_t)(3 * v), SEEK_SET) < 0 || read(fd, back, sizeof back) != (ssize_t)sizeof back) return 0xfffffffcu;
    close(fd);
    unsigned result = acc ^ heap[v] ^ ((unsigned)back[0] << 8) ^ ((unsigned)back[1] << 16);
    free(heap);
    return result;
}

#ifdef NATIVE
int main(int argc, char **argv) {
    if (argc != 2) return 2;
    printf("%u\n", work((unsigned)strtoul(argv[1], 0, 10)));
    return 0;
}
#else
#include "bennett.h"
/* musl's startup passes `main` to __libc_start_main by pointer, and a function with quantum operations
 * may not have its address taken: the experiment is its own entry, and `main` stays ordinary. */
int main(void) { return 0; }
int applied_entry(void) {
    unsigned char v = 0;
    unsigned r = 0;
    bennett_h(bennett_bit(&v, 0));
    bennett_h(bennett_bit(&v, 1));
    bennett_h(bennett_bit(&v, 2));
    r ^= work(v);
    return (int)(bennett_measure(&v) + bennett_measure(&r));
}
#endif
