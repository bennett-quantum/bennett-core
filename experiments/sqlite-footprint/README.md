# sqlite-footprint: worst case and superposed addresses on native SQLite

Question: for the project's own target workload (Grover search whose oracle is a SQL query on a fixed
in-memory SQLite database, parameterised by a row id), how far is the slowest branch from the typical one
(n4), and how many memory accesses have an address that differs between branches (n5)?

Every number here is a **dynamic measurement over all N inputs**. It is a lower bound on what a coherent
region must handle: a sound static analysis can only find more variation (more varying accesses, larger
address sets, a larger step bound), never less.

## Method

- **Program.** The SQLite 3.54.1 amalgamation, compiled `-O2` with `SQLITE_THREADSAFE=0`, `SQLITE_ENABLE_MEMSYS5`
  (all SQLite memory comes from one 64 MiB arena handed over with `SQLITE_CONFIG_HEAP`, so the allocator is
  SQLite code and is traced too) and `SQLITE_OMIT_LOAD_EXTENSION`.
- **Classical setup** (`driver.c`). Open `:memory:`; create
  `t(id INTEGER PRIMARY KEY, a INTEGER, b INTEGER, name TEXT, note TEXT)` and insert N rows from a fixed LCG:
  `a` uniform in 0..N-1, `b` in 0..999, `name` 4..40 random letters with spaces, `note` 20..399 characters, except
  about 1 row in 32 whose note is 4,000..11,999 characters and spills into overflow pages (4 KiB pages). Dump
  per-row facts to `rows.tsv`, prepare the query, and run it once (warm-up with the first parameter) so first-run
  allocations are part of the starting state.
- **Region.** Exactly `sqlite3_bind_int64(?1 = r)`, one `sqlite3_step`, `sqlite3_column_int64(0)` if a row came
  back, `sqlite3_reset`. For each r the parent `fork()`s; the child runs the region once with tracing on and
  writes its trace. Every branch therefore starts from the identical parent state.
- **Queries** (one line of justification each):
  - `arith`: `SELECT (a*7+b)%13=5 FROM t WHERE id=?1`. Rowid point lookup plus integer arithmetic: the minimal
    oracle shape; variation comes only from B-tree search and record decoding.
  - `like_name`: `SELECT name LIKE '%qu%' FROM t WHERE id=?1`. Text predicate on a short, variable-length
    column: data-dependent loop length, still small.
  - `like_note`: `SELECT note LIKE '%zq%' FROM t WHERE id=?1`. Text predicate on a long column, some rows in
    overflow pages: the shape where data size drives work.
  - `scan`: `SELECT count(*) FROM t WHERE a=?1`. No index on `a`: a full table scan whose work does not go
    through the rowid index; the parameter only changes predicate outcomes.
  B-tree depth is the same for every row of a table (a B-tree is balanced), so variation in the lookup comes
  from in-page binary search, varint lengths and overflow chains, not from depth.
- **Tracing** (`build.sh`, `rewrite_ir.py`, `hooks.c`). Clang 18 `-fsanitize-coverage=trace-loads,trace-stores`
  (plus `func,trace-pc-guard` to enable the pass) and `-finstrument-functions-after-inlining`, emitted as IR,
  then rewritten by `rewrite_ir.py`: each `__sanitizer_cov_loadN/storeN` becomes a hook with a static site id (one
  per IR instruction), the `i24` loads/stores SanitizerCoverage skips get the same hook, each basic block gets a
  counter hook carrying its IR instruction count, and the guard calls are removed. The rewritten IR is compiled
  with no further optimisation, so the traced code is the `-O2` code. Every IR load and store in SQLite is covered
  (checked: 43,990 IR loads = 43,822 instrumented + 168 `i24`; 21,696 stores = 21,658 + 38).
- **Not instrumented, and how it is handled.** libc routines SQLite calls (`memcpy`, `memmove`, `memset`,
  `memcmp`, `strlen`, `strcmp`, `strncmp`, `memchr`, `strchr`, `strrchr`, `strcspn`, `strspn`) and the LLVM memory
  intrinsics are not instrumented inside. A hook before each call records the bytes it reads and writes (computed
  exactly from its arguments, e.g. the length `strcspn` will scan) as one "memory-routine" record; these are
  reported separately from scalar accesses. Their *instructions* are not in the block or IR counts; the x86 count
  includes them. `malloc` is not involved (memsys5 is SQLite code). Register spills and other stack traffic the
  backend introduces are not IR loads/stores and are not counted.
- **Step counts.** Executed basic blocks and executed LLVM IR instructions (per-block counts from the IR,
  excluding instrumentation) from the traced build; x86 user-mode instructions of the region in an uninstrumented
  build via `perf_event_open` (the host CPU is hybrid, so the child is pinned to a P-core and counts on the
  `cpu_core` PMU). The plain and traced builds produce identical tables and identical per-branch results.
- **Alignment of "the same access" across branches.** Two definitions:
  - **A**: (site, k) where k is the index of this execution of the site within the whole region.
  - **B**: (call path, site, k) where the call path is the chain of (call site, index of that call within its
    caller's frame) and k is counted within the frame instance.
  Both are exact while branches follow the same path. Where paths differ, A shifts every later execution of a site
  whose earlier count differed, so it over-counts "varying"; B confines the shift to the frame where the paths
  differ. A position executed by only some branches is counted at its address set over those branches.
- **Lookup size** of a position = number of distinct addresses (and, for the sums, distinct 8-byte words) it takes
  over all branches. "Naive sum" = positions × words touched by any branch, i.e. every access pays for all memory
  the region touches.
- **Read-only vs written.** A varying load is "from written memory" if any byte it may read is written by any
  branch during the region.
- **Final values.** For each byte written by any branch, the final value per branch (unwritten = initial value);
  "differs" means not all branches agree.

## Reproduce

```sh
SQLITE_DIR=<dir with sqlite3.c and sqlite3.h> OUT=.tmp/sqlite-footprint/build experiments/sqlite-footprint/build.sh
OUT=.tmp/sqlite-footprint/build RUNS=.tmp/sqlite-footprint/runs experiments/sqlite-footprint/run.sh 256 arith   # etc.
python3 experiments/sqlite-footprint/report.py .tmp/sqlite-footprint/runs > experiments/sqlite-footprint/results.md
```

Raw per-branch data stays in `.tmp/sqlite-footprint/runs/<N>/<query>/` (git-ignored): `acc.<r>.bin` (one
24-byte record per access in program order: u32 site|flags, u32 size, u64 address, u64 call-path key),
`blk.<r>.bin` (executed block ids in order), `wr.<r>.bin` (every 64-byte line written: address, byte mask,
initial and final bytes), `sum.<r>.txt` (counters), `perf.tsv` (x86 counts), `rows.tsv` (per-row facts),
`meta.txt` (memory map at the start state), `analysis.json` (everything below, plus per-branch raw counters).
`build/sites.tsv` and `build/blocks.tsv` map site and block ids to function, source line and IR instruction count.

## Results

Full per-workload tables (step histograms, both alignments, byte counts by region, top functions):
[`results.md`](results.md), generated by `report.py`. Summary (alignment A unless marked):

| N | query | blocks min / median / max | max/median blocks | max/median x86 | accesses with varying address, A / B | of those: read-only loads / loads of written memory / stores | lookup size median / max | lookup sum / naive | final bytes differing |
|---|---|---|---|---|---|---|---|---|---|
| 256 | arith | 545 / 577 / 608 | 1.05 | 1.04 | 10.1% / 10.1% | 95% / 2.4% / 2.4% | 19 / 256 | 0.0049 | 53 |
| 256 | like_name | 530 / 564 / 630 | 1.12 | 1.16 | 10.8% / 10.8% | 90% / 5.2% / 4.7% | 19 / 256 | 0.0029 | 82 |
| 256 | like_note | 539 / 647 / 3,356 | 5.19 | 8.17 | 13.8% / 12.3% | 79% / 15.6% / 5.3% | 3 / 256 | 0.0002 | 15,456 |
| 256 | scan | 15,743 / 15,894 / 16,050 | 1.01 | 1.00 | 2.6% / 2.6% | 100% / 0.1% / 0% | 3 / 5 | 0.0008 | 12 |
| 1024 | arith | 567 / 601 / 627 | 1.04 | 1.03 | 11.4% / 11.4% | 96% / 2.1% / 2.1% | 81 / 1024 | 0.0052 | 57 |
| 1024 | like_name | 548 / 588 / 648 | 1.10 | 1.17 | 12.1% / 12.0% | 91% / 4.6% / 4.1% | 81 / 1024 | 0.0029 | 82 |
| 1024 | like_note | 557 / 672 / 3,378 | 5.03 | 7.85 | 15.1% / 13.6% | 81% / 14.5% / 4.8% | 6 / 1024 | 0.0001 | 19,769 |
| 1024 | scan | 62,851 / 63,388 / 63,927 | 1.01 | 1.00 | 2.6% / 2.6% | 100% / 0% / 0% | 3 / 6 | 0.0002 | 14 |

"Accesses" are dynamic scalar loads and stores summed over all branches. Lookup size = distinct addresses a varying
position takes over all branches. "Lookup sum / naive" = Σ over positions of distinct 8-byte words, divided by
positions × words the region touches. Memory-routine calls (libc/intrinsics) are counted separately: in like_note at
N=1024, 164 of their positions have varying addresses and they move 1.16 MB of the 1.38 MB they move in total.

Absolute lookup sums (Σ distinct words over positions, alignment A), N=256 → 1024: arith 5,709 → 22,168;
like_name 5,177 → 19,426; like_note 9,658 → 29,386; scan 31,580 → 125,927 (scan's grows because the run is 4×
longer, its lookup sizes stay ≤ 6).

### Measured causes

- **Step variation, point lookups.** 96% of arith's block-count variance at N=1024 is in `sqlite3BtreeTableMoveto`
  (B-tree search, 57–113 blocks per branch); rowid varint length (1 vs 2 bytes) explains R² = 0.13 of it. like_name:
  55% `patternCompare`, 44% B-tree search.
- **Step tail, like_note.** x86 count = 2,941 + 4.39 × (characters LIKE scans before its first match, or the whole
  note) + 1,112 × (row has overflow pages), R² = 0.96 at N=1024 (0.98 at N=256). All 14 branches above 2× the median are
  overflow rows (33 of 1024 rows overflow); the slowest is a 5,278-character note with no match, not the longest note
  (11,586 characters, matched early). Non-overflow branches reach at most 1.52× the median. The tail is a tail of a few
  data-defined rows, not a second mode. The block and IR counts understate it (5.0× vs 7.85× x86) because LIKE's
  inner skip is libc `strcspn`, whose instructions only the x86 count sees.
- **Scan.** All varying accesses but one position are the VDBE opcode fetch `switch(pOp->opcode)`: after a row
  matches, branches execute different opcodes, so the interpreter's program counter differs between branches. Every
  branch touches the same data bytes (22,803 of 22,877 at N=1024).
- **Varying stores, arith.** The only ones are the page reference counts `pPgHdr->nRef++` and `--p->nRef` on the page
  holding row r. In like_note the memsys5 allocator and the copy of the note into a register buffer add more.
- **Final bytes that differ** are left-overs `sqlite3_reset` does not clear: VDBE registers, the cursor's cached
  record header and offsets, parsed-cell info and, in like_note, the copied note text (`memcpy(pDest->z, ...)` in
  `OP_Column`).
- **Alignments A and B agree** to within 1.5 points everywhere; for arith they define the same 872 positions. Where
  they differ (like_note) B finds fewer varying accesses, as expected from its smaller misalignment.
- Control flow is identical for the first ~190–250 blocks (about 35% of a point-lookup run, 0.4–1.6% of a scan run);
  every point-lookup branch has a distinct block sequence.

### What could make these numbers misleading

- They are dynamic lower bounds. A sound static analysis, or a compiler that cannot prove which addresses a pointer
  may take, will see larger address sets.
- Lookup sizes are counted in this heap layout's addresses; a different layout (or a compiler that lays data out
  for coherent access) changes them. Distinct addresses are not distinct pages or lines.
- Alignment by execution count is one choice of how branches are padded to a common schedule. A coherent compiler
  that aligns differently (for example per loop iteration) can do better or worse.
- Data is synthetic: uniform random text, about 3% overflow rows. A table with one huge text value sets the step
  budget for every branch of like_note-shaped queries.
- Not counted: instructions inside libc routines (block and IR counts; the x86 count includes them), backend
  register spills, and the setup query and warm-up run that precede the region and fix the page-cache state.

## Findings (2026-11-13)

Full tables are in `results.md`. Measured, for N = 256 and 1024 rows:

- **Worst case against typical.** For the two point lookups on short data and for the full scan, the slowest row
  costs at most 1.10 times the mean in executed LLVM instructions (1.04, 1.09 and 1.00). For the lookup that
  searches a long text column, where about 1 row in 32 is 10 to 30 times longer and spills into overflow pages, the
  slowest row costs 4.4 times the mean (7.4 times in x86 instructions, which include the libc copy routines). The
  slow rows are a thin tail, not a second mode.
- **Accesses at an address that differs between rows.** 10 to 15% of memory accesses for the point lookups, 2.6% for
  the scan. For the arithmetic lookup at N = 1024 that is about 90 loads per row from memory the region never
  writes (the database pages), 2 loads from memory the region writes, and 2 stores. The varying stores are in the
  page cache (reference counts of the page fetched).
- **How much each such access ranges over.** For the point lookups the median is 81 distinct addresses at N = 1024
  (19 at N = 256) and the maximum is N: lookups into the table scale with the table. Summed over all access
  positions, the lookup sizes are 0.5% of the naive cost (every access pays for all memory the region touches) for
  the arithmetic lookup, and 0.01% to 0.3% for the other shapes.
- **Memory left in superposition.** Bytes whose final value differs between rows: 57 to 82 for the short lookups, 14
  for the scan, about 20,000 for the long-text lookup (the assembled overflow text).
- **Control flow** is identical for the first third of a point lookup and then differs for every row: all 1024 rows
  take distinct block sequences.

What these numbers do not show:

- They are a lower bound. They say how much variation exists; a compiler has to prove it, and a sound analysis
  will find more.
- One program, four query shapes, small tables, one build configuration (`-O2`, one fixed-arena allocator).
- Instructions inside libc copy and compare routines are counted only in the x86 column; their memory traffic is
  recorded per call, not per access.
- Nothing here is a gate count. The cost of one access of a given lookup size, and of one step of a machine whose
  program counter is in superposition, is not measured.
