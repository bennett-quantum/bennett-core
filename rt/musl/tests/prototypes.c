/* C library functions called through prototypes other than musl's, where x86-64 determines what each side
 * reads (front/src/Abi.h, lowerMismatchedCalls): unprototyped K&R declarations, narrower results, a
 * variadic function called without its prototype, and the same through pointers. */
int strlen(char *);              /* size_t result read as int (c-testsuite 00025) */
int printf();                    /* variadic, unprototyped: the call sets %al */
char *strchr();                  /* an int argument for an int parameter, a pointer result */
void *memset();
int puts();
char toupper();                  /* an int result read as char */
long labs();
int (*volatile length)() = (int (*)())strlen;
long (*volatile absolute)(long, long) = (long (*)(long, long))labs; /* an extra argument */

int main(void) {
  char buffer[32];
  memset(buffer, 'a', (unsigned long)8);
  buffer[8] = 0;
  int n = strlen("hello") + length(buffer);
  printf("%d %s %c %.2f\n", n, strchr("hello", 'l'), toupper('q'), 2.5);
  puts("done");
  return n - 13 + (int)(labs(-4L) + absolute(-3L, 99L)) - 7;
}
