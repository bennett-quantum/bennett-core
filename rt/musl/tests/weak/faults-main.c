/* The fault set of bq.h is one object for the whole program (docs/HANDLES.md, 8.2 and section 15, R4): what another
 * unit (faults-note.c) writes is read here. Prints the count and returns 0 if it is the one written. */
#include <bq.h>
#include <stdio.h>

void note(void);

int main(void) {
    note();
    note();
    printf("faults %llu\n", (unsigned long long)bq_fault_set_.count);
    return bq_fault_set_.count == 2 ? 0 : 1;
}
