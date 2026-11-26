/* stops.c: a check of the header's own named stops, not an example.
 *
 * The run ends in the way the first argument names (default: pop):
 *   wide-slice   a scalar form with a 65-qubit slice: the header stops with bad_argument
 *   field        a 9-qubit field in a one-byte member: the header stops with bad_argument
 *   pop          a pop without a push: the header stops with bad_argument
 *   grow         a list that would need 2^64 bytes or more: the header stops with out_of_memory
 * Each stop comes before any apply, so the check needs no budget. */
#include "bq.h"
#include <string.h>

struct io { uint8_t x, y; };

static uint64_t id(uint64_t a) { return a; }

int main(int argc, char **argv) {
    const char *ending = argc > 1 ? argv[1] : "pop";
    bq_reg r = bq_alloc(9);
    if (strcmp(ending, "wide-slice") == 0) {
        bq_apply_scoped_1(id, BQ_UNBOUNDED, bq_fault(), bq_slice_of(r, 0, 1), bq_slice_of(r, 1, 65));
    } else if (strcmp(ending, "field") == 0) {
        bq_field field = BQ_IN_ALL(r, struct io, x);
        (void)field;
    } else if (strcmp(ending, "pop") == 0) {
        bq_pop();
    } else if (strcmp(ending, "grow") == 0) {
        uint64_t capacity = (uint64_t)1 << 59;      /* 2^60 entries of 16 bytes: 2^64 bytes */
        bq_grow_(NULL, &capacity, capacity, sizeof(bq_bit));
    }
    printf("not reached\n");
    return 0;
}
