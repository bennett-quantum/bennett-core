/* The scope list of bq.h is one object for the whole program (docs/HANDLES.md, 8.4 and section 15, R4): pushes in
 * another unit (scope-push.c) are seen here. No interface primitive is called, so the program runs on the device
 * today (bq_pop is left out: its `__bennett_bad_argument` is not lowered by the front yet). Prints the depth and the
 * pushed qubits and returns 0 if they are the ones pushed. */
#include <bq.h>
#include <stdio.h>

void enter(uint64_t id, uint64_t bit);

int main(void) {
    enter(7, 3);
    enter(9, 1);
    uint64_t depth = bq_scope_depth();
    const bq_bit *list = bq_scope_list();
    printf("depth %llu handle %llu bit %llu\n", (unsigned long long)depth, (unsigned long long)list[0].reg.id,
           (unsigned long long)list[1].bit);
    return depth == 2 && list[0].reg.id == 7 && list[0].bit == 3 && list[1].reg.id == 9 && list[1].bit == 1 ? 0 : 1;
}
