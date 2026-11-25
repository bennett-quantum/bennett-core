/* malloc until the arena is exhausted: the run ends with the fault out_of_memory, never a null result,
 * and the lines flushed before it stay on the console. Small and large (mapped) requests alternate. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
int main(void) {
    for (int i = 1;; ++i) {
        size_t n = i % 2 ? 1000 : 200000;
        char *p = malloc(n);
        if (!p) {
            printf("null\n");
            return 1;
        }
        memset(p, i, n);
        printf("%d\n", i);
        fflush(stdout);
    }
}
