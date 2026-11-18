# Salvage survey of the earlier workspace

What in the archived `bennett-*` repositories demonstrably worked, the evidence for it, and what carrying
each piece into this repository would cost. Ordered by the next step of the plan of the time (`docs/RECORDS.md`, "Next steps, in order") that would use it
(steps 1–6 there, then the optimiser, which is not started). The candidates are the ones that plan named; the hub's list of `wip/` branches was checked against them (§8).

Method, 2026-11-15: every piece below was read in the archive (read-only). Where the evidence was cheap
to re-run — a test suite that finishes in minutes against the checked-in artifacts, with build output
outside the archive — it was re-run in this survey and the result is marked **re-ran** with the command.
Everything else is marked **read**: the claim rests on the archive's own records, which are named so they
can be weighed. A re-run here verifies the archive piece against its own tests, not against this
repository's executor; nothing below was run through `bennett-front` or the device unless it says so.

## Summary

| Piece | Step | Verdict |
|---|---|---|
| `bennett-runtimes` clock/entropy models, VFS and syscall dispatch | 1 | Carry the code, adapted: small, ordinary C, tested. The console model is superseded here. |
| `bennett-vm` work counters; `bennett-pebble` residue/chain tables | 2 | Carry the pattern and the arithmetic that turns the measurement into a decision. No code. |
| `bennett-pebble` design, proofs, model | 3 | Carry the design as reference. Do not carry the model code. |
| `bennett-vm` history, persistent collections, sparse memory | 3 | Carry three data-structure ideas with their tests' shapes. Do not carry the code. |
| `bennett-vm` branch `wip/bake-muse-8f3` | 3 | Carry the failure it fixed, as a regression-test shape for `mark`/`rewind`. |
| (front fallback) | 4 | Nothing in the archive helps; the archive's analysis is the opposite route (§6). |
| `bennett-arith` soft-float bitcode | 5 | Do not carry as-is: its integer widths exceed the device's 128-bit cap. Value is its test oracles. |
| Bennett.jl branch-free soft-float (binary64) | 5 / optimiser | Carry later, by porting to C behind the existing `--float-runtime` interface. The one cheap port. |
| `bennett-arith` adder/multiplier/QROM circuit objects | optimiser | Carry later, as constructions and counts, when an optimiser exists to consume them. |
| SQLite analysis findings; rung-4 gate and fixtures | 6 | Carry the findings and the acceptance shape. The analysis itself never completed: there is no oracle to carry. |
| `bennett-plugin` branch `wip/rung4-plugin` | 6 | Do not carry the code. One representation idea is worth keeping on record. |

## 1. The musl runtime: device models for files and clocks (`bennett-runtimes`)

### What there is

Ordinary C, compiled into the published runtime bitcode for LLVM 18 and 23 (`dist/llvm18/`,
`dist/llvm23/`), specified in `device-models/README.md`:

- `src/device/models.c` (80 lines): three bound models whose state is plain globals —
  a **clock** (a `uint64` counter; each read writes its 8 little-endian bytes and increments it),
  **entropy** (a byte image with a position; a read past the end faults *before* any mutation),
  and a **block device** (a preserved disk image plus a sparse overlay of 512-byte sectors allocated
  with `malloc`; writes never touch the image).
- `src/device/console.c` (35 lines): a bounded output buffer with two modes (private oracle: excess
  discarded; action: capacity faults before mutation).
- `src/musl/syscall.c` (143 lines): `__bennett_rt_syscall`, the whole musl syscall boundary in one
  function — exit, `clock_gettime`/`gettimeofday`/`time`, `getrandom`, open/close/dup, read/write/
  pread/pwrite, readv/writev with whole-vector preflight, lseek, ioctl (`-ENOTTY`), and a metadata
  group delegated to the VFS. Anything unmodelled calls `__bennett_nz_fault(8)`; nothing reaches a host.
- `src/musl/vfs.c` (328 lines): a data-image filesystem — namespace, inodes, sparse 4 KiB pages and
  descriptors are ordinary program data. Initially `/`, `/dev`, `/dev/random`, `/dev/urandom`,
  `/dev/null`, `/dev/zero`, `/dev/block`, `/dev/vda`, `/dev/console` and descriptors 0–2 exist.
  `/dev/random` and `/dev/urandom` read from the entropy model; the block nodes from the block model.
- The tier-specific parts sit behind six `__bennett_rt_transport_*` functions (console write, clock,
  entropy, block read/write, exit) plus the fault function. The published device bitcode leaves the
  console transport as a call to an undefined marker; the comparison publication supplies Linux
  definitions instead (`src/device/transports.c`, `src/native/`).

This maps closely onto the boundary already decided here (`HANDOFF.md`, "Decisions in force", 2026-11-15): one function
taking a syscall number, implemented by device effects, an arena and `exit`, everything else trapping
by name. The stopped-in-flight runtime on `wip/rt-musl` implements write/writev to descriptor 1,
empty stdin, close, brk/mmap over a 1 MiB arena, signal masking and exit — and traps on `open`/
`openat`, on descriptor 2, and on every clock or randomness syscall, none of which it has. The archive
code is exactly the missing middle layer: descriptor and file semantics above transports, below musl.

### Evidence

**Re-ran** (2026-11-15, against the checked-in `dist/` bitcode, both LLVM majors; the tests
native-compile the published bitcode, they do not run it on a device):

- `tests/test_devices.py` — PASS on 18 and 23. Instruments every store in the published models and
  allocator with a preimage recorder; checks clock wrap and byte order, every entropy position/length
  over a 17-byte tape, fault-before-mutation, immutable image sectors, distant sectors, repeated
  writes, and full restoration of model and heap state followed by re-execution.
- `tests/test_file_io.py` — PASS on 18 and 23: real musl `fmemopen`/`open_memstream`/`fread`/`fclose`/
  seek/append/growth over memory images, statically linked, no host libc.
- `tests/test_libc_routes.py` — PASS on 18 and 23: file creation, sparse data, shared offsets, `FILE`
  I/O, append/truncate/rename/unlink and descriptor reuse through the VFS, plus the clock/entropy routes.

**Read**: the wider claims in `README.md` and `docs/quality.md` (byte-for-byte reproducible
publications, `scripts/gate --fast` passing in 171.77 s, 20.93% C line coverage) were not re-run; the
gate builds both publications from scratch. Executor-level acceptance of these models was the
conformance rung-3 gate's job and was still outstanding in the hub's handoff (hello-world rows passed;
the `printf` rows timed out on the executors, not on these models).

### What carrying it over would cost

Moderate, and mostly decisions rather than typing. The code to take is `syscall.c`, `vfs.c`, `vfs.h`
and the clock/entropy parts of `models.c` — about 550 lines of C with no dependency beyond musl
headers, `malloc` and the transport functions.

1. **Transports.** Define the six `__bennett_rt_transport_*` functions and the fault in core terms:
   console byte-at-a-time over `__bennett_effect(char)` (the stopped runtime already does this),
   exit via `__bennett_exit`, clock/entropy/block by calling the models directly, fault as the front's
   named trap. The archive's fault numbers (2 capacity, 7 invalid state, 8 unbound/unmodelled,
   10 entropy exhausted) are old-ISA codes; here they must become named device faults or `-errno`
   returns. The syscall layer already returns `-errno` for ordinary errors; faults are only for
   unmodelled operations and corrupt model state, which is the right split for "trap by name".
2. **Binding.** Archive models fault on *any* use, even an empty read, while their `*_bound` global is
   zero; a wrapper outside control was supposed to prepare buffer, capacity and images. Here globals
   start zero and there is no wrapper, so either startup code sets the bindings (clock bound with a
   chosen start value; entropy bound to a chosen image, possibly empty) or the binding concept is
   dropped and "no image" means the empty image. Someone must choose what time and randomness *mean*
   on this device: the archive's semantics — a deterministic counter and a fixed byte tape, both
   ordinary memory that the journal restores on reversal — fit the device, but the choice of start
   value and tape contents is currently unmade, and musl startup itself may ask for randomness.
3. **Console.** Do not carry `console.c`: here output is the journalled `effect`, already reachable
   from C. Its one idea worth keeping — capacity failure before any mutation — is a property to
   preserve if a bounded-output mode is ever added; the test technique behind it (store preimage
   journal over the published bitcode) is reusable for the front's own runtime tests.
4. **Block device.** Carry only with a use: nothing here can supply a disk image yet, and no step-1
   refusal needs one. It costs 40 lines plus its binding, so leaving it out saves a decision, not work.
5. **Tests.** The three re-ran tests become useful only once ported to run *through the device*
   (compile with the runtime, run with `bennett run`, compare output and status with native, in the
   style of `experiments/libc-probe`). The store-instrumentation undo test is the archive's substitute
   for an executor; here the executor's own forward/reverse checks subsume it.

Expected effect on step 1: `open`/`fopen` stop trapping and give musl a filesystem whose initial state
is empty — which is also what a native comparison run must then start from — and the clock and
`getrandom` syscalls that real programs call early stop being traps. The archive's bump allocator
(`src/allocator/bump.c`, zero-filling, free faults on invalid pointers, no reclaim) is **not** needed:
the stopped runtime already serves musl's own allocator from the arena via brk/mmap.

## 2. The stopped measurements: counters and the arithmetic that decides

Step 2 asks for two measurements, and the journal-content one needs a
measurement hook in the executor that changes nothing when off. Two archive pieces bear on it;
neither is code to copy.

**`bennett-vm`'s work counters** (`src/history_work.rs`, `src/execution_work.rs`, plus a third in
`src/work.rs` on `wip/bake-muse-8f3`). **Read.** The pattern: thread-local counters behind an
`ENABLED` cell checked at each record site, compiled only under `cfg(test)`, counting deterministic
events (frame visits, index probes, appended payload bytes, path copies, payload clones) rather than
wall time. Tests then assert bounds on ratios between two run lengths — the technique §3's bake
branch uses to prove a quadratic is gone. That is the shape a journal-content hook wants: off means
one predictable branch, and the numbers are machine-independent. Cost to carry: none beyond imitating
it; this repository's journal (`crates/exec/src/journal.rs`) is two `Vec`s, so the record sites are
`Journal::push`/`push_word` and the branch enumeration in `crates/exec/src/region.rs`.

**`bennett-pebble`'s residue and chain arithmetic** (`theory/kary_frontier.py`, `docs/THEORY.md`).
**Re-ran**: `python3 theory/kary_frontier.py` reproduces its table — over 85 small instances the
balanced κ-ary family's cost is within 1.523× of the exact nested optimum, and at 10⁹ steps with
1,000 live bits and 1 residue bit per step the replay factor is 551× at a 30k-qubit cap, 27× at 100k,
4.0× at 3M, with a floor of 2×; at 100 residue bits per step, 100k qubits costs 101×. The
journal-content measurement classifies journal words as the same on every branch, recoverable from
live state, or irreducible. Pebble's vocabulary is the circuit-side analogue — live-state width *m*,
residue width *r* per step, checkpoints — and its frontier is precisely the function that turns a
measured *r* into "a constant or orders of magnitude", which is the decision the measurement exists
to make. Its handoff is explicit about the gap this fills: "Nobody has measured live-state width m or
residue width r on a real workload. Every kernel-scale number is conditional on them." Cost to carry:
cite the formulas and re-derive the two-page special case the measurement needs; the scripts are
self-contained Python and can be re-run as-is.

## 3. The journalled machine: Pebble, the VM's journals, and `wip/bake-muse-8f3`

Step 3 is already designed (`docs/DESIGN.md`, "The next extension"): `mark` and `rewind … keep`,
memory as an overlay over a classical base, gates taking addresses, `apply` kept until the new forms
pass every test. The archive bears on that step in three different ways — a design with proofs, three
working data structures, and one measured failure to avoid repeating.

### 3.1 `bennett-pebble`: a proved design for shrinking history

One commit (`c364a2f`), research status by its own `README.md`: a design for compiling a lifted oracle
to a circuit by keeping, per step, only a *residue* large enough to invert that step, and for
scheduling recomputation (pebbling) under a peak-qubit cap. Contents: `docs/DESIGN.md` (the compiler),
`docs/CORE.md` (definitions), `docs/THEORY.md` (proofs; 1,579 lines), `formal/` (Lean 4 mechanisation),
`model/` (a std-only Rust executable model, about 6,200 lines), `theory/` (the Python behind the
tables) and `fixtures/` (eleven `.bobj` inputs: Collatz, gcd, popcount, an irreducible loop, …).

What demonstrably worked:

- **Lean: re-ran.** `lake build` in a copy of `formal/` completes successfully; the audit file
  (`formal/axioms.txt`) shows the 65 theorems depending on the standard axioms only. Headline results
  are the oracle contract (a computed–copied–uncomputed region is the XOR-oracle, including on
  entangled inputs), soundness of the residue calculus (Eval, Copy, InPlace, Relabel, Guard), and
  soundness and completeness of a checker for chain-game plays. Two theorems are counterexamples that
  correct the design's first revision.
- **Model tests and one cost record: re-ran.** `cargo test --offline` in `model/`: 39 passed.
  `cargo run -- --one collatz 2048 10545` reports residue width 4 and 88,395,600 Toffoli-equivalents
  at a peak of 8,662 qubits, matching the recorded frontier (Collatz at budget 2,048: 4.9M
  Toffoli-equivalents at 387k qubits down to 168M at 6.7k; residue per step 206→4 bits for Collatz,
  70→3 for gcd, 78→4 for popcount).
- **Read** (its own handoff marks these unchecked): the full measurement grid in `model/results/` was
  the delegate's last run, never re-run then or now, and the model source was never line-reviewed.
  Its scheduler is binary only; at 10⁹ steps it replays each step ~3,900 times where a wide schedule
  should need ~8 — recorded as its largest gap. Anything with a superposed address is rejected
  (`sum_db` does not compile), per-step data paths are unoptimised (~19,400 gates per Collatz step),
  and there is no checker for data-path rewrites — those rest on the exhaustive fixture tests.

Carry cost: this is reference material, not a component. Its target is ISA 0.9 oracle objects and a
gate stream; nothing here emits or consumes circuits yet, so the model code (~6,200 lines against a
different IR) should not move. What transfers, at the cost of reading and restating: (i) the residue
calculus as the vocabulary for what `rewind` must retain — the InPlace rule is the same bijection
idea the front already implements in `InPlace.h`, proved there in circuit terms; (ii) the chain game
as the checkpoint-placement problem inside a long `mark`…`rewind`, with an exact small-instance
solver to test a scheduler against; (iii) the oracle-contract proofs as the template for the
experiments step 3 already lists (`apply` equals mark, call, rewind-keep; every journalled step is a
permutation). The Lean development has no toolchain home among this repository's four checks; keep
it as cited external evidence unless that changes.

### 3.2 `bennett-vm`: journals that checkpoints do not copy

The VM is a std-only Rust executor for the old ISA (0.8, partly 0.9), about 31,000 lines with 74 test
files. As a whole it is a second executor for a different device program and should not be carried.
Three of its structures solve problems step 3 will have in this repository's executor, whose journal
today is `Vec<Undo>` plus `Vec<u128>` and whose memory is one flat, fully-copied-per-region buffer:

- `src/history.rs` (410 lines): chunked append storage (256-entry chunks behind a directory of
  `Rc`s) where a checkpoint is a handful of *cursors*, never a copy. On top of rows it tracks
  completed and cancelled segments (nested cancellation absorbs inner ranges), and sorted indexes of
  impure and measured rows answered by binary search. `restore` rolls back exactly the unpublished
  tail, including edits to earlier reversal marks. This is `mark`/`rewind` bookkeeping, already
  designed: a mark is a cheap clone of cursors, rewind is a truncate, and "did anything impure or
  measured happen since the mark" — the question the two warnings in step 4 must answer — is an
  index lookup, not a scan.
- `src/persistent.rs` (517 lines): persistent AVL maps/sequences whose updates copy only the search
  path (`O(log n)` nodes), used so that three checkpoints taken per step share history instead of
  deep-copying it.
- `src/classical_memory.rs` (349 lines): sparse paged memory as a radix tree of `Rc`ed 4 KiB pages
  with a displaced-bytes journal; checkpoints share page versions. It is the data structure behind
  "memory as an overlay over a classical base that never becomes quantum, so qubits scale with bytes
  written" — the write overlay is explicit and enumerate-able.

Evidence: **re-ran** `cargo test --lib history::` (4 passed, including frozen records surviving
chunk-tail replacement and rollback restoring marks, live segments and the append tail) and
`cargo test --lib persistent::` (3 passed, including checkpoints sharing tails at 20,000 and 40,000
rows). **Read**: the full and ignored suites and the differential tests against the pinned reference
interpreter (`docs/quality.md` records 44 unit + 311 integration tests passing, 81.71% line coverage
of the default suite); they need the pinned ISA checkout materialised and were not re-run.

Carry cost: reimplement, do not copy — the code is generic over the VM's frame/segment types and
laced with its ISA's identities, budgets and C ABI. The transferable units are the three designs and
their unit tests' shapes (a few hundred lines each in this executor's terms, where a journal word is
already a plain `u128`). The work counters of §2 come from the same place and are how the VM keeps
these structures honest; carry them together.

### 3.3 `wip/bake-muse-8f3`: the quadratic to not rebuild

Branch `wip/bake-muse-8f3` of `bennett-vm` (tip `05cc3af`; three commits on an older base, never
landed on its `main`). **Read**, including its independent review
(`docs/reviews/2026-11-07-bake-muse-8f3-r1.md`, verdict CLEAR with caveats). It measured the VM's
cooperative stepping cloning its whole event journal three times per step: 379,250 event clones per
500 steps growing to 37,542,500 per 5,000 — quadratic in run length — plus a snapshot accounting
pass pairwise-checking all events (25.5 billion pairs on the 225,956-row test object). The fix made
the journals persistent sequences (§3.2) and restricted overlap checks to events that can share a
wire. The review is careful about what remains: append is `O(log history)`, not constant; cumulative
allocations per step still grow (523→813 over the measured windows); a gate-emitting workload was
never measured, as the test object contains no quantum instructions.

Nothing here is reusable code — it is three small diffs to that VM. Its value is as a named trap for
step 3: the moment marks, branch enumeration or region entry snapshot the journal by cloning it, this
repository grows the same quadratic. Carry its test shape: a deterministic counter (clones, appended
bytes), asserted flat within a stated factor between two run lengths, on an object long enough for
the quadratic to show (theirs: 500 vs 15,000 steps, bound 2×, measured 1.61×). One such test in
`crates/exec` when `mark` lands costs little and settles the question permanently.

## 4. The front's fallback: nothing to carry

No candidate was named for step 4, and the survey found none. The archive's front end
(`bennett-plugin`) took the opposite route from the fallback decided here — a whole-program static
analysis that must converge before anything is emitted, rather than the program almost verbatim under
marks with a recognised coherent class. What that analysis cost on a real program, and why, is
recorded in §6 because it is evidence about the *workload*; as a technique for the fallback it is a
route already, in effect, decided against.

## 5. Floating point's remainder, and the optimiser's arithmetic

Step 5's open items are the complex helpers, `fmal`, 128-bit-integer conversions, `_Float16`, `__bf16`
and `__float128`. The archive has two soft-float libraries. They point in opposite directions: one is
far wider than SoftFloat but cannot currently run on the device at all; the other is narrower but
fits the interface this repository already has.

### 5.1 `bennett-arith` soft-float: complete on paper, blocked by integer width

`softfloat/` (kernel in `core.c`, 365 lines, plus about 1,000 lines of headers and generators)
publishes `dist/llvm{18,23}/bennett-arith.bc` with 1,141 entry points per major: for each of f16,
bf16, f32, f64, f80, f128 and PPC double-double — add/sub/mul/div/rem, comparisons (quiet and
signalling), conversions to and from integers up to and beyond 128 bits, sqrt, rounding operations,
min/max, `fma`, `ldexp`/`frexp`/`modf`, `powi` at many widths, and a full own-format libm
(sin…log10, atan2, pow). Rounding-mode variants take the mode as a parameter and accumulate flags
into a runtime global. An offline generator (`softfloat/instantiate.py`) emits arbitrary-format
conversions and arbitrary-width `powi` on request.

Evidence: **re-ran** only the cheap end. `SHA256SUMS` in both `dist/` trees verify; the manifests
list 1,141 exports each, no staged bodies, and two entries under `conformance_pending`
(`ppc-full-domain-arithmetic`, `elementary-full-domain-accuracy`); `softfloat/test_publication.py`
passes. The bounded execution suite (`scripts/test`, which native-compiles the published bitcode
behind a dispatcher and checks it against integer/rational oracles) was attempted in a scratch copy
and stopped at pinned-sibling resolution — the gate cache it needs is not materialised — before any
numerical test ran, so the execution evidence is **read**: the repo records exhaustive checks over
all 65,536 encodings of half and bfloat for several operations, same-major APFloat and directed-MPFR
differentials for the rest, and is explicit that finite arithmetic beyond that is "edge/seeded
differential coverage, not an exhaustive proof", that libm carries no correct-rounding claim, and
that passing the sampled tests is "measured evidence, not a universal proof".

Why it cannot be carried as the device runtime today, in order of severity:

1. **Integer width.** The kernel computes in `unsigned _BitInt(512)`; the published bitcode contains
   i192, i256, i512, i513, i1024, i1025, i2176, i4352 and wider values (counted in this survey from
   the disassembly of `dist/llvm18/bennett-arith.bc`). The device program has integers up to 128 bits
   and wider ones are refused by name (`docs/DESIGN.md`); splitting them would need the type
   legaliser the design already notes LLVM only has in SelectionDAG. Carrying the library therefore
   means rewriting its kernel on 128-bit limbs — a new implementation that happens to share an
   algorithm — not a link-time swap.
2. **Interface.** Exports are `__bennett_sf_*` with a carrier ABI, not the libgcc names the front
   emits; and the `_rm` variants write a flags global, where f32/f64/f80 here are promised (and
   arranged) to write no memory. Adapters are mechanical but must be written and measured.
3. **A weaker NaN promise.** This library canonicalises arithmetic NaN payload and sign, and maps
   x87 unnormals/pseudo-denormals to a canonical NaN. The present runtime is measured bit-for-bit
   against the hardware, NaN bits included (`rt/softfloat/README.md`). Substituting it for f32/f64/f80
   would silently shrink a promise the maintainer has on record; its only clean use is for formats
   and operations the device currently refuses.

What is worth carrying now is its *checking* apparatus, which is stronger than what step 5 will
otherwise have: independent Fraction/integer oracles that import no implementation source, directed
MPFR interval tests for libm, subnormal-binade sweeps, and the all-encodings half/bfloat exhaustion.
Those are Python and test-only C++; they transfer at reading cost and apply unchanged to whatever
implements `fmal`, the 128-bit conversions and the small formats — including the compiler-rt helpers
since decided on for the complex multiply/divide routines (`docs/RECORDS.md`, "Decisions the maintainer took on 2026-11-15: the two open choices"; since superseded by libgcc), whose own
`__mulsc3`/`__divsc3` family the arith library does not provide in any case.

### 5.2 Bennett.jl's branch-free soft-float: the cheap port, for when branches cost

`Bennett.jl` (the Julia research lab listed as information-only in the hub's manifest),
`src/softfloat/`: 32 exported binary64 primitives in a self-contained module (~9,100 lines including
a full branch-free libm: exp, log, pow, sin, tan, atan, asin, …). Every path is computed
unconditionally and combined with `ifelse`, so LLVM emits `select` rather than branches — in that
setting, branching caused false-path sensitisation in reversible compilation (documented at the head
of `src/softfloat/fadd.jl`). Only f64 exists; conversions cover f32↔f64 and integers.

Evidence: the package as a whole does not currently load in this survey's Julia depot (its `LLVM`
dependency is not instantiated; nothing was downloaded to fix that), so its full test suite — 50-odd
`test/test_soft*.jl` files — is **read** only. The soft-float module itself needs no dependency, and
was **re-ran** standalone: including `src/softfloat/softfloat.jl` alone and comparing against hardware
Float64 gave 0 mismatches in 200,000 random operand pairs each for add, sub, mul and div (NaN results
compared as a class), 0 in 300 edge pairs (±0, ±1, ±Inf, NaN, smallest subnormal, largest finite),
and an exact bit match for `sqrt(2)`. That verifies the four basic operations in this survey; the
rest of the surface rests on the unread test files and the repo's worklog.

Carry cost: the lowest of any piece in this survey *because the socket already exists* —
`bennett-front --float-runtime file.bc` accepts any bitcode defining the libgcc names, and
`front/tests/tests/softfloat.rs` already checks a runtime bit-for-bit. Porting means transliterating
the f64 arithmetic and conversion routines (~1,500 lines of the Julia, ignoring its libm, which step 1
supplies from musl instead) into C with identical `ifelse` structure, behind `libgcc.c`-style
wrappers, then running the existing bit-for-bit suite plus the §5.1 oracles. The benefit is not
correctness — SoftFloat is the reference it must match — but uniform step counts: a branch-free
`__adddf3` costs the same on every branch of a coherent region, where SoftFloat's cost varies by
operand class (today: 67 steps normal, 22 subnormal, 43 NaN; `rt/softfloat/README.md`). That matters
to the optimiser and to circuit cost, not to branch enumeration, which is why `docs/DESIGN.md`
already schedules it as "a later, cheaper alternative for the optimiser, checked bit for bit against
[SoftFloat]". This survey confirms the routines are still there, self-contained, and — for the
operations re-ran — still correct.

### 5.3 `bennett-arith` circuit objects: a catalogue for an optimiser that does not exist yet

`adders/`, `multipliers/`, `qrom/`: Python constructions (~1,700 lines) emitting ISA 0.9 objects with
certificates — Cuccaro, ripple, Gidney and QCLA adders; schoolbook, Karatsuba and tree multipliers;
Babbush–Gidney QROM and select-swap; plus the in-place bijections (add, xor, rotate, negate, bswap,
modular add and multiply-by-unit). 130 object files are published under `adders/objects/`.
**Re-ran**: `python3 adders/generate.py check` in a scratch copy reproduces all 130 files
byte-for-byte, so the catalogue is exactly what its generators say. **Read**: the correctness
receipts (the repo records 116 proofs among 129 publication receipts, read by the ISA repo's pinned
checker) and the per-family tests in `tests/circuits/`; the checker is a different repository's tool
against a different device program and was not re-run.

Carry cost: nothing here has a consumer until `opt` exists — the executor defines regions by branch
enumeration and has no circuit form for an adder object to *be*. When it does, the transferable
assets are the constructions with their gate/ancilla counts and the test conventions in
`docs/PORT_PLAN.md` (exhaustive at 8 bits, NaN as bit patterns, pinned doubling relations such as
ripple `total(2W) = 2·total(W) − 2`), re-emitted in whatever form the optimiser uses. The `.bisa`
objects themselves are ISA-0.9 text and stay behind.

## 6. A SQL query as an oracle: what the archive measured, and what it never built

The honest headline: **no lifted SQLite oracle ever existed in the archive.** The acceptance gate
(`bennett-conformance`, `ladder/rung-4/`) is red by its own `RESULTS.md` (run of 2026-11-01), and the
hub's handoff records the same for 2026-11-12: "the plugin analysis has never completed on full
SQLite". So there is no step-6 implementation to salvage. There are three assets instead: a precisely
measured explanation of where the old route breaks, a native-side evidence base that already works,
and an acceptance shape worth reusing.

### What demonstrably worked

- **Native SQLite as its own oracle (read).** `ladder/rung-4/RESULTS.md`: native SQLite equals an
  independent predicate on every row at every size up to N = 2²⁰ (all 2²⁰ rows checked), on both LLVM
  majors, with byte-identical database images across majors; unmodified SQLite compiles against the
  musl sysroot and survives link and `lto<O3>` on both. This repository's own
  `experiments/sqlite-footprint` now covers the dynamic side of the same ground.
- **SQLite over an in-memory image as a component (read).** `bennett-runtimes`
  `tests/test_sqlite_memdb.py` with `tests/sqlite_memdb.c` runs unmodified SQLite 3.54.1 over a
  read-only `sqlite3_deserialize` image with exhaustive rowids, natively. It sits in the slow suite
  (~1,500 s by `docs/quality.md`) and was not re-run. The OS shim it relies on is the pattern to
  reuse: `ladder/rung-4/fixtures/os_shim.c` registers a no-files VFS under `SQLITE_OS_OTHER=1` —
  open fails, randomness is a fixed byte pattern, the clock is a fixed constant, sleep is a no-op —
  and the database itself arrives through SQLite's own memdb VFS. Sixty-one lines, and it is the
  step-1 boundary (§1) seen from SQLite's side: fixed classical constants for everything but the
  superposed input.
- **The gate's glue (re-ran).** `python3 ladder/rung-4/check_gate.py` reports 56 tests passing in
  this survey. They include rejection of edited SQLite source, of wrong oracle shape, and of a
  "proof" label on the fixed image — the negative cases an acceptance for step 6 will need again,
  whatever produces the oracle.

### The analysis findings (all read; re-running them means hours and gigabytes)

The record is `bennett-plugin`'s exploration log on branch `wip/sqsize`,
`docs/design/2026-11-11-sqlite-analysis-divide-and-conquer.md` — 22 findings, each tagged measured,
source or conjecture, written against the *plugin's* flow-insensitive points-to/value analysis. Most
of its walls are properties of that analysis and do not transfer to this front, which performs no
whole-program points-to at all. Four findings are about SQLite itself and should be carried as
expectations for step 6:

1. **The callback/sentinel idiom collapses precision.** One destructor call through a possibly-⊤
   callee (`xDel(z)` in `sqlite3VdbeMemSetStr`, where `SQLITE_TRANSIENT` is the integer −1 cast to a
   function pointer, invisible as a sentinel to a flow-insensitive analysis) dispatched to all 40
   address-taken `void(ptr)` functions and merged 191 string literals into their parameters; 356
   indirect sites went type-wide in one run (finding 14). SQLite has ~2,400 indirect call sites.
   Here the same idiom will not blow up an analysis — there isn't one — but `call_indirect` under
   qubit control and region outlining will meet destructor and VFS function-pointer tables whose
   targets must be resolved or paid for branch by branch. Budget for this idiom first; it, not the
   B-tree, is where the old route died.
2. **Cost is brutally concentrated.** Per-function census of the 1,473-function closure (each
   function extracted alone with all globals): 1,457 finish, median 155 ms; 16 named functions
   (`sqlite3VdbeExec`, `sqlite3Pragma`, `wherePathSolver`, …) carry essentially all the pain, by two
   mechanisms — integer counter crawls (a value set growing one element per round towards ⊤) and
   large mask-enumerated integer sets (finding 2). The census method itself transfers directly:
   extract each SQLite function alone and run this front on it, to learn cheaply which functions are
   refused or expensive *here*, before attempting the closure. Expect a different short list.
3. **Earlier SQLite numbers were unsound, and why.** The analysis did not model varargs, so SQLite's
   heap was absent from every earlier result (findings 16–19); with varargs modelled the heap appears
   as two objects and different stalls follow. A field-sensitive Andersen analysis (SVF) finished the
   same module in 3.5 min / 2.6 GB with equally coarse sets (~300 objects average, 38 targets per
   indirect site), so the coarseness is normal for this program; the old solver's cost was not
   (round 1 was 85% one linear scan, finding 21, since partly indexed on the plugin's `main`).
   Anything in the archive that quotes a pre-varargs SQLite address set should be treated as void.
4. **Large address sets are usually one residue class.** Every large per-function address set
   measured — in musl (`iconv` tables: 24,227 atoms over a stride-2 range) and in SQLite (the
   0..255 character tables, the pragma-name table at stride 16) — was dense, i.e. exactly
   representable as object × (residue class in an interval) instead of an enumeration (findings 1–2);
   the connected-program sets were the exception. If this repository ever needs address sets (for
   coherent memory across regions, say), that representation is the measured right one; enumerating
   atoms is how the archive reached 44M of them in round 0.

Two method results also carry, at no cost: identical large sets were being copied, not recomputed
(4.5M slot atoms reduced 532× by content hashing, finding 3) — share immutable sets if sets ever
appear — and the idiom micro-benchmarks on `wip/idioms` record which C shapes stayed benign under
analysis (direct calls, constant-index data, bounded walks, resolved function tables, memcpy) versus
superlinear (malloc-linked lists and trees, struct-stored callbacks, many-site variadics,
pointer/integer round trips), which is a useful prior for what the *front* will find easy or hard in
SQLite even though the mechanisms differ.

### `wip/rung4-plugin`: do not carry the code

Branch `wip/rung4-plugin` of `bennett-plugin` (tip `33ff4be`) is the incremental/exact-acceleration
work on that same analysis plus a reviewed factored-points-to design note, against an older pinned
contract (dialect 0.2.10, ISA 0.8). Parts of its direction later landed on the plugin's `main` by
other commits (shared atom sets, additive-cycle acceleration — see the log's "Where the prototypes
live"); the branch itself was never resumed, and its prototypes were, by that log, throwaway and
uncommitted. **Read only.** Nothing in this repository consumes its output, and its input is a
different front end's module and marker ABI. Its one keepable idea is finding 4 above, already
recorded there: factored (object × offset-rectangle) representation with payloads shared per stored
width.

Carry cost for §6 as a whole: a day of reading, already summarised here; the fixtures
(`ladder/rung-4/fixtures/`: `oracle.c`, `host_build.c`, `native_oracle.c`, `grover.c`, `os_shim.c`,
`expect.json` and the pinned-amalgamation fetch with its hashes) if the acceptance is rebuilt in that
shape; and the per-function census as the first step-6 experiment. The rung-4 gate machinery (pins
across ten repositories, systemd units, revision selectors) is archive process and stays behind.

## 7. Re-run log for this survey

Re-ran on 2026-11-15, archive untouched, all build output outside it. Commands are given relative to
each repository.

| Piece | Command | Result |
|---|---|---|
| `bennett-pebble` model | `cargo test --offline` in `model/` | 39 passed |
| `bennett-pebble` model | `cargo run -- --one collatz 2048 10545` | residue 4; peak 8,662; 88,395,600 Toffoli-equiv. |
| `bennett-pebble` theory | `python3 theory/kary_frontier.py` | worst family/DP ratio 1.523; table as recorded |
| `bennett-pebble` Lean | `lake build` in a copy of `formal/` | build succeeded; standard axioms only |
| `bennett-vm` history | `cargo test --lib history::` | 4 passed |
| `bennett-vm` persistent | `cargo test --lib persistent::` | 3 passed |
| `bennett-runtimes` devices | `python3 tests/test_devices.py` | PASS, LLVM 18 and 23 |
| `bennett-runtimes` file I/O | `python3 tests/test_file_io.py` | PASS, LLVM 18 and 23 |
| `bennett-runtimes` libc/VFS | `python3 tests/test_libc_routes.py` | PASS, LLVM 18 and 23 |
| `bennett-arith` publications | verify `dist/llvm{18,23}/SHA256SUMS`; read manifests | hashes OK; 1,141 exports each; 2 pending items each |
| `bennett-arith` metadata | `python3 softfloat/test_publication.py` | PASS, both majors |
| `bennett-arith` circuits | `python3 adders/generate.py check` (scratch copy) | 130 files reproduced |
| Bennett.jl soft-float | standalone probe including `src/softfloat/softfloat.jl` vs hardware Float64 | 0 mismatches: 200,000 random pairs × add/sub/mul/div; 300 edge pairs; `sqrt(2)` exact |
| `bennett-conformance` rung 4 | `python3 ladder/rung-4/check_gate.py` | 56 tests, OK |

Attempted and stopped before any test ran: `bennett-arith`'s `scripts/test` in a scratch copy
(pinned-sibling gate cache not materialised), and Bennett.jl's package test suite (its `LLVM`
dependency is not installed in the survey's Julia depot; no download was made). Read without
re-running: the plugin's SQLite analysis and its 22 findings, the rung-4 gate itself, the runtimes
SQLite memdb test and full gates, the VM's full, ignored and differential suites, the bake branch's
scaling test, Pebble's measurement grid, arith's numerical oracle suites, and all gate/quality
records quoted in §§1–6.

## 8. Checked and not carried

The hub's handoff lists four further `wip/` survivals; each was checked and none is a step 1–6 asset:

- `wip/pa-sol` in `bennett-driver` and `bennett-runtimes`: native-comparison varargs storage. This
  front already implements variadics its own way (`docs/DESIGN.md`, the register save area in
  memory); the archive work solves a native-codegen problem this device does not have.
- `wip/laptop-main` in `bennett-isa-spec` and `bennett-dialect-spec`: those repositories' local
  `main`s, each carrying one issue-tracker entry the published `main` lacks (a reference-interpreter
  speed report; a dispatch row-cost formula correction). Specification bookkeeping for contracts
  this repository does not implement.
- Beyond the named candidates, `bennett-circuits`' literal specialiser — the component Pebble was
  designed to replace after it ran out of memory on a basic Collatz oracle (its state is recorded in
  `bennett-pebble`'s handoff) — was not surveyed further; the optimiser here should start from §3.1
  and §5.3, not from it.
- The archive's process machinery (lanes, beads, gates, shifts) was used only as evidence, as the
  commission required, and nothing in it is proposed for adoption.


