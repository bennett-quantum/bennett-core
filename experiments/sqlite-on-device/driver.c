/* Unmodified amalgamation is included by the generated build wrapper. */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#ifndef NROWS
#define NROWS 4
#endif
#ifndef HEAP_BYTES
#define HEAP_BYTES (256 * 1024)
#endif
#include "vfs.c"
static const char *sod_names[] = {"arith", "like_name", "like_note", "scan"};
static const char *sod_sql[] = {
  "SELECT (a*7+b)%13=5 FROM t WHERE id=?1",
  "SELECT name LIKE '%qu%' FROM t WHERE id=?1",
  "SELECT note LIKE '%zq%' FROM t WHERE id=?1",
  "SELECT count(*) FROM t WHERE a=?1"
};
static sqlite3 *sod_db;
static sqlite3_stmt *sod_st[4];
static _Alignas(16) unsigned char sod_heap[HEAP_BYTES];
static uint64_t sod_lcg = UINT64_C(0x2545F4914F6CDD1D);
static uint32_t sod_rnd(void) {
  sod_lcg = sod_lcg * UINT64_C(6364136223846793005) + UINT64_C(1442695040888963407);
  return (uint32_t)(sod_lcg >> 33);
}
static void sod_text(char *buf, int n) {
  for (int i = 0; i < n; ++i)
    buf[i] = (sod_rnd()%6 == 0 && i && buf[i-1] != ' ') ? ' ' : (char)('a'+sod_rnd()%26);
  buf[n] = 0;
}
static void sod_check(int rc) {
  if (rc != SQLITE_OK && rc != SQLITE_DONE && rc != SQLITE_ROW) {
    printf("sqlite error: %d\n", rc); exit(2);
  }
}
/* External names survive lowering for the handwritten device harness. */
__attribute__((noinline)) int experiment_query(int q, int r) {
  sqlite3_stmt *st = sod_st[q];
  sod_check(sqlite3_bind_int64(st, 1, r + (q != 3)));
  int rc = sqlite3_step(st);
  sod_check(rc);
  int result = rc == SQLITE_ROW ? sqlite3_column_int64(st, 0) : -1;
  sod_check(sqlite3_reset(st));
  return result;
}
__attribute__((noinline)) int experiment_setup(void) {
  sod_check(sqlite3_config(SQLITE_CONFIG_HEAP, sod_heap, sizeof sod_heap, 32));
  sod_check(sqlite3_initialize());
  sod_check(sqlite3_open(":memory:", &sod_db));
  sod_check(sqlite3_exec(sod_db, "CREATE TABLE t(id INTEGER PRIMARY KEY,a INTEGER,b INTEGER,name TEXT,note TEXT)", 0, 0, 0));
  sod_check(sqlite3_exec(sod_db, "BEGIN", 0, 0, 0));
  sqlite3_stmt *ins;
  sod_check(sqlite3_prepare_v2(sod_db, "INSERT INTO t VALUES(?1,?2,?3,?4,?5)", -1, &ins, 0));
  static char name[64], note[16384];
  for (int id = 1; id <= NROWS; ++id) {
    int nl = 4 + (int)(sod_rnd()%37);
    int is_long = sod_rnd()%32 == 0;
    int tl = is_long ? 4000+(int)(sod_rnd()%8000) : 20+(int)(sod_rnd()%380);
    sod_text(name, nl); sod_text(note, tl);
    sod_check(sqlite3_bind_int64(ins, 1, id));
    sod_check(sqlite3_bind_int64(ins, 2, sod_rnd()%NROWS));
    sod_check(sqlite3_bind_int64(ins, 3, sod_rnd()%1000));
    sod_check(sqlite3_bind_text(ins, 4, name, nl, SQLITE_STATIC));
    sod_check(sqlite3_bind_text(ins, 5, note, tl, SQLITE_STATIC));
    sod_check(sqlite3_step(ins)); sod_check(sqlite3_reset(ins));
  }
  sod_check(sqlite3_finalize(ins));
  sod_check(sqlite3_exec(sod_db, "COMMIT", 0, 0, 0));
  for (int q = 0; q < 4; ++q) {
    sod_check(sqlite3_prepare_v2(sod_db, sod_sql[q], -1, &sod_st[q], 0));
    experiment_query(q, 0);
  }
  return 0;
}
int main(void) {
  experiment_setup();
  for (int q = 0; q < 4; ++q) {
    for (int r = 0; r < NROWS; ++r)
      printf("%s %d %d\n", sod_names[q], r, experiment_query(q, r));
    printf("memory %s %lld %lld\n", sod_names[q],
           (long long)sqlite3_memory_used(), (long long)sqlite3_memory_highwater(0));
  }
  for (int q = 0; q < 4; ++q) sod_check(sqlite3_finalize(sod_st[q]));
  sod_check(sqlite3_close(sod_db));
  printf("vfs open=%u random=%u clock=%u\n", sod_open_calls, sod_random_calls, sod_clock_calls);
  return 0;
}
