/* constant_expressions.c: values LLVM computes as constant expressions over addresses (truncated,
 * offset, shifted, combined, compared), which the front evaluates once it has placed every global. Each is
 * compared with the same computation done at run time through a pointer LLVM cannot see through, so the
 * check does not depend on where anything is placed. Returns 0, or the number of the first check that
 * failed. */
typedef unsigned long ul;

int a[8];
char b[3];

volatile unsigned char low;
volatile unsigned short mid;
volatile unsigned shifted;
volatile int compared;
volatile ul mixed;

int *volatile pa = a;
char *volatile pb = b;

int main(void) {
  low = (unsigned char)((ul)&a[1] + 300);
  mid = (unsigned short)(((ul)&a[2] + 1000) << 12);
  shifted = (unsigned)(((ul)&a[3] << 3) ^ ((ul)b >> 1));
  compared = (unsigned char)(ul)&a[0] + 1 == (unsigned char)((ul)&a[0] + 1);
  mixed = ((ul)&b[2] * 3 - (ul)&a[5]) & 0xfff0f0f0;

  int *ra = pa;
  char *rb = pb;
  if (low != (unsigned char)((ul)&ra[1] + 300)) return 1;
  if (mid != (unsigned short)(((ul)&ra[2] + 1000) << 12)) return 2;
  if (shifted != (unsigned)(((ul)&ra[3] << 3) ^ ((ul)rb >> 1))) return 3;
  if (compared != ((unsigned char)(ul)&ra[0] + 1 == (unsigned char)((ul)&ra[0] + 1))) return 4;
  if (mixed != (((ul)&rb[2] * 3 - (ul)&ra[5]) & 0xfff0f0f0)) return 5;
  return 0;
}
