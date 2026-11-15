# journal-content: how much of a coherent region's history is irreducible

Question (`HANDOFF.md`, "Journal content"; `docs/DESIGN.md`, "The next extension"): when ordinary code runs on
every branch of a superposition, each step keeps what it destroys as history. How much of that history is the
same on every branch, how much can be recovered from what the machine still holds, and how much is irreducible?
And does the irreducible part stay constant or grow with the number of steps? The answer decides whether keeping
history under superposition costs a constant or orders of magnitude, and it is the falsifier named for two of the
audition's proposals (`docs/AUDITION.md`: B, "align journal entries by (instruction, count)"; C, "classify
journal words zero / recoverable / irreducible").

**Answer on these workloads.** With the witnesses defined below (copies only, no recomputation), irreducible
history **grows linearly with steps** wherever a loop runs more than once: 0.41 to 0.86 irreducible words per
step, against 1.23 to 1.36 journal words per step. The classification buys a constant factor (1.4 to 3.1 on
the lookups), not orders of magnitude. Regions without a repeating loop are the exception: there every journal
word is the same on all branches (49 of the 67 runs that are not lookups). Almost all irreducible words are old values of
registers rewritten by later loop iterations; memory stores are mostly the same on every branch. Whether
recomputation (which this measurement deliberately does not credit) turns the linear growth into a constant is
not measured here.

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
- **Classes**, in this order of priority:
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
front/test.sh --test journal_content -- --include-ignored --nocapture --test-threads=1
```

prints every table below as tab-separated lines starting with `table` (about 35 s; the in-place updates' 2^17
branches take most of it). Without `--include-ignored` only the lookups run (about 5 s, part of `front/test.sh`).
The classification is `bennett_exec::classify`; the case worked by hand is the `HAND` program in
`crates/exec/tests/journal_content.rs` (two branches, every word listed in the test's comments: 14 positions,
11 same and 3 irreducible; and a two-iteration loop, 19 positions, 11 same, 5 recoverable, 3 irreducible).

## Results: the lookups, 256 branches each

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

## Results: everything else that runs coherently

67 runs. In 49 every journal word is the same on all branches: every in-place update (2^16 and 2^17 branches),
the controlled-gate programs except `phase` and `block`, `thesis.c` and its variants, `kickback`, `fault_cancel`,
Grover over one solution, `folded` in the float program, the `grover.bn` and `fault.bn` examples, and the
corpus functions without a loop
(`square_plus_one`, `shifts`, `calls_helper`, `table_lookup`, `clamp_and_abs`, `local_array`). Their regions
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
| float_long_double_controlled.c | ratio | 68 | 160 | 837 | 923 | 398 | 473 | 52 | 52 | 3.4 | 6.2 |
| controlled.c | phase | 6 | 6 | 30 | 32 | 26 | 4 | 2 | 2 | 3.6 | 6.7 |
| | block | 15 | 8 | 46 | 47 | 39 | 5 | 3 | 3 | 3.1 | 6.5 |
| coarse.c | coarse | 13 | 3 | 34 | 37 | 29 | 6 | 2 | 2 | 3.4 | 5.9 |
| thesis.bn | main | 4 | 4 | 16 | 16 | 4 | 8 | 4 | 4 | 20.0 | 25.0 |
| | entangled | 2 | 4 | 8 | 8 | 2 | 4 | 2 | 2 | 20.0 | 25.0 |

(Branches and positions are summed over the run's `apply` executions; the front's wrappers add one-branch
`apply`s, which is why the corpus functions show 258 branches.) Alignments A and B differ only on
`recursive_split`, where branches recurse to different depths: B confines the shift to the frame where the paths
differ: 323 positions irreducible under A are recoverable under B (836 irreducible become 512). In the float,
controlled, `coarse` and `thesis.bn` runs every irreducible position is control only.

## Reading against the audition's falsifiers

- **B** ("share of entries whose old value differs between branches; refuted if near 1"): the share that is not
  the same on every branch is 0 to 79% on the lookups (100% minus "same % of words") and 0 to 96% elsewhere
  (`fibonacci_mod`). Not near 1 on short regions; it approaches it on the long loop-heavy ones.
- **C** ("kill: irreducible > 25% of journal, or irreducible slot words grow with step count in loop-heavy
  programs"): both conditions hold under these witnesses. Irreducible exceeds 25% of the journal on binary
  search, the B-tree, the write workload at 8 reps, and five other runs (`collatz_steps`, `recursive_split`,
  `fibonacci_mod`, `bit_count`, `search_digit_sum_12`); irreducible `Slot` positions grow at 0.35 to 0.70 per
  step. Because recomputation is not credited, this kills a history of copies, not a compiler-planned tape that
  recomputes.

## What the numbers do not show

- **Recomputation.** Irreducible is an upper bound: a loop-carried value such as `acc * 31 + v` or a
  temporary of an earlier iteration is irreducible here even when the producer could be re-run or inverted.
  Whether the linear growth survives Bennett-style recomputation (at its price in time) is the next
  measurement; it needs a recomputation witness in the hook (the old value equals what re-evaluating its
  defining instruction from values the post-step state holds would give).
- **Bits.** Counts are journal words, whatever their width (the executor keeps each as up to 128 bits; most
  here are 1 to 64). The qubit cost is the sum of the widths, which is not computed.
- **Layout.** An irreducible position is counted once however many branches reach it. Packing positions that
  no branch reaches together into one register, and the cost of the circuits that recover a word (a first
  store needs a lookup of classical memory at a superposed address), are not measured.
- **Memory writes.** The write workload writes only its own stack: the front refuses a coherent call to a
  function that writes memory outside its frame, so a global hash-table insert could not be measured. Route:
  the journalled machine (next step 3 in `HANDOFF.md`) lets coherent code write memory; rerun then.
- **Scale.** 8-bit superpositions (256 branches), 1,024 keys, at most 8 repetitions; slopes are fitted on four
  points. Runs other than the lookups take one argument value each (every argument 1).
- **The code measured** is the device program the front makes from `-O0` C through its own pipeline, not the
  native `-O2` code. Residue in fresh slots happens to be the same on every branch in this executor (each
  branch's journal is unwound before the next runs), so first writes count as Same; a machine whose branches do
  not start from one residue would see them as Recoverable instead.
- **Not measured on the SQLite traces** (`experiments/sqlite-footprint`), the audition's second test for C.
