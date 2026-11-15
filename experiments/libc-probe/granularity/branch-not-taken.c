/* Floating point on a branch the run never takes, but that the optimiser cannot prove dead. */
volatile int flag = 0;
volatile double d = 1.5;
int main(void) {
  if (flag) return (int)(d * 3);
  return 0;
}
