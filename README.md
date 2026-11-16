# bennett-core

One workspace for the Bennett toolchain: ordinary programs, a few marker calls, and a toolchain that decides what
runs classically, reversibly or coherently. Each piece of plumbing exists once, behind a module boundary.

| Module | What it is | State |
|---|---|---|
| `ir` | the device program: one data structure, one reader/writer, one validator | step 2 |
| `exec` | the executor: forward, reverse (undo journal), history (`mark`, `rewind`), worlds, coherent regions as mark, call, rewind-keep, gates | step 2 |
| `cli` | `bennett run`, `bennett check` | step 2 |
| `front` | `bennett-front` (C++, LLVM 18): bitcode with marker calls to `ir` text; taint, outlining, lowering | step 3 |
| `opt` | optimiser and cost model | not started |
| `rt` | runtime bitcode: floating point (Berkeley SoftFloat 3e under the libgcc names), the complex helpers (libgcc 13.3.0), built with `front`; the C library (musl) | step 3 |

```sh
cargo test                                   # the Rust crates, in seconds; needs no LLVM
cargo run -p bennett -- run examples/thesis.bn
front/test.sh                                # builds bennett-front, runs its tests (LLVM 18, clang 18, CMake)
```

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
The budget is the step budget of every coherent region; the stack is the memory above static data.
Constructors and destructors wrap `main` by default; `bennett-front --entry name` chooses another automatic
startup entry (use the same `bennett run --entry name`). A C runtime that references the standard init/fini
array bounds owns startup instead; the front supplies those arrays for it.

The design, and what is deliberately left open, is in `docs/DESIGN.md`. It is a working note, not a contract.
The text form of the device program is in `docs/SYNTAX.md`; `examples/` has one program per feature.
