/* standin.c: a native stand-in for the device behind bq.h, for testing programs written in the handle
 * interface on the host. It is a state-vector simulator and a comparison aid: it is not the device, makes
 * no claim about the device's costs, and the device is the reference for everything it does.
 *
 * What it claims. For applied functions that compute on their block and on ordinary memory (allocating and
 * releasing it included, every allocation succeeding), call no effect, exit or interface primitive, register
 * no handler, do no buffered output, give out values that do not depend on numeric addresses, and return,
 * bq_apply gives the device's unitary (docs/HANDLES.md, 6.4). Outside that class it claims nothing: a
 * result there, a fault code included, says nothing about the device's. Examples of what is outside, each of
 * which gives a result here that the device does not: an exit handler registered inside the applied
 * function, buffered output to a file stream (fputc into a buffer), fork callbacks of the program (they
 * run at the stand-in's fork, before the applied function reads memory), output the program left buffered
 * before the apply (the stand-in flushes standard output and error at each apply), and an explicit trap
 * (natively a signal, mapped to out_of_bounds; on the device unreachable).
 *
 * - The state is a pure state vector over every live qubit, at most STANDIN_QUBITS of them.
 * - bq_free (the partial trace) is simulated by measuring the register with the run's generator and
 *   dropping its qubits, never showing the outcome. Averaged over outcomes this is the partial trace; a
 *   probability printed later is conditional on that hidden outcome.
 * - bq_apply evaluates the applied function once for each definite value of the in fields that has a
 *   nonzero amplitude where every control is 1, each time in a fork()ed child, which is a private copy of
 *   all memory. The child places the block, copies the image, writes the in fields, calls f, and writes the
 *   out values into a page shared with the parent. The stand-in then applies the XOR permutation of the out
 *   and fault qubits.
 * - Steps are the stand-in's own count, not the device's: the program is compiled with
 *   -fsanitize-coverage=trace-pc-guard,pc-table,no-prune, and a step is the entry into f plus one for each
 *   basic block of program code entered inside it (the C library's own code is not counted). A budget
 *   measured here is a budget for the stand-in only. An input that needs more steps than the budget gives
 *   the fault budget; the budget is the program's, and the stand-in has no bound of its own (a function
 *   that does not return is outside the class and runs until the test runner stops it). After each apply
 *   the stand-in writes one line to standard error:
 *       bq: apply <k> budget <b> inputs <e> steps <s> out-of-budget <n>
 *   where b is the budget passed, e the number of inputs evaluated, s the most steps an input used that
 *   ended within the budget and n the number of inputs that did not. With a budget at least s, every input
 *   that ended within the budget ends the same way.
 * - Approximate, outside the class: an applied function that ends otherwise than by returning still gets a
 *   fault code, so that a program's fault check has something to report (SIGFPE divide_by_zero, SIGSEGV
 *   below the end of its stack of STANDIN_APPLY_STACK bytes stack_overflow and elsewhere out_of_bounds,
 *   SIGKILL out_of_memory, another signal out_of_bounds, the primitives their codes, a child that ends
 *   without a result exit_in_apply). None of these is claimed to be the device's.
 * - A failed check of an interface call ends the run: `bq: fault <name> in <call>` on standard error, exit
 *   status STANDIN_FAULT_STATUS. The stop primitives print `bq: stop <name>` and end it the same way. These
 *   checks are a debugging aid; they are not claimed to be the device's.
 * - Measurement outcomes come from a generator seeded by BQ_SEED (default 1), with the device's sampling
 *   rule: the outcomes, in increasing order, own consecutive half-open intervals of [0, total) as wide as
 *   their weights, and a draw on a boundary belongs to the second outcome. With BQ_PROBS=1 every bq_measure
 *   first prints the probability of each outcome of at least STANDIN_PRINT_THRESHOLD.
 * - BQ_ALLOCATIONS (default 0) sets the allocation count at the start of the run, so that the first handle
 *   is that number plus 1.
 *
 * Stops. Some calls outside the class are caught and stop the run with a line `bq: limit <what> in <call>`
 * (exit status STANDIN_FAULT_STATUS) instead of giving a result. This is a help, not a check of the class:
 * a call outside it may also go unnoticed.
 * - system call <name>: the applied function made a system call other than one that maps or unmaps
 *   anonymous memory or ends the process (a seccomp filter in the child). Standard output and error are
 *   unbuffered in the child and standard input holds no buffered bytes, so stdio on them reaches a system
 *   call; the child's vDSO is unmapped, so a clock read is `system call vdso`.
 * - shared memory: the program has a shared mapping at bq_apply, which a child would change for the parent.
 * - function: f is not a function entry in the pc table of the program's code: a C library function
 *   (natively in a shared library, on the device part of the program), a function clang left uncounted (one
 *   whose entry block ends the run without a branch), or an address that names no function. Null and the
 *   primitives are bad_function. The compiled signature of f is not checked.
 * - memory: the simulator's own storage for the state cannot be had.
 */
#define _GNU_SOURCE
#include "bq.h"
#include <complex.h>
#include <linux/audit.h>
#include <linux/filter.h>
#include <linux/seccomp.h>
#include <math.h>
#include <signal.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdio_ext.h>
#include <string.h>
#include <sys/auxv.h>
#include <sys/mman.h>
#include <sys/prctl.h>
#include <sys/syscall.h>
#include <sys/uio.h>
#include <sys/wait.h>
#include <ucontext.h>
#include <unistd.h>

#if defined(__x86_64__)
#define STANDIN_AUDIT_ARCH AUDIT_ARCH_X86_64
#elif defined(__aarch64__)
#define STANDIN_AUDIT_ARCH AUDIT_ARCH_AARCH64
#else
#error "the stand-in's system call filter knows x86-64 and AArch64 only"
#endif

#define STANDIN_QUBITS 24               /* live qubits: the state vector has at most 2^24 amplitudes */
#define STANDIN_MAX_BLOCK (1u << 20)    /* the stand-in's memory size: the largest block of an apply  */
#define STANDIN_PRINT_THRESHOLD 5e-7    /* BQ_PROBS prints outcomes at least this probable            */
#define STANDIN_FAULT_STATUS 3          /* the exit status of a run ended by a failed check or a stop */
#define STANDIN_ALT_STACK (1u << 16)    /* the signal stack of a child, in bytes                      */
#define STANDIN_APPLY_STACK (1u << 23)  /* the stack an applied function runs on, in bytes            */
#define STANDIN_GUARD (1u << 16)        /* unmapped bytes below it: a fault there is stack_overflow   */

typedef double complex amp;

static amp *psi;                        /* 2^nq amplitudes; bit p of an index is the qubit at position p */
static unsigned nq;
static struct reg { bool live; unsigned width; unsigned pos[STANDIN_QUBITS]; } *regs;  /* regs[id - 1 - base] */
static uint64_t base;                   /* the allocation count at the start of the run */
static uint64_t nregs;                  /* allocations since then */
static bool in_child;
static uint64_t rng;
static bool print_probs;
static uint64_t measures, applies;
static uintptr_t vdso, vdso_end;        /* the vDSO's pages, unmapped in a child */

/* How a child ended, and the steps of its applied function (see the header comment), in a page shared
 * with the parent, so that both survive however the child ends. */
enum ending { ENDED_SILENTLY, RETURNED, FAULTED, LIMIT_SYSTEM_CALL };
static struct outcome {
    uint64_t steps;
    uint64_t ending;                    /* enum ending */
    uint64_t detail;                    /* FAULTED: the code; LIMIT_SYSTEM_CALL: the number, or -1 (vDSO) */
    uint64_t outs[STANDIN_QUBITS];      /* RETURNED: the out fields' values */
} *shared;
static volatile bool counting;
static uint64_t step_limit;

/* The program's function entries, from the coverage pc tables: pairs (address, flags), flag 1 an entry. */
static struct { const uintptr_t *begin, *end; } *pc_tables;
static size_t npc_tables;

/* ---- helpers --------------------------------------------------------------------------------------- */

static _Noreturn void limit(const char *what, const char *call) {
    fflush(stdout);
    fprintf(stderr, "bq: limit %s in %s\n", what, call);
    exit(STANDIN_FAULT_STATUS);
}

/* The simulator's own storage: a null result is the limit `memory`. */
static void *room(void *old, uint64_t count, size_t size, const char *call) {
    if (size && count > SIZE_MAX / size) limit("memory", call);
    void *p = realloc(old, count * size ? count * size : 1);
    if (!p) limit("memory", call);
    return p;
}

static void start(void) {
    if (psi) return;
    psi = room(NULL, 1, sizeof *psi, "start");
    psi[0] = 1;
    shared = mmap(NULL, sizeof *shared, PROT_READ | PROT_WRITE, MAP_SHARED | MAP_ANONYMOUS, -1, 0);
    if (shared == MAP_FAILED) limit("memory", "start");
    vdso = (uintptr_t)getauxval(AT_SYSINFO_EHDR);
    FILE *maps = fopen("/proc/self/maps", "r");
    char line[512];
    unsigned long low, high;
    while (vdso && maps && fgets(line, sizeof line, maps))
        if (strstr(line, "[vdso]") && sscanf(line, "%lx-%lx", &low, &high) == 2 && low == vdso) vdso_end = high;
    if (maps) fclose(maps);
    if (vdso && !vdso_end) limit("vdso", "start");
    const char *seed = getenv("BQ_SEED");
    rng = seed ? strtoull(seed, 0, 10) : 1;
    const char *probs = getenv("BQ_PROBS");
    print_probs = probs && probs[0] == '1';
    const char *allocations = getenv("BQ_ALLOCATIONS");
    base = allocations ? strtoull(allocations, 0, 10) : 0;
}

/* The child's last acts: how it ended, into the shared page. */
static _Noreturn void child_end(enum ending ending, uint64_t detail) {
    counting = false;
    shared->ending = ending;
    shared->detail = detail;
    _exit(0);
}
static _Noreturn void child_report(uint8_t code) { child_end(FAULTED, code); }

/* The coverage callbacks (trace-pc-guard: one call per block entered; pc-table: the entries). */
void __sanitizer_cov_trace_pc_guard_init(uint32_t *first, uint32_t *last) { (void)first; (void)last; }
void __sanitizer_cov_pcs_init(const uintptr_t *begin, const uintptr_t *end) {
    for (size_t k = 0; k < npc_tables; k++)
        if (pc_tables[k].begin == begin && pc_tables[k].end == end) return;
    pc_tables = room(pc_tables, npc_tables + 1, sizeof *pc_tables, "start");
    pc_tables[npc_tables].begin = begin;
    pc_tables[npc_tables++].end = end;
}
void __sanitizer_cov_trace_pc_guard(uint32_t *guard) {
    (void)guard;
    if (counting && ++shared->steps > step_limit) child_report(BQ_FAULT_BUDGET);
}

/* The first thing every interface function does: inside an applied function it is quantum_in_apply. */
#define ENTER() do { if (in_child) child_report(BQ_FAULT_QUANTUM_IN_APPLY); start(); } while (0)

static _Noreturn void fault(const char *name, const char *call) {
    fflush(stdout);
    fprintf(stderr, "bq: fault %s in %s\n", name, call);
    exit(STANDIN_FAULT_STATUS);
}

static _Noreturn void stop(const char *name) {
    fflush(stdout);
    fprintf(stderr, "bq: stop %s\n", name);
    exit(STANDIN_FAULT_STATUS);
}

static double uniform(void) {           /* splitmix64 */
    uint64_t z = (rng += 0x9E3779B97F4A7C15u);
    z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9u;
    z = (z ^ (z >> 27)) * 0x94D049BB133111EBu;
    return (double)((z ^ (z >> 31)) >> 11) * 0x1p-53;
}

static uint64_t gather(uint64_t i, const unsigned *pos, unsigned k) {
    uint64_t v = 0;
    for (unsigned b = 0; b < k; b++) v |= ((i >> pos[b]) & 1) << b;
    return v;
}
static uint64_t scatter(uint64_t v, const unsigned *pos, unsigned k) {
    uint64_t i = 0;
    for (unsigned b = 0; b < k; b++) i |= ((v >> b) & 1) << pos[b];
    return i;
}

/* Whether `bytes` bytes from p can be read as ordinary memory: one byte of every page is read through
 * process_vm_readv, which reports an unreadable page instead of faulting. A count past the end of the
 * address space is not readable. */
static bool readable(const void *p, uint64_t bytes) {
    if (bytes == 0) return true;
    uintptr_t first = (uintptr_t)p;
    if (bytes - 1 > UINTPTR_MAX - first) return false;
    uintptr_t last = first + (bytes - 1), page = (uintptr_t)sysconf(_SC_PAGESIZE);
    for (uintptr_t a = first;; a = (a | (page - 1)) + 1) {
        char byte;
        struct iovec local = { &byte, 1 }, remote = { (void *)a, 1 };
        if (process_vm_readv(getpid(), &local, 1, &remote, 1, 0) != 1) return false;
        if ((a | (page - 1)) >= last) break;
    }
    char byte;
    struct iovec local = { &byte, 1 }, remote = { (void *)last, 1 };
    return process_vm_readv(getpid(), &local, 1, &remote, 1, 0) == 1;
}

static struct reg *allocated(bq_reg r) {
    return r.id <= base || r.id - base > nregs || !regs[r.id - base - 1].live ? NULL : &regs[r.id - base - 1];
}
static struct reg *live(bq_reg r, const char *call) {
    struct reg *g = allocated(r);
    if (!g) fault("bad_handle", call);
    return g;
}

/* The checks of a control list (5.4: readable, then each entry's handle and position); returns the mask of
 * the control positions. A qubit may occur many times. */
static uint64_t control_mask(const bq_bit *c, uint64_t n, const char *call) {
    if (n > UINT64_MAX / sizeof *c || !readable(c, n * sizeof *c)) fault("bad_argument", call);
    uint64_t mask = 0;
    for (uint64_t k = 0; k < n; k++) {
        struct reg *g = live(c[k].reg, call);
        if (c[k].bit >= g->width) fault("bad_range", call);
        mask |= 1ull << g->pos[c[k].bit];
    }
    return mask;
}

/* Measures the qubits at pos[0..k) in the computational basis, collapses the state, returns the outcome. */
static uint64_t measure_positions(const unsigned *pos, unsigned k, bool report, const char *call) {
    uint64_t size = 1ull << nq, outcomes = 1ull << k;
    double *p = room(NULL, outcomes, sizeof *p, call), total = 0;
    memset(p, 0, outcomes * sizeof *p);
    for (uint64_t i = 0; i < size; i++) p[gather(i, pos, k)] += creal(psi[i] * conj(psi[i]));
    for (uint64_t v = 0; v < outcomes; v++) total += p[v];
    if (report) {
        printf("P[measure %llu]:", (unsigned long long)measures);
        for (uint64_t v = 0; v < outcomes; v++)
            if (p[v] >= STANDIN_PRINT_THRESHOLD) printf(" %llu:%.6f", (unsigned long long)v, p[v]);
        printf("\n");
    }
    /* Outcome v owns [start, start + p[v]): the first whose interval ends above the draw. */
    double draw = uniform() * total, start = 0;
    uint64_t chosen = outcomes;
    for (uint64_t v = 0; v < outcomes && chosen == outcomes; v++) {
        if (p[v] > 0 && draw < start + p[v]) chosen = v;
        start += p[v];
    }
    if (chosen == outcomes)             /* rounding at the top end: the last outcome of positive weight */
        for (uint64_t v = outcomes; v-- > 0;) if (p[v] > 0) { chosen = v; break; }
    double kept = p[chosen];
    free(p);
    double scale = 1 / sqrt(kept);
    for (uint64_t i = 0; i < size; i++) psi[i] = gather(i, pos, k) == chosen ? psi[i] * scale : 0;
    return chosen;
}

static void one_qubit(const amp m[2][2], unsigned target, uint64_t cmask) {
    uint64_t t = 1ull << target;
    for (uint64_t i = 0; i < (1ull << nq); i++) {
        if ((i & t) || (i & cmask) != cmask) continue;
        amp a0 = psi[i], a1 = psi[i | t];
        psi[i] = m[0][0] * a0 + m[0][1] * a1;
        psi[i | t] = m[1][0] * a0 + m[1][1] * a1;
    }
}

/* ---- registers --------------------------------------------------------------------------------------- */

bq_reg bq_alloc(uint64_t bits) {
    ENTER();
    if (bits < 1 || bits > STANDIN_QUBITS - nq) fault("bad_width", "bq_alloc");
    if (base + nregs == UINT64_MAX) fault("out_of_handles", "bq_alloc");   /* the count is 2^64 - 1 */
    uint64_t old = 1ull << nq, size = 1ull << (nq + bits);
    psi = room(psi, size, sizeof *psi, "bq_alloc");
    memset(psi + old, 0, (size - old) * sizeof *psi);
    regs = room(regs, nregs + 1, sizeof *regs, "bq_alloc");
    struct reg *g = &regs[nregs++];
    g->live = true;
    g->width = (unsigned)bits;
    for (unsigned b = 0; b < bits; b++) g->pos[b] = nq + b;
    nq += (unsigned)bits;
    return (bq_reg){ base + nregs };
}

void bq_free(bq_reg r) {
    ENTER();
    struct reg *g = live(r, "bq_free");
    uint64_t v = measure_positions(g->pos, g->width, false, "bq_free");
    uint64_t gone = scatter(v, g->pos, g->width), gone_mask = scatter(~0ull, g->pos, g->width);
    unsigned keep[STANDIN_QUBITS], nkeep = 0;
    for (unsigned p = 0; p < nq; p++) if (!((gone_mask >> p) & 1)) keep[nkeep++] = p;
    amp *next = room(NULL, 1ull << nkeep, sizeof *next, "bq_free");
    for (uint64_t j = 0; j < (1ull << nkeep); j++) next[j] = psi[scatter(j, keep, nkeep) | gone];
    free(psi);
    psi = next;
    nq = nkeep;
    g->live = false;
    for (uint64_t k = 0; k < nregs; k++) {      /* renumber the positions of the other registers */
        if (!regs[k].live) continue;
        for (unsigned b = 0; b < regs[k].width; b++) {
            unsigned p = regs[k].pos[b], below = 0;
            for (unsigned q = 0; q < p; q++) below += (gone_mask >> q) & 1;
            regs[k].pos[b] = p - below;
        }
    }
}

uint64_t bq_width(bq_reg r) {
    ENTER();
    return live(r, "bq_width")->width;
}

uint64_t bq_measure(bq_reg r, uint64_t low, uint64_t bits) {
    ENTER();
    struct reg *g = live(r, "bq_measure");
    if (bits < 1 || bits > 64 || bits > g->width || low > g->width - bits) fault("bad_range", "bq_measure");
    measures++;
    return measure_positions(g->pos + low, (unsigned)bits, print_probs, "bq_measure");
}

/* ---- gates ---------------------------------------------------------------------------------------------- */

static void gate_checks(bq_reg r, uint64_t bit, const bq_bit *c, uint64_t n, const char *call, unsigned *target,
                        uint64_t *cmask) {
    struct reg *g = live(r, call);
    if (bit >= g->width) fault("bad_range", call);
    *cmask = control_mask(c, n, call);
    *target = g->pos[bit];
    if ((*cmask >> *target) & 1) fault("control_in_use", call);
}

void bq_gate(uint64_t kind, bq_reg r, uint64_t bit, const bq_bit *controls, uint64_t n) {
    ENTER();
    if (kind < BQ_GATE_X || kind > BQ_GATE_TDG) fault("bad_gate", "bq_gate");
    unsigned target;
    uint64_t cmask;
    gate_checks(r, bit, controls, n, "bq_gate", &target, &cmask);
    const double h = 1 / sqrt(2);
    const amp w = cexp(I * M_PI / 4);
    amp m[2][2] = { { 1, 0 }, { 0, 1 } };
    switch (kind) {
    case BQ_GATE_X:   m[0][0] = 0; m[0][1] = 1;  m[1][0] = 1; m[1][1] = 0;  break;
    case BQ_GATE_Y:   m[0][0] = 0; m[0][1] = -I; m[1][0] = I; m[1][1] = 0;  break;
    case BQ_GATE_Z:   m[1][1] = -1; break;
    case BQ_GATE_H:   m[0][0] = h; m[0][1] = h;  m[1][0] = h; m[1][1] = -h; break;
    case BQ_GATE_S:   m[1][1] = I; break;
    case BQ_GATE_SDG: m[1][1] = -I; break;
    case BQ_GATE_T:   m[1][1] = w; break;
    case BQ_GATE_TDG: m[1][1] = conj(w); break;
    }
    one_qubit((const amp (*)[2])m, target, cmask);
}

void bq_rotate(uint64_t kind, double theta, bq_reg r, uint64_t bit, const bq_bit *controls, uint64_t n) {
    ENTER();
    if (kind < BQ_ROT_X || kind > BQ_ROT_PHASE) fault("bad_gate", "bq_rotate");
    if (!isfinite(theta)) fault("bad_angle", "bq_rotate");
    unsigned target;
    uint64_t cmask;
    gate_checks(r, bit, controls, n, "bq_rotate", &target, &cmask);
    double c = cos(theta / 2), s = sin(theta / 2);
    amp m[2][2] = { { 1, 0 }, { 0, 1 } };
    switch (kind) {                     /* exp(-i theta P / 2), and the phase rotation diag(1, e^{i theta}) */
    case BQ_ROT_X: m[0][0] = c; m[0][1] = -I * s; m[1][0] = -I * s; m[1][1] = c; break;
    case BQ_ROT_Y: m[0][0] = c; m[0][1] = -s;     m[1][0] = s;      m[1][1] = c; break;
    case BQ_ROT_Z: m[0][0] = cexp(-I * theta / 2); m[1][1] = cexp(I * theta / 2); break;
    case BQ_ROT_PHASE: m[1][1] = cexp(I * theta); break;
    }
    one_qubit((const amp (*)[2])m, target, cmask);
}

/* ---- the stop primitives and the effect primitives ------------------------------------------------------ */

void __bennett_bad_argument(void) {
    if (in_child) child_report(BQ_FAULT_BAD_ARGUMENT);
    stop("bad_argument");
}
void __bennett_out_of_memory(void) {
    if (in_child) child_report(BQ_FAULT_OUT_OF_MEMORY);
    stop("out_of_memory");
}
void __bennett_effect(char byte) {
    if (in_child) child_report(BQ_FAULT_EFFECT_IN_APPLY);
    putchar(byte);
}
void __bennett_exit(int status) {
    if (in_child) child_report(BQ_FAULT_EXIT_IN_APPLY);
    exit(status);
}

/* ---- apply --------------------------------------------------------------------------------------------- */

struct field_info { unsigned pos[STANDIN_QUBITS], bits; uint64_t offset; bool out; };

static char *apply_stack;                       /* the lowest byte of the applied function's stack */
static bq_fn *child_f;
static void *child_block;
static ucontext_t child_main, child_apply;

static void on_segv(int sig, siginfo_t *info, void *context) {
    (void)sig;
    (void)context;
    char *addr = info->si_addr;
    if ((uintptr_t)addr >= vdso && (uintptr_t)addr < vdso_end) child_end(LIMIT_SYSTEM_CALL, UINT64_MAX);
    bool stack = apply_stack && addr >= apply_stack - STANDIN_GUARD && addr < apply_stack + STANDIN_APPLY_STACK;
    child_report(stack ? BQ_FAULT_STACK_OVERFLOW : BQ_FAULT_OUT_OF_BOUNDS);
}
static void on_fpe(int sig) { (void)sig; child_report(BQ_FAULT_DIVIDE_BY_ZERO); }
static void on_sigsys(int sig, siginfo_t *info, void *context) {
    (void)sig;
    (void)context;
    child_end(LIMIT_SYSTEM_CALL, (uint64_t)info->si_syscall);
}

/* From here on the child may make only the system calls that map or unmap anonymous memory or end the
 * process; any other gives SIGSYS (seccomp's trap). */
static void filter_system_calls(void) {
    enum { ARGS_FD = offsetof(struct seccomp_data, args[4]) };   /* mmap's descriptor, low half first */
    struct sock_filter program[] = {
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS, offsetof(struct seccomp_data, arch)),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, STANDIN_AUDIT_ARCH, 0, 11),            /* else trap */
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS, offsetof(struct seccomp_data, nr)),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, SYS_mmap, 0, 2),
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS, ARGS_FD),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, 0xFFFFFFFFu, 8, 7),                    /* fd -1: allow */
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, SYS_brk, 7, 0),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, SYS_munmap, 6, 0),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, SYS_mremap, 5, 0),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, SYS_madvise, 4, 0),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, SYS_mprotect, 3, 0),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, SYS_exit, 2, 0),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, SYS_exit_group, 1, 0),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_TRAP),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
    };
    struct sock_fprog fprog = { .len = sizeof program / sizeof program[0], .filter = program };
    if (prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 || syscall(SYS_seccomp, SECCOMP_SET_MODE_FILTER, 0, &fprog) != 0)
        child_report(BQ_FAULT_OUT_OF_MEMORY);
}

static const struct field_info *child_fi;
static uint64_t child_m;

/* f on its own stack, under the filter; then the out values into the shared page. Entering f is the first
 * step. It never returns (the context has no successor). */
static void call_applied(void) {
    filter_system_calls();
    counting = true;
    if (++shared->steps > step_limit) child_report(BQ_FAULT_BUDGET);
    child_f(child_block);
    counting = false;
    const unsigned char *block = child_block;
    unsigned o = 0;
    for (uint64_t k = 0; k < child_m; k++) {
        if (!child_fi[k].out) continue;
        uint64_t v = 0;
        for (unsigned b = 0; b < (child_fi[k].bits + 7) / 8; b++)
            v |= (uint64_t)block[child_fi[k].offset + b] << (8 * b);
        shared->outs[o++] = v & (child_fi[k].bits == 64 ? ~0ull : (1ull << child_fi[k].bits) - 1);
    }
    child_end(RETURNED, 0);
}

/* The child: build the block, run f with the step bound and the filter. */
static _Noreturn void run_child(bq_fn *f, uint64_t budget, const void *image, uint64_t size,
                                const struct field_info *fi, uint64_t m, uint64_t index) {
    in_child = true;
    /* Standard output and error unbuffered, standard input without buffered bytes: stdio on them reaches
     * a system call. The parent flushed the two outputs before the fork, so nothing is written here. */
    setvbuf(stdout, NULL, _IONBF, 0);
    setvbuf(stderr, NULL, _IONBF, 0);
    __fpurge(stdin);
    signal(SIGFPE, on_fpe);
    static char alt[STANDIN_ALT_STACK];
    stack_t ss = { .ss_sp = alt, .ss_size = sizeof alt };
    sigaltstack(&ss, NULL);
    struct sigaction sa = { .sa_sigaction = on_segv, .sa_flags = SA_SIGINFO | SA_ONSTACK };
    sigaction(SIGSEGV, &sa, NULL);
    sigaction(SIGBUS, &sa, NULL);
    struct sigaction sys = { .sa_sigaction = on_sigsys, .sa_flags = SA_SIGINFO | SA_ONSTACK };
    sigaction(SIGSYS, &sys, NULL);

    char *region = mmap(NULL, STANDIN_GUARD + STANDIN_APPLY_STACK, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    unsigned char *block = mmap(NULL, size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (region == MAP_FAILED || block == MAP_FAILED ||
        mprotect(region + STANDIN_GUARD, STANDIN_APPLY_STACK, PROT_READ | PROT_WRITE) != 0)
        child_report(BQ_FAULT_OUT_OF_MEMORY);
    apply_stack = region + STANDIN_GUARD;
    memcpy(block, image, size);
    for (uint64_t k = 0; k < m; k++) {          /* in fields: least significant byte first, zero padded */
        if (fi[k].out) continue;
        uint64_t v = gather(index, fi[k].pos, fi[k].bits);
        for (unsigned b = 0; b < (fi[k].bits + 7) / 8; b++) block[fi[k].offset + b] = (unsigned char)(v >> (8 * b));
    }
    child_f = f;
    child_block = block;
    child_fi = fi;
    child_m = m;
    getcontext(&child_apply);
    child_apply.uc_stack.ss_sp = apply_stack;
    child_apply.uc_stack.ss_size = STANDIN_APPLY_STACK;
    child_apply.uc_link = NULL;
    makecontext(&child_apply, call_applied, 0);
    step_limit = budget;
    if (vdso && munmap((void *)vdso, vdso_end - vdso) != 0) child_report(BQ_FAULT_OUT_OF_MEMORY);
    swapcontext(&child_main, &child_apply);
    _exit(0);                                   /* not reached */
}

/* The names of the system calls a C library function most often makes; others are given by number. */
static void name_system_call(uint64_t nr, char *name, size_t size) {
    static const struct { long nr; const char *name; } names[] = {
        { SYS_read, "read" }, { SYS_write, "write" }, { SYS_openat, "openat" }, { SYS_close, "close" },
        { SYS_lseek, "lseek" }, { SYS_readv, "readv" }, { SYS_writev, "writev" }, { SYS_pread64, "pread64" },
        { SYS_pwrite64, "pwrite64" }, { SYS_ioctl, "ioctl" }, { SYS_fstat, "fstat" }, { SYS_newfstatat, "newfstatat" },
        { SYS_dup, "dup" }, { SYS_dup3, "dup3" }, { SYS_fcntl, "fcntl" }, { SYS_rt_sigaction, "rt_sigaction" },
        { SYS_rt_sigprocmask, "rt_sigprocmask" }, { SYS_tgkill, "tgkill" }, { SYS_getpid, "getpid" },
        { SYS_gettid, "gettid" }, { SYS_clock_gettime, "clock_gettime" }, { SYS_getrandom, "getrandom" },
        { SYS_futex, "futex" },
#ifdef SYS_open
        { SYS_open, "open" }, { SYS_dup2, "dup2" },
#endif
    };
    if (nr == UINT64_MAX) { snprintf(name, size, "vdso"); return; }
    for (size_t k = 0; k < sizeof names / sizeof names[0]; k++)
        if ((uint64_t)names[k].nr == nr) { snprintf(name, size, "%s", names[k].name); return; }
    snprintf(name, size, "number %llu", (unsigned long long)nr);
}

/* G on the input carried by basis index `index`: returns the fault code (0: none); the steps used go to *used.
 * A child that went outside what the stand-in models ends the run with a limit. */
static uint8_t evaluate(bq_fn *f, uint64_t budget, const void *image, uint64_t size, const struct field_info *fi,
                        uint64_t m, uint64_t index, uint64_t *outs, unsigned nout, uint64_t *used) {
    fflush(stdout);
    fflush(stderr);
    memset(shared, 0, sizeof *shared);
    pid_t pid = fork();
    if (pid < 0) limit("process", "bq_apply");
    if (pid == 0) run_child(f, budget, image, size, fi, m, index);
    int status;
    if (waitpid(pid, &status, 0) != pid) limit("process", "bq_apply");
    *used = shared->steps;
    char name[64];
    switch ((enum ending)shared->ending) {
    case RETURNED:
        memcpy(outs, shared->outs, nout * sizeof *outs);
        return 0;
    case FAULTED:
        return (uint8_t)shared->detail;
    case LIMIT_SYSTEM_CALL:
        name_system_call(shared->detail, name, sizeof name);
        fflush(stdout);
        fprintf(stderr, "bq: limit system call %s in bq_apply\n", name);
        exit(STANDIN_FAULT_STATUS);
    case ENDED_SILENTLY:
        break;
    }
    if (WIFSIGNALED(status)) return WTERMSIG(status) == SIGKILL ? BQ_FAULT_OUT_OF_MEMORY : BQ_FAULT_OUT_OF_BOUNDS;
    return BQ_FAULT_EXIT_IN_APPLY;              /* the process ended without a result (approximate) */
}

static bool is_primitive(bq_fn *f) {
    void (*g)(void) = (void (*)(void))f;
    return g == (void (*)(void))bq_alloc || g == (void (*)(void))bq_free || g == (void (*)(void))bq_width ||
           g == (void (*)(void))bq_measure || g == (void (*)(void))bq_gate || g == (void (*)(void))bq_rotate ||
           g == (void (*)(void))bq_apply || g == (void (*)(void))__bennett_effect ||
           g == (void (*)(void))__bennett_exit || g == (void (*)(void))__bennett_out_of_memory ||
           g == (void (*)(void))__bennett_bad_argument;
}

/* Whether f is a function entry of the program's instrumented code. */
static bool known_entry(bq_fn *f) {
    for (size_t k = 0; k < npc_tables; k++)
        for (const uintptr_t *pc = pc_tables[k].begin; pc + 1 < pc_tables[k].end; pc += 2)
            if ((pc[1] & 1) && pc[0] == (uintptr_t)f) return true;
    return false;
}

/* A shared mapping of the program (other than the stand-in's page) at an apply is the limit `shared memory`. */
static void no_shared_mappings(const char *call) {
    FILE *maps = fopen("/proc/self/maps", "r");
    if (!maps) limit("shared memory", call);
    char line[512];
    unsigned long low, high;
    char perms[8];
    bool found = false;
    while (fgets(line, sizeof line, maps))
        if (sscanf(line, "%lx-%lx %7s", &low, &high, perms) == 3 && perms[3] == 's' && low != (uintptr_t)shared)
            found = true;
    fclose(maps);
    if (found) limit("shared memory", call);
}

void bq_apply(bq_fn *f, uint64_t budget, bq_reg fault_reg, const void *image, uint64_t size,
              const bq_field *fields, uint64_t m, const bq_bit *controls, uint64_t n) {
    ENTER();
    const char *call = "bq_apply";
    /* The checks of 6.2, in order. */
    if (!f || is_primitive(f)) fault("bad_function", call);
    if (!known_entry(f)) limit("function", call);
    struct reg *fr = live(fault_reg, call);
    if (fr->width < BQ_FAULT_BITS) fault("bad_range", call);
    if (size < 1 || size > STANDIN_MAX_BLOCK || m > UINT64_MAX / sizeof *fields || n > UINT64_MAX / sizeof *controls)
        fault("bad_argument", call);
    if (!readable(image, size) || !readable(fields, m * sizeof *fields)) fault("bad_argument", call);
    uint64_t cmask = control_mask(controls, n, call);
    struct field_info *fi = room(NULL, m ? m : 1, sizeof *fi, call);
    uint64_t inmask = 0, outmask = 0;
    unsigned nout = 0;
    for (uint64_t k = 0; k < m; k++) {
        const bq_field *d = &fields[k];
        if (d->direction != BQ_DIR_IN && d->direction != BQ_DIR_OUT) fault("bad_argument", call);
        struct reg *g = allocated(d->slice.reg);
        if (!g) fault("bad_argument", call);
        uint64_t bits = d->slice.bits, low = d->slice.low, bytes = (bits + 7) / 8;
        if (bits < 1 || bits > g->width || low > g->width - bits) fault("bad_argument", call);
        if (bytes > size || d->offset > size - bytes) fault("bad_argument", call);
        fi[k].bits = (unsigned)bits;
        fi[k].offset = d->offset;
        fi[k].out = d->direction == BQ_DIR_OUT;
        uint64_t mask = 0;
        for (unsigned b = 0; b < bits; b++) { fi[k].pos[b] = g->pos[low + b]; mask |= 1ull << fi[k].pos[b]; }
        if (fi[k].out) {
            if (outmask & mask) fault("bad_argument", call);
            outmask |= mask;
            nout++;
        } else {
            inmask |= mask;
        }
    }
    for (uint64_t j = 0; j < m; j++)            /* no two fields' bytes overlap */
        for (uint64_t k = j + 1; k < m; k++) {
            uint64_t a = fi[j].offset, ae = a + (fi[j].bits + 7) / 8, b = fi[k].offset, be = b + (fi[k].bits + 7) / 8;
            if (a < be && b < ae) fault("bad_argument", call);
        }
    uint64_t faultmask = 0;
    for (unsigned b = 0; b < BQ_FAULT_BITS; b++) faultmask |= 1ull << fr->pos[b];
    if ((outmask & inmask) || (faultmask & (inmask | outmask)) || (cmask & (outmask | faultmask)))
        fault("bad_argument", call);
    no_shared_mappings(call);

    /* G on every input that has a nonzero amplitude where every control is 1. */
    unsigned inpos[STANDIN_QUBITS], kin = 0;
    for (unsigned p = 0; p < nq; p++) if ((inmask >> p) & 1) inpos[kin++] = p;
    uint64_t keys = 1ull << kin, most = 0, ran_out = 0, inputs = 0;
    uint8_t *done = room(NULL, keys, 1, call), *code = room(NULL, keys, 1, call);
    uint64_t *outs = room(NULL, keys, (nout ? nout : 1) * sizeof *outs, call);
    memset(done, 0, keys);
    memset(code, 0, keys);
    for (uint64_t i = 0; i < (1ull << nq); i++) {
        if ((i & cmask) != cmask || psi[i] == 0) continue;
        uint64_t key = gather(i, inpos, kin);
        if (done[key]) continue;
        done[key] = 1;
        inputs++;
        uint64_t used;
        code[key] = evaluate(f, budget, image, size, fi, m, i, outs + key * nout, nout, &used);
        if (code[key] == BQ_FAULT_BUDGET) ran_out++;
        else if (used > most) most = used;
    }
    fprintf(stderr, "bq: apply %llu budget %llu inputs %llu steps %llu out-of-budget %llu\n",
            (unsigned long long)++applies, (unsigned long long)budget, (unsigned long long)inputs,
            (unsigned long long)most, (unsigned long long)ran_out);
    /* The unitary |x>|o>|e> -> |x>|o ^ out(G(x))>|e ^ code(G(x))>, an involution: swap pairs. */
    for (uint64_t i = 0; i < (1ull << nq); i++) {
        if ((i & cmask) != cmask) continue;
        uint64_t key = gather(i, inpos, kin);
        if (!done[key]) continue;
        uint64_t flip = 0;
        if (code[key]) {
            flip = scatter(code[key], fr->pos, BQ_FAULT_BITS);
        } else {
            unsigned o = 0;
            for (uint64_t k = 0; k < m; k++)
                if (fi[k].out) flip |= scatter(outs[key * nout + o++], fi[k].pos, fi[k].bits);
        }
        uint64_t j = i ^ flip;
        if (j > i) { amp t = psi[i]; psi[i] = psi[j]; psi[j] = t; }
    }
    free(done);
    free(code);
    free(outs);
    free(fi);
}
