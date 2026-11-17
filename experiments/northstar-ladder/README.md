# North Star ladder: C rungs 1–3

Rung 2 measured on 2026-11-17 against production base `bd2e384`, with the
persistent independent interpreter in this revision. **The C portion of all
three rungs now passes as worded** against [NorthStar §8](../../docs/NorthStar.md).
The rung-1/rung-3 receipts below remain the earlier 2026-11-16 measurements at
`5361c717bd4c05068424a0694b62175fb8a3fd36`; those full experiments were not rerun
for this storage change. Rust and Julia are outside this run.

| Acceptance | Result |
|---|---|
| 1: every Int8 source input, both implementations equal native, zero faults | PASS: 131,840 inputs per compiler input; 791,040 comparisons and 527,360 complete reversals across the two implementations and production modes |
| 2: original 258-line hashtable, native agreement and byte-exact reversal, both implementations | PASS: all ten cases at both compiler inputs, native agreement, production/reference state comparisons and independent byte-exact reversals |
| 3: musl hello, native output, reset and deterministic console tape replay | PASS: 13 exact native bytes, status 0; three identical same-executor replays and four exact resets per compiler input |

The two implementations are `bennett-exec` and the existing independently
written whole-world test interpreter, imported from
`crates/exec/tests/oracle_support/worlds.rs` with `dense.rs`'s separate integer
operations. Its snapshot storage now uses `rpds` persistent collections and
standard `Rc` copy-on-write metadata, without importing production executor
history. Two modes of the production executor do not count as two
implementations. Both share the frontend and IR parser; the native oracle
separately compiles the unchanged C.

## Closing the second-executor gap

Before changing storage, the original timeout was reproduced under the same
**120-second / 1 GiB address-space cap**. The O0 startup reached 256/512/1,024/
2,048 steps in 1.207/5.422/19.323/82.867 seconds, still in musl. At 1,024 steps,
cloning and dropping history took 9.301 ms versus 11.538 us for the current
classical state alone (139 slots, three frames, empty overlay). A ten-second
99 Hz `cpu-clock` profile collected 813 samples, none lost: slot-vector copying,
its enumeration/iteration and `Value::clone` accounted for 30.50%, 19.93%,
13.41% and 4.92% of sampled CPU time. Accumulated snapshots repeatedly cloned
all their slot vectors; this was validation cost, not a device wrong result.

The independent model still records complete classical snapshots and interprets
forward/inverse instructions from DESIGN. History, frames, slots and console
use the established `rpds` persistent vector; the normal byte overlay uses its
ordered red-black map. Slot nodes branch eight ways. Cold register/cell/mark
metadata and deadline/guard tables share standard `Rc` ownership, detaching on
write. Content equality is exact, including history and dead slots; no pointer
identity or digest replaces it. Initial prototypes without all this sharing
hit the memory cap after 524,288 steps and remained UNMEASURED. The final
representation completes both images and restores their own recorded histories.

**Result: PASS**, 41.091 seconds for the isolated optimized process (40.95 s
inside the test), with the original cap. O0/O2 independent forward times are
17.526/8.002 s; forward plus independent and production reversal 24.694/15.294 s.
Every power-of-two checkpoint, the complete final public machine state and the
reset agree with production. Both interpreters check all 195 native console
bytes and status 0, and restore every byte of their 2,108,912-byte entry memory.
No executor disagreement was found. The storage-sharing regression explicitly
failed on the old model, then passed with retained-state reversal checks.
These observations are not a throughput benchmark; peak RSS was not measured.

The runner now prepares an optimized test binary before applying the execution
cap. It exits zero for the completed rung 2; a future resource stop remains
UNMEASURED and nonzero. No case or source input was removed, sampled or weakened.

## Domains and independent checks

The five unchanged scalar functions are saturating add and gcd, each on
all 65,536 Int8 pairs, and popcount, sign/absolute-value and lookup, each on
all 256 Int8 values: **131,840 tuples**, unsigned encoding order, last
argument fastest. Gcd's magnitude 128 is returned as the Int8 bit pattern
0x80. There are no excluded inputs. Native O2 and O0/UBSan streams agree
byte for byte. The device is built from clang O0 and O2 bitcode. Every
result is compared and every reversible run restored exactly.

These are ordinary source-function entries, not lifted XOR oracles with
additional output/fault/dirty-ancilla input wires; no larger §5.1 oracle
interface domain is claimed. The frontend's normal 65,536-byte stack above
static data is preserved. Production budget: 10,000,000 steps; independent
scalar budget: 10,000, far above the observed paths.

The unchanged hashtable and its unchanged `main_golden.c` run
`ht_demo_basic` and `ht_demo_grow`, each at **n = 0, 1, 7, 64, 1000**.
`table_entry.c` only includes both files in one translation unit for the
musl linker. Real musl startup, allocator, stdio and exit execute; no test
allocator replaces them. Complete native O2 and O0/ASan/UBSan stdout agree.
The device compares all ten complete decimal i64 results, not a reduced
exit status or a hash of checksums. Both clang inputs and both production
modes pass. The independent stage uses the same linked image and a paired
production execution, comparing full public states at checkpoints and completion.

Hello is `printf("hello, world\n")`, linked to musl 1.2.5. Native stdout and
status are compared with forward and reversible execution from musl's
startup entry. The same reversible executor is un-run and re-run three
times, comparing the entire final snapshot and console byte tape each
time, then un-run once more. The private undo journal has no serialized
replay API; its size is checked on replay. Console tape determinism and
complete machine-state replay are checked directly.

All resets compare every byte of the full memory image, live frames and
slots, stack pointer, console, outcome, steps, registers and worlds with
an externally captured entry snapshot, and check empty undo history.
There is no memory digest or approximate amplitude shortcut. The
independent interpreter reverses its own history and compares its entire
initial world over its immutable base image.

Musl defaults: 1 MiB arena, 1 MiB stack, clock zero, empty entropy image,
no supplied user files; seed 0, one configuration, 10,000,000 steps.
No clock or entropy read is part of these source programs. The runtime's
lazy device initialization and allocator bookkeeping are included in the
full reset comparison.

## Measurements

Machine: x86-64 Intel i7-1365U, 12 logical CPUs; clang 18.1.3, Rust 1.99.1,
CMake 3.28.3. Shared host; two build workers for the earlier receipts, three
for the new rung-2 build. Wall times are observations, not performance gates.

Scalar production: **527,360 comparisons**, **263,680 full reversals**,
**9,320,952 forward steps**, zero faults/exclusions; **118.233 seconds**.
Independent scalar: **263,680 comparisons and full reversals**, zero
faults/exclusions; **1,278.459 seconds**. Gcd dominates that interpreter's
run: its O0 block finishes at 539.255 seconds, its O2 block at 1,277.340.
These complete sweeps are explicit slow experiments.

| Hashtable input | Steps per run, either production mode | Reversible journal bytes | Full reset memory bytes |
|---|---:|---:|---:|
| clang O0 | 804,215 | 15,994,967 | 2,108,912 |
| clang O2 | 803,622 | 16,000,182 | 2,108,912 |

All **40 full i64 comparisons** pass; output is **195 bytes**, status 0.
Forward/reversible-and-reset times: O0 0.103/0.144 seconds,
O2 0.568/0.298 seconds; 10.784 seconds including native builds and linking.
The paired production runs add 20 complete i64 comparisons and two resets;
the independent model adds its own 20 comparisons and two exact resets.
Native basic checksums, in the declared n order:
`0, 23131069394, 138063414195, 1083288249908, 54380828142828`;
grow: `4, 23131069404, 138063414247, 1083288250354, 54380828149874`.
The closed timeout and new independent measurements are recorded above.

| Hello input | Exact native stdout | Steps | Journal bytes | Reset memory bytes |
|---|---:|---:|---:|---:|
| clang O0 | 13 bytes | 3,379 | 68,371 | 2,108,896 |
| clang O2 | 13 bytes | 2,917 | 58,357 | 2,106,160 |

Each has three identical same-executor replays, four exact resets and
status 0. Test time including native compilation and musl linking: 16.80 s.
These fresh results supersede the early receipts before the file-runtime
changes; the ten table values and hello output are unchanged.

## Reproduce

LLVM/clang 18, CMake, Rust/Cargo, GNU make, Python 3; no network needed.
From the repository root:

```sh
export CARGO_BUILD_JOBS=3 CMAKE_BUILD_PARALLEL_LEVEL=3
python3 experiments/northstar-ladder/run.py --rung 2
```

The runner verifies fixture hashes, builds current frontend prerequisites
and fingerprint-checks musl, prepares the release test binary, then explicitly
runs the selected slow stages (all five if no rung is selected).
`--rung 1`, `--rung 2` or `--rung 3` selects a rung. On a shared machine,
`--slot SCRIPT` acquires the site's heavy/build slot before build/run timers
start. Do not also wrap that invocation in the same slot.

The independent table defaults to `--reference-memory-mib 1024` and
`--reference-timeout 120`. Its isolated process group, cargo children
included, is killed on timeout; core dumps are disabled. Resource stops
remain **UNMEASURED**, never passes. The full source domain is unchanged.
The named test refuses unguarded direct invocation.

Slow sweeps and musl-dependent tests are ignored in the ordinary frontend
suite and selected explicitly by this runner. Each selected exact test must
actually run; cargo's successful zero-test result cannot satisfy the gate.
`.tmp/northstar-ladder/report.json` and per-stage logs persist after each
stage, including a failure. Build receipts are in `prerequisites.txt`.
Native binaries and musl images stay in this ignored directory; scalar
lowering products follow the frontend harness in `target/front-tests`.

## Repository verification

The storage regression failed against the original deep-copying history. A
separate private mutation copy retained a history record after undo instead of
consuming it: both new storage tests failed, then both passed after restoration.
The changed Rust is test-only: the scope preview returned "No mutants to
filter", then hit the wrapper's already documented empty-JSON error. No
generated library mutation coverage is claimed for these helpers. The runner's Python changes retain the named-test,
resource-stop and zero-test guards.

After rebasing on `bd2e384`, all four required checks pass: `cargo test`
(**471 passed, 20 ignored**), `cargo clippy --all-targets -- -D warnings`,
`cargo fmt --check`, and `front/test.sh` (**182 passed, 26 ignored**).
Workspace-library clippy and tree guards also pass. Exact `--ignored --list`
discovery selects all five acceptance stages named by the runner. All source
domains remain unchanged; rung 2 passes separately under its original cap.

## Source provenance

Copied byte-for-byte from public `bennett-quantum/bennett-conformance` at
`cabc240750e7a09df1fccf449f4b2149cb090471`, AGPL-3.0 like this repository:

| Local file under `fixtures/` | Archive path | SHA-256 |
|---|---|---|
| `scalars.c` | `ladder/rung-1/fixtures/c/scalars.c` | `171bb4f3fcd4e5aec23250c48ee4e5c8b07331357263c1052a18ed6025fa5f78` |
| `hashtable.c` (258 lines) | `ladder/rung-2/fixtures/hashtable.c` | `8cbd5a3698aea0da1f7187b97be34a0ed33743394f045c49c943323f94abd4b9` |
| `main_golden.c` | `ladder/rung-2/fixtures/main_golden.c` | `83c6d020567460b10e0f70aec25399b34f5dda8e2c2feb47f157f71fff0e8820` |

Each was checked against the pinned Git blob. The archive records the table
and native harness as unchanged copies of `BennettVM.jl/test/reference/c/`.
No archive file was modified. `scalar_native.c`, `table_entry.c` and
`hello.c` are this experiment's harnesses, separate from the carried sources.

## Risks for an independent testing pass

- Native byte-stream tuple order and signed Int8 encodings, especially gcd
  returning magnitude 128. Both arities must consume every native byte.
- Full reset equality, including allocator/device initialization, dead stack
  bytes, console and empty history; not merely live allocations.
- The two implementations share the frontend/parser, but have separate
  instruction semantics; native comparison supplies another independent check.
- This closes the declared ten-case rung-2 domain, not every possible table
  workload. Larger histories can still hit the process cap; those stops remain
  unmeasured and never count as a source fault or partial-domain pass. Long
  multiworld history comparisons still walk full contents and were not profiled
  here; a shared-prefix-aware exact comparison or independent compact undo log
  is the route if that cost matters.
