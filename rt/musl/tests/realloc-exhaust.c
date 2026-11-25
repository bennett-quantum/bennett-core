/* realloc doubling a block until the arena is exhausted: the fault out_of_memory, never a null result. */
#include <stdio.h>
#include <stdlib.h>
int main(void) {
    char *p = malloc(16);
    for (size_t n = 32;; n *= 2) {
        char *q = realloc(p, n);
        if (!q) {
            printf("null\n");
            return 1;
        }
        p = q;
        p[n - 1] = 1;
        printf("%zu\n", n);
        fflush(stdout);
    }
}
