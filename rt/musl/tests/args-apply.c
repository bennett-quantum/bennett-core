/* Relinking with other arguments changes only the arguments' data: the steps inside an apply stay the same.
 * main prints the budget it receives; applied_entry, a second entry not called by main (quantum operations in
 * main of a musl-linked program are refused today), applies a bounded search to a register in superposition. */
#include "bennett.h"
#include <stdio.h>
#include <stdlib.h>

#define SEARCH_BOUND 200

/* Collatz steps from start, at most SEARCH_BOUND: one bound for the whole search. */
static unsigned search(unsigned start)
{
    unsigned steps = 0, n = start;
    while (n != 1 && steps < SEARCH_BOUND) {
        n = n % 2 ? 3 * n + 1 : n / 2;
        steps++;
    }
    return steps;
}

int applied_entry(void)
{
    unsigned char row = 0;
    unsigned result = 0;
    bennett_h(bennett_bit(&row, 0));
    bennett_h(bennett_bit(&row, 1));
    bennett_h(bennett_bit(&row, 2));
    result ^= search(row + 3u);
    return (int)(bennett_measure(&row) + bennett_measure(&result));
}

int main(int argc, char **argv)
{
    printf("budget %lu\n", argc > 1 ? strtoul(argv[1], NULL, 10) : 0ul);
    return 0;
}
