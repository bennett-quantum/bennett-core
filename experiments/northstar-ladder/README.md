# North Star ladder, C rungs 1–3

Tests of [NorthStar §8](../../docs/NorthStar.md), using today's C pipeline.
Implementation: `front/tests/tests/ladder.rs`. The unchanged archived source
functions are compiled separately from their native harnesses, so native
results are computed at run time. Every result is compared, without hashing
or reducing it to a process exit status.

## Acceptance status

**Rungs 1 and 2 are not yet established as worded:** “both executors” in the
archive meant the VM and an independently implemented ISA interpreter.
`bennett-core` has one production executor, with forward and reversible modes.
This experiment exercises both modes and reports them separately; that is
not a claim of two independent executors. Route: run these same native
oracles and full domains through a second implementation of this restart's
device program. The archived ISA interpreter accepts a different ISA and
cannot consume these `.bn` programs. The snapshot-based whole-world test
oracle is not wired into this experiment.

Measured results, including rung 3, are pending the first run. No semantic
failure has yet been observed. Rust and Julia are outside this C-only run.

## Domain and checks

1. The five archived scalar functions: saturating add and gcd, each on all
   65,536 pairs of Int8 encodings; popcount, sign/abs and lookup, each on all
   256 Int8 encodings. Total **131,840 inputs**, no exclusions. Zero faults
   required. Compare each result byte against separate native O2 and
   O0/UBSan builds, then run both forward and reversibly from clang O0 and
   O2 bitcode. Every reversible run compares the entire entry snapshot
   after reversal, exactly, and checks that no undo step remains. Scalar
   stack: 256 bytes above static data; step budget: 10,000,000.
   These are ordinary source-function entries, not lifted XOR oracles with
   extra output/fault/dirty-ancilla input wires; §5.1's larger oracle domain
   is not claimed.
2. The archived hashtable's **ten cases**: `ht_demo_basic` and `ht_demo_grow`,
   each at `n = 0, 1, 7, 64, 1000`, preserving its native harness's order.
   Compare complete native O2 and O0/ASan/UBSan stdout, then every full i64
   result on the device, through the unchanged native harness's complete
   decimal output. `table_entry.c` includes both unchanged source files in
   one translation unit. musl supplies real startup, allocator, stdio and
   exit; no test allocator replaces it. The whole ten-case run is reversed
   back to its pre-startup entry image.
   Both clang O0 and O2 inputs, forward and reversible modes. Reversal
   compares every byte of the full memory, live slots/frames, stack pointer,
   console, outcome, steps, register state and worlds against an externally
   taken snapshot. No memory hash stands in for equality.
3. `printf("hello, world\n")`, linked to musl 1.2.5, entering through musl's
   own startup and exit. Native stdout/status compared with forward and
   reversible runs, from clang O0 and O2. Un-run to the externally captured
   reset state, re-run three times in the **same executor**, compare the
   whole final snapshot and tape exactly each time, and un-run once more.
   Console state is the journalled byte tape, not output sent to a host
   terminal. No input or entropy device is involved. This checks
   deterministic console tape replay, not serialization of the private
   undo journal (no such public API exists).

The musl runs use the linker's default 1 MiB arena and 1 MiB stack, a
10,000,000-step budget, one configuration, seed 0. Unsupported outcomes,
refusals, wrong values and non-restored bytes are test failures with the
source name, arguments, compiler level and mode as their witnesses.

## Reproduce

LLVM/clang 18, CMake, Rust/Cargo, GNU make, Python 3; no network needed.
Run from the repository root. On shared machines, apply the site's build
and heavy-run slot wrappers to these commands and set the parallelism
variables before invoking them.

```sh
export CARGO_BUILD_JOBS=2 CMAKE_BUILD_PARALLEL_LEVEL=2
cmake -S front -B target/front
cmake --build target/front --parallel 2
python3 rt/musl/build.py --jobs 2
front/test.sh --test ladder -- --nocapture
cargo test -p bennett-front-tests --test ladder -- --ignored --nocapture --test-threads=1
python3 experiments/northstar-ladder/run.py
```

The scalar exhaustive check runs in `front/test.sh`; musl tests are ignored
there because they require the separate library build. Scratch and native
binaries go to `.tmp/northstar-ladder`; scalar lowering products follow the
front harness in `target/front-tests`. A failing test exits nonzero.
`run.py` verifies fixture hashes, runs the selected tests, and writes
`.tmp/northstar-ladder/report.json` after each rung. Its default acceptance
gate stays **nonzero** while “both executors” is unmet, even if all available
mode checks pass. `--rung 3` selects hello alone; `--slot SCRIPT` wraps each
test in a shared-machine heavy slot.

## Source provenance

Copied byte-for-byte from public `bennett-quantum/bennett-conformance` at
`cabc240750e7a09df1fccf449f4b2149cb090471`, under AGPL-3.0 (the same license
as this repository):

| Local file under `fixtures/` | Archive path | SHA-256 |
|---|---|---|
| `scalars.c` | `ladder/rung-1/fixtures/c/scalars.c` | `171bb4f3fcd4e5aec23250c48ee4e5c8b07331357263c1052a18ed6025fa5f78` |
| `hashtable.c` (258 lines) | `ladder/rung-2/fixtures/hashtable.c` | `8cbd5a3698aea0da1f7187b97be34a0ed33743394f045c49c943323f94abd4b9` |
| `main_golden.c` | `ladder/rung-2/fixtures/main_golden.c` | `83c6d020567460b10e0f70aec25399b34f5dda8e2c2feb47f157f71fff0e8820` |

The archive records the table and native harness as unchanged copies of
`BennettVM.jl/test/reference/c/`. No archive file was modified. New files
`scalar_native.c`, `table_entry.c` and `hello.c` belong to this experiment.

## Risks for an independent testing pass

- Check that the native result stream's order matches every source-input
  tuple, especially signed Int8 encodings and gcd(−128, 0).
- Check reset equality includes allocator bookkeeping in linear memory,
  console state, and empty history; no approximate quantum comparison is
  needed because these programs contain no quantum operations.
- The two modes share instruction semantics; they can agree on a wrong
  answer. Native comparison is independent, but does not supply the second
  device executor required by rungs 1–2.
