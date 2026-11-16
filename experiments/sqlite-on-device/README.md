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
inspected. There is no resource obstacle at this size; no larger coherent database was tested.

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

## What the numbers do not show

These are numerical simulation and compilation evidence, not proofs, gate counts, hardware runtime,
or evidence of scaling to large databases. These first 16 rows have no long-note overflow case (notes are 49–366 bytes); they do not expose
the overflow tail seen at N=256/1024 in `sqlite-footprint`. The complete SQL engine is present, but this workload does
not exercise most of its features. The flags and VFS intentionally remove OS services rather than
implementing a host filesystem. Build and execution wall times are observations on a shared laptop,
not performance thresholds. Neither compiler, executor nor musl runtime is changed by this experiment.
