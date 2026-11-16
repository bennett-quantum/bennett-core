# North Star ladder, C rungs 1–3

Tests of [NorthStar §8](../../docs/NorthStar.md), using today's C pipeline.
Implementation: `front/tests/tests/ladder.rs`. The unchanged archived source
functions are compiled separately from their native harnesses, so native
results are computed at run time. Every result is compared, without hashing
or reducing it to a process exit status.

## Acceptance status

**Rung 2 is not yet established as worded:** “both executors” in the
archive meant the VM and an independently implemented ISA interpreter.
`bennett-core` has one production executor, with forward and reversible modes.
This experiment reports those modes separately. For rung 1 it additionally
runs the existing, independently written whole-world test interpreter
(`crates/exec/tests/oracle_support/worlds.rs`, with `dense.rs`'s separate
integer operations), unchanged, on every source input and reverses its own
snapshot history. It uses no production executor semantics.

Rung 2's full ten-case table execution on that independent implementation
is also implemented, with its own history reversal and exact reset check,
and awaits measurement. Route if resource-limited: improve the independent
interpreter's history storage or supply another independent interpreter. The archived
ISA interpreter accepts a different ISA and cannot consume these `.bn`
programs. Two modes alone cannot satisfy the two-implementation clause.

Rung 3 passes its acceptance test in the first measured run below; the
full runner will repeat it from the committed source. Rung 1 and rung 2's independent interpreter await their queued measurement.
Rung 2's production runs pass native agreement and exact reversal below. No semantic failure has yet been observed. Rust and
Julia are outside this C-only run.

## Measurements on 2026-11-16

Machine: x86-64 Intel i7-1365U (12 logical CPUs), clang 18.1.3,
Rust 1.99.1, CMake 3.28.3; shared laptop, two build workers.

First short integration run (the hello test body is unchanged between
`6806fbe` and `efdfade`), 19.17 seconds including native compilation and
musl linking:

| Hello input | Native stdout | Device steps | Journal bytes | Full reset memory bytes | Replay/reset checks |
|---|---:|---:|---:|---:|---|
| clang O0 | 13 bytes, exact | 3,227 | 65,131 | 2,106,920 | 3 identical replays, 4 exact resets |
| clang O2 | 13 bytes, exact | 2,698 | 53,722 | 2,104,192 | 3 identical replays, 4 exact resets |

Both native and device exit status: 0. All memory, frames, slots, stack,
console, outcome, steps, registers and worlds return exactly to the
pre-startup entry snapshot, with empty history. The complete five-stage
run is still queued; its report will supersede this early receipt.

Short production table check, 19.02 seconds including native builds and
musl linking: all ten original outputs (195 console bytes) agree with
native O2 and O0/ASan/UBSan. Both modes, both clang inputs: **40 complete
i64 comparisons**, zero faults, two full reset checks.

| Table input | Steps per run, either mode | Reversible journal bytes | Full reset memory bytes |
|---|---:|---:|---:|
| clang O0 | 803,856 | 15,987,376 | 2,108,000 |
| clang O2 | 803,263 | 15,992,591 | 2,108,016 |

Forward/reversible-and-reset wall times: O0 0.515/0.584 seconds,
O2 0.236/0.427 seconds (loaded shared host, not performance gates).
The ten native checksums in harness order are
`0, 23131069394, 138063414195, 1083288249908, 54380828142828`
for basic, and
`4, 23131069404, 138063414247, 1083288250354, 54380828149874`
for grow. As with the short hello check, this used the earlier compiled
binary whose tested function body is unchanged; the full queued runner
will confirm current source and attempt the independent table stage.
**No rung 2 pass is claimed until that stage also completes.**

## Domain and checks

1. The five archived scalar functions: saturating add and gcd, each on all
   65,536 pairs of Int8 encodings; popcount, sign/abs and lookup, each on all
   256 Int8 encodings. Total **131,840 inputs**, no exclusions. Zero faults
   required. Compare each result byte against separate native O2 and
   O0/UBSan builds, then run both forward and reversibly from clang O0 and
   O2 bitcode. Every reversible run compares the entire entry snapshot
   after reversal, exactly, and checks that no undo step remains. The
   independent interpreter additionally compares every native result and
   reverses every run to its complete initial world and empty history, on
   both clang inputs. Scalar stack: 256 bytes above static data; production
   step budget: 10,000,000, independent interpreter: 10,000.
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
   taken snapshot. No memory hash stands in for equality. The second test
   runs the identical linked image on the independent whole-world model,
   compares all ten native results and reverses its own snapshot history.
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
python3 experiments/northstar-ladder/run.py
```

The production scalar exhaustive check runs in `front/test.sh`; the slower
independent exhaustive check and musl tests are ignored there. `run.py`
explicitly runs both scalar implementations and the musl tests, including
the independent table attempt. The latter refuses direct invocation without
the runner's resource guard. Scratch and native
binaries go to `.tmp/northstar-ladder`; scalar lowering products follow the
front harness in `target/front-tests`. A failing test exits nonzero.
`run.py` verifies fixture hashes, runs the selected tests, and writes
`.tmp/northstar-ladder/report.json` after each rung. Its default acceptance
gate stays **nonzero** while any acceptance stage fails or remains unmeasured.
`--rung 3` selects hello alone; `--slot SCRIPT` acquires a
shared-machine heavy slot around the whole runner before its timers start. The independent table process is limited to
1 GiB address space and 120 seconds (configurable with
`--reference-memory-mib` and `--reference-timeout`). A resource stop is
`UNMEASURED`, not a wrong-result verdict or evidence of a semantic refusal.
The full ten-case source domain is unchanged. On timeout the entire isolated
process group is killed, so no orphan interpreter keeps running.

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
  device executor required by rung 2. Rung 1 also uses the independently
  written whole-world interpreter; assess its shared frontend/IR and separate
  instruction semantics when judging independence.
