/* SQLite OS_OTHER shim: empty namespace, deterministic entropy and time.
 * No file can be opened; the :memory: database uses SQLite's own pager. */
#include "sqlite3.h"
#include <string.h>
static unsigned sod_random_calls, sod_clock_calls, sod_open_calls;
static int sod_open(sqlite3_vfs *v, const char *p, sqlite3_file *f, int flags, int *out) {
  (void)v; (void)p; (void)f; (void)flags; (void)out;
  ++sod_open_calls;
  return SQLITE_CANTOPEN;
}
static int sod_delete(sqlite3_vfs *v, const char *p, int sync) {
  (void)v; (void)p; (void)sync; return SQLITE_IOERR_DELETE_NOENT;
}
static int sod_access(sqlite3_vfs *v, const char *p, int flags, int *out) {
  (void)v; (void)p; (void)flags; *out = 0; return SQLITE_OK;
}
static int sod_fullpath(sqlite3_vfs *v, const char *p, int n, char *out) {
  (void)v;
  if ((int)strlen(p) >= n) return SQLITE_CANTOPEN;
  strcpy(out, p); return SQLITE_OK;
}
static int sod_random(sqlite3_vfs *v, int n, char *out) {
  (void)v; ++sod_random_calls;
  for (int i = 0; i < n; ++i) out[i] = (char)(i * 37 + 11);
  return n;
}
static int sod_sleep(sqlite3_vfs *v, int n) { (void)v; (void)n; return 0; }
static int sod_clock(sqlite3_vfs *v, double *out) {
  (void)v; ++sod_clock_calls; *out = 2440587.5; return SQLITE_OK;
}
static sqlite3_vfs sod_vfs = {
  .iVersion = 1, .szOsFile = sizeof(sqlite3_file), .mxPathname = 512,
  .zName = "experiment-no-files", .xOpen = sod_open, .xDelete = sod_delete,
  .xAccess = sod_access, .xFullPathname = sod_fullpath, .xRandomness = sod_random,
  .xSleep = sod_sleep, .xCurrentTime = sod_clock
};
int sqlite3_os_init(void) { return sqlite3_vfs_register(&sod_vfs, 1); }
int sqlite3_os_end(void) { return SQLITE_OK; }
