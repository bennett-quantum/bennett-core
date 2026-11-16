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
# Independent executor path: same cases, checking history against branch enumeration at each apply.
BENNETT_APPLY=compare python3 experiments/sqlite-on-device/coherent.py target/sqlite-on-device/n4
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
 %b = trunc i32 %v to i1
 ret %b
}
```

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
The tables use normal history mode; compare mode reports the combined cost of both computations.

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
# Independent apply implementation, same amplitude/native/memory assertions:
SQLITE_DIR=<amalgamation directory> BENNETT_APPLY=compare python3 experiments/sqlite-on-device/grover.py target/sqlite-on-device/n4 --n 4
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
set `SQLITE_DIR` as above. The same command with `BENNETT_APPLY=compare` checks every apply
against branch enumeration. Both modes pass. The observer allows 16 configurations (the
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
or evidence of scaling to large databases. These first 16 rows have no long-note overflow case (notes are 49–366 bytes); they do not expose
the overflow tail seen at N=256/1024 in `sqlite-footprint`. The complete SQL engine is present, but this workload does
not exercise most of its features. The flags and VFS intentionally remove OS services rather than
implementing a host filesystem. Build and execution wall times are observations on a shared laptop,
not performance thresholds. Neither compiler, executor nor musl runtime is changed by this experiment.

## Larger SQL oracles

`SQL_ORACLE=1` enables four additional **Boolean** statements without changing the original four.
The schema, seed, generated row contents, bind/step/column/reset calls, SQLite source and VFS remain
those above. The new `scale.py` entry uses the generated table directly, with no SQL updates to plant
matches. Every native result must be 0 or 1 before constructing a one-bit oracle; counts are never
silently truncated to their parity.

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

Build the ordinary prerequisites above, then the optimized standalone observer and each size:

```sh
CARGO_BUILD_JOBS=2 cargo build --release --manifest-path experiments/sqlite-on-device/harness/Cargo.toml --target-dir target
SQLITE_DIR=<amalgamation directory> SQL_ORACLE=1 NROWS=16 OUT=target/sqlite-on-device/oracle16 experiments/sqlite-on-device/build.sh
python3 experiments/sqlite-on-device/scale.py target/sqlite-on-device/oracle16 --n 16
# Repeat with NROWS=32, etc. Run heavy commands sequentially on shared machines.
# Independent history-versus-branches comparison, with the same strict checks:
python3 experiments/sqlite-on-device/scale.py target/sqlite-on-device/oracle16 --n 16 --modes coherent grover --apply compare
# Portable tabular report from the raw, ignored JSON:
python3 experiments/sqlite-on-device/summarize.py target/sqlite-on-device/oracle16/scale-history/results.json
```

Each child has an explicit default 300-second timeout and 4,096 MiB virtual-address-space limit
(`--seconds`, `--memory-mib`); reports retain process status, timeout state, stderr, stdout, wall time
and `/usr/bin/time` peak RSS. A stopped child fails the run, and its limits remain in the report.
These are shared-host experiment limits, not claimed physical limits. Executor forward/reverse
seconds exclude module parsing; wall time includes parsing, snapshot checks and dense comparisons.

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
measurement. No measurement is used as evidence for the distribution. The dense vector holds 32N
complex doubles (row, output and four fault bits); it uses the existing matrix/tensor methods, without
materializing a quadratic matrix. No dense allocation bound in the older reference interpreter is
changed. All SQLite instructions still run in the device; native truth replaces SQL only in the
independent expected-state calculation.

Fast glue coverage is in `front/tests/tests/sql_oracle_glue.rs`: the actual Python generator at
N=4/8/16/32 and all five query indices, with a memory-writing fixture, forward truth tables, full
reversal, coherent XOR cancellation and signed Grover amplitudes. It needs no SQLite build.
The standalone observer's unit test also checks dense no/single/half/all-match Grover phases through
N=256. Real SQLite execution remains an explicit experiment and does not slow `cargo test`.
