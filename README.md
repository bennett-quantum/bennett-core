# bennett-core

One workspace for the Bennett toolchain: ordinary programs, a few marker calls, and a toolchain that decides what
runs classically, reversibly or coherently. Each piece of plumbing exists once, behind a module boundary.

| Module | What it is | State |
|---|---|---|
| `ir` | the device program: one data structure, one reader/writer, one validator | step 2 |
| `exec` | the executor: forward, reverse (undo journal), coherent regions by branch enumeration, gates | step 2 |
| `cli` | `bennett run`, `bennett check` | step 2 |
| `front` | LLVM plugin: markers, taint, outlining, normalisation, lowering to `ir` | not started |
| `opt` | optimiser and cost model | not started |
| `rt` | runtime and arithmetic bitcode | not started |

```sh
cargo test                                   # everything, in seconds
cargo run -p bennett -- run examples/thesis.bn
```

The design, and what is deliberately left open, is in `docs/DESIGN.md`. It is a working note, not a contract.
The text form of the device program is in `docs/SYNTAX.md`; `examples/` has one program per feature.
