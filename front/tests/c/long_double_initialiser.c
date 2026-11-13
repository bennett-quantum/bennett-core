/* long_double_initialiser.c: byte 8 of a long double initialised to 1.0L, the low byte of its exponent.
 * Native: 255. The initialiser was once truncated to eight bytes. */
volatile long double x = 1.0L;

int main(void) { return ((volatile unsigned char *)&x)[8]; }
