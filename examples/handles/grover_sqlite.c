/* grover_sqlite.c: Grover search over a SQL query, answered by unmodified SQLite.
 *
 * An in-memory table holds 16 elementary particles, one row per id from 0 to 15. The question "is particle
 * `id` electrically neutral and heavier than 100 GeV?" is a prepared SQL statement. The applied function
 * binds the id, steps the statement and stores the answer: ordinary C calling ordinary SQLite, with the
 * prepared statement as ordinary data in the block. The id is a 4-qubit register in superposition. The
 * oracle computes the answer into a flag qubit, applies Z to the flag and computes the answer again, which
 * returns the flag to 0. Exactly one row matches, so 3 iterations find it with probability 0.96. The name
 * of the particle found is then looked up classically. */
#include "grover.h"
#include "sqlite3.h"
#include <math.h>

#define ID_BITS 4

static const char *const create_table =
    "CREATE TABLE particle(id INTEGER PRIMARY KEY, name TEXT, charge_thirds INTEGER, mass_mev REAL);"
    "INSERT INTO particle VALUES"
    " (0, 'electron', -3, 0.511), (1, 'muon', -3, 105.7), (2, 'tau', -3, 1776.9),"
    " (3, 'electron neutrino', 0, 0), (4, 'muon neutrino', 0, 0), (5, 'up quark', 2, 2.2),"
    " (6, 'down quark', -1, 4.7), (7, 'charm quark', 2, 1270), (8, 'strange quark', -1, 93),"
    " (9, 'top quark', 2, 172690), (10, 'bottom quark', -1, 4180), (11, 'photon', 0, 0),"
    " (12, 'gluon', 0, 0), (13, 'W boson', 3, 80377), (14, 'Z boson', 0, 91188),"
    " (15, 'Higgs boson', 0, 125250);";

static const char *const question = "SELECT charge_thirds = 0 AND mass_mev > 100000 FROM particle WHERE id = ?1";

/* ---- the classical part: plain C and SQLite ---------------------------------------------------------- */

struct query { sqlite3_stmt *statement; uint64_t id, match; };

static void neutral_and_heavy(void *block) {
    struct query *q = block;
    sqlite3_bind_int(q->statement, 1, (int)q->id);
    q->match = sqlite3_step(q->statement) == SQLITE_ROW && sqlite3_column_int(q->statement, 0);
    sqlite3_reset(q->statement);
}

static void check(int status, sqlite3 *db) {
    if (status != SQLITE_OK) { printf("sqlite: %s\n", sqlite3_errmsg(db)); exit(2); }
}

/* ---- the quantum part: the oracle; the diffusion step is in grover.h ------------------------------------- */

/* |id> to (-1)^match(id) |id>: compute match into the flag, Z, compute it again. */
static void oracle(bq_reg id, bq_reg flag, const struct query *image) {
    bq_field fields[] = { BQ_IN_ALL(id, struct query, id), BQ_OUT_ALL(flag, struct query, match) };
    bq_apply_scoped(neutral_and_heavy, BQ_BUDGET(query), bq_fault(), image, sizeof *image, fields, 2);
    bq_z(flag, 0);
    bq_apply_scoped(neutral_and_heavy, BQ_BUDGET(query), bq_fault(), image, sizeof *image, fields, 2);
    bq_faults_check("query");
}

int main(void) {
    sqlite3 *db;
    int opened = sqlite3_open(":memory:", &db);     /* db is set before check reads it */
    check(opened, db);
    check(sqlite3_exec(db, create_table, 0, 0, 0), db);
    struct query image = { 0 };
    check(sqlite3_prepare_v2(db, question, -1, &image.statement, 0), db);

    const double matches = 1, rows = 1 << ID_BITS;
    const int iterations = (int)round(M_PI / (4 * asin(sqrt(matches / rows))) - 0.5);

    bq_reg id = bq_alloc(ID_BITS), flag = bq_alloc(1);
    hadamard_all(id);
    for (int k = 0; k < iterations; k++) {
        oracle(id, flag, &image);
        diffusion(id);
    }
    uint64_t found = bq_measure(id, 0, ID_BITS);
    bq_free(id);
    bq_free(flag);

    sqlite3_stmt *name;
    check(sqlite3_prepare_v2(db, "SELECT name FROM particle WHERE id = ?1", -1, &name, 0), db);
    sqlite3_bind_int(name, 1, (int)found);
    sqlite3_step(name);
    image.id = found;
    neutral_and_heavy(&image);
    printf("iterations %d, measured id %llu: %s, matches = %llu\n", iterations, (unsigned long long)found,
           (const char *)sqlite3_column_text(name, 0), (unsigned long long)image.match);
    sqlite3_finalize(name);
    sqlite3_finalize(image.statement);
    sqlite3_close(db);
    return image.match ? 0 : 1;
}
