/* A program that defines the fault set itself, linked before faults-note.c, which carries the header's
 * zero-initialised weak definition: as for the scope list (scope-own.c), this unit's definition is kept and the
 * other unit writes to it. Returns 0 if the count starts at 11 and the other unit's write is seen. */
#include <bq.h>
#include <stdio.h>

void note(void);

struct bq_fault_set bq_fault_set_ = { 0, 11, 0 };

int main(void) {
    note();
    printf("faults %llu\n", (unsigned long long)bq_fault_set_.count);
    return bq_fault_set_.count == 12 ? 0 : 1;
}
