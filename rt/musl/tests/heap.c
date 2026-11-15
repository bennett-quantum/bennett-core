/* musl's malloc: its first allocation maps a fixed guard page over the start of the break and maps
 * PROT_NONE areas it then opens with mprotect; a large realloc falls back when mremap fails. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
int main(void) {
    char *small = malloc(100);
    strcpy(small, "heap");
    char *large = malloc(200000);
    large[199999] = 7;
    large = realloc(large, 400000);
    int *zeros = calloc(1000, sizeof *zeros);
    long sum = 0;
    for (int i = 0; i < 1000; ++i) sum += zeros[i];
    char *many[64];
    for (int i = 0; i < 64; ++i) {
        many[i] = malloc(16 + i * 24);
        memset(many[i], i, 16 + i * 24);
    }
    for (int i = 0; i < 64; i += 2) free(many[i]);
    long check = 0;
    for (int i = 1; i < 64; i += 2) check += many[i][i * 24 + 15];
    printf("%s %d %ld %ld\n", small, large[199999], sum, check);
    free(small);
    free(large);
    free(zeros);
    return 0;
}
