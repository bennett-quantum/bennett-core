/* A measured step budget received as a run argument: parsed with strtoul and passed on to the code it bounds.
 * Usage: budget N [START]. Prints what it parsed and how far the bounded work got. */
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>

/* Collatz steps from start, stopping after budget steps: the bound is one number for the whole search. */
static unsigned long bounded_search(unsigned long budget, unsigned long start, int *finished)
{
    unsigned long steps = 0, n = start;
    while (n != 1 && steps < budget) {
        n = n % 2 ? 3 * n + 1 : n / 2;
        steps++;
    }
    *finished = n == 1;
    return steps;
}

int main(int argc, char **argv)
{
    if (argc < 2) {
        printf("usage: %s N [START]\n", argv[0]);
        return 2;
    }
    char *end;
    errno = 0;
    unsigned long budget = strtoul(argv[1], &end, 10);
    printf("budget=%lu errno=%d rest=\"%s\"\n", budget, errno, end);
    if (errno || end == argv[1] || *end)
        return 3;
    unsigned long start = argc > 2 ? strtoul(argv[2], NULL, 0) : 27;
    int finished;
    unsigned long steps = bounded_search(budget, start, &finished);
    printf("start=%lu steps=%lu %s\n", start, steps, finished ? "finished" : "reached the budget");
    return finished ? 0 : 1;
}
