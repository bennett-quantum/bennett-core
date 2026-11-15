/* Floating point on a branch the optimiser proves dead. */
static int flag = 0;
volatile double d = 1.5;
int main(void) {
  if (flag) return (int)(d * 3);
  return 0;
}
