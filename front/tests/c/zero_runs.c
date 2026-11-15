/* zero_runs.c: initialisers with runs of zero bytes, which the front writes as `zero N` data items from 16
 * bytes on (front/src/Emit.cpp): a whole zero array, runs of 15 and 16 bytes inside a global, runs at its
 * start and end, and runs next to a function address. Returns 0, or the number of the first failed check. */
static int one(void) { return 1; }

static volatile unsigned char big[65536];                    /* all zero */
static volatile unsigned char tail[40] = {1, 2};             /* 38 zero bytes at the end */
static volatile unsigned char head[40] = {[39] = 3};         /* 39 zero bytes at the start */
static volatile unsigned char short_run[17] = {4, [16] = 5}; /* 15 zero bytes between */
static volatile unsigned char exact_run[18] = {6, [17] = 7}; /* 16 zero bytes between */
static volatile struct {
  long before[3];
  int (*f)(void);
  long after[3];
} with_function = {{0, 0, 0}, one, {0, 0, 9}};

static int zero_except(volatile unsigned char *bytes, unsigned size, unsigned at, unsigned char value) {
  for (unsigned i = 0; i < size; i++) {
    if (bytes[i] != (i == at ? value : 0)) return 0;
  }
  return 1;
}

int main(void) {
  for (unsigned i = 0; i < sizeof big; i++) {
    if (big[i]) return 1;
  }
  big[sizeof big - 1] = 8; /* static data, not stack: a call may not overwrite it */
  if (!zero_except(big, sizeof big, sizeof big - 1, 8)) return 2;
  if (tail[0] != 1 || !zero_except(tail + 1, sizeof tail - 1, 0, 2)) return 3;
  if (!zero_except(head, sizeof head, 39, 3)) return 4;
  if (short_run[0] != 4 || !zero_except(short_run + 1, sizeof short_run - 1, 15, 5)) return 5;
  if (exact_run[0] != 6 || !zero_except(exact_run + 1, sizeof exact_run - 1, 16, 7)) return 6;
  for (unsigned i = 0; i < 3; i++) {
    if (with_function.before[i] || with_function.after[i] != (i == 2 ? 9 : 0)) return 7;
  }
  if (with_function.f() != 1) return 8;
  return 0;
}
