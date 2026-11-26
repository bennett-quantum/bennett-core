/* limits.c: a check of the stand-in's sampling and of its stops, not an example.
 *
 * The first argument names the check:
 *   midpoint     a Hadamard qubit measured, the outcome printed (the test picks a seed whose first draw is
 *                1/2, which belongs to the outcome 1)
 *   state        allocates 24 qubits (the test limits the stand-in's memory)
 * and applied functions outside the class the stand-in models, which it stops by name:
 *   printf       stdio output to standard output
 *   descriptor   a read of a file the program opened before the apply
 *   open         an append to a file opened inside the applied function
 *   clock        a read of the clock
 *   shared       an apply while the program has a shared mapping the function writes
 *   foreign      an apply of a C library function (free, which has the type of an applied function)
 *   inside       an apply of an address inside a function, not its entry */
#include "bq.h"
#include <string.h>
#include <sys/mman.h>
#include <time.h>
#include <unistd.h>

struct io { uint8_t x, y; };

static void prints(void *p) { struct io *io = p; printf("%d\n", io->x); }
static int file_fd = -1;
static void reads(void *p) {
    struct io *io = p;
    char byte;
    io->y = read(file_fd, &byte, 1) == 1 && byte == 'A';
}
static const char *file_name;
static void appends(void *p) {
    struct io *io = p;
    FILE *file = fopen(file_name, "a");
    io->y = file && fputs("B", file) >= 0 && fclose(file) == 0;
}
static void clocks(void *p) { struct io *io = p; io->y = time(NULL) & 1; }
static volatile uint8_t *mapping;
static void writes_shared(void *p) { struct io *io = p; *mapping = 1; io->y = 1; }
static void copies(void *p) { struct io *io = p; io->y = io->x; }

int main(int argc, char **argv) {
    const char *check = argc > 1 ? argv[1] : "midpoint";
    bq_fn *outside = NULL;
    if (strcmp(check, "midpoint") == 0) {
        bq_reg q = bq_alloc(1);
        bq_h(q, 0);
        printf("midpoint %llu\n", (unsigned long long)bq_measure(q, 0, 1));
        bq_free(q);
    } else if (strcmp(check, "state") == 0) {
        bq_alloc(12);
        bq_alloc(12);
    } else if (strcmp(check, "printf") == 0) {
        outside = prints;
    } else if (strcmp(check, "descriptor") == 0) {
        FILE *file = tmpfile();
        if (!file || fputs("AB", file) < 0 || fflush(file) != 0) return 2;
        file_fd = fileno(file);
        lseek(file_fd, 0, SEEK_SET);
        outside = reads;
    } else if (strcmp(check, "open") == 0) {
        if (argc < 3) return 2;
        file_name = argv[2];
        outside = appends;
    } else if (strcmp(check, "clock") == 0) {
        outside = clocks;
    } else if (strcmp(check, "shared") == 0) {
        void *m = mmap(NULL, 1, PROT_READ | PROT_WRITE, MAP_SHARED | MAP_ANONYMOUS, -1, 0);
        if (m == MAP_FAILED) return 2;
        mapping = m;
        outside = writes_shared;
    } else if (strcmp(check, "foreign") == 0) {
        outside = free;
    } else if (strcmp(check, "inside") == 0) {
        outside = (bq_fn *)((uintptr_t)copies + 1);
    }
    if (outside) {
        bq_reg x = bq_alloc(1), y = bq_alloc(1);
        bq_h(x, 0);
        bq_field fields[] = { BQ_IN_ALL(x, struct io, x), BQ_OUT_ALL(y, struct io, y) };
        struct io image = { 0, 0 };
        bq_apply_scoped(outside, BQ_UNBOUNDED, bq_fault(), &image, sizeof image, fields, 2);
        printf("not reached\n");
    }
    return 0;
}
