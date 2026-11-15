/* F2 of the testing pass of setjmp, found as a wrong result and fixed: siglongjmp restores the target
 * buffer's mask, not the last buffer passed at this call site. Self-checking, returns 0 natively.
 * docs/DESIGN.md, "Non-local jumps": sigsetjmp saves on first return and restores after a jump. */
#include <setjmp.h>
#include <signal.h>

static sigjmp_buf buffers[2];
static volatile int turn;

int main(void) {
  sigset_t usr2, current;
  sigemptyset(&usr2);
  sigaddset(&usr2, SIGUSR2);
again:
  if (sigsetjmp(buffers[turn], 1)) {
    sigprocmask(SIG_BLOCK, 0, &current);
    return sigismember(&current, SIGUSR2);
  }
  sigprocmask(SIG_BLOCK, &usr2, 0);
  ++turn;
  if (turn < 2) goto again;
  siglongjmp(buffers[0], 1);
}
