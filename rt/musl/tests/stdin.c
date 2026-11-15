/* stdin is empty: every read reports end of file. scanf's alias makes a variadic function's address
 * taken, which once broke musl's unprototyped calls of the destructor array at exit. */
#include <stdio.h>
int main(void) {
    char line[16];
    int c = getchar(), n = 0, r = scanf("%d", &n);
    char *got = fgets(line, sizeof line, stdin);
    printf("%d %d %d %d\n", c, r, got == 0, feof(stdin) != 0);
    return 0;
}
