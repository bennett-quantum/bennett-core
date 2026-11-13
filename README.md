# bennett-core

One workspace for the Bennett toolchain: ordinary programs, a few marker calls, and a toolchain that decides what
runs classically, reversibly or coherently. Each piece of plumbing exists once, behind a module boundary.

| Module | What it is | State |
|---|---|---|
| `ir` | the device program: one data structure, one reader/writer, one validator | step 2 |
| `exec` | the executor: forward, reverse (undo journal), coherent regions by branch enumeration, gates | step 2 |
| `cli` | `bennett run`, `bennett check` | step 2 |
| `front` | `bennett-front` (C++, LLVM 18): bitcode with marker calls to `ir` text; taint, outlining, lowering | step 3 |
| `opt` | optimiser and cost model | not started |
| `rt` | runtime and arithmetic bitcode | not started |

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

`bennett-front` runs its own O2 pipeline, so the input need not be optimised (it must not be vectorised).
The budget is the step budget of every coherent region; the stack is the memory above static data.

The design, and what is deliberately left open, is in `docs/DESIGN.md`. It is a working note, not a contract.
The text form of the device program is in `docs/SYNTAX.md`; `examples/` has one program per feature.
