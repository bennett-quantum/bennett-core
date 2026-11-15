/* setjmp and longjmp with the C library: across frames, the value 0, sigsetjmp with and without the signal
 * mask (a constant and a run-time savemask), and jumps out of signal handlers. */
#include <setjmp.h>
#include <signal.h>
#include <stdio.h>
static jmp_buf jb;
static sigjmp_buf sjb;
static volatile int depth;
static volatile int savemask;

static void down(int n)
{
    if (n == 0) longjmp(jb, 7);
    depth = depth + 1;
    down(n - 1);
}

static int blocked(int sig)
{
    sigset_t set;
    sigprocmask(SIG_BLOCK, 0, &set);
    return sigismember(&set, sig);
}

static void sig_handler(int sig) { siglongjmp(sjb, sig); }
static void plain_handler(int sig) { longjmp(jb, sig); }

int main(void)
{
    int r = setjmp(jb);
    if (!r) down(5);
    printf("longjmp %d depth %d\n", r, depth);
    r = setjmp(jb);
    if (!r) longjmp(jb, 0);
    printf("zero %d\n", r);

    sigset_t usr2;
    sigemptyset(&usr2);
    sigaddset(&usr2, SIGUSR2);
    r = sigsetjmp(sjb, 1);
    if (!r) {
        sigprocmask(SIG_BLOCK, &usr2, 0);
        printf("blocked %d\n", blocked(SIGUSR2));
        siglongjmp(sjb, 3);
    }
    printf("restored %d %d\n", r, blocked(SIGUSR2));
    r = sigsetjmp(sjb, 0);
    if (!r) {
        sigprocmask(SIG_BLOCK, &usr2, 0);
        siglongjmp(sjb, 4);
    }
    printf("kept %d %d\n", r, blocked(SIGUSR2));
    sigprocmask(SIG_UNBLOCK, &usr2, 0);

    for (int m = 0; m < 2; m++) {
        savemask = m;
        r = sigsetjmp(sjb, savemask);
        if (!r) {
            sigprocmask(SIG_BLOCK, &usr2, 0);
            siglongjmp(sjb, 5 + m);
        }
        printf("savemask %d: %d %d\n", m, r, blocked(SIGUSR2));
        sigprocmask(SIG_UNBLOCK, &usr2, 0);
    }

    /* Out of a handler with siglongjmp: the saved mask comes back, so the signal is unblocked again. */
    signal(SIGUSR1, sig_handler);
    r = sigsetjmp(sjb, 1);
    if (!r) raise(SIGUSR1);
    printf("siglongjmp from handler %d %d\n", r, blocked(SIGUSR1));
    /* With longjmp, which saves no mask: the handler's mask stays, as the kernel leaves it. */
    signal(SIGUSR1, plain_handler);
    r = setjmp(jb);
    if (!r) raise(SIGUSR1);
    printf("longjmp from handler %d %d\n", r, blocked(SIGUSR1));
    return 0;
}
