/* SQLite under apply, from C: each entry sets the database up with ordinary code, puts a three-qubit row
 * register in uniform superposition and applies one query of the driver whose bound key depends on it
 * (docs/DESIGN.md, "Applied calls"). Linked with the unchanged lowered driver (`link/linked.bc` of
 * build.sh with SQL_ORACLE=1, NROWS=8); the entries are not called by `main`, so `main` stays ordinary. */
#include "bennett.h"
int experiment_setup(void);
int experiment_query(int q, int r);
#define ENTRY(Q)                                                                                              \
    int experiment_applied_##Q(void) {                                                                     \
        experiment_setup();                                                                                \
        unsigned char row = 0;                                                                             \
        unsigned result = 0;                                                                               \
        bennett_h(bennett_bit(&row, 0));                                                                   \
        bennett_h(bennett_bit(&row, 1));                                                                   \
        bennett_h(bennett_bit(&row, 2));                                                                   \
        result ^= (unsigned)experiment_query(Q, row);                                                      \
        return (int)(bennett_measure(&row) + bennett_measure(&result));                                    \
    }
ENTRY(0)
ENTRY(1)
ENTRY(2)
ENTRY(3)
ENTRY(4)
ENTRY(5)
ENTRY(6)
ENTRY(7)
