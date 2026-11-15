# sqlite-footprint: worst case and superposed addresses on native SQLite

Question: for the project's own target workload (Grover search whose oracle is a SQL query on a fixed
in-memory SQLite database, parameterised by a row id), how far is the slowest branch from the typical one
(n4), and how many memory accesses have an address that differs between branches (n5)?

The footprint tables below are **dynamic measurements over all N inputs**. They bound the variation a coherent
region must handle: a sound static analysis can only find more varying accesses, larger address sets and a larger
step bound. The control-schedule section separately costs candidate schedules; a heuristic schedule need not be
close to the cheapest one.

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

## Control schedules (2026-11-15)

The cheap alignment in the initial measurement survives for the short point lookups, but does not generalise to
all four queries. Exact structural alignment costs about 1.1–1.2 times a mean run for arith/like_name, about 4.9
for like_note, and 3.0–3.1 for scan. Replacing iteration-specific paths with padded loop bodies is substantially
more expensive. These are **forward IR instruction costs in a trace-derived control model**, not gate counts,
execution times, or bounds obtained by a compiler. Every family still depends on observations.

### Cost model and families

An event is entry to one LLVM basic block, including entries to callees. Its weight is that block's original IR
instruction count, including its terminator and calls, excluding instrumentation. Each branch supplies a sequence
of these events. A schedule supplies blocks at positions; its cost is the sum of their weights, counting a block
again at each position that provides it. Costs below are divided by that workload's **mean** executed IR count,
not its mean block count or its maximum. Nothing is multiplied by N: this is code provision for a coherent run,
not the total work of enumerating N branches.

- **0a, waiting / greedy:** a common supersequence of the block traces. Emit whichever block is next for the most
  unfinished branches (ties: smallest block id); matching branches advance, others wait. This constructs a valid
  schedule on the traces, with no optimality guarantee.
- **0b, step exact:** at block-event offset t, provide each distinct block observed at t once. Branches advance
  at the same rate. **0c** provides the union of reached blocks at every block-event step up to the longest run;
  **0d** provides every reached IR instruction at every instruction step up to the largest executed IR count.
  0d uses a different step unit; both still charge in instructions.
- **2a, global intervals:** provide block b at every block-event offset from its earliest to its latest occurrence.
  **2b, function intervals:** use block offsets within frames and call-entry offsets within callers, merge those
  intervals across all frames, add the intervals along each reached call path, then union overlapping intervals
  for the same block. This discards correlations; it need not improve on 2a.
- **3a, exact structure:** identify an event by its call-site path, enclosing loop iteration indices and block.
  Provide each distinct key once. This costs the union of the iteration-specific paths, not a Cartesian product
  of every body's alternatives. A common order is an extra requirement: the independent check below verifies it.
- **3b, instance padding:** remove iteration indices from the structural keys, union the reached body blocks, and
  multiply each block's weight by the product of enclosing loop maxima. A maximum is taken per normalised loop
  instance (call-site/loop path). This also repeats reached callees at their call sites, and combines maxima that
  may never occur together. **3c** instead shares one observed maximum across every instance of a static loop.
- **3d, all arms:** use 3c's loop maxima, but include all static blocks in each reached function/loop body, not
  just the blocks observed there. Calls and nested-loop instances are still included only where observed; this
  is not an all-static schedule.
- **4b / 4c, hybrid:** use 3b / 3c outside selected regions and a step machine inside them. The regions are the
  main `sqlite3VdbeExec` dispatch loop and the two irreducible SCC loops. Within each normalised region instance,
  use exact step sets, per-block step intervals, or every reached block at every step; multiply by the surrounding
  structured loop maxima. Each region instance's step offset includes its callees.

The CFG supplies an SCC loop forest: identify cycle headers, remove edges back into them, then recurse. Two
loops are irreducible (multiple headers). The trace pass reconstructs calls and iterations, checks every
intra-frame CFG transition, requires each frame to end on `ret`, and rejects duplicate exact keys within a branch.
Indirect calls are attributed by the next compatible call slot in the current block, not by a separately recorded
call-site event; this remains an attribution limit even when reconstruction reports no issues.

### All workloads

Full integer costs, steps, function attribution, all five split outcomes and independent checks are saved in
[`schedules.json`](schedules.json). Columns below use A = arith, L = like_name, T = like_note, S = scan.
Family rows are multiples of the mean run; the first two rows give absolute instruction counts.

| Family | A256 | L256 | T256 | S256 | A1024 | L1024 | T1024 | S1024 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Mean run (IR instructions) | 3,748.61 | 3,567.27 | 4,142.44 | 125,336.86 | 3,871.01 | 3,684.42 | 4,255.09 | 501,812.75 |
| Max run (IR instructions) | 3,909 | 3,907 | 18,616 | 125,753 | 4,016 | 4,006 | 18,738 | 503,009 |
| 0a waiting / greedy | 7.96 | 6.47 | 12.36 | 5.01 | 7.84 | 5.89 | 12.37 | 5.74 |
| 0b step exact | 22.30 | 28.25 | 52.87 | 51.31 | 24.07 | 32.44 | 87.78 | 69.29 |
| 0c all reached / block step | 422.35 | 490.08 | 2940.04 | 368.29 | 421.78 | 488.06 | 2822.22 | 366.38 |
| 0d all reached / instr step | 2715.42 | 3039.28 | 16308.61 | 2885.55 | 2701.53 | 3017.21 | 15655.02 | 2882.86 |
| 2a global intervals | 32.34 | 41.38 | 692.45 | 108.18 | 32.43 | 41.70 | 681.25 | 112.21 |
| 2b function intervals | 29.69 | 40.19 | 755.10 | 118.57 | 29.62 | 40.70 | 715.33 | 120.28 |
| 3a exact structure | 1.08 | 1.16 | 4.85 | 3.04 | 1.07 | 1.18 | 4.86 | 3.12 |
| 3b instance padding | 12.63 | 10.12 | 77.10 | 17.46 | 13.38 | 10.87 | 75.43 | 17.21 |
| 3c static-loop padding | 12.63 | 10.17 | 77.16 | 17.46 | 13.38 | 10.92 | 75.47 | 17.21 |
| 3d all arms | 67.51 | 51.78 | 319.49 | 94.09 | 67.57 | 53.05 | 311.95 | 92.77 |
| 4b hybrid exact | 12.02 | 13.69 | 23.80 | 50.38 | 13.43 | 15.54 | 39.93 | 68.42 |
| 4b hybrid intervals | 20.23 | 21.49 | 105.45 | 106.57 | 21.24 | 22.45 | 111.87 | 110.83 |
| 4b hybrid all reached | 162.57 | 204.52 | 1791.83 | 236.42 | 165.07 | 206.45 | 1729.24 | 237.37 |
| 4c hybrid exact | 12.02 | 13.69 | 23.81 | 50.38 | 13.43 | 15.54 | 39.93 | 68.42 |
| 4c hybrid intervals | 20.23 | 21.49 | 105.45 | 106.57 | 21.24 | 22.45 | 111.87 | 110.83 |
| 4c hybrid all reached | 162.57 | 204.52 | 1791.83 | 236.42 | 165.07 | 206.45 | 1729.24 | 237.37 |

The difference between 3b and 3c is real but small here: sharing maxima across loop instances adds work for the
text queries; it adds none for arith or scan. At N=1024, `patternCompare` accounts for 86.2% of like_note's padded
3b cost (276,740 of 320,962), and 86.0% of its 3d cost. In arith/1024, the largest 3b contributions are
`sqlite3BtreeTableMoveto` (37.3%), `sqlite3VdbeExec` (21.9%) and `btreeBeginTrans` (10.5%). This is measured
attribution of the padding cost, not evidence that these functions dominate a circuit's gate count. Keeping the
VDBE interpreter as a step machine (family 4) does not automatically make this cost smaller.

### Does half the input domain cover the other half?

For each workload and seed 0,1,2,3,4, NumPy's `default_rng(seed).permutation(N)` selects N/2 training rows and
N/2 held-out rows. Build the schedule and all bounds from training rows only. A branch fails if any of its
positions/keys/loop trips is absent or outside those bounds; 0a is tested as a subsequence. For structured and
hybrid families this checks the model's presence and trip conditions, not an emitted circuit. Each table entry
is the **minimum–maximum number of missed held-out rows** across five splits (out of 128 or 512). The JSON
contains the individual counts, so a zero minimum cannot be mistaken for five clean splits.

| Family | A256 | L256 | T256 | S256 | A1024 | L1024 | T1024 | S1024 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 0a waiting / greedy | 2–9 | 1–7 | 2–11 | 3–8 | 1–10 | 1–4 | 1–6 | 1–14 |
| 0b step exact | 77–89 | 48–75 | 108–110 | 108–115 | 71–82 | 41–54 | 178–204 | 432–441 |
| 0c all reached / block step | 0–5 | 0–2 | 0–7 | 0–1 | 0–1 | 0–1 | 0–1 | 0–3 |
| 0d all reached / instr step | 0–5 | 0–2 | 0–7 | 0–1 | 0–1 | 0–2 | 0–1 | 0–1 |
| 2a global intervals | 9–22 | 5–20 | 9–21 | 2–12 | 6–24 | 3–16 | 8–19 | 5–12 |
| 2b function intervals | 7–17 | 2–11 | 5–13 | 1–8 | 6–23 | 1–11 | 3–15 | 3–7 |
| 3a exact structure | 0–4 | 0–4 | 2–12 | 75–94 | 0–2 | 0–3 | 7–14 | 315–333 |
| 3b instance padding | 0–4 | 0–2 | 0–7 | 0 | 0 | 0–1 | 0–1 | 0–3 |
| 3c static-loop padding | 0–4 | 0–2 | 0–7 | 0 | 0 | 0–1 | 0–1 | 0–3 |
| 3d all arms | 0 | 0 | 0–5 | 0 | 0 | 0–1 | 0–1 | 0–3 |
| 4b hybrid exact | 77–89 | 48–75 | 105–108 | 108–115 | 71–82 | 41–54 | 163–187 | 432–441 |
| 4b hybrid intervals | 9–22 | 5–20 | 7–20 | 2–12 | 6–24 | 3–16 | 6–18 | 5–12 |
| 4b hybrid all reached | 0–5 | 0–2 | 0–7 | 0–1 | 0–1 | 0–1 | 0–1 | 0–3 |
| 4c hybrid exact | 77–89 | 48–75 | 105–108 | 108–115 | 71–82 | 41–54 | 163–187 | 432–441 |
| 4c hybrid intervals | 9–22 | 5–20 | 7–20 | 2–12 | 6–24 | 3–16 | 6–18 | 5–12 |
| 4c hybrid all reached | 0–5 | 0–2 | 0–7 | 0–1 | 0–1 | 0–1 | 0–1 | 0–3 |

Even a family with five clean splits has only passed this particular coverage experiment. The N=256 text tail
shows why maxima learned from samples are not sound bounds. The exact structural union also misses paths that
only held-out rows take. Exhaustively tracing all N inputs covers this fixed database, warm state and parameter
domain; it says nothing about another database, SQLite build, query, layout or input domain.

### Checks and the 7.96 versus 2.04 discrepancy

The previously completed arith/256 report was first rerun from fresh prep/trace data and matched all 16 families'
costs, steps and split failures exactly. The trace pass completed all eight workloads without reconstruction
issues. [`check_schedule.py`](check_schedule.py) independently recomputes 0a, 0b, 0c, 0d and 2a from `blk.*.bin`
and `blocks.tsv`, without importing `schedule.py`; all match. Its heap/bucket majority merge also verifies that
every original branch is a subsequence. Mean/max block and instruction counts match the separate footprint
analysis. The structural check verifies unique per-branch exact keys, one block per key, and an acyclic union of
all per-branch key orders. Thus a topological order supplies a **valid waiting schedule with 3a's cost on every
measured workload**. Merely counting keys would not establish that. The normalised-key order is cyclic, so
normalised keys cannot simply be emitted once.

A further lower bound on any waiting schedule is `sum_b weight(b) * max_r count_r(b)`: every block must be
provided at least as often as the branch using it most. This ignores order conflicts, so need not be attainable.

| Workload | Multiplicity lower bound, cost (× mean) | Exact-key waiting schedule, cost (× mean) | Greedy waiting, cost (× mean) |
|---|---:|---:|---:|
| 256/arith | 4,042 (1.078) | 4,052 (1.081) | 29,843 (7.961) |
| 256/like_name | 4,085 (1.145) | 4,151 (1.164) | 23,079 (6.470) |
| 256/like_note | 19,344 (4.670) | 20,087 (4.849) | 51,198 (12.359) |
| 256/scan | 127,158 (1.015) | 380,544 (3.036) | 627,723 (5.008) |
| 1024/arith | 4,143 (1.070) | 4,158 (1.074) | 30,348 (7.840) |
| 1024/like_name | 4,261 (1.156) | 4,365 (1.185) | 21,712 (5.893) |
| 1024/like_note | 19,350 (4.547) | 20,701 (4.865) | 52,616 (12.365) |
| 1024/scan | 508,394 (1.013) | 1,566,995 (3.123) | 2,880,519 (5.740) |

The exact-key construction proves that greedy-majority's cost is far from optimal on arith; the constructed
waiting schedules are within 0.4% of the multiplicity lower bound at both sizes. At N=256 the
original 7.96 is 29,843 instructions / 3,748.609375; at N=1024 the same algorithm gives 30,348 / 3,871.009765625
= 7.84, in 5,373 **block** slots. Those slots / mean block count is 8.96, not 2.04. N and the block/instruction
unit conversion do not explain the difference.

[`docs/AUDITION.md`, Opus B](../../docs/AUDITION.md) reported “7,886 slots = 2.04x” for arith/1024; dividing by
3,871.01 shows that its “slots” were charged in instructions. Its accompanying step-aligned 93,186 reproduces
exactly here. No implementation for that merge is recorded in the proposal or the measurement files checked
here, and 7,886 is **not reproduced** by the specified majority rule. An independently tried sum-distance greedy gives 54,560 instead
(14.09x). The 4,158-cost exact-key waiting schedule and the 4,143 multiplicity bound show that 7,886 is entirely
plausible as a different, suboptimal merge; there is no contradiction in possible schedule cost. There is no
basis to identify its exact merge rule or event granularity, or retain 2.04 as a validated result. Use the reproducible figures above.

### What can a compiler build without running the program?

The schedule *forms* are available statically; their measured bounds and pruning generally are not.

| Form | What makes it sound without observing executions |
|---|---|
| 0a waiting / 3a iteration-specific structure | A bounded CFG/call expansion and a common order for its admitted paths; a sound over-approximation suffices. Matching the small traced union additionally needs iteration-specific reachability information. Enumerating CFG paths can explode in size; the traced key DAG establishes only these measured paths. |
| 0b exact step sets | Propagate reachable control states for each bounded step, including call/return state. Sound over-approximation is possible; matching these exact sets requires resolving their data-dependent reachability. |
| 2a / 2b intervals | Sound earliest/latest block and call-entry bounds, from loop/recursion analysis or a region step budget. Without either, a finite latest offset need not be provable; interval propagation also loses correlations. |
| 3b / 3c / 3d structured padding | Restructure the CFG, resolve call targets and bound every loop and recursion depth. 3c's global per-loop bound and 3d's static bodies are straightforward forms, but the observed maxima and reached call/loop instances here still need proof or replacement. |
| 4 hybrid | Use structured code where such bounds are known, bounded dispatch inside the remaining regions. Its inner block sets and outer bounds must be sound; observed region sizes are insufficient. |
| 0c / 0d universal fallback | Provide all possibly executable code, not just reached code, with a chosen whole-region step budget and explicit call/return/indirect-target state. Include a done/fault state and handle budget exhaustion according to the region contract. 0d is the direct instruction-step form. This needs no data-dependent loop bound to be semantically total. |

A whole-region instruction budget B also supplies conservative per-loop iteration and recursion-depth bounds.
Using B throughout nested structured expansion can make the product enormous; it does not recover the cheap
observed maxima. No exact reachability solver is required for the expensive fallback. The done/fault behaviour
must preserve the [documented region contract](../../docs/DESIGN.md).

The static measurements explain part of the gap. There are 48,365 blocks / 251,828 IR instructions in this
SQLite build, versus 404 / 2,604 reached by arith. Inlining direct callees at every call site (loops counted once,
indirect calls omitted) encounters a direct-call cycle from `sqlite3_step`, `sqlite3_reset` and
`sqlite3_bind_int64`; the naive expansion is infinite. This is a syntactic call-graph result, not a claim that
these executions recurse without bound. `sqlite3_column_int64` expands to 611 instructions.

The dispatch region plus its direct-call closure contains 34,944 blocks / 188,938 instructions, 845 called
functions and 1,323 indirect-call slots. The irreducible VDBE loop's closure is 5,568 / 29,638, with 598 indirect
slots; the tokenizer loop is 4 / 17. These closures **omit indirect callees**, so even these are lower bounds on
an IR-only region machine's code provision. Taking all 251,828 instructions and arith/1024's observed maximum
of 4,016 instruction steps would cost about 261,260 mean runs in the 0d model; the maximum itself is still an
observation, not a proved budget. Specialising the prepared VDBE bytecode and propagating concrete database
facts might narrow the code and bounds. That improvement has not been measured here.

### Reproduce and limits

After building and tracing the workloads as above (Python 3 and NumPy required):

```sh
mkdir -p .tmp/sqlite-footprint/sched
python3 experiments/sqlite-footprint/schedule.py prep .tmp/sqlite-footprint/build .tmp/sqlite-footprint/sched/ir.pkl
# Repeat for N=256,1024 and q=arith,like_name,like_note,scan:
N=256 q=arith
JOBS=3 python3 experiments/sqlite-footprint/schedule.py trace .tmp/sqlite-footprint/build .tmp/sqlite-footprint/sched/ir.pkl .tmp/sqlite-footprint/runs/$N/$q .tmp/sqlite-footprint/sched/$N/$q
JOBS=3 python3 experiments/sqlite-footprint/schedule.py report .tmp/sqlite-footprint/build .tmp/sqlite-footprint/sched/ir.pkl .tmp/sqlite-footprint/runs/$N/$q .tmp/sqlite-footprint/sched/$N/$q 5
python3 experiments/sqlite-footprint/check_schedule.py .tmp/sqlite-footprint/build .tmp/sqlite-footprint/runs/$N/$q .tmp/sqlite-footprint/sched/$N/$q
```

The scan report aggregates about 65 million events at N=1024. Measuring N=256 first gave a 694,488 KiB
peak; scaling by events predicted about 10.6 GiB. The N=1024 report completed in 13:31.51 with a measured peak
of 9,847,624 KiB (9.39 GiB), under a 14 GiB address-space ceiling. Run these sequentially;
the full report retains several event-sized arrays. The checker reads structural data one branch at a time.

Limits beyond the footprint study's data and coverage caveats:

- These schedules have not been lowered to coherent circuits. Branch masks, program-counter comparisons,
  coherent call/return and loop state, reversible history, uncomputation, memory/address selection, and gates
  for arithmetic are not charged. Reverse execution is not included. An instruction-weight ratio is no speedup
  claim, and a valid block supersequence does not itself establish a reversible implementation.
- The trace records **block entries**, while calls may occur within a block. The caller's weight is charged at
  entry even though its instructions surround callee executions. A real compiler must split at calls and respect
  SSA/data dependencies; the exact block-offset tables would change. Instructions inside libc/intrinsics remain
  uncounted, particularly in like_note's expensive text scan and copies.
- The independent checks cover the trace-only families and 3a's waiting order. The padded, interval-composition
  and hybrid costs additionally rely on the documented model and code audit; no separate circuit validates them.
  Reconstruction checks constrain the trace parser, but do not independently identify indirect call sites.
- The implementation packs block ids into 16 bits, region offsets into 18 bits and uses 64-bit structural hashes.
  The measured traces fit those ranges, have unique exact keys within every branch, and show no key-to-block
  conflicts. These checks are evidence for this dataset, not a general collision-free key representation.
- Sample coverage is not a proof of static reachability or trip bounds. Keeping every instruction and enforcing
  the documented region budget is the sound fallback; recovering the cheap measured schedules statically remains
  compiler work.
