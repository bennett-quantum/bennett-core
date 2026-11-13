/* Driver: classical setup, then one region per branch r in a forked child.
 *
 *   driver N QUERY OUTDIR
 *
 * QUERY is one of arith, like_name, like_note, scan (see QUERIES below).
 * Build with -DTRACE (linked against the rewritten SQLite + hooks.c) or
 * -DPERF (plain SQLite; counts x86 user instructions with perf_event_open).
 *
 * The region is exactly: bind ?1, one sqlite3_step, read column 0 if a row
 * came back, sqlite3_reset. The child runs it once from the parent's state;
 * the parent is never modified by a region.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <unistd.h>
#ifdef PERF
#include <linux/perf_event.h>
#include <sched.h>
/* Hybrid CPUs expose one PMU per core type; count on the P-core PMU and pin there. */
static void perf_target(unsigned long long *config, int *cpu) {
  *config = PERF_COUNT_HW_INSTRUCTIONS; *cpu = -1;
  FILE *f = fopen("/sys/bus/event_source/devices/cpu_core/type", "r");
  if (!f) return;                                  /* not hybrid: plain hardware event */
  unsigned long long t = 0;
  if (fscanf(f, "%llu", &t) == 1) *config |= t << 32;  /* PERF_PMU_TYPE_SHIFT */
  fclose(f);
  f = fopen("/sys/bus/event_source/devices/cpu_core/cpus", "r");
  if (f && fscanf(f, "%d", cpu) != 1) *cpu = -1;
  if (f) fclose(f);
}
#endif
#include "sqlite3.h"

#ifdef TRACE
void __tr_begin(void);
void __tr_end(const char *dir, int r, long long result, int rc);
#endif

static const struct { const char *name, *sql; int bind_offset; } QUERIES[] = {
  /* arithmetic predicate on one row fetched through the rowid B-tree */
  {"arith", "SELECT (a * 7 + b) % 13 = 5 FROM t WHERE id = ?1", 1},
  /* text predicate on a short text column of one row fetched by rowid */
  {"like_name", "SELECT name LIKE '%qu%' FROM t WHERE id = ?1", 1},
  /* text predicate on a long, variable-length column (some rows overflow) */
  {"like_note", "SELECT note LIKE '%zq%' FROM t WHERE id = ?1", 1},
  /* full table scan, parameter in the predicate, no index on a */
  {"scan", "SELECT count(*) FROM t WHERE a = ?1", 0},
};

static uint64_t lcg_state = 0x2545F4914F6CDD1Dull;
static uint32_t rnd(void) {
  lcg_state = lcg_state * 6364136223846793005ull + 1442695040888963407ull;
  return (uint32_t)(lcg_state >> 33);
}

static void rand_text(char *buf, int len) {
  for (int i = 0; i < len; i++) buf[i] = (rnd() % 6 == 0 && i && buf[i - 1] != ' ') ? ' ' : (char)('a' + rnd() % 26);
  buf[len] = 0;
}

static void check(int rc, sqlite3 *db, const char *what) {
  if (rc != SQLITE_OK && rc != SQLITE_DONE && rc != SQLITE_ROW) {
    fprintf(stderr, "%s: %s\n", what, db ? sqlite3_errmsg(db) : sqlite3_errstr(rc));
    exit(2);
  }
}

static int region(sqlite3_stmt *st, long long param, long long *out) {
  sqlite3_bind_int64(st, 1, param);
  int rc = sqlite3_step(st);
  if (rc == SQLITE_ROW) *out = sqlite3_column_int64(st, 0);
  sqlite3_reset(st);
  return rc;
}

int main(int argc, char **argv) {
  if (argc != 4) { fprintf(stderr, "usage: %s N QUERY OUTDIR\n", argv[0]); return 1; }
  int N = atoi(argv[1]);
  const char *qname = argv[2], *dir = argv[3];
  int q = -1;
  for (unsigned i = 0; i < sizeof QUERIES / sizeof *QUERIES; i++)
    if (!strcmp(QUERIES[i].name, qname)) q = (int)i;
  if (q < 0) { fprintf(stderr, "unknown query %s\n", qname); return 1; }

  /* All SQLite memory comes from one memsys5 heap (allocator code is traced too). */
  size_t heap_bytes = (size_t)64 << 20;
  void *heap = malloc(heap_bytes);
  check(sqlite3_config(SQLITE_CONFIG_HEAP, heap, (int)heap_bytes, 32), 0, "config heap");
  check(sqlite3_initialize(), 0, "init");

  sqlite3 *db;
  check(sqlite3_open(":memory:", &db), db, "open");
  check(sqlite3_exec(db, "CREATE TABLE t(id INTEGER PRIMARY KEY, a INTEGER, b INTEGER, name TEXT, note TEXT)", 0, 0, 0), db, "create");
  check(sqlite3_exec(db, "BEGIN", 0, 0, 0), db, "begin");
  sqlite3_stmt *ins;
  check(sqlite3_prepare_v2(db, "INSERT INTO t VALUES(?1,?2,?3,?4,?5)", -1, &ins, 0), db, "prep ins");
  static char name[64], note[16384];
  for (int id = 1; id <= N; id++) {
    int nlen = 4 + (int)(rnd() % 37);                  /* 4..40 */
    int long_note = rnd() % 32 == 0;                   /* ~3% of rows overflow a 4 KiB page */
    int tlen = long_note ? 4000 + (int)(rnd() % 8000) : 20 + (int)(rnd() % 380);
    rand_text(name, nlen);
    rand_text(note, tlen);
    sqlite3_bind_int64(ins, 1, id);
    sqlite3_bind_int64(ins, 2, rnd() % (uint32_t)N);
    sqlite3_bind_int64(ins, 3, rnd() % 1000);
    sqlite3_bind_text(ins, 4, name, nlen, SQLITE_STATIC);
    sqlite3_bind_text(ins, 5, note, tlen, SQLITE_STATIC);
    check(sqlite3_step(ins), db, "insert");
    sqlite3_reset(ins);
  }
  sqlite3_finalize(ins);
  check(sqlite3_exec(db, "COMMIT", 0, 0, 0), db, "commit");

  /* per-row properties for later correlation */
  char path[4096];
  snprintf(path, sizeof path, "%s/rows.tsv", dir);
  FILE *rows = fopen(path, "w");
  fprintf(rows, "id\ta\tb\tname_len\tnote_len\tname_qu_at\tnote_zq_at\n");
  sqlite3_stmt *rs;
  check(sqlite3_prepare_v2(db, "SELECT id, a, b, length(name), length(note), instr(name, 'qu'), instr(note, 'zq') FROM t ORDER BY id", -1, &rs, 0), db, "prep rows");
  while (sqlite3_step(rs) == SQLITE_ROW)
    fprintf(rows, "%lld\t%lld\t%lld\t%lld\t%lld\t%lld\t%lld\n", sqlite3_column_int64(rs, 0), sqlite3_column_int64(rs, 1),
            sqlite3_column_int64(rs, 2), sqlite3_column_int64(rs, 3), sqlite3_column_int64(rs, 4),
            sqlite3_column_int64(rs, 5), sqlite3_column_int64(rs, 6));
  sqlite3_finalize(rs);
  fclose(rows);

  sqlite3_stmt *st;
  check(sqlite3_prepare_v2(db, QUERIES[q].sql, -1, &st, 0), db, "prepare");
  long long warm = 0;
  region(st, QUERIES[q].bind_offset, &warm);          /* warm-up with r = 0; classical setup */

  /* memory map of the parent at the start state */
  snprintf(path, sizeof path, "%s/meta.txt", dir);
  FILE *meta = fopen(path, "w");
  int local_marker;
  fprintf(meta, "heap %p %p\nstack_marker %p\nquery %s\nsql %s\nN %d\n", heap, (char *)heap + heap_bytes,
          (void *)&local_marker, qname, QUERIES[q].sql, N);
  sqlite3_int64 cur, hi;
  sqlite3_status64(SQLITE_STATUS_MEMORY_USED, &cur, &hi, 0);
  fprintf(meta, "sqlite_memory_used %lld\n", (long long)cur);
  FILE *maps = fopen("/proc/self/maps", "r");
  char line[4096];
  while (fgets(line, sizeof line, maps)) fprintf(meta, "map %s", line);
  fclose(maps);
  FILE *status = fopen("/proc/self/status", "r");
  while (fgets(line, sizeof line, status))
    if (!strncmp(line, "Vm", 2) || !strncmp(line, "Rss", 3)) fprintf(meta, "status %s", line);
  fclose(status);
  fclose(meta);

#ifdef PERF
  snprintf(path, sizeof path, "%s/perf.tsv", dir);
  FILE *pf = fopen(path, "w");
  fprintf(pf, "r\trc\tresult\tx86_user_instructions\n");
  fflush(pf);
#endif
  for (int r = 0; r < N; r++) {
    fflush(0);
    pid_t pid = fork();
    if (pid < 0) { perror("fork"); return 2; }
    if (pid == 0) {
      long long result = -1;
      long long param = r + QUERIES[q].bind_offset;
#ifdef TRACE
      __tr_begin();
      int rc = region(st, param, &result);
      __tr_end(dir, r, result, rc);
#endif
#ifdef PERF
      struct perf_event_attr pa;
      memset(&pa, 0, sizeof pa);
      pa.size = sizeof pa;
      pa.type = PERF_TYPE_HARDWARE;
      unsigned long long cfg; int cpu;
      perf_target(&cfg, &cpu);
      if (cpu >= 0) {
        cpu_set_t cs; CPU_ZERO(&cs); CPU_SET(cpu, &cs);
        if (sched_setaffinity(0, sizeof cs, &cs)) { perror("sched_setaffinity"); _exit(6); }
      }
      pa.config = cfg;
      pa.disabled = 1;
      pa.exclude_kernel = 1;
      pa.exclude_hv = 1;
      int fd = (int)syscall(SYS_perf_event_open, &pa, 0, -1, -1, 0);
      if (fd < 0) { perror("perf_event_open"); _exit(4); }
      ioctl(fd, PERF_EVENT_IOC_RESET, 0);
      ioctl(fd, PERF_EVENT_IOC_ENABLE, 0);
      int rc = region(st, param, &result);
      ioctl(fd, PERF_EVENT_IOC_DISABLE, 0);
      long long count = 0;
      if (read(fd, &count, sizeof count) != sizeof count) _exit(5);
      fprintf(pf, "%d\t%d\t%lld\t%lld\n", r, rc, result, count);
      fflush(pf);
#endif
      _exit(0);
    }
    int ws;
    if (waitpid(pid, &ws, 0) < 0 || !WIFEXITED(ws) || WEXITSTATUS(ws) != 0) {
      fprintf(stderr, "branch %d failed (status %d)\n", r, ws);
      return 2;
    }
  }
  return 0;
}
