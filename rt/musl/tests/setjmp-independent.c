/* Independent native comparison: heap/struct copies, changed run-time savemask, repeated handler exits,
 * and a pending signal delivered while siglongjmp restores its mask. */
#define _GNU_SOURCE
#include <setjmp.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static sigjmp_buf saved, handler_saved;
static volatile sig_atomic_t mask_arg, hits, pending_hits;
static sigset_t usr1, usr2, empty;

static int blocked(int sig) {
    sigset_t current;
    if (sigprocmask(SIG_BLOCK, 0, &current)) return -1;
    return sigismember(&current, sig);
}

static void escape_handler(int sig) {
    (void)sig;
    ++hits;
    siglongjmp(handler_saved, hits);
}

static void pending_handler(int sig) {
    (void)sig;
    ++pending_hits;
}

struct record { unsigned before; jmp_buf buf; unsigned after; };
static struct record *heap, *copy;
static volatile int heap_local;

int main(void) {
    sigemptyset(&empty);
    sigemptyset(&usr1);
    sigaddset(&usr1, SIGUSR1);
    sigemptyset(&usr2);
    sigaddset(&usr2, SIGUSR2);
    heap = malloc(sizeof *heap);
    copy = malloc(sizeof *copy);
    if (!heap || !copy) return 90;
    heap->before = 123;
    heap->after = 456;
    if (!setjmp(heap->buf)) {
        memcpy(copy, heap, sizeof *copy);
        memset(heap->buf, 0xa5, sizeof heap->buf);
        heap_local = 23;
        longjmp(copy->buf, -17);
    }
    printf("heap %u %u %d\n", copy->before, copy->after, heap_local);
    free(heap);
    free(copy);

    /* The mask argument's value at the original call is used on both returns, even when its source
     * (a volatile global) changes before the jump. Negative also means true. */
    for (volatile int initial = -1; initial <= 1; ++initial) {
        sigprocmask(SIG_SETMASK, &empty, 0);
        mask_arg = initial;
        if (!sigsetjmp(saved, mask_arg)) {
            mask_arg = !initial;
            sigprocmask(SIG_BLOCK, &usr2, 0);
            siglongjmp(saved, 0);
        }
        printf("mask %d %d %d\n", initial, mask_arg, blocked(SIGUSR2));
    }
    sigprocmask(SIG_SETMASK, &empty, 0);
    struct sigaction action = {0};
    action.sa_handler = escape_handler;
    sigemptyset(&action.sa_mask);
    sigaddset(&action.sa_mask, SIGUSR2);
    if (sigaction(SIGUSR1, &action, 0)) return 91;
    if (!sigsetjmp(handler_saved, 1)) raise(SIGUSR1);
    if (hits < 8) {
        printf("handler %d %d %d\n", hits, blocked(SIGUSR1), blocked(SIGUSR2));
        raise(SIGUSR1);
    }
    printf("handler final %d %d %d\n", hits, blocked(SIGUSR1), blocked(SIGUSR2));

    action.sa_handler = pending_handler;
    sigemptyset(&action.sa_mask);
    if (sigaction(SIGUSR2, &action, 0)) return 92;
    if (!sigsetjmp(saved, 1)) {
        sigprocmask(SIG_BLOCK, &usr2, 0);
        raise(SIGUSR2);
        sigset_t pending;
        sigpending(&pending);
        printf("pending %d %d\n", sigismember(&pending, SIGUSR2), pending_hits);
        siglongjmp(saved, 7);
    }
    printf("delivered %d %d\n", pending_hits, blocked(SIGUSR2));
    return 0;
}
