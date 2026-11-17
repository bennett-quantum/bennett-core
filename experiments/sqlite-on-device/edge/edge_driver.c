/* Edge rows for the SQL oracle: NULLs, duplicates, 64-bit boundaries, empty and
 * uppercase text, LIKE metacharacters, an all-rows and a no-rows predicate, and an
 * integer-overflow error. Same experiment_setup/experiment_query interface as
 * ../driver.c, so ../scale.py's generated device entries apply unchanged. */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#define NROWS 8
#ifndef HEAP_BYTES
#define HEAP_BYTES (256 * 1024)
#endif
#include "vfs.c"
static const char *sod_names[] = {
  "eq_null", "any_null", "text_order", "like_case", "duplicate_a", "all_rows",
  "no_rows", "two_where", "add_overflow", "range_agg", "real_compare", "abs_error"};
static const char *sod_sql[] = {
  "SELECT a=3 FROM e WHERE id=?1",
  "SELECT a IS NULL OR b IS NULL OR name IS NULL OR note IS NULL FROM e WHERE id=?1",
  "SELECT name<'m' FROM e WHERE id=?1",
  "SELECT note LIKE '%zq%' FROM e WHERE id=?1",
  "SELECT (SELECT count(*) FROM e AS x WHERE x.a=e.a)>1 FROM e WHERE id=?1",
  "SELECT b IS NULL OR b>=-9223372036854775808 FROM e WHERE id=?1",
  "SELECT coalesce(b>9223372036854775807,0) FROM e WHERE id=?1",
  "SELECT EXISTS(SELECT 1 FROM e WHERE id=?1 AND a<8 AND b>=500)",
  "SELECT a+1>a FROM e WHERE id=?1",
  "SELECT count(*)>0 FROM e WHERE a BETWEEN ?1 AND ?1+1",
  "SELECT CAST(b AS REAL)=b FROM e WHERE id=?1",
  "SELECT abs(b)>=0 FROM e WHERE id=?1"};
#define SOD_QUERIES (sizeof sod_sql / sizeof sod_sql[0])
#define SOD_RANGE 9
/* No row is distinct from every negated SQLite result code. */
#define SOD_NO_ROW (-1000)
static sqlite3 *sod_db;
static sqlite3_stmt *sod_st[SOD_QUERIES];
static _Alignas(16) unsigned char sod_heap[HEAP_BYTES];
/* A failed call reports its code as a negative result; it never exits. */
#define SOD_TRY(x) do { int rc_ = (x); if (rc_ != SQLITE_OK && rc_ != SQLITE_DONE && rc_ != SQLITE_ROW) return -rc_; } while (0)
static int sod_null(sqlite3_stmt *s, int i) { return sqlite3_bind_null(s, i); }
__attribute__((noinline)) int experiment_query(int q, int r) {
  sqlite3_stmt *st = sod_st[q];
  SOD_TRY(sqlite3_bind_int64(st, 1, r + (q != SOD_RANGE)));
  int rc = sqlite3_step(st);
  int result = rc == SQLITE_ROW ? (int)sqlite3_column_int64(st, 0) : rc == SQLITE_DONE ? SOD_NO_ROW : -rc;
  int reset = sqlite3_reset(st);
  if (result > SOD_NO_ROW && reset != SQLITE_OK) return -reset;
  return result;
}
struct sod_row { int64_t a, b; int a_null, b_null; const char *name, *note; };
static const struct sod_row sod_rows[NROWS] = {
  {3, INT64_MAX, 0, 0, "m", "ZQ upper"},
  {0, INT64_MIN, 1, 0, "", 0},
  {3, 0, 0, 0, "M", "a%zq_b"},
  {-1, 500, 0, 0, 0, "zzq"},
  {-1, 499, 0, 0, "l\xc3\xbf", "z q"},
  {3, -500, 0, 0, "ma", ""},
  {8, 1000, 0, 0, "lzzzz", "zQ"},
  {INT64_MAX, 0, 0, 1, "a", "q z"},
};
__attribute__((noinline)) int experiment_setup(void) {
  SOD_TRY(sqlite3_config(SQLITE_CONFIG_HEAP, sod_heap, sizeof sod_heap, 32));
  SOD_TRY(sqlite3_initialize());
  SOD_TRY(sqlite3_open(":memory:", &sod_db));
  SOD_TRY(sqlite3_exec(sod_db, "CREATE TABLE e(id INTEGER PRIMARY KEY,a INTEGER,b INTEGER,name TEXT,note TEXT)", 0, 0, 0));
  sqlite3_stmt *ins;
  SOD_TRY(sqlite3_prepare_v2(sod_db, "INSERT INTO e VALUES(?1,?2,?3,?4,?5)", -1, &ins, 0));
  for (int i = 0; i < NROWS; ++i) {
    const struct sod_row *w = &sod_rows[i];
    SOD_TRY(sqlite3_bind_int64(ins, 1, i + 1));
    SOD_TRY(w->a_null ? sod_null(ins, 2) : sqlite3_bind_int64(ins, 2, w->a));
    SOD_TRY(w->b_null ? sod_null(ins, 3) : sqlite3_bind_int64(ins, 3, w->b));
    SOD_TRY(w->name ? sqlite3_bind_text(ins, 4, w->name, -1, SQLITE_STATIC) : sod_null(ins, 4));
    SOD_TRY(w->note ? sqlite3_bind_text(ins, 5, w->note, -1, SQLITE_STATIC) : sod_null(ins, 5));
    SOD_TRY(sqlite3_step(ins)); SOD_TRY(sqlite3_reset(ins));
  }
  SOD_TRY(sqlite3_finalize(ins));
  for (int q = 0; q < (int)SOD_QUERIES; ++q) {
    SOD_TRY(sqlite3_prepare_v2(sod_db, sod_sql[q], -1, &sod_st[q], 0));
    experiment_query(q, 0);
  }
  return 0;
}
int main(void) {
  int rc = experiment_setup();
  if (rc) { printf("setup error: %d\n", rc); return 2; }
  for (int q = 0; q < (int)SOD_QUERIES; ++q)
    for (int r = 0; r < NROWS; ++r)
      printf("%s %d %d\n", sod_names[q], r, experiment_query(q, r));
  for (int q = 0; q < (int)SOD_QUERIES; ++q) sqlite3_finalize(sod_st[q]);
  printf("close %d\n", sqlite3_close(sod_db));
  return 0;
}
