/* The other unit of scope-main.c and scope-own.c: pushes onto the program's one scope list. */
#include <bq.h>

void enter(uint64_t id, uint64_t bit) {
    bq_reg r = { id };
    bq_push(r, bit);
}
