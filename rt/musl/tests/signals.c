/* Signals a program sends itself, delivered before the call that raises or unblocks them returns. */
#define _GNU_SOURCE
#include <signal.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>
static volatile int seen;
static void count(int sig) { seen += sig; }
static void info(int sig, siginfo_t *si, void *context) { (void)context; seen += 1000 * (si->si_signo == sig); }
static void never(int sig) { (void)sig; puts("never"); }
int main(void) {
    signal(SIGFPE, never);                       /* installed, never raised */
    signal(SIGUSR1, count);
    raise(SIGUSR1);
    printf("handler %d\n", seen);
    signal(SIGUSR1, SIG_IGN);
    raise(SIGUSR1);
    printf("ignored %d\n", seen);
    struct sigaction action;
    memset(&action, 0, sizeof action);
    action.sa_sigaction = info;
    action.sa_flags = SA_SIGINFO | SA_RESETHAND;
    sigaction(SIGUSR2, &action, 0);
    kill(getpid(), SIGUSR2);
    struct sigaction old;
    sigaction(SIGUSR2, 0, &old);
    printf("siginfo %d reset %d\n", seen, old.sa_handler == SIG_DFL);
    sigset_t block, waiting;
    sigemptyset(&block);
    sigaddset(&block, SIGUSR1);
    signal(SIGUSR1, count);
    sigprocmask(SIG_BLOCK, &block, 0);
    raise(SIGUSR1);
    sigpending(&waiting);
    printf("pending %d %d\n", sigismember(&waiting, SIGUSR1), seen);
    sigprocmask(SIG_UNBLOCK, &block, 0);
    printf("delivered %d\n", seen);
    raise(SIGCHLD);                              /* ignored by default */
    return 0;
}
