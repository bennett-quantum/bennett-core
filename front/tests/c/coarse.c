/* coarse.c: a quantum computation with ordinary inputs: an argument, values computed from it, and qubit
 * variables whose values are known (measured, or assigned). The front end computes those classically;
 * with --taint-all it computes them coherently instead. The state and the results must be the same
 * either way. Also the paths that overwrite a variable holding a known, non-constant value. */
#include "bennett.h"

unsigned long long coarse(unsigned char param) {
    _Bool a = 0, b = 0, c = 0;
    unsigned char k = 0;
    int out = 0;
    bennett_h(&a);
    unsigned long long first = bennett_measure(&a);  /* a holds a known value */
    bennett_h(&c);
    unsigned long long second = bennett_measure(&c); /* c too */
    k = (unsigned char)(param * 3 + 1);              /* k too */
    unsigned long long early = bennett_measure(&k); /* certain: k is in a basis state */
    k = (unsigned char)(param ^ 0x5a);               /* replacing one known value by another */
    bennett_h(&b);
    c = b;                                           /* replacing a measured value by a quantum one */
    out = (b ? param + k : a * 5 + param) ^ (int)first;
    unsigned long long y = bennett_measure(&out);
    unsigned long long kept = bennett_measure(&k);
    unsigned long long bit = bennett_measure(&b);
    unsigned long long copy = bennett_measure(&c);
    return y | kept << 32 | bit << 40 | first << 48 | second << 49 | copy << 50 | early << 56;
}
