/* Enter musl's own startup. It initializes TLS, walks constructors, calls main, then calls exit.
 * The arguments, environment and auxiliary vector are the run input's (start.h). */
#include <stddef.h>
#include "start.h"
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

int __bennett_entry(void)
{
    char **argv = &__bennett_start_vector[0].pointer;
    int status = __libc_start_main(invoke_main, __bennett_argc, argv, NULL, NULL, NULL);
    __bennett_exit(status);
}
