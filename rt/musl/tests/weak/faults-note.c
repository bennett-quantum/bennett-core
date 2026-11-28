/* The other unit of faults-main.c and faults-own.c: adds to the count of the program's one fault set, as bq_fault
 * does (without allocating a register, so that the program runs on the device today). */
#include <bq.h>

void note(void) { bq_fault_set_.count++; }
