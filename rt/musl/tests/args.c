/* Run arguments: argc, each argument's length and bytes, the null pointer after the last, the empty
 * environment after it, and writes into the strings and the argv array as C allows. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static void show(int argc, char **argv)
{
    printf("argc=%d\n", argc);
    for (int i = 0; i < argc; i++) {
        size_t length = strlen(argv[i]);
        printf("%d %zu:", i, length);
        for (size_t j = 0; j < length; j++)
            printf(" %02x", (unsigned char)argv[i][j]);
        printf("\n");
    }
    printf("argv[argc] %s\n", argv[argc] ? "set" : "null");
}

int main(int argc, char **argv, char **envp)
{
    show(argc, argv);
    int environment = 0;
    while (envp[environment])
        environment++;
    printf("envp=%s count=%d PATH=%s\n", envp == argv + argc + 1 ? "after argv" : "elsewhere", environment,
           getenv("PATH") ? "set" : "unset");
    /* The strings are writable to their terminating NUL; the array's pointers too (getopt-style permutation). */
    for (int i = 0; i < argc; i++)
        for (char *p = argv[i]; *p; p++)
            *p = (char)(*p ^ 0x20);
    if (argc > 2) {
        char *first = argv[1];
        argv[1] = argv[argc - 1];
        argv[argc - 1] = first;
    }
    show(argc, argv);
    return argc % 256;
}
