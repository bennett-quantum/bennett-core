# Design audition: running any code under a qubit (2026-11-13)

Working notes, kept as a record. Five independent proposals for the extension the device program lacks: code that is
not a clean function, run under a qubit, with its history kept and with memory that stays in superposition. Astra
and Muse were given only the problem; three Opus champions were each given one seed idea (A, B, C). The numbers in
these notes are each proposal's own and were not recomputed. The outcome and the decisions are in `HANDOFF.md` and
`docs/DESIGN.md`.

## Muse [done]: two proposals
  - Muse-1 "One machine, no tiers": coherent epoch first gate..measure; basis config = (pc, frames, write-overlay over an
    immutable classical base memory, journal, effect tape, fault, halted); padded to budget B; new `mark %h`,
    `rewind %h keep (src -> dst)`; gates take addresses `h [%addr:bit] ctrl [...]`; removes apply, qalloc, qfree.
    Front fallback = verbatim program. Cost: qubits O(B) (13 B/step journal + overlay <= W written), no dependence on
    memory size; base loads at varying address = QROM. Falsifier: search tiny programs for an augmented-step collision;
    mark/if(q){memset at q-dependent address; loop with gates}/rewind/h/measure must give 0.
  - Muse-2 "Store time, not history": state = (checkpoint root, step index t, workspace); `fetch %v <- [%addr] @%t`
    replays from last checkpoint; forced checkpoint after every gate; un-run = decrement t. Space small, time O(B) per
    fetch; nested replay O(B^(d+1)) capped by forced checkpoints. Says: Muse-1 and Muse-2 are endpoints of one
    pebbling spectrum; build 1, use 2 as the optimiser's dial. Silent-failure risk: replay manufactures an
    uncorrelated copy; needs Bell-style correlation witnesses.


## Astra [done]: "loss-ranked actions"
- Idea: specify WHICH garbage exists before designing journal/optimiser. For total classical update f define
  L_f|x,0> = |f(x), r_f(x)>, r = rank of x among lexicographically ordered preimages of f(x). Bijection -> rank 0,
  no garbage. Reason: keeping the whole old value for an invertible update leaves unnecessary entanglement, and
  "subsequently discovering an inverse must not change the program's interference".
- Conditionals: compute predicate into fresh bit, run body, XOR predicate evaluated on the RESULTING state again;
  clears if unchanged, else the residual is retained loss (handles `if(q) X(q)`).
- Instructions: keep `apply`; add `lift @f(state) -> loss`, `action @p(state) -> history, fault budget B`,
  `adjoint %action`. Registers become named slices of memory.
- Cost, stated as deliberately unattractive: reference construction O(B*S) retained bits; exhaustive permutation
  synthesis 2^O(S) gates per lift; duplicated bounded control P^O(B). Local cases cheap (overwrite independent of
  destination: rank = old destination; bijection: rank 0). SQLite keeps today's apply.
- Channels: `trace environment`, instruments, noise dilations. Cites Abbott et al. arXiv:1810.09826: coherent
  control of a channel is not determined by the channel map alone; and QCM arXiv:2304.15000.
- Falsifier: exhaustively build two-bit ranked lifts, compose with controlled Hadamards, aliased stores,
  unequal-length branches; compare full matrices against an independently built circuit.
- My note: Astra's real contribution is the semantic point: when history is not un-run, WHICH garbage exists
  decides later interference, so it is part of meaning; journal-everything (Muse-1, my A) would dephase even
  `x += 1` unless an inverse is recognised, making analysis load-bearing for meaning. Maintainer says leftover
  entangled history is a warning, not a bug. Proposed resolution to put to maintainer: classical statistics always
  exact; coherence guaranteed for a documented class of updates and for anything un-run; elsewhere unspecified
  between minimal and full dephasing, with a warning.

## Opus B, quantum objects [done]
- Verdict by its own champion: sound, never needs exact analysis to be correct, but NOT a complete alternative: for
  qubit-dependent control it needs the same step machine, and with no analysis its fallback IS approach A. Strongest
  as A's optimiser. Recommended combination: A defines run/unrun and is fallback; B is cost model and lowering
  (backings, candidate sets, tables, versions); C's tapes are history layout + checkpoint schedule.
- Every byte has a backing: classical, qubits, or a classical table indexed by a small quantum "world register".
  Accesses name candidate set S and cost |S|: `qload r,[a] in S`, `qstore [a] in S ^= v`, `qswap [a] in S, r`,
  with `miss e` fault `outside` if a not in S (S is cost annotation only; coarsening lemma). `promote`/`demote`
  replace qalloc/qfree. `run @f ... history h` / `unrun`; h defined only up to a unitary on h alone ("history
  independence"). `apply` stays.
- Lowering tiers chosen by VM at region entry from classical data: T table (k <= k_max quantum input bits: run f
  classically 2^k times, bytes that differ become table-backed; boot under one qubit = 2 classical boots, 1 qubit of
  history); C compiled (predicated schedule, explicit address sets from the concrete heap); U universal = A.
- Allocation: VM malloc shim, address a classical function of schedule position (same on every branch).
- NEW MEASUREMENTS (from existing sqlite arith N=1024 traces): reached code 404 blocks/2,604 instrs of 48,365/251,828
  static. Control: waiting schedule (greedy common supersequence) 7,886 slots = 2.04x a run; step-aligned with exact
  block set per step 93,186 = 24.1x; every reached block every step 422x. Memory: exact per-position sets 22,195
  units; per-site merged sets 64,948 (2.9x); interval (address-range) analysis 2.5e7 (1,128x): ranges or
  one-object-per-allocation-site give nothing; needs explicit address sets on the concrete heap.
- Worksheet SQLite, knowing nothing (tier U): history 7.2e4 qubits control + write log 5.1e5 qubits; access work
  6.9e10 units; time factor 1.8e7 (1e4 with select-swap QROM). Best analysis (tier C): history <= 4.2e5 qubits
  (how much is quantum unmeasured); quantum memory >= 1.06e4 qubits; access work 1.3e5 units; time 38x (waiting
  schedule) to 82x (step-aligned). Toy x=x*x: 8 qubits version (4 with compression: 0 has 16 preimages), 8 qubits
  x1, 2 units, time ~3.
- Raises: `unrun` after something changed run's outputs is undefined (isometry inverse only on its image):
  "history mismatch"; needs a ruling on what "undo boot" means after later writes. Affects A and C equally.
- Falsifier: in exec, enumerate branches with reversible mode on, align journal entries by (instruction, count),
  report share of entries whose old value differs between branches (B's quantum history) vs all entries (A's
  journal). B refuted if share near 1.

## Opus C, the tape [done]
- Verdict by its champion: sound; strongest OPTIMISER of history; cannot be the DEFINITION of meaning (when history
  is observed — discarded dirty, or unrun after operands changed — the state depends on tape content, so an
  analysis-chosen tape would make meaning depend on analysis). Meaning must come from a canonical journal (A's);
  planned tape only inside the window where history is unobserved. No memory model: adopts A's per-branch memory as
  classical base + quantum overlay, B-style indexed access. Best combination: A = meaning + fallback, B = memory,
  C = optimiser ("a reversible-form instruction means no journal entry needed", window by window).
- Unobservability lemma: for two tapes h, h' each making (F,h) injective, `run; V; unrun` gives the same state for
  every V that leaves the run's read/write sets unchanged (V may use them as controls, copy, measure). Outside the
  window the state depends on h (example: `y = x; x = 0;` with y known 0 is a swap, minimal tape empty, y coherent;
  journal holds old x so y decoheres). Says the project must decide "what information irreversible code leaks";
  proposes the canonical journal of the normalised program.
- Corrections to my seed: (1) Enzyme cannot be linked as a library (activity analysis keeps only FP-adjoint values;
  it caches what loads saw rather than restoring stores; forward pass forgets for free; tape malloc'd from classical
  trip count). Reuse LLVM's MemorySSA/AA/SCEV/LoopInfo; copy Enzyme's design (cache vs recompute, min-cut cache,
  loop caches by induction variable, split-mode tapes). (2) "Griewank's binomial checkpointing = Bennett's pebble
  game" is WRONG: Griewank solves the one-way game (a checkpoint can be dropped); a quantum checkpoint can only be
  uncomputed; relevant theory is the reversible pebble game, T^(1+eps) time.
- Instructions: `run @f -> out, tape %T, fault %e budget B`, `unrun`; `%T = tape cap`; reversible forms inside f+:
  `swapstore`, `tpush`, `clear %v`, join annotation `from %c | tape`; gates take a memory operand. `apply` removable
  (= run; copy; unrun). Knowing nothing = journal mode = A's journal, compiled.
- Call depth exponential: closing every callee inside a closed caller doubles per level (2^d). Split mode (callee
  tapes stay open until caller's unrun): at most 4 passes regardless of depth; price: history = everything the open
  subtree destroyed. Close every k-th level: time ~ 4*2^(d/k).
- Physics floor: in-place update kept needs >= log2(largest fibre) bits (x*x on 8 bits: 4 qubits).
- MEASURED on sqlite arith N=1024 traces: per branch 1,067 bytes stored: 688 first writes (free), 379 rewrites;
  <= 592 bytes loaded then overwritten later; 134 join executions. => at most ~1.0 KB irreducible memory history per
  branch vs 52 KB journal. Unmeasured, could sink C: temporaries that cannot be cleared inside their frame.
- Worksheet SQLite: knowing nothing: history 417,664 qubits; quantum memory 9.4e6 qubits (all heap in use);
  access work 2.46e8; time ~6.4e4 sigma. Best: history 3.4k-8.4k qubits (P1) / 9.2k-14.1k (P2); quantum memory
  10,624 qubits; access work 91.7k (P1) / 27.4k (P2); time 28 sigma (P1) / 11 sigma (P2). sigma = cost factor of a
  superposed program counter, common to all, unmeasured. Toy: 8 qubits (floor 4), time ~2x.
- Falsifier: add counters at journal push sites in crates/exec; classify each journal word zero / recoverable /
  irreducible over examples/ and front/tests/c in reversible mode. Kill: irreducible > 25% of journal, or
  irreducible slot words grow with step count in loop-heavy programs. Second: on sqlite traces, union of tape-slot
  positions aligned by (call path, site, k) > 2x rewrites per branch kills the static layout.
- Open semantic decision now raised by Astra, C champion and me: what does irreversible code leak when not un-run?
  Options: canonical journal of normalised program (C champion); canonical minimum rank (Astra, exponential);
  documented coherent class + unspecified elsewhere with warning (mine, from maintainer's "warning not bug").

## Opus A, many-worlds machine [done]
- World = (memory, slots, frames/PC/SP, tape). Step = today's instruction; swaps destroyed value into tape slot t then
  XORs new value into cleared destination: (d,h) -> (h xor f(d,..), d): bijection on EVERY (d,h), clean or dirty, so
  un-run after intervening ops gives U^dagger V U. Global classical clock t: every world writes slot t (own record or
  HALT); tape writes at classical index. Budget = global steps. Gates controlled on world's PC, record "gate".
- Rules fixing meaning without analysis: R0 device program = linear extension of journaled steps; R1 a value computed
  for a quantum op is a lifted oracle, inverse runs right after last use; R2 overwrite of world-dependent location not
  on the fixed bijection list keeps old value in history until un-run. Analysis may change representation only of
  history it proves unobserved.
- Instructions: `%h = run @f(args) -> %r fault %e budget B`, `unrun %h`; gates/measure take memory cell; apply derived
  (literally what region.rs evaluate does per branch). C markers bennett_begin/end/unrun for case 6.
- Knowing nothing: stores append (addr,value) to tape = redo log; loads scan log then QROM over classical base; no
  memory cell quantum. Nested un-run under superposed control: O(B^2 W) worst, never multiplies budgets.
- MEASURED: step-aligned control 93,186 instr = 24x one run per direction; by instruction step 92x; all reached code
  each step 2,700x; knowing nothing 2.6e5. "The number that decides whether SQLite is affordable."
- Worksheet SQLite: nothing: history 1.03M qubits, quantum memory 0, access 1.8e8, time 5.2e5. best: history 271k
  (0.65 x 4016 x 13 B x 8; up to 2x), memory 10.6k, access 44k, time 48 (floor) .. 5,400 (static S_t). Toy: 8 qubits.
- Falsifier: per-world journals behind run/unrun in crates/exec, exhaustive at 4-bit: (1) every step bijection on
  (world, dirty slot) incl. fault exit, call_indirect, alloca; (2) run containing H on a bit its tape depends on, outside
  X, unrun == conjugation by independent dense matrix; (3) apply == derived form on every example and front test.
  Second: static step-interval S_t on traces: near 2,604 union or near 24x floor?

## Decisions already taken by maintainer (2026-11-13)
- Coherence contract (4 rules) — see memory coherence-contract.
- Tension program verified in executor: .tmp/tension/tension.bn (journal 105/95, minimal 200/0, at once 105/95 both).

## Outcome
- Open decisions put to maintainer there (my recommendation in brackets): 1 adopt layering A=meaning/fallback,
  B=memory lowering, C=history optimiser [yes]; 2 instructions run/unrun vs Muse's mark / rewind-keep [Muse's, after
  the apply==derived experiment]; 3 memory = overlay over immutable classical base [yes]; 4 gates take addresses
  [yes]; 5 remove apply/qalloc/qfree [keep until new instructions pass every existing test].
- Experiments cheapest first: (1) every journalled step a permutation, clean and dirty tape; (2) apply == run, copy,
  unrun on all examples/tests; (3) classify journal words zero/recoverable/irreducible and share differing between
  branches; (4) static control schedule on SQLite traces [started 2026-11-13, stopped unfinished at the pause; see
  experiments/sqlite-footprint/schedule.py + README section]; (5) un-run through gates == conjugation vs matrix.
- Experiments 1, 2, 3, 5 need changes in crates/exec: wait for step 5 (ordinary C widening) to land first.
