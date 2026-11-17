# journal-content: how much of a coherent region's history is irreducible

Question (`HANDOFF.md`, "Journal content"; `docs/DESIGN.md`, "The next extension"): when ordinary code runs on
every branch of a superposition, each step keeps what it destroys as history. How much of that history is the
same on every branch, how much can be recovered from what the machine still holds, and how much is irreducible?
And does the irreducible part stay constant or grow with the number of steps? The answer decides whether keeping
history under superposition costs a constant or orders of magnitude, and it is the falsifier named for two of the
audition's proposals (`docs/AUDITION.md`: B, "align journal entries by (instruction, count)"; C, "classify
journal words zero / recoverable / irreducible").

**Measured answer: linear growth survives these common-circuit producer chains, without a depth limit.**
The corrected live-state DAG witness leaves fitted slopes of 0.848/0.341/0.606/0.641 irreducible positions per
step for binary search/hash/B-tree/stack writes. Allowing already counted classical or retained journal
words as additional inputs lowers them to **0.308/0.033/0.331/0.347** (both alignments). Depth-one slopes
remain 0.840/0.388/0.711/0.781; all copy-only and depth-one tables remain unchanged.

The saving has a time price. At 8 repetitions the journal-assisted recipes re-evaluate
1,506,782/362,752/1,020,248/572,976 instructions over the 256 branches, or
9.279/18.645/4.896/10.131 instructions per recovered word instance. These are forward evaluations of
recovery DAGs, not gates or a complete reversible cleanup schedule. The residuals include common-circuit
failures and unsupported origins: **Irreducible is still an upper bound**, not proof that the underlying
information is lost. For binary search at 8 reps, 282 of 322 remaining journal-assisted positions have
individual-branch recipes; 40 require control-presence replay. Its residual is not demonstrated data loss.
Individual-branch recoverability is measured separately below. Four fitted points do not establish an
asymptotic law; unrestricted replay and optimal pebbling remain open.

## Method

- **The hook** (`crates/exec/src/measure.rs`). `Executor::measure_journal_content(keep_raw)` turns on an
  observer; it is off by default and nothing else changes. While an `apply` runs one branch, it copies each
  journal record the step already pushed (the journal itself is built and unwound by the existing code) and
  notes, for each word, a witness from the state just after the step. It writes no executor state: with it on,
  outcome, statistics, the snapshot (classical exactly, amplitudes within 1e-12) and reversal are those of the
  run with it off (`crates/exec/tests/journal_content.rs`, on the thesis, entangled, Grover and fault examples).
  At the end of each branch it checks that it copied exactly the records and words in that branch's journal.
- **Unit.** One journal word (a slot value, the old bytes of a store, a stack pointer, a source block, ...),
  regardless of its width. Words of each record kind have named roles (a `Return` has the packed callee and
  block, the callee stack pointer, and the caller's old result slot).
- **Alignment of "the same word" across branches** (as in `experiments/sqlite-footprint`):
  - **A**: (site, k), k the index of this execution of the site within the whole region run.
  - **B**: (call path, site, k), the call path being the chain of (call site, index of that call within its
    caller's frame), k counted within the frame instance.

  A position is one aligned word of one `apply` execution; each branch that reached it holds one instance.
- **Copy-only classes**, in this order of priority (`classify`, unchanged):
  - **Same**: every branch reached the position and all hold one value. Such a word is classical.
  - **Recoverable**: every instance has a witness of one kind, so one circuit recovers it on every branch:
    - *first write*: the first write to that slot in its frame activation (the old value is executor residue
      in a slot no code of this activation defined; a machine with zeroed frames has nothing to keep);
    - *unchanged*: the step wrote the value already there;
    - *first store*: no earlier store of this branch's region run wrote these bytes, so the old bytes are the
      classical memory at region entry, at an address the machine still holds;
    - *live copy*: one live location (the same slot of the same call path, or the stack pointer) holds the
      value just after the step on every branch. When the step is un-run every later step is already undone,
      so the machine is in exactly that state again and the copy is there.

    Mixed witnesses (first write on one branch, live copy on another) do not count.
  - **Irreducible**: everything else. Recomputing a value from its producer, inverting an operation, or
    replaying the region from preserved inputs is not credited, so this is an **upper bound** for a machine
    that recomputes.
  - **Control only**, reported inside Irreducible: positions reached by some branches only, whose value is the
    same wherever reached (a `Jump` source block or a `Return`'s packed callee is fixed by the site). What they
    hold is which branches got there.
- **Depth-one recomputation** (`classify_with_recompute`). Keeps Same and Recoverable exactly as above;
  then adds **Recomputed**, before Irreducible. A Recomputed position has one common producer and one common
  live source for **each** of its slot operands on every branch that reached it. Mixed copy/recompute witnesses
  do not count. Constants are literals of that producer, never captured old values treated as free constants.
  The observer records which instruction last defined each slot and the values of its operands at that time.
  After the destructive step it finds live copies of those operands, re-evaluates that one instruction from the
  actual live sources and current memory, and checks exact equality with the journal word. Across branches it
  requires the same (producer site, producer call path, output) and intersects each operand's live-source list.
  Merely finding some individually successful recipe on every branch is insufficient.

  Supported producers: integer arithmetic, comparisons, casts, selects (all three operands must be available),
  loads (the address must be available and the **current bytes** must give the old result), function addresses
  and `stacksave`. A block parameter's producer is its actual predecessor edge's argument, a depth-one identity
  or literal; a store's producer is the value operand of the last store to exactly those bytes. These identities
  do not follow the operand's own arithmetic producer. Distinct edges to the same block are distinguished.
  Overlapping stores invalidate whole-word provenance; fragments are not assembled. Calls, allocations, jump
  source blocks, packed return control and old stack-pointer records have no recompute recipe. A returned
  callee's result is not reconstructed by replaying the call. Recipes may use any live frame's slot or the
  existing stack-pointer sources; no operand is recovered recursively. Observer snapshots of operands are only
  used to **find** copies and never substitute for a missing live input.
- **Unbounded chains** (`classify_with_chains(measurement, false)`, `chain-*` rows). Starts with the
  existing Same, Recoverable and depth-one Recomputed classes, then credits additional Recomputed positions.
  A branch-local arena records every supported dynamic producer and links its slot operands to their
  defining-time producers, before the destructive instruction executes. The links refer to dynamic values,
  not the current contents of the slot with that name. There is no depth or work cutoff; traversal uses an
  explicit stack, and cycles cannot occur because parents precede their definitions.

  For each old word, the leaves must be actual live slot/stack-pointer sources in its **post-step** state,
  or literal operands of the original instruction. A value may be copied from a different live slot with
  exactly the same bits, as in the original observer. A successful producer requires the same site, call
  path and output on every executing branch; each leaf requires one common source. The solver descends
  across all branches together, so an alias on only one branch does not prematurely stop its search.
  Constants in different producer sites are not unified. Loads read current bytes: the verifier reconstructs
  the byte writes since the defining load, checks the result with its original width mask, and rejects a
  changed value; a change followed by restoration is allowed. Arithmetic uses the executor's existing
  integer semantics. Historical values validate equalities and find aliases, and never become free inputs.
  The supported producers are exactly the depth-one set above, now linked recursively, including transfer
  arguments and store-value identities. Jump source blocks, packed return control and `longjmp` control words are kept opaque: a
  numeric slot/tape alias does not reconstruct their branch-presence predicate. Calls/returned results and allocations remain opaque origins;
  this is not callee replay. Overlapping stores still invalidate whole-word store provenance.

  Step-generated raw word instances retain their post-step context, including first writes, unchanged values and
  first stores. Those cheap witnesses can differ across branches, so they do not by themselves settle an
  aligned position. The chain search still tries one common live/tape leaf or producer DAG on all its
  instances. Copy-only and depth-one keep their original mixed-witness rules.

  Non-local jumps are observed too. Each half of a `setjmp` buffer's old bytes and each `longjmp`
  control word has its own indexed role, so distinct words never share an alignment position. Old
  buffer bytes and overwritten result slots are scalar data; the existing base/first-write/copy
  rules apply, with chain inputs where available. Jump control words get no scalar recovery witness.
  Both halves of every 32-byte `setjmp` write enter the current-memory verifier, and overlapping
  store provenance is invalidated. Tests check word-instance counts, branch-only control with live
  numeric aliases, and loads changed in either half of the buffer. Neither jump instruction is a
  supported producer for recomputing its new result; jump/call replay remains unmeasured.
- **Journal-assisted chains** (`classify_with_chains(measurement, true)`, `tape-*` rows). Uses the same
  DAG search and additionally permits one common aligned journal word as a leaf. Positions are processed
  in order of (maximum dynamic record index across their instances, alignment key). Only previously
  processed **Same or still-Irreducible** positions are available: the former are counted classical data,
  and the latter are counted retained tape. The source record must strictly precede the target record on
  **every** executing branch. Recoverable and Recomputed words are omitted and never become free inputs,
  including ones credited by depth one; their producers can instead be followed from the current state.
  Same-record and circular dependencies are disallowed. The first unhandled word stays on the tape and can
  help later words. This is an explicit greedy tape choice, not a minimum tape or a global recovery schedule.
- **Time and resistance diagnostics.** A successful common recovery DAG counts each distinct dynamic
  producer node once per recovered word. Copies and journal reads cost zero *re-evaluated instructions*;
  a store-value or block-argument identity costs one. No common subexpression cache is shared between
  different recovered words. Total cost is the sum of this count over executing word instances; average
  cost divides by recovered instances, including earlier depth-one witnesses. Maximum instructions and
  producer depth are also printed. Same/Recoverable positions have no added instruction cost. This measures
  one forward evaluation with intermediates available, excludes their clearing, scratch storage, lookup and
  control costs, and is not observer wall time. Time slopes fit total evaluations/branch against mean original
  steps, using the same four repetition counts.

  For failures the instrument records missing function/slot origins, unavailable journal-role producers,
  changed current memory/stack, and differing producer identities. It also reruns each executing branch's
  search separately with the **same retained tape**, crediting its individual copy witness where present.
  The count of remaining positions where every individual branch succeeds separates a common-circuit
  restriction from missing operands. These diagnostics come from measured dynamic graphs, not C inspection.
- **Workloads.**
  - `front/tests/c/journal_content.c`: 1,024 keys in a sorted array, an open-addressing hash table of 2,048
    slots, and a B-tree of at most 7 keys per node (170 nodes, depth 4), built classically before the qubits
    exist. Each lookup takes an 8-bit x in uniform superposition (256 branches), looks up the key at index 4x,
    and lets the accumulated result choose the next index, `reps` times (1, 2, 4, 8), so branches diverge more
    as reps grow. The write workload is the hash lookup that also counts visits per group of slots and inserts
    each found value into a sorted array, shifting larger ones up, both on its own stack. Every branch's result
    is checked against the native `-O2` build of the same file, and whole runs against it too.
  - Everything else that runs coherently in the front's tests: every function of the eleven C test programs
    that applies ordinary code (the corpus wrappers of `coherent.c`, Grover, the in-place updates with their
    2^16 and 2^17 branches, controlled code, float and `long double` code over SoftFloat), each run once with
    every argument 1; and the examples `thesis.bn`, `grover.bn`, `fault.bn`.
- **Columns.** *Max words/branch* is the longest branch journal: what a machine keeping every branch's whole
  journal in superposition holds. *Irreducible % of words* weights each position by its instances (the
  audition's "share of the journal"). *Irreducible / max words* compares the positions that need storage with
  that naive journal. Slopes are least-squares fits over reps 1, 2, 4, 8 against mean steps per branch.

## Reproduce

```sh
CARGO_BUILD_JOBS=3 CMAKE_BUILD_PARALLEL_LEVEL=3 front/test.sh --test journal_content -- --include-ignored --nocapture --test-threads=1
```

prints the underlying tables as tab-separated lines starting with `table` (several minutes on the shared laptop).
`workload` / `role` / `slope` lines retain the copy-only results; `recompute-workload` / `recompute-role` /
`recompute-slope` lines add the witness. The `chain-*` and `tape-*` rows add the two unbounded measurements. Both A and B are always printed, including unchanged runs. Workload
and role lines append Recomputed counts and instance percentages. Without `--include-ignored` only the original copy/depth-one lookup checks run;
the new chain lookup measurement and the full coherent suite stay ignored. The runner checks native results for every
lookup branch and checks that each word instance is classified exactly once and only copy-Irreducible words
move to Recomputed.

The independent checker runs separately:

```sh
CARGO_BUILD_JOBS=3 cargo test -p bennett-exec --lib recompute_independent
BENNETT_JC_BN="$PWD/target/front-tests/journal_content.bn" \
BENNETT_JC_RUNS=jc_binary_search:1,jc_binary_search:2,jc_binary_search:4,jc_binary_search:8,jc_hash_lookup:1,jc_hash_lookup:2,jc_hash_lookup:4,jc_hash_lookup:8,jc_btree_lookup:1,jc_btree_lookup:2,jc_btree_lookup:4,jc_btree_lookup:8,jc_write_lookup:1,jc_write_lookup:2,jc_write_lookup:4,jc_write_lookup:8 \
CARGO_BUILD_JOBS=3 cargo test --release -p bennett-exec --lib c_lookups_recompute_soundly -- --ignored --nocapture
```

The first measurement command writes the lowered C lookup program the heavy checker needs. That
checker independently replays every journal record on every branch, verifies credited recipes and
requires zero missed recoveries and exact agreement with its own greedy tape rerun.

The case small enough to check by hand is `HAND/@f` in `crates/exec/tests/journal_content.rs`: 14 positions,
11 Same and 3 copy-Irreducible. One of the three is a branch-only store overwriting literal 40 with 41.
Re-evaluating its defining `store i8 40, %cell` value operand returns literal 40 without any live input, so
it becomes Recomputed. The two branch-only Jump source blocks remain Irreducible. Both alignments give
**11 Same + 0 Recoverable + 1 Recomputed + 2 Irreducible**. The same test file's `ALIAS` program checks actual
arithmetic: the second multiply destroys 6/10 when producing 3/5, but its old inputs 2 and 3/5 remain in
`%saved` and `%seed`; multiplying those live values recovers the old word. Overwriting `%saved` earlier removes
the witness. The suite also rejects a changed-memory load, invalidated store provenance and disjoint operand
sources across branches, and retains the hook off/on and reversal checks.

## Copy-only results: the lookups, 256 branches each

Alignment A; B gives identical numbers on every lookup (each branch makes the same calls in the same order).

| Workload | reps | steps/branch | max words/branch | positions | same | recov. | irred. | of which control only | irred. % of words | irred. / max words | same % of words |
|---|---|---|---|---|---|---|---|---|---|---|---|
| binary search | 1 | 216 | 246 | 246 | 136 | 19 | 91 | 0 | 37.0 | 37.0 | 55.3 |
| | 2 | 366 | 441 | 450 | 145 | 79 | 226 | 36 | 50.2 | 51.2 | 34.8 |
| | 4 | 672 | 831 | 858 | 208 | 151 | 499 | 36 | 58.6 | 60.0 | 27.1 |
| | 8 | 1,295 | 1,575 | 1,636 | 316 | 297 | 1,023 | 47 | 63.7 | 65.0 | 21.4 |
| hash table | 1 | 75 | 89 | 89 | 89 | 0 | 0 | 0 | 0.0 | 0.0 | 100.0 |
| | 2 | 105 | 127 | 127 | 101 | 19 | 7 | 0 | 5.5 | 5.5 | 79.5 |
| | 4 | 165 | 203 | 203 | 121 | 48 | 34 | 0 | 16.7 | 16.7 | 59.6 |
| | 8 | 285 | 355 | 355 | 161 | 111 | 83 | 0 | 23.4 | 23.4 | 45.4 |
| B-tree | 1 | 328 | 456 | 463 | 203 | 60 | 200 | 46 | 35.6 | 43.9 | 53.5 |
| | 2 | 604 | 837 | 845 | 292 | 117 | 436 | 70 | 45.8 | 52.1 | 41.6 |
| | 4 | 1,148 | 1,539 | 1,549 | 473 | 209 | 867 | 92 | 51.2 | 56.3 | 35.4 |
| | 8 | 2,252 | 2,918 | 2,929 | 859 | 420 | 1,650 | 118 | 53.3 | 56.5 | 32.7 |
| writes memory | 1 | 494 | 576 | 576 | 576 | 0 | 0 | 0 | 0.0 | 0.0 | 100.0 |
| | 2 | 564 | 660 | 661 | 617 | 28 | 16 | 1 | 2.4 | 2.4 | 93.9 |
| | 4 | 726 | 870 | 876 | 676 | 80 | 120 | 14 | 11.8 | 13.8 | 80.2 |
| | 8 | 1,131 | 1,440 | 1,463 | 786 | 178 | 499 | 55 | 29.0 | 34.7 | 60.4 |

Slopes against steps per branch (reps 1 to 8):

| Workload | max journal words per step | irreducible positions per step | of which `Slot` | `Jump` parameters | `Jump` source | `Store` |
|---|---|---|---|---|---|---|
| binary search | 1.23 | 0.86 | 0.70 | 0.13 | 0.03 | - |
| hash table | 1.27 | 0.41 | 0.35 | 0.04 | 0.00 | - |
| B-tree | 1.27 | 0.75 | 0.61 | 0.10 | 0.04 | - |
| writes memory | 1.36 | 0.81 | 0.58 | 0.10 | 0.07 | 0.05 |

By record kind and word, all lookup runs together (A and B identical):

| Kind / word | instances | same % | recoverable % | irreducible % | positions | irreducible positions | control only |
|---|---|---|---|---|---|---|---|
| `Slot` | 2,180,809 | 39.8 | 14.1 | 46.0 | 9,318 | 4,531 | 104 |
| `Jump` parameter | 376,525 | 41.7 | 13.6 | 44.7 | 1,709 | 768 | 3 |
| `Jump` source block | 365,420 | 83.4 | 0.0 | 16.6 | 1,597 | 406 | 406 |
| `Store` (old bytes) | 77,648 | 87.4 | 4.1 | 8.5 | 318 | 39 | 2 |
| `Call` parameter | 35,840 | 68.6 | 31.4 | 0.0 | 140 | 0 | 0 |
| `Return` result slot | 19,456 | 42.1 | 48.7 | 9.2 | 76 | 7 | 0 |
| `Return` packed callee, stack pointer | 19,456 each | 100.0 | 0.0 | 0.0 | 76 each | 0 | 0 |
| `Alloca` slot, stack pointer | 2,048 each | 100.0 | 0.0 | 0.0 | 8 each | 0 | 0 |

## Copy-only results: everything else that runs coherently

67 runs. In 49 no journal word is irreducible. In 46 every word is Same; three further runs (`controlled/computed`, `grover/search_one_solution`, `coherent/coherent_calls_helper`) also use copy-Recoverable words. The zero-Irreducible runs include: every in-place update (2^16 and 2^17 branches),
the controlled-gate programs except `phase` and `block`, `thesis.c` and its variants, `kickback`, `fault_cancel`,
Grover over one solution, `folded` in the float program, the `grover.bn` and `fault.bn` examples, and the
corpus functions without a loop
(`square_plus_one`, `shifts`, `calls_helper`, `table_lookup`, `clamp_and_abs`, `local_array`). The 46 entirely Same regions
are straight-line or take the same path on every branch, and each slot is written once, over residue that is
the same on every branch. The rest (alignment A unless marked):

| Program | function | branches | steps/branch | max words/branch | positions | same | recov. | irred. | control only | irred. % of words | irred. / max words |
|---|---|---|---|---|---|---|---|---|---|---|---|
| coherent.c | collatz_steps | 258 | 445 | 1,447 | 1,448 | 47 | 333 | 1,068 | 267 | 71.3 | 73.8 |
| | recursive_split | 258 | 520 | 1,272 | 1,272 | 55 | 381 | 836 | 352 | 67.3 | 65.7 |
| | recursive_split (B) | | | | 1,272 | 56 | 704 | 512 | 424 | 40.7 | 40.3 |
| | fibonacci_mod | 258 | 675 | 2,089 | 2,089 | 49 | 917 | 1,123 | 870 | 52.0 | 53.8 |
| | bit_count | 258 | 78 | 106 | 107 | 47 | 18 | 42 | 16 | 38.7 | 39.6 |
| | gcd_with_60 | 258 | 53 | 87 | 87 | 52 | 16 | 19 | 14 | 11.8 | 21.8 |
| | compare_chain | 258 | 46 | 57 | 61 | 47 | 10 | 4 | 4 | 4.1 | 7.0 |
| | signed_divide | 258 | 47 | 52 | 52 | 43 | 7 | 2 | 2 | 4.0 | 3.8 |
| | unsigned_divide | 258 | 47 | 52 | 52 | 42 | 8 | 2 | 2 | 4.0 | 3.8 |
| | switch_on_residue | 258 | 44 | 52 | 64 | 48 | 11 | 5 | 5 | 1.8 | 9.6 |
| grover.c | search_digit_sum_12 | 907 | 19 | 101 | 103 | 37 | 38 | 28 | 14 | 26.3 | 27.7 |
| float_controlled.c | scaled | 4 | 49 | 220 | 221 | 13 | 182 | 26 | 26 | 11.2 | 11.8 |
| | ratio | 51 | 136 | 538 | 577 | 166 | 380 | 31 | 31 | 4.0 | 5.8 |
| float_long_double_controlled.c | ratio | 68 | 175 | 929 | 1,015 | 430 | 523 | 62 | 60 | 4.2 | 6.7 |
| | ratio (B) | | | | 1,015 | 432 | 523 | 60 | 60 | 4.0 | 6.5 |
| controlled.c | phase | 6 | 6 | 30 | 32 | 26 | 4 | 2 | 2 | 3.6 | 6.7 |
| | block | 15 | 8 | 46 | 47 | 39 | 5 | 3 | 3 | 3.1 | 6.5 |
| coarse.c | coarse | 13 | 3 | 34 | 37 | 29 | 6 | 2 | 2 | 3.4 | 5.9 |
| thesis.bn | main | 4 | 4 | 16 | 16 | 4 | 8 | 4 | 4 | 20.0 | 25.0 |
| | entangled | 2 | 4 | 8 | 8 | 2 | 4 | 2 | 2 | 20.0 | 25.0 |

(Branches and positions are summed over the run's `apply` executions; the front's wrappers add one-branch
`apply`s, which is why the corpus functions show 258 branches.) Alignments A and B differ on `recursive_split` and the current `long double` ratio. For
`recursive_split`, where branches recurse to different depths: B confines the shift to the frame where the paths
differ: 323 positions irreducible under A are recoverable under B (836 irreducible become 512). In the short-float,
controlled, `coarse` and `thesis.bn` runs every irreducible position is control only. The current long-double
ratio has two additional non-control irreducible positions under A.

## Results with depth-one recomputation

Same workloads, branches, steps and positions as the copy-only tables. A and B are identical for every lookup.
Same and copy-Recoverable counts retain priority; Recomputed is a separate, additional class.

| Workload | reps | same | copy recov. | recomputed | irreducible | control only | irred. % of words | irred. / max words |
|---|---|---|---|---|---|---|---|---|
| binary search | 1 | 136 | 19 | 0 | 91 | 0 | 37.0 | 37.0 |
| binary search | 2 | 145 | 79 | 7 | 219 | 34 | 48.7 | 49.7 |
| binary search | 4 | 208 | 151 | 14 | 485 | 36 | 57.0 | 58.4 |
| binary search | 8 | 316 | 297 | 25 | 998 | 47 | 62.1 | 63.4 |
| hash table | 1 | 89 | 0 | 0 | 0 | 0 | 0.0 | 0.0 |
| hash table | 2 | 101 | 19 | 2 | 5 | 0 | 3.9 | 3.9 |
| hash table | 4 | 121 | 48 | 3 | 31 | 0 | 15.3 | 15.3 |
| hash table | 8 | 161 | 111 | 4 | 79 | 0 | 22.3 | 22.3 |
| B-tree | 1 | 203 | 60 | 13 | 187 | 40 | 33.5 | 41.0 |
| B-tree | 2 | 292 | 117 | 21 | 415 | 65 | 43.5 | 49.6 |
| B-tree | 4 | 473 | 209 | 37 | 830 | 90 | 48.6 | 53.9 |
| B-tree | 8 | 859 | 420 | 85 | 1565 | 113 | 50.3 | 53.6 |
| writes memory | 1 | 576 | 0 | 0 | 0 | 0 | 0.0 | 0.0 |
| writes memory | 2 | 617 | 28 | 3 | 13 | 1 | 1.9 | 2.0 |
| writes memory | 4 | 676 | 80 | 8 | 112 | 13 | 10.9 | 12.9 |
| writes memory | 8 | 786 | 178 | 19 | 480 | 54 | 27.8 | 33.3 |

Least-squares slopes against full-precision mean steps/branch (reps 1, 2, 4, 8), both alignments.
The component slopes below are the **remaining** irreducible positions per step.

| Workload | max words/step | copy irred./step | recompute irred./step | `Slot` | `Jump` param | `Jump` source | `Store` | `Return` result |
|---|---|---|---|---|---|---|---|---|
| binary search | 1.228 | 0.862 | 0.840 | 0.687 | 0.120 | 0.033 | 0.000 | 0.000 |
| hash table | 1.267 | 0.405 | 0.388 | 0.348 | 0.030 | 0.000 | 0.000 | 0.010 |
| B-tree | 1.275 | 0.749 | 0.711 | 0.578 | 0.098 | 0.036 | 0.000 | 0.000 |
| writes memory | 1.364 | 0.810 | 0.781 | 0.562 | 0.095 | 0.067 | 0.050 | 0.007 |

All other record-kind components (`Alloca`, `Call`, return packed callee and stack pointer) have zero
irreducible slope. Among the lookup roles, only `Slot` and `Jump` parameters gain recompute witnesses:

| Kind / word | instances | same % | copy recov. % | recomputed % | irreducible % | positions | recomputed positions | irreducible positions | control only |
|---|---|---|---|---|---|---|---|---|---|---|
| `Alloca/AllocaSlot` | 2,048 | 100.0 | 0.0 | 0.0 | 0.0 | 8 | 0 | 0 | 0 |
| `Alloca/AllocaStack` | 2,048 | 100.0 | 0.0 | 0.0 | 0.0 | 8 | 0 | 0 | 0 |
| `Call/param` | 35,840 | 68.6 | 31.4 | 0.0 | 0.0 | 140 | 0 | 0 | 0 |
| `Jump/JumpSource` | 365,420 | 83.4 | 0.0 | 0.0 | 16.6 | 1,597 | 0 | 406 | 406 |
| `Jump/param` | 376,525 | 41.7 | 13.6 | 2.5 | 42.2 | 1,709 | 42 | 726 | 1 |
| `Return/ReturnPacked` | 19,456 | 100.0 | 0.0 | 0.0 | 0.0 | 76 | 0 | 0 | 0 |
| `Return/ReturnResult` | 19,456 | 42.1 | 48.7 | 0.0 | 9.2 | 76 | 0 | 7 | 0 |
| `Return/ReturnStack` | 19,456 | 100.0 | 0.0 | 0.0 | 0.0 | 76 | 0 | 0 | 0 |
| `Slot/Slot` | 2,180,809 | 39.8 | 14.1 | 2.0 | 44.0 | 9,318 | 199 | 4,332 | 84 |
| `Store/Store` | 77,648 | 87.4 | 4.1 | 0.0 | 8.5 | 318 | 0 | 39 | 2 |

On the other 67 runs, these six gain Recomputed positions; every unlisted row keeps the copy-only counts,
including all 49 zero-Irreducible runs (46 entirely Same). A and B coincide except for recursive split (and the unchanged
long-double ratio already listed above). No fixed-input run here supplies a repetition slope.

| Program / function | alignment | same | copy recov. | recomputed | irreducible | control only | irred. % of words | irred. / max words |
|---|---|---|---|---|---|---|---|---|
| grover.c / search_digit_sum_12 | A | 37 | 38 | 10 | 18 | 10 | 16.4 | 17.8 |
| coherent.c / coherent_collatz_steps | A | 47 | 333 | 89 | 979 | 183 | 64.4 | 67.7 |
| coherent.c / coherent_bit_count | A | 47 | 18 | 6 | 36 | 14 | 32.6 | 34.0 |
| coherent.c / coherent_gcd_with_60 | A | 52 | 16 | 6 | 13 | 8 | 7.4 | 14.9 |
| coherent.c / coherent_fibonacci_mod | A | 49 | 917 | 253 | 870 | 617 | 40.2 | 41.6 |
| coherent.c / coherent_recursive_split | A | 55 | 381 | 1 | 835 | 352 | 67.3 | 65.6 |
| coherent.c / coherent_recursive_split | B | 56 | 704 | 28 | 484 | 408 | 37.7 | 38.1 |

The copy-only tables were rerun with the original observer from base `a29ea90` and compared with this
observer: all 166 workload/alignment rows and all role/slope rows agree. One historical README row predates
this base: long-double `ratio` formerly reported 160 steps, 837 max words, 923 positions, 398 Same,
473 Recoverable and 52 Irreducible (all control). The table above is refreshed to the current copy-only result;
this drift is present with the original observer as well. Its cause was not measured in this experiment.
The historical table and its software remain available at `613b925`; the current command always emits the
copy-only classification as well as recomputation, so the witness comparison uses identical code and runs.

## Results with unbounded producer chains

All 83 workloads, both alignments, are in [chains.tsv](chains.tsv), including unchanged runs, role
components, failure origins and time fits. Its comment header defines every field. The corrected
four-test full reproduction passed in 333 s on the shared laptop; heavy measurements remain ignored by
default. A separate source-export build of the pre-fix observer (`a67de88`), using the same front binary,
reproduces **all 428 copy-only/depth-one data rows exactly**. Neither older mode is affected by the fix.
The corrected chain/tape output has 1,428 data rows (including fewer failure-diagnostic rows).

The independent check (`b1454ec`, `crates/exec/src/recompute_independent_tests.rs`) found a completeness
gap: some instances had cheap copy witnesses and therefore no chain context, preventing a common
recovery from being tried. Retaining that context fixes the cause. The formerly ignored
`chains_credit_a_common_live_copy_across_mixed_copy_witnesses` now passes for both alignment keys.
All 16 corrected lookup point counts and both sets of slopes agree exactly with the checker's independent
greedy rerun, for A and B; the live slopes are 0.848/0.341/0.606/0.641. Tape slopes compare as follows:

| Workload | previously published | independent corrected | fixed observer |
|---|---:|---:|---:|
| binary search | 0.434 | 0.308 | 0.308 |
| hash table | 0.109 | 0.033 | 0.033 |
| B-tree | 0.444 | 0.331 | 0.331 |
| stack writes | 0.500 | 0.347 | 0.347 |

The fresh independent replay checks hand cases, 200 random programs and all 16 C lookup runs: every
credited word re-evaluates correctly, no recovery of the stated kind is missed, and its greedy retained
tape matches the observer. On/off checks compare state, outcomes, statistics and outer journals after
every random-program step and after reversal. The observer remains off by default. Separate executor
regressions retain the indexed jump-word roles, full `setjmp` memory-write tracking and opaque
`longjmp` control rules. The workload set remains the same 83 runs.

The small new hand check is `chains_recover_vanished_operands_at_unbounded_depth`: with the saved copy overwritten before the second multiply, 6/10 is destroyed when producing 3/5. The old operand 2 is gone. Re-evaluating the initial `br loop(i8 2)` argument gives 2 (one identity), then multiplying it by live `%seed` = 3/5 gives 6/10 (one multiply): **two instructions, depth two**, both alignments. The 40-iteration variant needs depth greater than 32. A separate alternating-call-result fixture retains the first two opaque results and recovers the repeated third from the first retained word; it cannot use itself or an omitted word as a seed. Another test rejects disjoint cross-branch leaf identities even when each branch separately succeeds. The control regression inserts live numeric aliases 1/2 for the branch predecessors, yet both control-presence words remain Irreducible.

Same and copy-Recoverable counts remain exactly those in the earlier tables. A=B for every lookup. Recomputed here includes depth-one recoveries.

| Workload | reps | live Recomputed | live Irreducible | tape Recomputed | tape Irreducible |
|---|---:|---:|---:|---:|---:|
| binary search | 1 | 91 | 0 | 91 | 0 |
| binary search | 2 | 208 | 18 | 208 | 18 |
| binary search | 4 | 146 | 353 | 376 | 123 |
| binary search | 8 | 151 | 872 | 701 | 322 |
| hash table | 1 | 0 | 0 | 0 | 0 |
| hash table | 2 | 4 | 3 | 6 | 1 |
| hash table | 4 | 9 | 25 | 31 | 3 |
| hash table | 8 | 14 | 69 | 76 | 7 |
| B-tree | 1 | 49 | 151 | 120 | 80 |
| B-tree | 2 | 86 | 350 | 238 | 198 |
| B-tree | 4 | 163 | 704 | 475 | 392 |
| B-tree | 8 | 324 | 1326 | 925 | 725 |
| stack writes | 1 | 0 | 0 | 0 | 0 |
| stack writes | 2 | 8 | 8 | 13 | 3 |
| stack writes | 4 | 34 | 86 | 80 | 40 |
| stack writes | 8 | 106 | 393 | 287 | 212 |

Least-squares fits against mean original steps (full-precision inputs, reps 1/2/4/8):

| Workload | depth-one I/step | live I/step | tape I/step | live evaluations/step | tape evaluations/step |
|---|---:|---:|---:|---:|---:|
| binary search | 0.840 | 0.848 | 0.308 | -1.283 | 1.979 |
| hash table | 0.388 | 0.341 | 0.033 | 0.029 | 7.021 |
| B-tree | 0.711 | 0.606 | 0.331 | 0.408 | 1.973 |
| stack writes | 0.781 | 0.641 | 0.347 | 0.081 | 3.668 |

The live binary slope is larger than depth one despite **fewer retained positions at every point**: much more is recovered at reps 1/2 than at 4/8. A fitted slope alone is not the saving. The negative live binary time slope likewise reflects loss of certifiable recoveries at higher reps, not faster recovery of the same words. All point counts and time costs must be read together.

Forward re-evaluations of the successful recovery DAGs, summed over all 256 branches; Same/copy-Recoverable costs excluded. A/B coincide. “max” counts instructions per recovered word; depth is producer depth. Zero recovered words gives zero cost (the displayed 0 average is a convention).

| Workload | reps | mode | total evaluations | evaluations/recovered instance | max | depth |
|---|---:|---|---:|---:|---:|---:|
| binary search | 1 | live | 762,880 | 32.747 | 85 | 61 |
| binary search | 1 | tape | 708,096 | 30.396 | 82 | 59 |
| binary search | 2 | live | 1,675,773 | 33.754 | 107 | 79 |
| binary search | 2 | tape | 1,453,518 | 29.278 | 97 | 71 |
| binary search | 4 | live | 796,756 | 23.985 | 96 | 69 |
| binary search | 4 | tape | 1,402,475 | 16.401 | 105 | 77 |
| binary search | 8 | live | 795,904 | 22.777 | 85 | 61 |
| binary search | 8 | tape | 1,506,782 | 9.279 | 105 | 77 |
| hash table | 1 | live | 0 | 0.000 | 0 | 0 |
| hash table | 1 | tape | 0 | 0.000 | 0 | 0 |
| hash table | 2 | live | 2,304 | 2.250 | 4 | 4 |
| hash table | 2 | tape | 3,328 | 2.167 | 4 | 4 |
| hash table | 4 | live | 2,304 | 1.000 | 4 | 4 |
| hash table | 4 | tape | 57,600 | 7.258 | 23 | 16 |
| hash table | 8 | live | 2,304 | 0.643 | 4 | 4 |
| hash table | 8 | tape | 362,752 | 18.645 | 63 | 28 |
| B-tree | 1 | live | 26,780 | 3.029 | 9 | 9 |
| B-tree | 1 | tape | 56,271 | 2.598 | 11 | 10 |
| B-tree | 2 | live | 62,988 | 3.411 | 15 | 11 |
| B-tree | 2 | tape | 167,533 | 3.619 | 26 | 24 |
| B-tree | 4 | live | 112,261 | 2.962 | 13 | 9 |
| B-tree | 4 | tape | 406,184 | 4.110 | 26 | 24 |
| B-tree | 8 | live | 230,933 | 2.974 | 15 | 11 |
| B-tree | 8 | tape | 1,020,248 | 4.896 | 41 | 24 |
| stack writes | 1 | live | 0 | 0.000 | 0 | 0 |
| stack writes | 1 | tape | 0 | 0.000 | 0 | 0 |
| stack writes | 2 | live | 2,560 | 1.250 | 4 | 4 |
| stack writes | 2 | tape | 6,144 | 1.846 | 5 | 5 |
| stack writes | 4 | live | 13,488 | 1.948 | 10 | 10 |
| stack writes | 4 | tape | 83,077 | 4.687 | 23 | 15 |
| stack writes | 8 | live | 13,792 | 0.841 | 10 | 10 |
| stack writes | 8 | tape | 572,976 | 10.131 | 75 | 35 |

At 8 reps, the remaining tape components are measured as follows. “individual success” counts retained positions for which **every executing branch** has its own recipe or copy witness with this same retained tape; these are not certified as one common circuit.

| Workload | tape I | individual success | Slot I/step | Jump parameter I/step | Jump source I/step | Store I/step | Return-result I/step |
|---|---:|---:|---:|---:|---:|---:|---:|
| binary search | 322 | 282 | 0.170 | 0.106 | 0.033 | 0.000 | 0.000 |
| hash table | 7 | 0 | 0.033 | 0.000 | 0.000 | 0.000 | 0.000 |
| B-tree | 725 | 622 | 0.223 | 0.072 | 0.036 | 0.000 | 0.000 |
| stack writes | 212 | 150 | 0.189 | 0.046 | 0.067 | 0.045 | 0.000 |

The corrected instrument distinguishes actual barriers from the former missing-context gap. Binary
search leaves 322 tape positions at reps 8: 282 have individual-branch recipes (174 `Slot` and 108
`Jump`-parameter positions with differing producers), and 40 require control-presence replay. Hashing
leaves seven `Slot` positions, all with missing origin `@jc_hash_lookup/%call`; none has an individual
recipe with this retained tape. The B-tree leaves 725 positions: 622 have individual recipes, and 103
need control-presence replay. Its common search records differing producers at 461 `Slot` and 161
`Jump`-parameter positions; three of the latter also lack a producer on some branches.

Stack writes leave 212 positions: 150 have individual recipes and 41 need control-presence replay.
Measured individual failures include changed current memory/stack at four `Slot` positions and missing
`@jc_write_lookup/%call` origins at seven `Slot` and ten `Store` positions. The common search also
records differing producers at 57 `Slot`, 28 `Jump`-parameter and 28 `Store` positions, changed current
memory/stack at 31 `Slot` positions, missing call origins at 27 `Slot` positions, and unavailable
whole-word provenance at 25 `Store` positions. These counts overlap; they are barriers encountered by
the searched DAGs, not a partition or proof that no richer recipe exists. Numeric aliases still do not
certify control presence. The residual is **not** demonstrated information loss.

Correcting context capture changes the greedy retained tape: earlier recovered positions cease to be
available as seeds. Consequently the residual origins and instruction costs must be measured again;
they cannot be obtained by subtracting the old missed-position counts. For example, hashing now has
seven individual failures, whereas the old tape had five alongside 17 spurious common-search failures.
The fresh independent greedy rerun agrees with all corrected counts.

Thus **the linear retained-tape count survives the measured unbounded common-producer search, with a substantial time price; unavoidable linear history has not been established**. In particular, this experiment supplies evidence for investigating guarded producer choices and call replay rather than treating common-circuit failures as lost information. The tape is greedy and local to the post-step reversal state; it is not a demonstrated global optimum.

The other 67 runs: 49 have no copy-Irreducible positions, of which 46 are entirely Same and three also use copies. Every non-Same run appears below (all applies in the run combined). A=B unless separate rows are shown; exact Same/copy/Recomputed counts and total/average/depth time columns for every row remain in the TSV. No repetition slope is fitted to these fixed-input runs. The 46/49 distinction corrects the older prose, not the measured classification.

| Program / function | alignment | live I | tape I | live total evaluations | tape total evaluations |
|---|---|---:|---:|---:|---:|
| controlled / phase | A=B | 2 | 2 | 0 | 0 |
| controlled / computed | A=B | 0 | 0 | 0 | 0 |
| controlled / block | A=B | 3 | 3 | 0 | 0 |
| grover / search_one_solution | A=B | 0 | 0 | 0 | 0 |
| grover / search_digit_sum_12 | A=B | 10 | 10 | 4,236 | 4,236 |
| coarse / coarse | A=B | 2 | 2 | 0 | 0 |
| float_controlled / scaled | A=B | 26 | 26 | 0 | 0 |
| float_controlled / ratio | A=B | 31 | 31 | 0 | 0 |
| float_long_double_controlled / ratio | A | 62 | 62 | 0 | 0 |
| float_long_double_controlled / ratio | B | 60 | 60 | 0 | 0 |
| coherent / coherent_collatz_steps | A=B | 207 | 207 | 18,041,919 | 17,921,715 |
| coherent / coherent_bit_count | A=B | 16 | 16 | 13,008 | 6,480 |
| coherent / coherent_gcd_with_60 | A=B | 7 | 7 | 2,265 | 2,265 |
| coherent / coherent_signed_divide | A=B | 2 | 2 | 0 | 0 |
| coherent / coherent_unsigned_divide | A=B | 2 | 2 | 0 | 0 |
| coherent / coherent_compare_chain | A=B | 4 | 4 | 0 | 0 |
| coherent / coherent_switch_on_residue | A=B | 5 | 5 | 0 | 0 |
| coherent / coherent_fibonacci_mod | A=B | 508 | 508 | 6,883,014 | 246,578 |
| coherent / coherent_calls_helper | A=B | 0 | 0 | 0 | 0 |
| coherent / coherent_recursive_split | A | 816 | 708 | 1,240 | 1,800 |
| coherent / coherent_recursive_split | B | 445 | 340 | 18,072 | 36,918 |
| examples / thesis.bn main | A=B | 4 | 4 | 0 | 0 |
| examples / thesis.bn entangled | A=B | 2 | 2 | 0 | 0 |

## Global-memory writes on the worlds machine

**Removed on 2026-11-17** with `mark`, `rewind` and machine states in superposition: the drivers
(`front/tests/tests/journal_global.rs`, `journal_global_check.rs`), the C workloads (`front/tests/c/journal_global.c`)
and the exec crate's independent check of this section. The measurements below are those of 2026-11-16; the
commands no longer run (the last commit that has them is `f81a70a`).

The front still refuses these coherent writes. The new experiment lowers **ordinary C functions**
from `front/tests/c/journal_global.c` with the normal `CLANG_FLAGS` and front pipeline, then appends
hand-written device drivers (`front/tests/tests/journal_global.rs`). It measures seven functions at
1/2/4/8 repetitions and all 256 eight-bit inputs: a global log append, global open-addressed hash
insertion, global degree-four B-tree insertion, and the four existing lookup/stack-write functions.
The hash starts half full (1,024 of 2,048 slots); the tree starts with 256 keys and inserts, replaces,
and splits root/child nodes. A noinline recursive insertion gives genuinely different A/B alignments.
The exported hash-value array keeps both key and value stores in the optimized device program;
a regular test pins 16 executed global byte writes for two inserts. The first trial's static value
array was eliminated by optimization, and its preliminary hash numbers are not used here.

**Coverage boundary:** these classifications are the existing observer's **`apply` callee journals**,
used as a proxy for the ordinary computation in the worlds driver. The hook is still guarded by
`in_region`: it does not follow ordinary worlds, their splits, merging, or rewinds. The extension
here is a read-only executed-byte-write query, not a claim to have implemented a worlds observer.
The real worlds driver's total journal and overlay costs are measured separately using `Stats`.
In particular, its outer mark, register-read metadata, root return to the driver, keep, and rewind
record are **not classified** by these tables. `apply`'s root finishes as a region outcome instead
of returning to the ordinary driver. No byte-for-byte equality of the two journals is asserted.

`jg_setup` runs once classically to construct the tables. The harness copies only its initialized
static bytes into a fresh module data image, with a clean stack; initialization is outside every
measured run. The driver allocates `%q` (8), `%out` (32), and a spare `%r` (1), Hadamards all eight
bits of `%q`, and runs:

```text
%m = mark
%v = call @work(%q, %reps)
rewind %m keep (%v -> %out)
```

The comparison driver instead allocates a four-bit fault register and runs
`apply @work(%q, %reps) -> %out, %fault budget 100000`. Every branch's result agrees with a fresh native
`clang -O2 -DJG_NATIVE` execution of the same C source (7,168 results). Every worlds run agrees with
the `apply` driver's complete final amplitudes (including phases) and memory, restores all global
and stack bytes, merges to one world without observation, and reverses to the full initial snapshot.
The regular seven-function check also compares observer off/on snapshots and all `Stats` fields
for both drivers; ordinary-world observation correctly produces an empty collection.

Reproduce the new experiment separately; all heavy runs remain ignored by default:

```sh
CARGO_BUILD_JOBS=3 CMAKE_BUILD_PARALLEL_LEVEL=3 front/test.sh --test journal_global -- --include-ignored --nocapture --test-threads=1 > .tmp/global-final.txt
python3 experiments/journal-content/global-tables.py .tmp/global-final.txt
```

Create `.tmp` first. The derived [global.tsv](global.tsv) contains 224 classification rows (all four
modes and both alignments), 28 actual worlds-cost rows, 36 overlay-copy rows, and 56 fits. Means count
executed steps or byte writes per callee branch; repeated writes count repeatedly. `mean_all_bytes`
includes stack writes; `mean_global_bytes` counts writes below the static-data end. Each raw write
range is counted from the observer's actual memory-write trace, including partial overlaps, with no
new executor instrumentation. Its 256-branch means are reconstructed as exact multiples of 1/256
before fitting with Python's `statistics.linear_regression`.

At eight repetitions, the global-write position counts are:

| Function | alignment | max words/branch | Same | copy-Recoverable | depth-one I | live-chain I | tape Recomputed | tape I | tape Store I |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| log append | A=B | 180 | 115 | 9 | 54 | 0 | 56 | 0 | 0 |
| hash insert | A=B | 385 | 93 | 77 | 222 | 156 | 134 | 91 | 12 |
| B-tree insert | A | 5,141 | 323 | 1,048 | 4,252 | 4,152 | 1,003 | 3,344 | 5 |
| B-tree insert | B | 5,141 | 326 | 2,404 | 3,288 | 2,970 | 2,473 | 1,167 | 2 |

Tape-I instance shares are 0%, 19.42%, 61.73% (tree A), and 16.77% (tree B), respectively;
position counts and instance shares answer different questions. The maximum journal, alignment union,
and number of instances must not be interchanged. All counts and control-only components are in the TSV.
Four-point least-squares growth (positions per executed callee step) is:

| Function | alignment | max words/step | copy I/step | depth-one I/step | live I/step | tape I/step | tape I/global byte written |
|---|---|---:|---:|---:|---:|---:|---:|
| log append | A=B | 1.167 | 0.462 | 0.448 | 0.000 | 0.000 | 0.000 |
| hash insert | A=B | 1.475 | 1.010 | 1.007 | 0.705 | 0.407 | 1.560 |
| B-tree insert | A | 1.343 | 1.228 | 1.213 | 1.197 | 0.992 | 47.537 |
| B-tree insert | B | 1.343 | 0.992 | 0.897 | 0.814 | 0.295 | 15.030 |

Thus **global writes do not themselves force a growing retained tape**: the log's chain/tape residual
is zero at every point. Hash and tree residuals grow under the measured common-circuit rules, with a
large alignment effect for recursion. B-tree global writes average 26.28/42.38/71.47/87.47 bytes at
1/2/4/8 reps: later repetitions increasingly replace existing keys, so a fit against bytes is not an
asymptotic law or a causal per-byte cost. At eight reps, tape forward evaluations across all branches
are 116,224 (log), 266,914 (hash), 7,011 (tree A), and 770,075 (tree B). Fewer common recoveries under
A also means fewer evaluations credited; this does not demonstrate a cheaper way to recover the same
words. Clearing temporaries, peak scratch storage, gates, and address-lookup costs remain unmeasured.

Actual 256-world driver costs at eight repetitions:

| Function | global bytes/branch | peak journal bytes, summed worlds | peak overlay bytes | copied overlay bytes | rewind steps, summed worlds |
|---|---:|---:|---:|---:|---:|
| log append | 36 | 818,688 | 5,976 | 0 | 39,424 |
| hash insert | 64 | 1,159,024 | 11,374 | 0 | 65,317 |
| B-tree insert | 87.47 | 12,503,757 | 6,188 | 0 | 941,736 |

Every run ends with zero overlay, one world, no observation and zero rerun steps. Splitting copies
23,460 journal bytes in each of these runs: the identical driver prefix, before any branch writes.
Copied overlay is zero because that first read precedes the writes; it does **not** imply that later
splits are free. Rewind performs exactly one undo per callee step plus the entering call on each
branch (e.g. tree `256 * (3677.65625 + 1) = 941736`), with no nested-rewind exponential here.
These are semantic work counters, not elapsed rewind times or a reversible circuit's gate counts.
The journal bytes count record/word payload and metadata, not Rust container allocation/RSS; overlay
bytes count differing byte payload, excluding map-node/address overhead. Their peaks need not occur
on the same tick.

The second cost sweep prepares only two bits of `%q`, Hadamards `%r`, and inserts `call @jg_id(%r)`
after the update and before the rewind: four initial worlds each split again after writing. It measures
all three update functions at the same four repetition counts, with 0/1 MiB/4 MiB extra classical memory.
Every cost counter is exactly independent of that extra memory. Splits and merges are seven each;
unequal C paths can merge a pair before the last world splits, so peak worlds need not be eight.
At eight repetitions the late-split measurements are:

| Function | peak worlds | peak overlay bytes | copied overlay bytes | copied journal bytes | peak journal bytes |
|---|---:|---:|---:|---:|---:|
| log append | 8 | 142 | 71 | 13,033 | 26,136 |
| hash insert | 5 | 229 | 215 | 21,563 | 23,005 |
| B-tree insert | 5 | 35 | 28 | 231,301 | 237,044 |

These smaller-input paths need not be representative of the 256-world run. Held/copy counters are
not monotonically increasing with repetition count: tree copied overlay is 33 bytes at four reps,
28 at eight, although its copied journal grows from 120,829 to 231,301 bytes. Normal overlays retain
only current differences, while the journal retains overwritten values. The TSV gives all 36 points.
The complete new three-test measurement passed in 269.29 s on the shared laptop.

**Independent check** (`c_globals_check_independently` in `crates/exec/src/recompute_independent_tests.rs`,
all 28 runs; it needs the image `front/test.sh --test journal_global_check -- --ignored` writes):

```sh
BENNETT_JG_BN="$PWD/target/front-tests/journal_global_base.bn" BENNETT_JG_RUNS=jg_log:8,jg_btree_insert:8 \
CARGO_BUILD_JOBS=3 cargo test --release -p bennett-exec --lib c_globals_check_independently -- --ignored --nocapture
```

Every credited depth-one, live and tape word re-evaluates to its journal word on every branch from the
replayed state, with no missed recovery, and the live/tape Irreducible counts equal the TSV on all 28 runs.
Each world's actual journal from its `mark` to its `rewind` has the same records, kinds and words as the
`apply` branch with the same input, except: stack addresses differ by one constant per word role (8 or 16
bytes: the driver's frame), never mixed within a position, so no class changes; the root ends in `Return`
(three words, one value on all worlds) instead of `Finish`; the call adds one register-read entry; and the
92-byte shared prefix precedes the mark. So for these drivers the proxy is the world's call history. Peak
overlay recomputed from the memory itself, peak journal, and rewound steps (the records after each mark,
summed) equal `Stats` and the TSV. Of the global writes' old bytes, the log's are all Same on every branch
(it appends at a branch-independent index over zeros: 9 Store positions at eight reps, none Irreducible);
the hash's are 12 Irreducible of 16 in copy-only and tape alike, the tree's (A) 9 copy-only and 5 tape of 57.
The log's zero residual is therefore a global write that destroys nothing branch-dependent; in the two
workloads whose global writes destroy branch-dependent bytes (hash, tree) the tape residual grows, mostly
in slots (hash: 12 of its 91 tape-Irreducible positions are stores).

The existing classification implementation and [chains.tsv](chains.tsv) are unchanged. The small
preprocessor guard allowing ordinary-only inclusion produces **byte-identical 34,382-byte device
text** for the original lookup C under the same flags. The existing full measurement passed all four
tests (314.48 s on the shared laptop), with **all 1,428 published chain/tape rows exactly equal**.
The standalone-function controls in this new
experiment have a different caller, initial slot residue and device lowering than the historical
coherent wrappers, so their rows supplement rather than replace the old tables.

**Remaining route:** put the collector's branch-local shadow frames, producer DAG, write trace and
alignment counters into each world, fork/swap them with the world, share call-path interning across
its siblings, and finish captures before each world's rewind. Then classify outer read/control and
nested/multiple-region histories too. A proxy validated by final-state equality does not establish
identical histories. Guarded producer/call replay, optimal retained tapes, packing branch-exclusive
positions, global recovery schedules, and full reversible cleanup cost remain open; residual counts
are upper bounds for richer recovery witnesses. These are small finite workloads, not SQLite traces.

## Reading against the audition's falsifiers

- **B** ("share of entries whose old value differs between branches; refuted if near 1"): the share that is not
  the same on every branch is 0 to 79% on the lookups (100% minus "same % of words") and 0 to 96% elsewhere
  (`fibonacci_mod`). Not near 1 on short regions; it approaches it on the long loop-heavy ones.
- **C** ("kill: irreducible > 25% of journal, or irreducible slot words grow with step count in loop-heavy
  programs"): both conditions **hold for depth one**. Irreducible exceeds 25% on binary
  search, the B-tree, writes at 8 reps, Collatz, recursive split, Fibonacci and bit count. Remaining irreducible
  `Slot` slopes are 0.348 to 0.687 words/step. This rules out copies plus this local recomputation as the way to
  make these histories constant over the measured range. The unbounded chain results above retain positive slopes under their common-circuit rule; this does not establish unavoidable linear information loss.

## What the numbers do not show

- **Optimal replay and its full time/storage price.** Chains of supported producers now have no depth
  limit, and their forward instruction evaluations are counted. They do not synthesize inverses, replay
  calls, unify differing producer sites using a branch-controlled circuit, or optimize the retained tape.
  Operand aliases are searched in slots and stack pointers, and now in counted journal words, rather than
  arbitrary memory. Clearing DAG temporaries, their peak storage, reversible gates and lookup costs are
  unmeasured. The route to a tighter answer is guarded producer/call replay and an explicit global pebbling
  schedule, checked against the retained-tape and time tables here.
- **When values exist.** The witness is local to the state just after the destructive step. Later steps can
  overwrite its inputs; ordinary reversal restores them before this step is undone. This does not show a global
  scheme that discards all such histories simultaneously, nor use values that become available only later.
  No alternate recovery schedule or strategy for preserving/recreating later-overwritten inputs is measured.
- **Conservative recipe coverage.** All select operands must be available (copies at depth one, supported chains in the new modes). Different producer sites or call
  paths and partial-store assemblies are not unified even when a more general
  circuit could handle them. Old call results and allocations are not recomputed. Thus the remaining
  Irreducible count is an upper bound under richer witnesses, not an information-theoretic lower bound.
- **Bits.** Counts are journal words, whatever their width (the executor keeps each as up to 128 bits; most
  here are 1 to 64). The qubit cost is the sum of the widths, which is not computed.
- **Layout.** An irreducible position is counted once however many branches reach it. Packing positions that
  no branch reaches together into one register, and the cost of the circuits that recover a word (a first
  store needs a lookup of classical memory at a superposed address), are not measured.
- **Memory writes.** The original write workload writes only its own stack. Global writes are now measured
  separately above using ordinary lowered functions and hand-written drivers. Callee journals are classified
  through `apply`; actual worlds overlay/journal/rewind costs are measured, with the explicit observer gap above.
  The front still needs its worlds fallback before these coherent C calls can be expressed through markers.
- **Scale.** 8-bit superpositions (256 branches), 1,024 keys, at most 8 repetitions; slopes are fitted on four
  points. Runs other than the lookups take one argument value each (every argument 1).
- **The code measured** is the device program the front makes from `-O0` C through its own pipeline, not the
  native `-O2` code. Residue in fresh slots happens to be the same on every branch in this executor (each
  branch's journal is unwound before the next runs), so first writes count as Same; a machine whose branches do
  not start from one residue would see them as Recoverable instead.
- **Not measured on the SQLite traces** (`experiments/sqlite-footprint`), the audition's second test for C.
