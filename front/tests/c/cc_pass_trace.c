/* Native call recorder for cc_pass.rs. The units under test are compiled separately. */
#include <bq.h>
#include <inttypes.h>
#include <setjmp.h>
#include <stdio.h>
#include <stdlib.h>

static jmp_buf stop;
static uint64_t widths[3];
static uint64_t allocated;

void run_case(const char *name, void (*entry)(unsigned, unsigned), unsigned times) {
    for (unsigned path = 0; path < 4; ++path) {
        printf("case %s path %u\n", name, path);
        allocated = 0;
        if (setjmp(stop) == 0) entry(path, times + path);
    }
}

bq_reg bq_alloc(uint64_t bits) {
    if (allocated == 2) abort();
    widths[++allocated] = bits;
    printf("bq_alloc %" PRIu64 "\n", bits);
    return (bq_reg){allocated};
}

void bq_free(bq_reg r) {
    printf("bq_free %" PRIu64 "\n", r.id);
}

uint64_t bq_width(bq_reg r) {
    printf("bq_width %" PRIu64 "\n", r.id);
    if (!r.id || r.id > allocated) abort();
    return widths[r.id];
}

uint64_t bq_measure(bq_reg r, uint64_t low, uint64_t bits) {
    printf("bq_measure %" PRIu64 " %" PRIu64 " %" PRIu64 "\n", r.id, low, bits);
    return 0;
}

void bq_gate(uint64_t kind, bq_reg r, uint64_t bit, const bq_bit *controls, uint64_t n) {
    if (n != 1) abort();
    printf("bq_gate %" PRIu64 " %" PRIu64 " %" PRIu64 " %" PRIu64 " %" PRIu64 "\n",
           kind, r.id, bit, controls[0].reg.id, controls[0].bit);
}

void bq_rotate(uint64_t kind, double angle, bq_reg r, uint64_t bit, const bq_bit *controls, uint64_t n) {
    printf("bq_rotate %" PRIu64 " %a %" PRIu64 " %" PRIu64 " %d %" PRIu64 "\n",
           kind, angle, r.id, bit, controls != NULL, n);
}

void bq_apply(bq_fn *f, uint64_t budget, bq_reg fault, const void *image, uint64_t size,
              const bq_field *fields, uint64_t m, const bq_bit *controls, uint64_t n) {
    if (m != 1 || size != 1) abort();
    unsigned char byte = *(const unsigned char *)image;
    f(&byte);
    bq_field field = fields[0];
    printf("bq_apply %" PRIu64 " %" PRIu64 " %u %" PRIu64 " %" PRIu64 " %" PRIu64
           " %" PRIu64 " %" PRIu64 " %d %" PRIu64 "\n",
           budget, fault.id, (unsigned)byte, field.slice.reg.id, field.slice.low,
           field.slice.bits, field.offset, field.direction, controls != NULL, n);
}

void __bennett_effect(char byte) {
    printf("__bennett_effect %u\n", (unsigned)(unsigned char)byte);
}

_Noreturn void __bennett_exit(int status) {
    printf("__bennett_exit %d\n", status);
    longjmp(stop, 1);
}

_Noreturn void __bennett_out_of_memory(void) {
    puts("__bennett_out_of_memory");
    longjmp(stop, 1);
}

_Noreturn void __bennett_bad_argument(void) {
    puts("__bennett_bad_argument");
    longjmp(stop, 1);
}
