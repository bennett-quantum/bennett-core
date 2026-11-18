# bennett-core

One workspace for the Bennett toolchain: ordinary programs, a few marker calls, and a toolchain that decides what
runs classically, reversibly or coherently. Each piece of plumbing exists once, behind a module boundary.

The device program is the quantum circuit model driven by ordinary code: ordinary code issues `qalloc`, gates,
`measure` and `qfree` (the partial trace), and `apply` runs an ordinary function for each definite value of its
argument registers, within one step bound, XORing the result into a target register (`docs/DESIGN.md`, "The
model"). Writes made inside an applied function are undone. Calls that must share writes belong in
one applied function.

| Module | What it is | State |
|---|---|---|
| `ir` | the device program: one data structure, one reader/writer, one validator | step 2 |
| `exec` | the classical controller and sparse simulator: forward, reverse (undo journal), applied functions by evaluation or emitted gates, qubits in memory | step 2 |
| `emit` | controlled gate lowering, circuit counts, and compilation of applied integer loops by instruction dispatch or saved decisions | M2 |
| `cli` | `bennett run`, `bennett check` | step 2 |
| `front` | `bennett-front` (C++, LLVM 18): bitcode with marker calls to `ir` text; taint, outlining, lowering | step 3 |
| `opt` | optimiser and cost model | not started |
| `rt` | runtime bitcode: floating point (Berkeley SoftFloat 3e under the libgcc names), the complex helpers (libgcc 13.3.0), built with `front`; the C library (musl) with journalled files, input clock and entropy images | step 3 |

```sh
cargo test                                   # Rust + guard witnesses, in seconds; Python 3, no LLVM
cargo run -p bennett -- run examples/thesis.bn
cargo run -p bennett -- run examples/calls.bn --device count
front/test.sh                                # builds bennett-front, runs its tests (LLVM 18, clang 18, CMake)
```

Counting compiles supported applied integer functions, including loops, into gates. Run the same
circuits on the simulator with `bennett run --apply-circuit auto program.bn`. Both devices accept
`--apply-circuit step-machine` to select general instruction dispatch when control depends on qubits.
The simulator defaults to direct evaluation. Either `--machine-meaning` or `--allow-region-faults`
lets a run continue after an `apply` faults for some argument values, with the fault code in its register.
The emitted subset and refusal routes are in [DESIGN.md](docs/DESIGN.md#apply).

A C program goes in through `clang` and `bennett-front` (the markers are in `docs/MARKERS.md`):

```sh
clang -O0 -Xclang -disable-O0-optnone -gline-tables-only -Ifront/include -emit-llvm -c prog.c -o prog.bc
target/front/bennett-front prog.bc -o prog.bn --region-budget 100000 --stack-bytes 65536
cargo run -p bennett -- run prog.bn
```

For a program using the C library, build musl once and use its member resolver (LLVM/clang 18, GNU make,
Python 3; no network):

```sh
python3 rt/musl/build.py
python3 rt/musl/link.py prog.c -o prog.bn
cargo run -p bennett -- run prog.bn --entry __bennett_entry --budget 100000000
python3 rt/musl/test.py                 # library integration tests, separate from the fast suites
```

The startup enters musl's own start and exit code. Limits and options, including the arena size and the
explicit stderr fault, are in [rt/musl/README.md](rt/musl/README.md).

`bennett-front` runs its own O2 pipeline, so the input need not be optimised (it must not be vectorised).
`float`, `double` and `long double` become calls of the floating-point runtime, `target/front/float-runtime.bc` by default
(`--float-runtime file.bc` takes any bitcode defining the libgcc soft-float names). Complex products and quotients
call libgcc's six self-contained helpers (including long-double division), from
`target/front/complex-runtime.bc` (`--complex-runtime file.bc`; source, licences and checks:
[rt/libgcc](rt/libgcc/README.md)).
`--region-budget` is the step bound `B` of every `apply` the front writes; the stack is the memory above static
data.
Constructors and destructors wrap `main` by default; `bennett-front --entry name` chooses another automatic
startup entry (use the same `bennett run --entry name`). A C runtime that references the standard init/fini
array bounds owns startup instead; the front supplies those arrays for it.

The current design and language guarantees are in `docs/DESIGN.md`.
The text form of the device program is in `docs/SYNTAX.md`; `examples/` has one program per feature.

The [SQLite device experiment](experiments/sqlite-on-device/README.md#larger-sql-oracles) checks
unmodified SQLite as a SQL oracle: native truth tables, exact classical reversal, independent dense
amplitudes and Grover for LIKE, integer equality, text ordering, a two-condition WHERE and a range
aggregate. Larger sizes, measured limits and reproducible commands are recorded with the experiment.

C North Star ladder checks are in [experiments/northstar-ladder](experiments/northstar-ladder/README.md).
The C portion of rung 1 passes exhaustively on the declared Int8 functions, and rung 3 passes native
output, exact reset and deterministic tape replay. Rung 2's ten original cases pass native agreement and
byte-exact reversal at both compiler inputs, under the unchanged resource cap. An independent second
interpreter agreed on rungs 1 and 2 until 2026-11-17, when it was removed with the forms it modelled
(`HANDOFF.md`, "Debts"). The experiment records the profile, complete domains and measurements.

Repository checks and the optional exact-revision pre-push hook: [docs/CHECKS.md](docs/CHECKS.md). The dated measurement and testing records are in [docs/RECORDS.md](docs/RECORDS.md). The current state, decisions and next steps are in `HANDOFF.md`.
