/* Enter musl's own startup. It initializes TLS, walks constructors, calls main, then calls exit. */
#include <elf.h>
#include <stddef.h>
#ifndef MAIN_ARGS
#define MAIN_ARGS 0
#endif
#if MAIN_ARGS == 0
int main(void);
#define CALL_MAIN main()
#elif MAIN_ARGS == 1
int main(int);
#define CALL_MAIN main(argc)
#elif MAIN_ARGS == 2
int main(int, char **);
#define CALL_MAIN main(argc, argv)
#else
int main(int, char **, char **);
#define CALL_MAIN main(argc, argv, envp)
#endif

static int invoke_main(int argc, char **argv, char **envp)
{
    (void)argc; (void)argv; (void)envp;
    return CALL_MAIN;
}
int __libc_start_main(int (*)(int, char **, char **), int, char **, void (*)(), void (*)(), void (*)());
_Noreturn void __bennett_exit(int);
static char name[] = "prog";
static struct {
    char *argv[2];
    char *envp[1];
    size_t auxv[6];
} initial_stack = {{name, NULL}, {NULL}, {AT_PAGESZ, 4096, AT_NULL, 0, 0, 0}};

int __bennett_entry(void)
{
    int status = __libc_start_main(invoke_main, 1, initial_stack.argv, NULL, NULL, NULL);
    __bennett_exit(status);
}
