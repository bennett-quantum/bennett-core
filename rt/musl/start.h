/* The run's arguments as musl's start-up reads them: argv[0..argc-1], a null pointer, the environment
 * (empty: a null pointer), then the auxiliary vector. musl finds the environment at argv + argc + 1 and the
 * auxiliary vector after the environment's null pointer, so the three are one array, as on Linux's stack.
 * link.py writes the definitions into the run-input file (inputs.c): run input, not start-up code. */
#ifndef BENNETT_START_H
#define BENNETT_START_H
#include <stddef.h>
union __bennett_start_word {
    char *pointer;
    size_t value;
};
_Static_assert(sizeof(union __bennett_start_word) == sizeof(char *), "argv indexes the words");
extern const int __bennett_argc;
extern union __bennett_start_word __bennett_start_vector[];
#endif
