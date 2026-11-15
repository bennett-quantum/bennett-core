/* F1 of the testing pass of setjmp, found as a wrong result and fixed: the run-time savemask belongs to the
 * saved environment, even after revisiting this call site. Self-checking, returns 0 natively.
 * docs/DESIGN.md, "Non-local jumps": buffers copy; sigsetjmp restores its originally saved mask. */
#include <setjmp.h>
#include <signal.h>
#include <string.h>

static sigjmp_buf original, copy;
static volatile int turn, save = 1;

int main(void) {
  sigset_t usr2, current;
  sigemptyset(&usr2);
  sigaddset(&usr2, SIGUSR2);
again:
  if (sigsetjmp(original, save)) {
    sigprocmask(SIG_BLOCK, 0, &current);
    return sigismember(&current, SIGUSR2);
  }
  if (!turn) memcpy(copy, original, sizeof copy);
  ++turn;
  save = 0;
  if (turn < 2) goto again;
  sigprocmask(SIG_BLOCK, &usr2, 0);
  siglongjmp(copy, 1);
}
