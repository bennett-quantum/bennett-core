/* <math.h> for programs on the device: musl's, with the quiet comparison macros as the compiler's builtins,
 * which glibc's are on x86-64 (ucomisd: invalid for a signalling NaN only). musl's test for a NaN first, so they
 * never raise invalid, also not for a signalling NaN. rt/musl/link.py puts this folder before musl's headers. */
#include_next <math.h>

#undef isunordered
#undef isless
#undef islessequal
#undef islessgreater
#undef isgreater
#undef isgreaterequal
#define isunordered(x, y) __builtin_isunordered(x, y)
#define isless(x, y) __builtin_isless(x, y)
#define islessequal(x, y) __builtin_islessequal(x, y)
#define islessgreater(x, y) __builtin_islessgreater(x, y)
#define isgreater(x, y) __builtin_isgreater(x, y)
#define isgreaterequal(x, y) __builtin_isgreaterequal(x, y)
