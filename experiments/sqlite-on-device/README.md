# SQLite on the device

This experiment runs **unmodified SQLite 3.54.1** through the musl linker and Bennett executor.
It is separate from `sqlite-footprint`: those are native LLVM/x86 measurements; these are device runs.
The three stages are ordinary execution, full reversal, and a query with its row id in superposition.

## Reproduce

Requirements: clang/LLVM 18, CMake, Python 3, Rust, and the SQLite amalgamation. No SQLite source is
vendored here. Download `https://sqlite.org/2026/sqlite-amalgamation-3540100.zip`, extract it outside
the repository and set `SQLITE_DIR` to the directory containing `sqlite3.c` and `sqlite3.h`.
The measured inputs have SHA-256:

```
sqlite3.c  0c657666d819f261cc4007526ad57e1669de9949df7a8ea847aa284a2bf0ee52
sqlite3.h  d6c84557261893439b341d2e6c35187ae3ea24fbfd843ad94616f72d89242b8c
```

From the repository root (at most four build/test processes):

```sh
python3 rt/musl/build.py --jobs 4
cmake -S front -B target/front
cmake --build target/front --parallel 4
cargo build -j 4 -p bennett
SQLITE_DIR=<amalgamation directory> NROWS=4 experiments/sqlite-on-device/build.sh
python3 experiments/sqlite-on-device/run.py target/sqlite-on-device/n4 --stack-sweep
# Repeat build/run for NROWS=8 and 16; only the default 1 MiB stack is supported by --stack-sweep.
```

`build.sh` compiles a generated wrapper that includes the original amalgamation and `driver.c`;
`rt/musl/link.py` supplies musl, runtime and startup. The native executable uses the same source,
configuration and driver with clang 18 `-O2`, vectorisers disabled, and the host C library. Device C
starts at `-O0 -disable-O0-optnone`; the front runs its own O2 pipeline. Output and returned program
status (not just the CLI's process status) are compared. Artifacts and raw JSON stay in `target/`.

Configuration, identical for both builds:

| Flag/limit | Purpose |
|---|---|
| `SQLITE_THREADSAFE=0` | Single-threaded execution; no mutex/thread backend |
| `SQLITE_OMIT_LOAD_EXTENSION` | No dynamic loading |
| `SQLITE_OS_OTHER=1` | The explicit no-files VFS in `vfs.c`, rather than the Unix syscall backend |
| `SQLITE_ENABLE_MEMSYS5` | SQLite's own buddy allocator over a fixed, aligned heap |
| `SQLITE_TEMP_STORE=3` | Temporary storage stays in memory; no file fallback |
| `NROWS=4/8/16` | Compile-time row count, since musl startup supplies no command-line arguments |
| `HEAP_BYTES=262144` | SQLite heap, passed to `SQLITE_CONFIG_HEAP` with minimum allocation 32 bytes |
| musl arena 4,096 bytes | Separate monotone libc allocation arena; not SQLite's heap |
| stack 1,048,576 bytes | Device slot frames and C allocations, above static data |

`vfs.c` registers an empty file namespace: opens return `SQLITE_CANTOPEN`, access says absent,
delete returns `SQLITE_IOERR_DELETE_NOENT`, pathname copying is bounded. Entropy is a fixed byte
pattern, time a fixed Julian date, sleep reports zero microseconds slept. There is no claim to
Unix files, real time or random entropy. Counters in the driver expose whether open, entropy or
clock was used. The measured runs use none of those three callbacks.

## Workload and measurement boundaries

The table, four SQL query shapes, LCG seed, text generator, row ids and parameter offsets are those of
`sqlite-footprint/driver.c`. One transaction inserts all rows. Each of four statements is prepared and
warmed once, then the full driver queries every row, prints results and SQLite's live/peak allocation
counts, finalizes statements and closes the database. The scan prints the **count**, not a Boolean.
The other queries are predicates. The region is bind, step, column read, reset, with API status checks.
This uses actual SQLite parsing, VDBE execution, B-tree lookup, record decoding and LIKE.

Per-query steps start from a fresh setup, as in the native footprint's forked branches. `run.py` appends
a device entry that calls the lowered `experiment_setup`, then `experiment_query(q,r)`. It subtracts
an otherwise identical setup-only entry's steps, so the difference includes entry/return of the query
but excludes setup and printing. It independently compares each returned query result with the
native driver's table. Full-run steps include startup, setup, all queries, output and cleanup.
SQLite allocation counts printed after each query shape include all four prepared statements and the
setup high-water mark; they are not bytes exclusively attributable to that query.

The stack sweep changes only the module's memory size, leaving the original static image and slot
layout intact, and binary-searches the first size at which the **whole driver** has identical output.
Every failed trial must report `stack_overflow`. This minimum is specific to this program and row range.

## Stage 1: ordinary execution

Measured 2026-11-16 at `9992a99`, clang 18, default flags above. All 112 fresh-setup query
results and all three complete stdout/status comparisons agree. No wrong result or crash.

| N | native equality | final lowering (s) | device text (bytes) | complete run steps | SQLite live / peak bytes, every query shape |
|---|---|---:|---:|---:|---:|
| 4 | yes | 24.09 | 9,061,696 | 957,295 | 101,632 / 107,264 |
| 8 | yes | 19.50 | 9,062,464 | 1,200,929 | 101,632 / 107,264 |
| 16 | yes | 20.88 | 9,062,992 | 1,841,474 | 101,632 / 107,264 |

Final lowering is a separately timed invocation on the fully resolved `linked.bc`, not the whole
compile/link process. The repeated output is byte-identical to `link.py`'s output for all three sizes.
The N=4 complete native/device build took 130.58 s, including archive resolution and retries for
library dependencies introduced by optimisation. No front refusal remains in this configuration.

| N | query | minimum device steps | maximum device steps |
|---|---|---:|---:|
| 4 | arith | 5,165 | 5,251 |
| 4 | like_name | 6,315 | 6,874 |
| 4 | like_note | 6,748 | 11,045 |
| 4 | scan | 9,317 | 10,310 |
| 8 | arith | 5,171 | 5,259 |
| 8 | like_name | 5,616 | 6,916 |
| 8 | like_note | 6,790 | 12,436 |
| 8 | scan | 13,479 | 14,730 |
| 16 | arith | 5,112 | 5,315 |
| 16 | like_name | 5,503 | 6,951 |
| 16 | like_note | 6,825 | 16,131 |
| 16 | scan | 21,875 | 23,301 |

All builds reserve 1,408,060 device memory bytes: 359,484 static bytes plus the 1 MiB stack.
SQLite's 256 KiB heap and libc's separate 4 KiB arena are included in those static bytes.
The N=4 whole driver needs exactly **155,099 stack bytes**, measured to a one-byte boundary;
155,098 faults `stack_overflow`. The selected 1 MiB stack passes at every N. The heap and libc arena
are successful configured capacities, not proven minimums. SQLite's live/peak counts are identical
for all four query shapes and all N here, and do not include allocator metadata or libc allocations.
No file, clock or entropy trap is reached (the VFS callback counters are all zero).

The only measured device fault so far is the deliberate stack exhaustion. At the N=4 one-byte
boundary it occurs in `btreeParseCell` at its indirect call of `pPage->xParseCell` (amalgamation line
74145): reserving the callee's device slot frame does not fit. More stack removes it. The unused Unix
VFS was not tested: no claim is made about which syscall it would encounter first.


## Stage 2: forward then back

**Pass**, the same three complete drivers, native stdout/status still equal. Run:

```sh
python3 experiments/sqlite-on-device/run.py target/sqlite-on-device/n4 --reverse
# Repeat for n8 and n16.
```

| N | steps forward and separately backward | journal bytes | journal u128 words | words per forward step | restored classical state / amplitude distance |
|---|---:|---:|---:|---:|---|
| 4 | 957,295 | 19,086,335 | 1,133,065 | 1.183611 | identical / 0 |
| 8 | 1,200,929 | 23,956,769 | 1,422,240 | 1.184283 | identical / 0 |
| 16 | 1,841,474 | 36,573,538 | 2,170,754 | 1.178813 | identical / 0 |

The existing CLI snapshot check compares memory, slots, frames, stack, tape and amplitudes with
its entry snapshot. The script requires exact classical equality and zero amplitude distance.
These runs have one world and no quantum registers. Journal words are derived from the executor's
representation: one one-byte Undo tag per step plus 16 bytes per word, hence `(bytes - steps)/16`.
There are no mark, qubit-read, jump-environment or cell side tables in these executed runs. These are
128-bit journal words, not 64-bit words; about 19.9 total journal bytes per executed step. This measures
the whole process including setup/printing/cleanup, not just the SQL region. No fault, wrong result
or crash occurred in these successful forward/backward runs.

## Stage 3: a query under a qubit

**Pass**, N=4, `like_note`, native bits `[0,0,0,1]`. The database and all prepared statements are
fixed by classical `experiment_setup` before allocating the qubits. Two Hadamards give uniform row
indices 0..3; the query still binds rowid = index + 1. Output is the query's Boolean XORed into one
qubit, with a separate four-qubit fault register. Only **7 qubits** and **4 basis configurations** are
needed (simulator limit 8). This is the nonconstant LIKE query, not a hand-written predicate or table.

```sh
cargo build -j 4 --offline --manifest-path experiments/sqlite-on-device/harness/Cargo.toml --target-dir target
python3 experiments/sqlite-on-device/coherent.py target/sqlite-on-device/n4
# These write the same ignored artifact names: run sequentially.
python3 experiments/sqlite-on-device/refusals.py target/sqlite-on-device/n4
```

`coherent.py` appends device text to the ordinary lowered module. **All SQLite and driver functions,
static data, allocator, VFS and musl code come from that unchanged lowered module.** Hand-written:
the entry calls classical setup, allocates/gates registers and issues `apply`; this width adapter
calls the ordinary lowered driver, which executes SQLite's bind/step/column/reset:

```
func @sqlite_lookup(i2 %row) -> i1 {
entry:
 %r = zext i2 %row to i32
 %v = call @experiment_query(i32 2, %r)
 %b = icmp eq i32 %v, 1
 ret %b
}
```

The adapter marks a row exactly when the query returns 1 (SQL true). It compares rather than truncating:
the driver returns -1 for a key absent from the table, whose low bit would mark the row. The runs recorded
in this file before 2026-11-17 used `trunc i32 %v to i1`, which agrees with the comparison on every row they
had (all results 0 or 1); their region step counts were not remeasured with the comparison.

There are four separate basis runs and two uniform-superposition runs (out initially 0 and 1).
The standalone observer uses the public executor snapshot interface; Python compares every resulting
configuration and complex amplitude with the native answers at tolerance 1e-12, including phase.
For out=0 the resulting state has amplitude +1/2 on each of `(row,out,fault)` =
`(0,0,0)`, `(1,0,0)`, `(2,0,0)`, `(3,1,0)`. For out=1 every output bit is complemented.
The row input is preserved and every successful fault register remains zero. The observer also
compares snapshots immediately before and after the uniform `apply`: every classical memory byte
(including the database), stack pointer, live slot and tape byte is unchanged. Two applies followed by
the inverse Hadamards permit all registers to be freed cleanly: interference, not just measurement
probabilities. A full forward/backward coherent run restores the classical entry exactly and the
amplitudes within 2.23e-16 (floating-point Hadamards); the observer requires less than 1e-12.
History and branch enumeration compare mode passes all cases, including the budget probes and reversal.
The tables use normal history mode; compare mode reports the combined cost of both computations. (Since 2026-11-17 the executor computes `apply` one way only, by evaluation for each definite value of the arguments; the mode switch these measurements used is gone.)

| row index | native bit | forward region steps | region journal bytes | region overlay bytes |
|---|---:|---:|---:|---:|
| 0 | 0 | 6,751 | 133,752 | 178 |
| 1 | 0 | 9,160 | 179,585 | 273 |
| 2 | 0 | 9,097 | 178,690 | 236 |
| 3 | 1 | 11,048 | 215,617 | 416 |

The uniform run performs four region runs, **36,056 forward region steps**, **72,116 work steps**
including uncomputation/call overhead. `region_cost` sums per-run high-water journal/overlay values:
**707,644 / 1,103 bytes**; these are sums, not simultaneous peaks. The largest individual run is
215,617 / 416 bytes. The classical base is shared and these four region worlds run one at a time;
the outer `Stats::worlds` stays 1, not 4 simultaneous worlds. No region split copies are needed.
The normal outer run keeps no setup journal; a separately reversed whole run retains 14,800,617 outer
journal bytes. Its incremental region counters are the same. These counters exclude host containers, IR and snapshot storage.
The same 256 KiB SQLite heap, 4 KiB libc arena and 1 MiB stack suffice.

An 11,048-step region budget passes; **11,047 faults `budget` only on row 3**, with inputs preserved,
other fault bits zero and weight 1/4. Budget 1,000 faults all four rows with weight 1. Raising the
budget removes those faults. Budget probes allow region faults explicitly so their registers can be
inspected. There is no resource obstacle at this size; stage 3 did not test a larger coherent
database (N=8 Grover below does).

### C front boundary, measured separately

`marker.c` attempts the same query using C markers, against the identical already linked bitcode:

| attempt | measured refusal | source construct / route past it |
|---|---|---|
| Override the ordinary `main` with the marker entry | `not handled yet: the address of @__stdio_seek, which performs quantum operations, is taken` (15.91 s) | Control-function classification reaches an address-taken musl stdio callback in this build (`fwrite.c:16`). Supporting address-taken control functions or refining their call graph is needed. Renaming the marker entry away from musl's callback-based `main` removes this first refusal, as the second attempt measures. |
| Keep classical `main`; add `experiment_coherent_c` as a separate entry | `not handled yet: a call that depends on a qubit, to a function that writes memory or performs quantum operations` (6.85 s) | `marker.c:11`, the call of `experiment_query(2,row)`. The query writes SQLite cursor/VM/allocator memory. The front's documented memory-writing fallback would remove this refusal; today's device already runs it through the hand-written `apply`. |

Both are compiler exit status 3; neither emits a runnable coherent C program. These are separate
refusal probes, not edits to SQLite or to the compiler. The text adapter is the explicitly allowed
workaround for stage 3, not evidence that the C marker front now accepts SQLite under qubit control.

### Stack observer

The standalone observer can also repeat the stack search without reparsing the large module at every
trial; it checks the whole-run status and tape against the original successful run and prints the
faulting frame/instruction:

```sh
target/debug/sqlite-on-device-harness target/sqlite-on-device/n4/program.bn stack
# n8 and n16 likewise.
```

Whole-driver minima N=4/8/16: **155,099 / 155,163 / 155,211 bytes**, all stopped one byte below by the
same indirect `xParseCell` call in `btreeParseCell`. These are execution measurements, not a general
static stack bound.

## Grover

Expected states, before comparing with the device: let `s` be uniform on N rows, `O` negate
the marked rows and `D = 2|s><s| - I`. After the oracle the mean amplitude is
`(N-2M)/(N sqrt(N))`; diffusion sends each amplitude `a` to `2 mean - a`.
For N=4 and M=1 this gives +1 on the marked row and zero elsewhere. For M=2 the mean
is zero, so the marked amplitudes are +1/2 and unmarked amplitudes -1/2: **all four
probabilities remain 1/4**, not support only on the nonmatches. With M=0 the state stays `s`.
For N=8, M=1 and `theta = asin(1/sqrt(8))`, after k iterations each marked amplitude is
`sin((2k+1) theta)` and each unmarked amplitude is `cos((2k+1) theta)/sqrt(7)`.
Thus k=1 gives `5/(2 sqrt(8))`, `1/(2 sqrt(8))`, success 25/32; k=2 gives
`11/(4 sqrt(8))`, `-1/(4 sqrt(8))`, success 121/128. For M=2,N=8,k=1 the
marked amplitudes are `1/sqrt(2)` and all others zero (success 1).

### Method and N=4 results

```sh
cargo build -j 4 --offline --manifest-path experiments/sqlite-on-device/harness/Cargo.toml --target-dir target
SQLITE_DIR=<amalgamation directory> python3 experiments/sqlite-on-device/grover.py target/sqlite-on-device/n4 --n 4
```

No rebuild of musl, SQLite or the lowered module is needed. `grover.py` retains every original
function and static data segment from `program.bn`. Hand-written device text: extra SQL strings,
an entry/setup adapter calling the original `experiment_setup`, a load of its database pointer,
`sqlite3_exec` and `sod_check`, the row-width adapter calling unchanged `experiment_query(2,row)`,
qubit allocation, gates, `apply`, diffusion, clean frees and measurement/return. All SQLite,
musl, allocator, VFS and query-driver instructions are unchanged. The extra SQL constants are
placed above the original memory extent, with a fresh 1 MiB stack above them; the original
static image and its addresses stay intact. This reserves about 2.46 MB rather than 1.41 MB;
it is an uncomplicated append-only harness layout, not a measured minimum.

Row indices here are zero-based, SQL ids are index+1. Singleton index 3 uses the original
database, unmodified even classically. For the other cases, before allocating qubits,
SQL `UPDATE t SET note=replace(lower(note),'zq','xx')` removes existing matches and
`UPDATE t SET note=note||'zq' WHERE id IN (...)` appends a marker to the selected rows.
The generated note contents are retained except for those replacements; lengths remain the
same except for the two appended characters. No shorter replacement notes or substitute
predicates are used. Native SQLite 3.54.1, compiled with the original flags/driver and a
small generated native main, executes the same SQL updates and original LIKE query to check
every row's expected bit. The quantum oracle still executes the real bind, step, column,
reset and API checks of the unchanged prepared `SELECT note LIKE '%zq%' FROM t WHERE id=?1`.
Preparation/warmup, updates and the database are classical and fixed before the quantum search.

The output qubit starts in `|->`. Each `apply` gives phase kickback, followed by diffusion
on the row register. `H X controlled-Z X H` implements `-D`; an X on the unchanged `|->`
qubit supplies the global minus sign, so the comparison checks **D**, including the phase.
After the last iteration H then X resets the kick qubit; both it and the fault register
are freed cleanly before the observer inspects the entire state and the program measures
the row. Tolerance is the stage-3 observer's 1e-12. The observer asserts every byte of
classical memory (database included), stack, live slots and tape is unchanged at every
`apply`, and memory/stack/tape still match classical setup just before measurement.
Only the row register remains live, so no workspace or fault entanglement is hidden.

Measured 2026-11-16, normal history mode; compare mode also passes all cases. All amplitudes
are real, with exactly zero imaginary parts. The vectors below list every row in index order;
`u = 0.5000000000000001`, `v = 1.0000000000000002`. No wrong result, device fault or resource refusal.

| N | marked indices | iterations | expected state | measured state | theoretical / measured success | largest amplitude deviation |
|---|---|---:|---|---|---|---:|
| 4 | 0 | 1 | (1,0,0,0) | (v,0,0,0) | 1 / 1.0000000000000004 | 2.22e-16 |
| 4 | 1 | 1 | (0,1,0,0) | (0,v,0,0) | 1 / 1.0000000000000004 | 2.22e-16 |
| 4 | 2 | 1 | (0,0,1,0) | (0,0,v,0) | 1 / 1.0000000000000004 | 2.22e-16 |
| 4 | 3 (original) | 1 | (0,0,0,1) | (0,0,0,v) | 1 / 1.0000000000000004 | 2.22e-16 |
| 4 | 0,3 | 1 | (1,-1,-1,1)/2 | (u,-u,-u,u) | 1/2 / 0.5000000000000002 | 1.11e-16 |
| 4 | 0,3 | 2 | (-1,-1,-1,-1)/2 | (-u,-u,-u,-u) | 1/2 / 0.5000000000000002 | 1.11e-16 |
| 4 | 0,3 | 3 | (-1,1,1,-1)/2 | (-u,u,u,-u) | 1/2 / 0.5000000000000002 | 1.11e-16 |
| 4 | none | 1 | (1,1,1,1)/2 | (u,u,u,u) | 0 / 0 | 1.11e-16 |

Probabilities slightly above one are unrenormalised floating-point sums, reported rather than
clamped. The seeded measurement returns the marked row in every singleton case; pair/no-match
runs sample row 2. The full amplitude comparison, not these single samples, checks the distribution.

### N=8 results

Use the existing N=8 ordinary build with `grover.py target/sqlite-on-device/n8 --n 8`;
set `SQLITE_DIR` as above. On 2026-11-16 the same command also checked every apply of history mode
against branch enumeration; both modes passed. The observer allows 16 configurations (the
minus qubit doubles the eight row values); peak live qubits are eight, and only three row
qubits remain before measurement. The database is prepared by the same classical SQL
updates, and native bits are `[0,0,0,1,0,0,0,0]` or `[1,0,0,1,0,0,0,0]`.

| N | marked indices | iterations | theoretical success | measured success | largest amplitude deviation |
|---|---|---:|---:|---:|---:|
| 8 | 3 | 1 | 25/32 = 0.78125 | 0.7812500000000004 | 2.22e-16 |
| 8 | 3 | 2 | 121/128 = 0.9453125 | 0.9453125000000014 | 7.77e-16 |
| 8 | 0,3 | 1 | 1 | 1.0000000000000004 | 2.22e-16 |

Every measured amplitude follows; every imaginary part is exactly zero. The expectations are
the closed forms worked above, including **negative** unmarked amplitudes after two singleton
iterations. The two-match case has amplitude `1/sqrt(2)` on rows 0 and 3, zero elsewhere.
All three seeded measurements return row 3, but the observer checks the state before collapse.

| row index | one match, k=1 | one match, k=2 | two matches, k=1 |
|---:|---:|---:|---:|
| 0 | 0.17677669529663692 | -0.08838834764831856 | 0.7071067811865477 |
| 1 | 0.17677669529663692 | -0.08838834764831856 | 0 |
| 2 | 0.1767766952966369 | -0.08838834764831868 | 0 |
| 3 | 0.8838834764831847 | 0.9722718241315036 | 0.7071067811865477 |
| 4 | 0.17677669529663692 | -0.08838834764831854 | 0 |
| 5 | 0.17677669529663692 | -0.08838834764831854 | 0 |
| 6 | 0.17677669529663692 | -0.08838834764831854 | 0 |
| 7 | 0.17677669529663692 | -0.08838834764831854 | 0 |

### Costs and repeated oracles

`region_steps` counts forward instructions **per configuration**. The minus qubit gives 2N
configurations, while history mode runs only N distinct row arguments; actual forward work is
half of `region_steps`, and `region_cost.work` includes that work and uncomputation plus call
overhead. Region journal/overlay bytes are **sums of each run's high-water values**, not peak
simultaneous storage or retained storage. Whole-run outer steps include setup/updates, quantum
glue and measurement, but exclude region work. Whole-run region totals are also shown.

| N | marked | k | outer steps | region steps total | region work total | journal bytes total | overlay bytes total |
|---|---|---:|---:|---:|---:|---:|---:|
| 4 | 0 | 1 | 988,920 | 76,676 | 76,680 | 751,350 | 1,199 |
| 4 | 1 | 1 | 992,527 | 76,676 | 76,680 | 751,350 | 1,199 |
| 4 | 2 | 1 | 991,616 | 76,676 | 76,680 | 751,350 | 1,199 |
| 4 | 3 (original) | 1 | 746,147 | 72,112 | 72,116 | 707,644 | 1,115 |
| 4 | 0,3 | 1 | 1,099,323 | 76,886 | 76,890 | 753,471 | 1,212 |
| 4 | 0,3 | 2 | 1,099,334 | 153,772 | 153,780 | 1,506,942 | 2,424 |
| 4 | 0,3 | 3 | 1,099,345 | 230,658 | 230,670 | 2,260,413 | 3,636 |
| 4 | none | 1 | 898,078 | 76,466 | 76,470 | 749,229 | 1,206 |
| 8 | 3 | 1 | 1,103,863 | 148,070 | 148,078 | 1,453,019 | 2,271 |
| 8 | 3 | 2 | 1,103,878 | 296,140 | 296,156 | 2,906,038 | 4,542 |
| 8 | 0,3 | 1 | 1,204,632 | 148,280 | 148,288 | 1,455,140 | 2,292 |

At N=4 every two-match call costs exactly 76,886 region steps, 76,890 work steps, 753,471 journal
bytes and 1,212 overlay bytes: the cumulative counters are exactly linear in 1/2/3 calls.
After **every** call retained outer journal is absent, overlay bytes zero, worlds one,
memory qubit bytes zero, and live qubits seven (two row, one kick, four fault); the peak is
also seven, with eight configurations. After clean frees only two row qubits remain.
Nothing accumulates across iterations. Copied bytes and outer splits/merges are zero.

At N=8 each measured singleton oracle costs exactly 148,070 region steps (74,035 actual forward),
148,078 work steps, 1,453,019 journal bytes and 2,271 overlay bytes: two calls double every
region counter. The two-match call costs 148,280 steps (74,140 actual forward), 148,288 work,
1,455,140 journal and 2,292 overlay bytes. At every call boundary the retained journal is
absent, overlay zero, worlds one, memory qubit bytes zero and live/peak qubits eight.
Extra iterations add only 15 outer instructions each. This observed linearity is for fixed
nonzero row support; sparse simulation can cost less after exact cancellations. It is not
a circuit cost claim. No N=8 case failed, faulted or exceeded a resource limit.

The four required repository checks pass (274 Rust tests, 158 front tests, clippy and fmt),
as do the standalone observer's clippy/fmt checks. Raw generated programs and full reports
are ignored build artifacts, `grover.bn`, `grover-report.txt` and `grover.json`; the JSON
contains every complex amplitude and per-call cost line, including the premeasurement and
whole-run statistics. Run the two apply modes sequentially because they write the same names.

These are tiny N numerical simulations, with branch enumeration as the independent comparison
and history mode grouping equal row arguments. They do not establish an efficient SQL quantum
circuit, scaling, hardware speedup or gate counts. The entry, phase and diffusion glue is
hand-written device text; the C marker front still refuses the program at the memory-writing
query boundary described above. No compiler, executor or runtime changes are made.

## What the numbers do not show

These are numerical simulation and compilation evidence, not proofs, gate counts, hardware runtime,
or evidence of scaling to large databases. The original N=4/8/16 runs have no long-note overflow case (notes are 49–366 bytes); they do not expose
the overflow tail seen at N=256/1024 in `sqlite-footprint`. The complete SQL engine is present, but this workload does
not exercise most of its features. The flags and VFS intentionally remove OS services rather than
implementing a host filesystem. Build and execution wall times are observations on a shared laptop,
not performance thresholds. Neither compiler, executor nor musl runtime is changed by this experiment.

## Larger SQL oracles

`SQL_ORACLE=1` enables four additional **Boolean** statements without changing the original four.
The schema, seed, generated row contents, bind/step/column/reset calls, SQLite source and VFS remain
those above. The new `scale.py` entry uses the generated table directly, with no SQL updates to plant
matches. A row is marked exactly when its query returns 1 (`MARKED` in `scale.py`): the generated lookup
compares (`icmp eq i32 %v, 1`), so false, NULL (read as 0), a key absent from the table (the driver's -1) and
any other count all leave the row unmarked, and the classical entry prints the same marking. `scale.py`
therefore accepts native results of any value; it no longer refuses a table that is not all 0 or 1, a
refusal that only kept the earlier truncating lookup (`trunc i32 %v to i1`) from marking odd results.

| Shape | SQL | Distinct work exercised |
|---|---|---|
| `like_note` | `SELECT note LIKE '%zq%' FROM t WHERE id=?1` | Existing B-tree lookup, record decoding and LIKE, including long notes at larger N |
| `int_eq` | `SELECT a=3 FROM t WHERE id=?1` | Integer equality after indexed rowid lookup |
| `text_order` | `SELECT name<'m' FROM t WHERE id=?1` | TEXT comparison with SQLite's default BINARY collation |
| `two_where` | `SELECT EXISTS(SELECT 1 FROM t WHERE id=?1 AND a<8 AND b>=500)` | Scalar subquery, two WHERE comparisons, conditional exits, and EXISTS returning false when no row qualifies |
| `range_agg` | `SELECT count(*)>0 FROM t WHERE a BETWEEN ?1 AND ?1+1` | Full-table range scan, aggregate accumulator and finalization; the result is Boolean even when several rows qualify |

The parameter is row index + 1 for the first four shapes and the zero-based index for `range_agg`.
The aggregate searches candidate values of `a`, rather than looking up the candidate's primary key.
The constants 3, 8, 500 and `'m'` stay fixed at every size. Prepared statements are warmed at setup;
SQLite performs the actual query at every oracle call.

The larger harness also needs Linux, GNU `time` at `/usr/bin/time`, and GNU `timeout`.
Build the ordinary prerequisites above, then the optimized standalone observer and each size:

```sh
CARGO_BUILD_JOBS=2 cargo build --release --manifest-path experiments/sqlite-on-device/harness/Cargo.toml --target-dir target
SQLITE_DIR=<amalgamation directory> SQL_ORACLE=1 NROWS=16 OUT=target/sqlite-on-device/oracle16 experiments/sqlite-on-device/build.sh
python3 experiments/sqlite-on-device/scale.py target/sqlite-on-device/oracle16 --n 16
# Repeat with NROWS=32, etc. Run heavy commands sequentially on shared machines.
# Larger heap after retaining the N=128 original-heap refusal:
SQLITE_DIR=<amalgamation directory> SQL_ORACLE=1 NROWS=128 HEAP_BYTES=1048576 OUT=target/sqlite-on-device/oracle128-h1m experiments/sqlite-on-device/build.sh
python3 experiments/sqlite-on-device/scale.py target/sqlite-on-device/oracle128-h1m --n 128
# Portable tabular report from the raw, ignored JSON:
python3 experiments/sqlite-on-device/summarize.py target/sqlite-on-device/oracle16/scale/results.json
```

Each child has an explicit default 300-second timeout and 4,096 MiB virtual-address-space limit
(`--seconds`, `--memory-mib`); reports retain process status, timeout state, stderr, stdout, wall time
and `/usr/bin/time` peak RSS. A stopped child fails the run, and its limits remain in the report.
These are shared-host experiment limits, not claimed physical limits. The entry has a
1,000,000,000-outer-step budget, each basis callee run has a 100,000,000-step budget, and the
configuration bound is 2N (row times the kick bit). Region faults stop the run. Forward/reverse phase
seconds exclude module parsing and include the observer work in that phase (setup estimation,
snapshot checks and dense comparisons). Wall time also includes module loading and process startup.

For each size/shape the complete native/device driver output and status are checked, then a separate
entry checks every row against the native truth table. The same entry in reversible mode restores the
initial full classical snapshot byte for byte and the quantum snapshot within 1e-12. Two coherent
calls with output initially 0 and initially 1 each compare **every** amplitude after **each** call
with the existing independent `oracle_support/dense.rs` matrices and native truth-table permutation.
Both coherent runs reverse all the way through setup to their original snapshots. Each successful
apply preserves the entire database, memory, stack, slots, tape and register table. No region faults
are allowed. Reversible outer execution retains one Apply tag per call; the callee history is erased.

Grover uses the native match set and a positive iteration count nearest the first theoretical peak
(at least one iteration, also for no/all matches). It checks every intermediate oracle state and
all final complex amplitudes against the independent dense calculation, plus the closed-form success
probability. The kick and fault registers must free cleanly and only row qubits remain before
measurement. No measurement is used as evidence for the distribution. The dense vector holds 64N
complex doubles (row, output and five fault bits (the 2026-11-16 decision)); it uses the existing matrix/tensor methods, without
materializing a quadratic matrix. No dense allocation bound in the older reference interpreter is
changed. All SQLite instructions still run in the device; native truth replaces SQL only in the
independent expected-state calculation.

Fast glue coverage is in `front/tests/tests/sql_oracle_glue.rs`: the actual Python generator at
N=4/8/16/32 and all five query indices, with a memory-writing fixture, forward truth tables, full
reversal, coherent XOR cancellation and signed Grover amplitudes. It needs no SQLite build.
The standalone observer's unit test also checks dense no/single/half/all-match Grover phases through
N=256. Real SQLite execution remains an explicit experiment and does not slow `cargo test`.

### Size and shape measurements

Measured 2026-11-16 with the optimized observer. All checks pass for every row below. N=16/32/64
use the original 256 KiB SQLite heap; N=128/256 use 1 MiB. History-versus-branches compare mode also
passes every coherent/Grover entry at N=16/32/64 (its timings are separate rows in the TSV). (Since 2026-11-17 the executor computes `apply` one way only, by evaluation for each definite value of the arguments; the mode switch these measurements used is gone.) The table
uses one classical entry per shape, executing setup followed by all N queries. Classical journal
is the full retained history before reversal. Grover region steps count configurations; region
journal is the **sum of per-run high-water bytes**, not simultaneous storage. These differ from
classical journal and process peak RSS. Full per-size/shape/mode steps, journal, forward/reverse
seconds, wall time, peak RSS, qubits, configurations and amplitude errors: [measurements.tsv](measurements.tsv).

| N | shape | matches | classical steps | classical journal B | Grover k | region steps | region journal sum B | success |
|---:|---|---:|---:|---:|---:|---:|---:|---:|
| 16 | like_note | 2 | 1,300,716 | 25,678,012 | 2 | 633,692 | 6,207,566 | 0.9453125 |
| 16 | int_eq | 1 | 1,218,790 | 24,120,006 | 3 | 458,814 | 4,635,903 | 0.96131897 |
| 16 | text_order | 6 | 1,228,686 | 24,312,206 | 1 | 172,730 | 1,737,501 | 0.84375 |
| 16 | two_where | 5 | 1,228,295 | 24,325,015 | 1 | 171,948 | 1,750,310 | 0.95703125 |
| 16 | range_agg | 13 | 1,509,891 | 29,579,939 | 1 | 735,140 | 7,005,234 | 0.05078125 |
| 32 | like_note | 8 | 2,690,930 | 52,362,962 | 1 | 914,068 | 8,861,066 | 1 |
| 32 | int_eq | 0 | 2,399,919 | 46,850,943 | 1 | 331,902 | 3,347,823 | -0 |
| 32 | text_order | 11 | 2,419,414 | 47,229,606 | 1 | 370,892 | 3,726,486 | 0.907714844 |
| 32 | two_where | 5 | 2,416,398 | 47,206,014 | 1 | 364,860 | 3,702,894 | 0.881347656 |
| 32 | range_agg | 26 | 3,560,138 | 68,581,098 | 1 | 2,652,340 | 25,077,978 | 0.05078125 |
| 64 | like_note | 16 | 3,702,201 | 72,035,369 | 1 | 1,600,670 | 15,585,631 | 1 |
| 64 | int_eq | 0 | 3,236,201 | 63,197,097 | 1 | 668,366 | 6,744,775 | -0 |
| 64 | text_order | 26 | 3,274,129 | 63,933,489 | 1 | 744,222 | 7,481,167 | 0.768066406 |
| 64 | two_where | 2 | 3,265,129 | 63,820,105 | 4 | 2,904,888 | 29,471,132 | 0.999182316 |
| 64 | range_agg | 51 | 7,814,231 | 148,894,839 | 1 | 9,824,426 | 92,442,517 | 0.0280151367 |
| 128 | like_note | 29 | 7,220,882 | 139,907,746 | 1 | 3,665,436 | 35,597,486 | 0.993202209 |
| 128 | int_eq | 0 | 6,061,397 | 117,902,165 | 1 | 1,345,786 | 13,586,125 | -0 |
| 128 | text_order | 52 | 6,136,969 | 119,369,625 | 1 | 1,496,930 | 15,053,585 | 0.768066406 |
| 128 | two_where | 4 | 6,119,063 | 119,144,151 | 4 | 5,844,472 | 59,312,444 | 0.999182316 |
| 128 | range_agg | 104 | 24,307,405 | 459,283,133 | 1 | 37,837,802 | 354,967,093 | 0.05078125 |
| 256 | like_note | 55 | 12,749,962 | 246,874,522 | 1 | 7,241,046 | 70,379,371 | 0.984473228 |
| 256 | int_eq | 0 | 10,492,868 | 204,001,364 | 1 | 2,725,426 | 27,494,041 | -0 |
| 256 | text_order | 113 | 10,641,287 | 206,884,231 | 1 | 3,022,264 | 30,376,908 | 0.672562599 |
| 256 | two_where | 5 | 10,606,851 | 206,456,019 | 5 | 14,766,960 | 149,743,480 | 0.999190766 |
| 256 | range_agg | 226 | 83,577,537 | 1,569,840,049 | 1 | 148,894,764 | 1,393,332,726 | 0.249153137 |

Wall times below include parsing and checks; each range spans all shapes (and both initial outputs
for coherent mode). RSS is the maximum among all four modes. Every individual measurement is in
the TSV, including forward and reverse phase times. Stats are sampled before full reversal;
reverse phase time includes replay of the coherent calls. These are observations on a shared host.

| N | forward wall s | reverse wall s | coherent wall s | Grover wall s | max RSS KiB |
|---:|---:|---:|---:|---:|---:|
| 16 | 0.827–1.083 | 0.880–1.225 | 1.548–2.868 | 0.621–1.945 | 178,636 |
| 32 | 0.675–2.854 | 0.881–2.439 | 2.642–7.057 | 0.836–2.007 | 197,588 |
| 64 | 0.576–1.645 | 0.924–2.634 | 4.827–10.002 | 1.334–2.720 | 252,548 |
| 128 | 1.376–2.279 | 1.583–7.319 | 17.103–90.278 | 2.424–5.242 | 558,348 |
| 256 | 3.332–10.583 | 5.433–24.282 | 135.460–202.439 | 5.282–22.153 | 1,642,764 |


### Measured limits and remaining work

At N=128 the original 256 KiB SQLite heap returns `SQLITE_NOMEM` (7) while the full driver
executes `like_note` at row index 24. Native exits 2 and the device returns 2 after 7,893,757
steps, with exactly identical output through the refusal. Its last printed SQLite allocation
counts are 232,704 live / 242,432 high-water bytes. Native wall/RSS: 0.011509 s / 3,024 KiB;
device wall/RSS: 7.568937 s / 180,268 KiB. It is an allocation refusal in the configured SQLite
heap, not a host-memory, dense-vector or qubit limit. The 1 MiB heap passes all five shapes and all checks at N=128 and N=256. Its largest dense amplitude
deviation is 3.14e-16. N=128 has 13 live qubits at each oracle boundary; the dense vector is only
8,192 complex doubles, about 128 KiB. N=256 uses 14 live qubits at the oracle boundary; the dense vector occupies 256 KiB.
Its largest dense amplitude deviation is 1.30e-16, and its largest process RSS is 1,642,764 KiB
(the complete aggregate reversal). The next section records the first complete-matrix stopping size under the declared bounds.

The fast fixture also exposed an executor defect independent of SQL: when the entry's slot frame
does not fit, `Executor::new` records `stack_overflow`, but `run` panics in `next_is_rewind` before
checking that outcome. The normal glue test reserves 64 KiB. The minimal refusal witness is kept
ignored, since this experiment does not edit the executor:

```sh
cargo test -p bennett-front-tests --test sql_oracle_glue an_entry_frame_that_does_not_fit -- --ignored
```

That explicit witness currently fails by panic. The repair belongs in the executor's stopped-entry
handling; then enable the witness. The successful SQL cases use the original 1 MiB device stack.

### N=512: complete-run limits and Grover

At N=512 the complete native/device driver agrees (155.682 s, 179,844 KiB peak RSS).
The matrix below retains unsuccessful checks. A timeout is the declared 300-second child limit;
it is not a device fault, physical host-memory limit, or qubit limit. The dense vector has
32,768 complex doubles (512 KiB), and the oracle boundary has 15 live register qubits.

| shape | forward | classical reverse | coherent output 0 | coherent output 1 | Grover |
|---|---|---|---|---|---|
| like_note | pass | pass | timeout | timeout | pass |
| int_eq | pass | pass | timeout | timeout | pass |
| text_order | pass | pass | timeout | timeout | pass |
| two_where | pass | pass | timeout | timeout | pass |
| range_agg | pass | process 134 | timeout | timeout | pass |

Grover checks native truth, every intermediate oracle amplitude and the final complete vector,
closed-form success, byte preservation, and clean fault/kick frees. Its final measurement is
not used to judge the distribution. Region counters below cover the complete Grover run; region journal is the cumulative sum
of per-run high-water bytes.

| shape | matches | k | region steps | region journal sum B | measured success | max amplitude error | wall s | peak RSS KiB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| like_note | 115 | 1 | 15,237,696 | 147,821,936 | 0.992001891136 | 4.16e-17 | 11.327 | 180,132 |
| int_eq | 0 | 1 | 5,519,150 | 55,644,807 | -0 | 0 | 16.114 | 179,976 |
| text_order | 232 | 1 | 6,111,808 | 61,401,088 | 0.638977050781 | 4.86e-17 | 14.567 | 179,828 |
| two_where | 6 | 7 | 41,803,202 | 423,634,673 | 0.996846047184 | 5e-16 | 16.861 | 180,152 |
| range_agg | 447 | 1 | 592,747,662 | 5,540,593,543 | 0.211494326591 | 3.12e-17 | 129.377 | 179,692 |

The first timeout is LIKE with output 0: both forward calls pass the dense comparison exactly,
with zero region faults. Forward phase: 226.066654 s, 20,232,993 outer steps, 15,237,696 region
steps, 30,476,416 region work, 295,643,872 cumulative region journal bytes and a 390,050,721-byte
outer journal. The process stops during full reversal at 300.243 s, peak RSS 882,292 KiB.
That full reversal is **not** counted as passing. No call count, native comparison, dense
comparison, memory check or fault check is reduced to make the size pass.

| failed check | wall s | peak RSS KiB | status | last phase | forward dense error |
|---|---:|---:|---:|---|---:|
| like_note / coherent / zero | 300.243 | 882,292 | 124 | reverse | 0e0 |
| like_note / coherent / one | 300.264 | 882,260 | 124 | reverse | 0e0 |
| int_eq / coherent / zero | 300.719 | 879,080 | 124 | apply 2 outer_steps 20232991 | NA |
| int_eq / coherent / one | 301.109 | 879,408 | 124 | apply 2 outer_steps 20232992 | NA |
| text_order / coherent / zero | 300.433 | 879,204 | 124 | apply 2 outer_steps 20232991 | NA |
| text_order / coherent / one | 300.347 | 878,980 | 124 | apply 2 outer_steps 20232992 | NA |
| two_where / coherent / zero | 300.316 | 878,772 | 124 | apply 2 outer_steps 20232991 | NA |
| two_where / coherent / one | 300.308 | 879,040 | 124 | apply 2 outer_steps 20232992 | NA |
| range_agg / reverse / classical | 73.983 | 2,322,360 | 134 | forward mode reverse | NA |
| range_agg / coherent / zero | 300.434 | 889,744 | 124 | apply 2 outer_steps 20232991 | NA |
| range_agg / coherent / one | 300.503 | 887,320 | 124 | apply 2 outer_steps 20232992 | NA |

The aggregate's classical reverse check aborts while building its forward history, before reversal:
`memory allocation of 4294967296 bytes failed`, process status 134, wall 73.983 s, peak RSS
2,322,360 KiB. The requested allocation alone equals the configured 4 GiB address-space cap;
this is a host allocation refusal under that cap, rather than `SQLITE_NOMEM`. Its exact-reversal
assertion is not reached. The aggregate coherent checks time out in their second apply, after the first apply passes its
dense and memory checks. The second apply, complete two-call forward check and full reversal
do not finish. Integer equality, TEXT ordering and the two-condition WHERE stop at that same
second-apply phase; only LIKE reaches full reversal before its timeout. The aggregate Grover run does complete all checks.

This baseline sweep used the executor before the later atomic-rewind fixes (`22a7cd5` production
code; intervening rebases added tests). Its observer used five fault bits. Updated-executor
validation after rebasing onto `fc47ab7` is recorded separately; the saved baseline costs are
not relabelled as measurements of the updated implementation. No cost model is inferred here.

### Revalidation after the upstream executor changes

After rebasing onto `fc47ab7`, the updated observer reran all five shapes and all modes at
N=16/32, plus all five Grover shapes at N=512, against the same saved lowered modules and native
executables. Both complete aggregate coherent reversals at N=256 also pass (227.914 / 209.356 s,
464,340 / 464,364 KiB peak RSS). All 57 cases pass. Outer steps, region steps, region work/journal/overlay counters
and retained journal sizes match the baseline exactly. New wall/RSS observations are kept in
[revalidation.tsv](revalidation.tsv), separate from the baseline table. This check exercises the
new atomic-rewind implementation without changing SQLite or substituting a predicate.

The final rebase onto `40c84d4` changes only fallback-oracle tests and handoff notes; its production
executor is the one used for this revalidation. Baseline and revalidation tables retain their own
wall/RSS observations, with identical device step and journal counters on all 57 rechecks.

Final verification after the rebase: `cargo test` passes 306 tests (16 ignored),
`front/test.sh` passes 171 (22 ignored), `cargo clippy --all-targets -- -D warnings` and
`cargo fmt --check` pass. The standalone observer's clippy, format check and dense Grover
unit test pass too. The ignored entry-frame exhaustion witness described above remains an
explicit executor debt; this experiment changes no executor implementation files.

### Independent testing pass (2026-11-17)

`edge/truth.py` rebuilds the generated table from `driver.c`'s LCG and text generator and answers every
query twice: with the host Python's own SQLite (3.45) and with a plain-Python reading of the SQL. Both agree,
and the native driver's printed truth table equals them for all eight shapes at N=16/32/64/128/256/512
(`oracle128` with the original heap stops at its `SQLITE_NOMEM` row as recorded). The match counts, the
iteration counts and the success probabilities in the tables above all follow from these sets. With
`--amplitudes` it runs each saved Grover entry to its measurement through the observer's `dump` mode and
compares every amplitude with a dense Grover iteration written in the script: all five shapes at all six sizes
agree within 2.7e-15, norm 1 within 1.3e-14, no region fault (largest run: `range_agg` at N=512, 42 s).

`edge/edge_driver.c` is a second database behind the same `experiment_setup`/`experiment_query` interface,
eight rows chosen for their edges: NULL in each column, duplicate keys, `INT64_MIN`/`INT64_MAX`, the empty
string, `'M'` against `'m'` and a two-byte UTF-8 name under BINARY collation, LIKE metacharacters and case,
integer overflow in `+` (a REAL) and in `abs` (an error, as a result code), an all-rows and a no-rows
predicate. `edge/edge.py` computes its truth table with the host SQLite and a plain reading, requires the
native driver to print the same, then runs `scale.py`'s checks on all twelve queries (until 2026-11-17 on the
eleven Boolean ones; `abs_error`, whose error row returns -1, was left out while the lookup truncated):

```sh
SQLITE_DIR=<amalgamation directory> python3 experiments/sqlite-on-device/edge/edge.py target/sqlite-on-device/edge
```

All 56 checks passed on the eleven (61 on all twelve since 2026-11-17, below): the complete native/device driver output (the `abs` error included), and for each query
the forward truth table, byte-exact classical reversal, both coherent calls against the dense matrices with
full reversal, and Grover with every amplitude. The Grover amplitudes were also compared with `truth.py`'s
dense iteration on the edge truth tables (the all-rows oracle gives every row -1/√8, the no-rows oracle +1/√8).
No wrong result, panic, hang or refusal was found.

Found then and fixed on 2026-11-17: a key absent from the table. The driver returns -1 for a query that finds
no row, and the generated `@scale_lookup` truncated the result to one bit, which marked the row; `scale.py`
refused such a table before building an oracle (every native result had to be 0 or 1), so no measured oracle
reached it. The lookups of `scale.py`, `coherent.py` and `grover.py` now compare with 1, and the refusal is
gone. `an_absent_key_in_superposition_is_not_marked` (`front/tests/tests/sql_oracle_glue.rs`) runs the
generator over a fixture whose row 5 is absent (-1), row 6 returns the count 3 and row 3 returns 1, at N=8:
the classical truth table, one coherent lookup over the uniform rows (amplitude 1/√8 at out = marked(row),
zero elsewhere) and two Grover iterations against a dense iteration in the test (sin 5θ at row 3,
cos 5θ/√7 elsewhere, θ = asin(1/√8)), each reversed to its start. It fails on the truncating lookup, which
marks rows 3, 5 and 6. Rerun with real SQLite after the change (`edge/edge.py`, 2026-11-17): native equals host
SQLite and the plain reading for all twelve queries, and all 61 checks pass (the full driver and five per query),
`abs_error` among them with six of eight rows marked, its error row unmarked (truncation marked seven). The
edge run took 190.5 s and 265,480 KiB peak RSS for its build and checks on a shared laptop.
