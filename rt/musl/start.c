/* Enter musl's own startup. It initializes TLS, walks constructors, calls main, then calls exit.
 * The arguments are the run input's (start.h); the environment is empty. */
#include <elf.h>
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
/* __bennett_exit is declared by bq-prefix.h, which bennett-cc includes by force (docs/HANDLES.md, section 15). */

int __bennett_entry(void)
{
    size_t argc = __bennett_argument_count;
    /* One array of pointers, as on a Linux process's initial stack: argv, its null pointer, the empty
     * environment's null pointer, then the auxiliary vector. musl finds the environment at argv + argc + 1 and
     * reads the auxiliary words after the environment's null pointer as size_t, which the x86-64 ABI represents
     * as it does a pointer. The array lives in this frame, which lasts until exit. */
    char *vector[argc + 8];
    char *string = __bennett_argument_bytes;
    for (size_t i = 0; i < argc; i++) {
        vector[i] = string;
        while (*string++)
            ;
    }
    vector[argc] = NULL;
    vector[argc + 1] = NULL;
    vector[argc + 2] = (char *)AT_PAGESZ;
    vector[argc + 3] = (char *)4096;
    vector[argc + 4] = (char *)AT_NULL;
    vector[argc + 5] = NULL;
    vector[argc + 6] = NULL;
    vector[argc + 7] = NULL;
    int status = __libc_start_main(invoke_main, (int)argc, vector, NULL, NULL, NULL);
    __bennett_exit(status);
}
