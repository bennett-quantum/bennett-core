/* The unchanged corpus and unchanged native harness in one translation unit
 * for link.py. Each complete i64 result is printed by the original harness;
 * musl supplies startup, malloc/free, stdio and exit on the device. */
#include "hashtable.c"
#include "main_golden.c"
