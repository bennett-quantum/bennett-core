# North Star sentences that downstream documents treat as binding

How to read this:
- Line numbers are in the current `bennett/docs/Bennett-NorthStar.md`. It has 857 lines and has not changed since commit `0b23100b`.
- **Round N** means the text first appears in the round-N council draft (`round-N/doc.md`). The remark that triggered it is from round N−1's `report.md`.
- **Iteration 7** means the text appears only in the final document. The editor added it after round 6 as "wording, not semantics" (`round-6/response.md`), and no council round ever reviewed it.
- The council seats are codex, muse and opus. "Blocking" means the remark was listed under *Blocking issues*. "Comment" means a scored-criterion remark or a numbered suggestion.
- Citation counts are lines that cite the sentence by NS section number. They cover the dialect spec 0.2 (current), outline-0.2, the 0.1 narrowing audit, both ISA specs, the hub's plan/design/council docs, the conformance ladder, and the other repos' `docs/` (lighter pass). The dialect `Bennett-Dialect-Spec-0.3-draft.md` repeats the 0.2 citations almost line for line and is not counted again. Counts are ±2.
- One rule makes every sentence below binding: `COMMON.md` rule 1, "No narrowing: nothing may reject, refuse, fault, declare inert or omit a capability bennett/docs/Bennett-NorthStar.md promises". The dialect spec's rejection table (§22, around dialect line 11864) cites a North Star clause for every compile-time rejection.

The round-1 draft (343 lines) is the text closest to the maintainer's own words. It contains almost none of the semantic contract below. Of the 25 entries, only one is round-1 text in substance (#23). Three more rest on a round-1 core or precursor (#6, #7, #11).

---

## Ranked list (heaviest downstream consequence first)

### 1. §4.2, lines 409–414: the budget unit and the "fault set"
> "The **budget unit** is steps of the ISA formal-model interpreter on the materialised outlined region (§3.2), so the fault set is a property of program plus budget plus recorded LLVM version and pipeline, never of Bennett's own lowering; compiled artefacts pad internally as needed and never fault where the reference does not."

- **In plain words:** A region's step budget is counted on one fixed reference program, not on whatever circuit a particular compiler emits. So which inputs run out of budget ("fault") must be the same for every conforming compiler.
- **Origin:** Two stages, both from opus and neither blocking.
  - Added in round 4 after opus's round-3 C6 comment: "Define the budget unit and the expected fault code against the ISA formal-model interpreter run on the outlined region … Require compiled artefacts never to fault where that reference does not". Opus's suggestion 1 in the same round added: "That makes the fault set a property of program plus budget, not of compiler version." (Opus's blocking items that round were B1 and B2, on other topics.)
  - The clause "plus recorded LLVM version and pipeline, never of Bennett's own lowering" was added in round 5, after opus's round-4 suggestion 2 (not blocking): "Say 'never of Bennett's own lowering', and record the LLVM version and pipeline in the certificate."
- **What is built on it** (2 citations: dialect §9.4 and outline-0.2 §9.4):
  - Dialect §9.4, lines 4136–4141: one budget step is "one reference-interpreter row of the reference object … so the fault set is a property of the program, the budget, the LLVM version and pipeline, the normaliser and mapping versions and the runtimes version, never of the plugin's lowering (NS §4.2)".
  - Dialect §16, lines 8136–8142: the reference object is "a function of its inputs alone, so two plugins on the same LLVM version and pipeline produce byte-identical reference objects". This line has no NS citation of its own; it inherits through §9.4.
  - Dialect §5, lines 962–967: the whole-program, Andersen-style pointer analysis "is specified exactly: every verdict, candidate set and dispatch domain … is computed from the reference analysis". A plugin may use a more precise analysis but "never to change a verdict". This has no NS citation. *Inferred link:* candidate sets become dispatch rows inside the reference object, and those rows are budget steps (see the "Cost, derived from the rows" section of `bennett/docs/design/2026-11-08-function-pointers-in-lifted-oracles.md`).
  - Also: dialect §9.5 (line 4165) forbids rounding a run-time budget because "it would change F_B". ISA 0.9 §4.2 (lines 1040–1045) defines "F_B and the budget unit … solely by the reference interpreter" with no NS citation.
- **Weight:** heavy. It forces a byte-canonical reference object and an exactly specified whole-program pointer analysis that every implementation must reproduce.
- **If struck or softened:** Step counting could be defined per implementation, with the compiler identity recorded in the certificate. The reference object and the pointer analysis could then become implementation choices. The ISA's own §4.2 text would still need a separate revision.

### 2. §3.2, lines 171–188: the reference semantics is a normalised copy of the optimised program
> "**the reference semantics is the ISA formal-model interpreter run on the *materialised* optimised bitcode of the outlined region** … the normaliser rewrites the region's optimised bitcode into a **poison-free, undef-free form**"

- **In plain words:** The meaning of a region is fixed by first rewriting LLVM's optimised code so that every "unspecified" or "poison" value becomes a stated concrete value. Both the native test build and the quantum device then run that rewritten program.
- **Origin:**
  - The interpreter-on-optimised-bitcode form was added in round 4, after codex's round-3 B1 (blocking: faults should "cover conditions encountered by Bennett's totalised optimised-IR machine") and opus's round-3 C6 comment.
  - "Materialised" normalisation replaced `freeze` in round 5, after two blocking issues from round 4: opus B1 ("normalisation *materialises* the refinement instead of freezing it") and codex B_FREEZE_DETERMINISM.
- **What is built on it** (6 citations):
  - Dialect §12, lines 4744–4746: the "materialised module" is the single object on which the reference semantics (§18), the lifting map (§16) and the native comparison build (§17.7) are defined.
  - Dialect §1.4, line 104: normalisation is extended from "the outlined region" to every device variant.
  - Dialect line 830 records the definedness assumption.
- **Weight:** heavy. It requires a complete normaliser over all of LLVM IR (rules N0–N35), whose output is consumed by three downstream definitions.
- **If struck or softened:** The reference could be the device's own semantics, with native comparison restricted or approximate. The full-IR normaliser and its rule set would stop being contractual.

### 3. §3.2, lines 214–227: the deterministic address layout
> "**Addresses follow a deterministic layout model.** The heap is laid out by the same `bennett-runtimes` allocator shim linked into both builds at a fixed base with no ASLR; globals and functions are placed by a driver-controlled comparison link (non-PIE, fixed section addresses, constant merging and identical-code folding disabled) whose link map the device loader replicates. Region allocas are rewritten by the normaliser onto a shim-managed shadow stack in both builds where possible …"

- **In plain words:** Every memory address a program can see, including heap, globals, function pointers and stack, must be numerically identical in the native test build and on the quantum device. Executions that still depend on native stack addresses are set aside and checked against the reference interpreter instead.
- **Origin:** Two stages.
  - The heap shim, fixed base, no ASLR and the "layout-dependent" class were added in round 6, after opus's round-5 B_RESIDUAL_NONDETERMINISM (blocking): "Addresses follow a deterministic layout model. Globals and heap are laid out through the same bennett-runtimes allocator shim … Option A".
  - The non-PIE comparison link, disabled constant merging and code folding, the replicated link map and the shadow stack are iteration 7, never reviewed. They follow opus's round-6 C2 comment and suggestions 2–3 and muse's round-6 path-to-accept.
- **What is built on it** (10 citations, plus 2 loose ones; see the final section):
  - Dialect §4.4, lines 921–928: function address A(F) is the comparison-link address in every tier, including inside lifted quantum code. The production host image is linked "with the comparison layout whenever an address may cross the host–device boundary".
  - Dialect §17.3 (line 10220): the shadow stack. Dialect §24 D12 and D15: comparison-link bases.
  - The 2026-11-08 function-pointer design depends on A(F) throughout.
- **Weight:** heavy. It constrains how production host binaries are linked, and it makes quantum oracles compare full 64-bit native function addresses.
- **If struck or softened:** Function pointers could use dense indices, and the comparison link and shadow stack would become optional. More executions would fall into a "layout-dependent" class with weaker checking.

### 4. §3.2, lines 209–213: the native comparison binary must not use the host's C library
> "The native comparison binary is the materialised region plus its entire materialised callee closure, linked against the same `bennett-runtimes` and `bennett-arith`, never against the host libc or libm."

- **In plain words:** The classical program used as the correctness yardstick must use the project's own C library and maths library, compiled through the same pipeline. It may not use the system ones.
- **Origin:** Iteration 7, never reviewed. It applies opus's round-6 suggestion 7: "State that the native comparison binary consists of the materialised region and its entire materialised callee closure … never against host libc or libm."
- **What is built on it** (1 explicit citation, 2 restatements):
  - ISA 0.9 §12.3, line 5459: "Native comparison uses the materialised entry and its entire materialised callee closure with the North Star §3.2 runtimes and arithmetic libraries."
  - Dialect §17.7, lines 10385–10388 (cited as NS §5.1): "never host libc or libm". Dialect §13.4 (lines 7052–7058): the runtimes' libm is "musl-derived and compiled through this pipeline".
  - Rung-4 DESIGN row 6: "no host libc".
- **Weight:** heavy. A full libc and libm must exist as pipeline-compiled bitcode before any native comparison can run.
- **If struck or softened:** The comparison could link the system libc and libm, with library-level differences classified rather than excluded by construction.

### 5. §3.2, lines 194–208: soft-float in both builds
> "**Floating point is deterministic soft-float in both builds.** The normaliser rewrites every floating-point operation of lifted code into calls to the project's bit-exact soft-float library … Hardware floating point never executes in either build"

- **In plain words:** Every floating-point operation, even in the native test build, is replaced by exact software emulation, so that rounding and NaN bits match bit for bit.
- **Origin:**
  - Round-1 precursor: open question 11, "Floating point (soft-float as a library, as today)".
  - "In both builds" was added in round 6, after codex's round-5 B_DEFINED_NONDETERMINISM (blocking): "Prefer a specified deterministic numerical contract … for example deterministic soft-float".
  - The format scope (lines 204–207) is iteration 7, never reviewed.
- **What is built on it** (4 citations):
  - Dialect §13 (line 6901): every format has a row and "none faults".
  - Dialect §13.3 (line 7013): all seven formats required, cited to NS §3.2. This overstates the North Star; see the final section.
  - Dialect §13.4 (line 7058): bit-exact libm.
- **Weight:** heavy. It needs a complete bit-exact soft-float and libm for every format, and native comparison runs never use hardware floating point.
- **If struck or softened:** Native comparison could use hardware floating point, with NaN and fusion differences classified. The formats beyond binary32 and binary64 could be trapped, as lines 204–207 already allow.

### 6. §8, line 802: rung 4, Grover over SQLite
> "SQLite compiled unmodified to the coherent tier under a region budget, with an in-memory VFS over quantum-addressed memory … (a) Oracle verified by §5.1 part 1 over all rows and all target values for N ≤ 2¹⁰, fault rate zero. (b) … N ≤ 2⁸. (c) A resource estimate for N = 2²⁰ …"

- **In plain words:** The flagship demo is a real SQL query used as a Grover oracle on unmodified SQLite. It has fixed instance sizes and a full exhaustive check of the oracle.
- **Origin:** The core is in the round-1 draft: "Grover over SQLite. The predicate is a real SQL query … Nobody rewrote SQLite." Additions:
  - Sizes and parts (a)–(c) were added in round 2, after opus's round-1 C6 comment ("Give each rung an acceptance test: instance size, which executor, which oracle").
  - "In-memory VFS" was added in round 2, after opus's round-1 B4 (blocking): "SQLite through an in-memory VFS over QROM/QRAM".
  - "All target values … fault rate zero" was added in round 3.
  - `rowid` was added in round 6, after opus's round-5 suggestion 7.
- **What is built on it** (about 18 citations):
  - Rung-4 README and DESIGN: the budget pins and the 32,768-element domain.
  - Dialect §8.2 `MEM_IN` binding (line 3178) and Appendix A20 (line 12081), which asks for ISA §12.2 fixed-region domains.
  - Path-to-North-Star plan (the core value proposition), the 2026-11-08 design, bennett-estimate EST-18, and bennett-vm PLAN.
- **Weight:** heavy. It is the gate that defines success, and most of the semantic contract exists to make it checkable.
- **If struck or softened:** The instance sizes and the exhaustive-over-targets clause in (a) set the checking cost. Changing them changes rung 4's gate directly.

### 7. §3.1, line 152: taint is a whole-program, context-sensitive analysis
> "a context-sensitive interprocedural analysis over data dependence, control dependence and memory effects, from marker results and region entries, over the whole-program call graph, with polyvariant specialisation when one function is used both classically and coherently"

- **In plain words:** The compiler decides which code must run quantumly by analysing the whole linked program. It makes separate copies of a function for each way it is used (classically or under a qubit).
- **Origin:**
  - "Taint by analysis" is round-1 text (draft line 120: "the plugin's taint analysis infers the lattice").
  - The whole-program call graph was added in round 2, after opus's round-1 B3 (blocking): "Define 'untainted' relative to the resolved call graph".
  - "Context-sensitive … polyvariant specialisation" was added in round 3, after muse's round-2 suggestion 3 (not blocking), which the North Star repeats almost word for word.
- **What is built on it** (3 citations):
  - Dialect §10 TDL intro (line 4284, "NS §3.1 taint row").
  - Dialect §10 MC5 (line 4347): "Interprocedural and context-sensitive by coherence signature".
  - Dialect §4.3 polyvariant cloning. *Inferred:* the dialect §5 analysis runs "over every device variant" that this cloning creates.
- **Weight:** heavy. It requires whole-program cloning per coherence signature, and that is the scope over which the exact pointer analysis of #1 runs.
- **If struck or softened:** Taint could be coarser, for example per function, at the price of more code running quantumly. Cloning and the coherence-signature machinery would become optional.

### 8. §4.2, lines 407–435: every lifted oracle is total, with a fault register
> "On failure, F_B(x) = (0, code): the output target is XORed with the **canonical zero payload** (left unchanged) and the fault register is XORed with a distinct code per cause … on a faulted branch the step machine continues to the budget with the fault predicate off, and its recorded history is reversed exactly as on a clean branch."

- **In plain words:** An oracle never just fails. On inputs where it runs out of budget or hits an error, it leaves the output untouched and writes an error code into a separate quantum register. Faulted branches still run to the full budget so they can be cleanly undone.
- **Origin:** Three stages.
  - The fault register was added in round 3, after opus's round-2 B1(c) (blocking): "Each region gets a fault/exhaustion output register, with a distinct code per cause".
  - The canonical zero payload was added in round 4, after codex's round-3 B3 (blocking): "I would choose the canonical zero payload."
  - The prefix-reversal invariant was added in round 4, after muse's round-3 C3 comment (not blocking).
- **What is built on it** (about 4 citations):
  - Dialect §25.1 BLK-5 (line 11969): "NS §4.2 named fault outcomes", which requires an ISA change for typed R-tier faults.
  - Dialect TDL-8 (line 4496).
  - Rung-4 DESIGN §4: fault-register entry values are enumerated.
- **Weight:** heavy. Every oracle carries a fault register, faulted branches cost the full budget, and the rule drives an ISA blocker.
- **If struck or softened:** Faulting inputs could be excluded by precondition instead of encoded, and branches could stop early.

### 9. §3.1, line 154: one classical budget per region, never a per-loop annotation
> "**A region-level classical budget**: dynamic steps, heap bytes and recursion depth … **No per-loop source annotation is ever required** … it is never a compile error and never a truncated result."

- **In plain words:** Unmodified code with arbitrary loops runs under a quantum condition by giving the whole region one step, heap and depth allowance. Running out is reported as a fault, never as a compile error.
- **Origin:**
  - Round-1 precursor: draft line 124 put bounds on loops as "a bound value … carried by loop metadata or a marker intrinsic".
  - The region-level budget was added in round 3, after opus's round-2 B1 (blocking): "Make the primary bound a region-level classical budget: dynamic steps, plus heap and recursion depth … so no loop identity has to survive optimisation."
- **What is built on it** (6 citations):
  - Dialect §9: the four-component budget record (line 3924) and "never a compile error" (line 4163).
  - Outline B5.
  - The 2026-11-08 design's "No per-loop source annotation is ever required".
  - Rung-4 pins of 1,000,000 steps (that pin has no NS citation).
- **Weight:** heavy. It forces the general step-machine construction (#11) for every region, so loops are never structurally bounded.
- **If struck or softened:** Per-loop bounds or annotations would become admissible, and region step machines could be replaced by structured loops where annotated.

### 10. §4.2, lines 473–479: effects by tier
> coherent tier: "**closed and effect-free**: no external effect may occur inside a coherent region … An effect the device cannot virtualise is a fault code on the executions that reach it." Reversible tier: "entered through `__bennett_reversible` or a driver mode".

- **In plain words:** Under a qubit, the program may not touch the outside world. Files, consoles and similar devices are simulated inside the quantum state. A whole program may also be run reversibly by a compiler switch.
- **Origin:**
  - Added in round 2 after blocking issues from all three seats in round 1 (codex B1, muse B4, opus B4). Muse: "coherent regions are closed and effect-free, with classical inputs fixed before entry and outputs emitted after measurement".
  - "Or a driver mode" was added in round 3, after opus's round-2 suggestion 8.
  - The fault code for unvirtualisable effects was added in round 3; the round-2 text had said "refused at compile time".
- **What is built on it** (about 13 citations):
  - Dialect §11.1 tier table (line 4595).
  - Dialect §19.2 whole-program reversible mode (line 11371). The audit (O-04) had flagged its absence as a narrowing.
  - Dialect §4: no host calls in R or L code (line 956). Dialect §3 `observe` (line 349).
- **Weight:** heavy. It defines the virtual-device obligation (in-memory VFS, console and entropy models) and the whole-program reversible mode that rungs 3 and 6 use.
- **If struck or softened:** Whole-program reversible mode and fault-on-unvirtualisable-effect would become optional. A compile-time refusal would be allowed instead.

### 11. §4.2, lines 362–374: coherent control is a predicated step machine
> "**predicated execution under a classical budget** … a **predicated step machine** … An allocation whose size depends on a qubit is padded to its bound. The budget is classical and fixed before entry; run-time budgets come from the host or from measurement results, never from superposed data."

- **In plain words:** A branch on a qubit runs both sides under a condition, stepping through the program for the whole budget. Budgets can be chosen at run time, but only from classical data.
- **Origin:**
  - Round-1 precursor: "a loop with a data-dependent trip count runs under a bound chosen when the program is loaded, from classical data … the circuit pads to it".
  - The two control disciplines and "run-time bounds come from the host or from measurement results" were added in round 2, after muse's round-1 B3 (blocking).
  - The step machine was added in round 3, after opus's round-2 B1(b) (blocking).
- **What is built on it** (about 13 citations):
  - Dialect §25.1 BLK-2 (line 11968): an ISA `budget.mint` instruction is needed for budgets computed on the device.
  - Dialect §10 padding (line 4542). Dialect §9.3 (line 4129): classical control is unbudgeted.
  - BLK-1 cites the neighbouring "real allocation" clause.
- **Weight:** heavy to moderate. The padding is physics; the measurement-derived budgets clause creates an open ISA blocker.
- **If struck or softened:** Budgets derived from measurements could wait. BLK-2 would stop being a North Star obligation.

### 12. §5.1, lines 628–636: the exhaustive check covers every register value
> "For a lifted oracle, D is every input x, every output-target value t, every fault-register value e, every value of every borrowed dirty ancilla d, clean ancillae in their declared state"

- **In plain words:** An oracle counts as verified only after it has been run on every combination of input, target value, fault-register value and borrowed-scratch value, and the whole final register state has been compared.
- **Origin:**
  - x, t and d were added in round 3, after codex's round-2 B4 and muse's round-2 B1 (both blocking). Codex: "including arbitrary target-register contents, input preservation, and dirty-borrow restoration".
  - e and the full register tuple were added in round 4, after codex's round-3 B4 (blocking, input preservation) and muse's round-3 C6 comment.
- **What is built on it** (about 5 citations):
  - Rung-4 DESIGN §4: |D| = 32,768, and "the spike's 32× shortcut (one run per x) is not used".
  - Rung-1 README, the 2026-11-08 design (line 107), and dialect §25.2's open ISA labelling question.
- **Weight:** moderate to heavy. It multiplies the cost of exhaustive checking (×2⁵ at rung 4).
- **If struck or softened:** Checks could enumerate x only and argue that XOR delivery holds structurally.

### 13. §3.1 line 151 and §4.2 lines 437–447: writes to quantum state; in-place updates need an inverse
> "a store into a register not known to be clean is a **compile-time error**" (L151) … "region inputs are preserved and declared outputs are XORed into caller-supplied targets, every other write being restored workspace; an in-place update x ← g(x) requires a total bijection with a bounded inverse … else it fails loud" (L442–447)

- **In plain words:** Classical code may write to qubit-held data only in reversible ways: XOR into a known-zero target, or an update whose inverse is supplied. Anything else is rejected when compiling.
- **Origin:**
  - Added in round 4, after opus's round-3 B1 (blocking): "Storing into a register not known to be clean is a compile-time error, not a fault … an in-place update with no inverse fails loud at compile time."
  - The total-bijection restriction was added in round 5, after codex's round-4 B_INPLACE_BOUNDED_INVERSE (blocking).
- **What is built on it** (about 22 citations):
  - Dialect `BD-E-COHERENT-OVERWRITE` (§10 TDL-5(d), line 4442, and table line 11886).
  - Dialect `BD-E-INPLACE-NOT-INVERSE` (§8.7, line 3397).
  - Dialect §1.4 and §6.7 depart from the qstore half and make it a warning only (lines 103, 2051). ISA 0.9 K4 (line 108).
- **Weight:** moderate. These are some of the few compile-time rejections the North Star licenses.
- **If struck or softened:** Non-bijective coherent overwrites could keep history as garbage instead of being rejected. Outline-0.2 option E3(b) already describes this and says it "would extend NS §4.2".

### 14. §3.2, lines 250–256: normaliser rules as spec text, and two independent validators
> "The normalisation rules are normative text in `bennett-dialect-spec`, enumerating every source they remove … and shipping a residual-poison validator implemented independently by the plugin and by `bennett-conformance`."

- **In plain words:** The rules for cleaning up LLVM's undefined values live in the dialect specification. Two separately written checkers must confirm that nothing undefined remains.
- **Origin:** Added in round 6, after opus's round-5 C4 comment and suggestion 1 (not blocking): "implemented independently by the plugin and by `bennett-conformance`". Muse's round-5 C2 comment (not blocking) asked for the validator.
- **What is built on it** (2 citations): dialect §12.4 (line 5324, two independent validators) and `BD-E-RESIDUAL` (line 11884).
- **Weight:** moderate. It requires a second, independent implementation of the validator.
- **If struck or softened:** One validator, or none, would satisfy the contract.

### 15. §5.1, lines 668–682: "equals native, or is flagged", and one outcome table
> "On every element of a rung's declared domain the artefact must return either the native result or a fault code … One outcome table governs §3.2, §5.1 and §8 … with each rung reporting and bounding both excluded fractions."

- **In plain words:** On every tested input, the quantum build must give the same answer as the native program or an explicit fault. Inputs set aside as undefined or address-dependent must be counted, and their share bounded.
- **Origin:**
  - The core was added in round 3, after opus's round-2 B1(d) (blocking).
  - The outcome table and "bounding both excluded fractions" are iteration 7, never reviewed. They follow codex's round-6 suggestion 3 and opus's round-6 C6 comment.
- **What is built on it** (about 5 citations): dialect §18.7 outcome table (line 10574), dialect §17.7 (line 10385), bennett-estimate EST-17.
- **Weight:** moderate. It sets the gate semantics of every rung.
- **If struck or softened:** Rungs would not have to bound their excluded fractions.

### 16. §3.2, lines 228–235: out-of-bounds accesses fault; atomics run single-threaded
> "**Faulting memory access is defined** by an object-bounds model … a lifted load or store outside any live allocation is `FAULT_UB_MEMFAULT`." / "**Atomics and volatile** in lifted code are sequentialised in program order (the single-thread assumption recorded in the certificate)"

- **In plain words:** A memory read or write outside any live object is an explicit fault, even where native code would silently read neighbouring memory. Multithreading is assumed away.
- **Origin:**
  - Both were added in round 6. The memory-fault rule follows opus's round-5 suggestion 6 (not blocking): "Define 'faulting memory access' in the reference semantics".
  - The atomics rule follows muse's round-5 C8 comment (not blocking). Round-1 precursor: open question 11, "concurrency (sequentialised with a recorded schedule …)".
- **What is built on it** (about 7 citations): dialect §17.1 bounds rule (line 10145, "valid iff all its bytes lie in one live allocation") and single thread (line 10152), dialect §24 D24, dialect §20 (line 11567).
- **Weight:** moderate. It implies live-allocation tracking and a bounds check on lifted memory accesses inside oracles (cost inferred).
- **If struck or softened:** Out-of-bounds behaviour could be left as "undefined, excluded" rather than defined as a fault.

### 17. §3.1, line 149: rules for qubit handles
> "a handle that flows through memory at a superposed address, or through an `inttoptr`/`ptrtoint` round trip, is **rejected fail-loud** in v1 of the dialect; any handle use that survives into host native code is rejected likewise."

- **In plain words:** Qubit references must stay in forms the compiler can track. Untrackable uses, and any use left in the host program, are compile errors.
- **Origin:**
  - "Never dereferenced by host code" was added in round 2 (round-1 blocking issues).
  - Portable handles were added in round 3, after codex's round-2 B2 (blocking).
  - Rejection of handles in host code was added in round 3, after opus's round-2 suggestion 4.
  - The `inttoptr` rejection was added in round 4, after muse's round-3 C5 comment.
  - The handle table was added in round 6, after muse's round-5 C5 comment.
- **What is built on it** (about 11 citations): `BD-E-HANDLE-USE`, `BD-E-HANDLE-ESCAPE` and `BD-E-HANDLE-UNTRACKED` (lines 11878–11880), dialect BLK-1 (line 11967). The dialect departs from the `inttoptr` clause (line 99, north-star amendment A2 drafted).
- **Weight:** moderate. It licenses three rejection codes.
- **If struck or softened:** Those rejections would lose their North Star licence under rule 1.

### 18. §3.1, line 148: the "positive legality requirement" for marker calls
> "**Positive legality requirement:** a marker is declared as an external function that may read and write memory and may not return (no `readnone`, `readonly`, `speculatable`, `willreturn`, `nounwind`-only combinations …)"

- **In plain words:** The quantum operation calls must be declared so that LLVM's optimiser can never delete, merge or move them.
- **Origin:**
  - Marker calls themselves were added in round 2 (round-1 blocking issues from codex B2, muse B1 and opus B1).
  - The "positive" requirement was added in round 3, after codex's round-2 suggestion 1 (not blocking): "specify their positive effect requirements and prohibit incompatible attributes".
- **What is built on it** (about 9 citations): dialect §3.6 `BD-E-MARKER-ATTRIBUTE` (line 704: every unclassified attribute is rejected until corpus evidence), and rejection-table rows at lines 11870–11877.
- **Weight:** moderate. Every unclassified LLVM attribute is a compile error until a corpus run on each LLVM major clears it.
- **If struck or softened:** Unknown attributes could be stripped or warned about instead of rejected.

### 19. §3.3, lines 295–310: whole-program compilation and the coverage report
> "Target intrinsics and asm are classified *lowered / trapped / shimmed* in the coverage report." / "Calls with no bitcode are coverage-report lines (*trapped*), never silent." / "A per-TU invocation remains available as a **fast path** …"

- **In plain words:** Anything the compiler cannot translate (assembly, library calls without source) becomes a reported runtime fault, never a silent gap. A quick per-file mode stays available.
- **Origin:**
  - "Lowered / trapped / shimmed" is round-1 text.
  - Link-time whole-program compilation and host-triple IR were added in round 2, after opus's round-1 B3 and muse's round-1 B2 (both blocking).
  - The fast path appeared in round 2; no reviewer request was found (inferred: an editor addition).
  - "Trapped" calls were added in round 3, after muse's round-2 C5 comment.
- **What is built on it** (about 17 citations): dialect §4 trapped calls are F(8) (line 953), dialect §15 asm tables (lines 8037, 8075), dialect §19.7 closed-unit mode (line 11444), audit B-54.
- **Weight:** moderate. It makes "trap with a report" the only allowed answer to untranslatable code.
- **If struck or softened:** Compile-time refusal of untranslatable calls would become admissible, and the per-file mode could be dropped.

### 20. §4.2, lines 376–400: two kinds of region, classified by content
> "Every coherent region is one of: a **lifted classical oracle** … a **quantum action** … a quantum action has no fault outcome of its own … the plugin classifies each outlined region by its contents and the verifier checks the declared interface."

- **In plain words:** A region is either a reversible classical computation (an XOR oracle with a fault flag) or a genuinely quantum operation (a unitary with no fault flag). The compiler decides which from the code itself.
- **Origin:**
  - Added in round 5, after codex's round-4 B1 (blocking): "Separately define quantum action regions as U on declared mutable wires".
  - "That bound is part of the action's content identity" is iteration 7, never reviewed (muse's round-6 C3 comment and suggestion 5).
- **What is built on it** (about 8 citations): dialect §7 classification (line 2572), dialect §10 TDL-8 (line 4470), dialect §8.7 (line 3358), audit G-14.
- **Weight:** moderate.
- **If struck or softened:** Regions could be declared by the front end rather than inferred.

### 21. §0, lines 51–52: what "unmodified" means
> "\"Unmodified\" means **no source edits**; build-configuration flags and runtime shims are allowed and recorded in the certificate."

- **In plain words:** Demos may change build options and swap in runtime libraries, but may not edit the program's source.
- **Origin:** Added in round 5, after opus's round-4 C5 comment and suggestion 6 (not blocking): "Define 'unmodified' as no source edits: build-configuration flags and runtime shims are allowed and recorded in the certificate."
- **What is built on it** (about 6 citations): dialect §3.6 (line 699: attributes from `-ffp-model=strict`, `-Os`, `-O0`), dialect §20 kernel profile "build flags only" (line 11613), dialect A20 certificate build record, ISA 0.9 §12.3 (line 5457), outline B8: "Require a source edit. Forbidden".
- **Weight:** moderate. It bans source patches across all rungs and puts build records in certificates.
- **If struck or softened:** Small recorded source patches could become admissible for SQLite or the kernel.

### 22. §3.2, lines 264–268: one plugin build per LLVM major; a tested compatibility envelope
> "**Per-LLVM-major builds** of the plugin … A mixed-language link (bitcode from different LLVM majors, different front-end IR idioms, one target datalayout) is supported only within a **tested compatibility envelope**"

- **In plain words:** One compiler plugin is built for each LLVM version. Mixing code from different compilers is supported only for tested combinations.
- **Origin:**
  - Per-version builds are round-1 text ("Built per LLVM version, as Enzyme is").
  - The envelope and "one target datalayout" were added in round 4, after codex's round-3 C5 comment (not blocking).
- **What is built on it** (4 citations): dialect §19.8 `BD-E-VERSION` (line 11459, which adds "and triple"), dialect §19.9 (line 11487). This underlies the rung-1 "N/A" ruling for Rust and Julia on LLVM 18 (the README cites dialect §19.8, not the North Star).
- **Weight:** moderate to light.
- **If struck or softened:** Cross-major links could be handled by translation rather than rejected.

### 23. §5, lines 582–583: quantum registers live as long as a REPL value (round-1 text)
> "Quantum registers are REPL values with lexical lifetime."

- **In plain words:** In the Julia REPL, a qubit register lives as long as the variable holding it, across separate calls into the device.
- **Origin:** Round-1 draft (draft line 204), unchanged.
- **What is built on it** (about 4 citations): dialect §6.6 (line 2034: QREG results persist across device entries), outline-0.3 (line 1117: "the north star requires REPL lifetimes"), audit B-30. It feeds dialect BLK-1.
- **Weight:** moderate. It forces handles to survive across separate device launches.
- **If struck or softened:** Registers could be scoped to one device entry.

### 24. §3.1, line 153: well-formedness of control regions
> "a `ctrl`/`adj`/`compute` region is unitary: user measurement inside it is refused (or, when the front end asks for it, deferred …); allocations inside are released clean or borrowed and restored … Exceptions and unwinding … a fault code where it is not."

- **In plain words:** Code run under a quantum control must not measure, must return its scratch qubits clean, and turns an exception that escapes the region into a fault.
- **Origin:**
  - Function-granularity regions are round-1 text.
  - Measurement refusal or deferral was added in round 3, after opus's round-2 C8 comment and suggestion 2 (not blocking).
  - Unwinding was added in round 4, after opus's round-3 C5 comment and suggestion 7.
- **What is built on it** (about 7 citations): `BD-E-REGION-MEASURE` (dialect §7.5, line 11882), the `measure_in_unitary` defer flag (line 2593), unwinding code 4 (line 7754), and T-LEAK/T-CLEAN (line 10544, which cites the wrong section; see below).
- **Weight:** light to moderate.
- **If struck or softened:** Measurement inside control regions would need another semantics or a different rejection licence.

### 25. §2 line 120, §3.3 lines 286–292, §4.4: the fat object and what the ISA spec owns
> "fat object: native host code + ISA object(s) + certificate (ISA spec, §4.4)"; §4.4 lists the object format, launch ABI, scheduling interface, certificate schema …

- **In plain words:** The build produces an ordinary native program with quantum device code embedded. The ISA specification owns every format exchanged across that boundary.
- **Origin:**
  - Round-1 precursor: decision 4, "host/device offload model … (outlining, fat binaries, a launch runtime)", and the two-contract split.
  - The driver-built fat object was added in round 2, after opus's round-1 B3 (blocking).
  - ISA ownership of the object, launch and certificate formats was added in round 2, after muse's round-1 B6 and opus's round-1 B5 (both blocking).
- **What is built on it** (about 6 citations):
  - Council `rubric.md` (line 120) and `rubric-isa.md` (line 127): every ISA or dialect change must keep "the mixed fat object reachable … north star §2, §3.3, §4.4".
  - Plan R8.
  - ISA spec founding text (§4.4, §6.1), where "the north star governs where this document is silent".
- **Weight:** moderate. It is a review criterion on every contract change.
- **If struck or softened:** The rubric check would lose its North Star anchor. The maintainer's own fat-object direction (bead bennett-4ht) would remain.

---

## Sentences added after round 1 that nothing cites

**Count:** about 110 of the roughly 280 sentence-level units that are new since round 1 (inferred, ±15). Method: 4-word-overlap comparison against the round-1 draft, then excluding line ranges that downstream documents cite. About half are §7 landscape-table and §6 language-table cells.

The three most consequential-looking:
1. **§4.2, lines 451–471, uncomputation validity.** Garbage may be uncomputed only while its dependency set is unmodified, and every scheduler is confined to valid schedules. Added in round 4 after opus's round-3 B2 (blocking). ISA 0.9 §6 implements it, but with no NS citation.
2. **§5.1 part 2, lines 647–661, channel comparison.** Diamond-norm and operator-norm ε budgets, and "Floating-point matrix agreement counts as proof only with a rigorous numerical error bound". Added in round 2 after the round-1 blocking issues (codex B4, muse B5); the metrics were added in round 4 after codex's round-3 suggestion 1.
3. **§0, lines 65–68, the hardware premise.** "logical gate times on the order of nanoseconds. Every cost in this document is counted in that regime". First appears in the round-6 draft. No reviewer remark or editor response mentions it, so its origin is not recorded in the council files.

---

## Cited as North Star authority but not actually in the North Star

1. **Dialect §13.3, line 7013.** The dialect says: "All seven formats of §13.1 are required by the same table and contract (NS §3.2) … never permission to reject, trap or silently narrow the format capability." The North Star says (§3.2, lines 204–207): "binary64 and binary32 are covered; `half`, `bfloat`, `x86_fp80` and `fp128` are coverage-report lines until the library covers them". So the North Star explicitly allows trapped coverage lines for the other formats.
2. **Dialect §22, line 11887 (`BD-E-COHERENT-ANGLE`), and §10 TDL-10.** The dialect says: "NS §4.2 (predication, not superposed parameters)" and "NS promises predication, not superposed parameters". The North Star §4.2 says nothing about rotation angles. The rejection rests on the North Star's silence, plus ISA §3.1.
3. **Dialect §4.3 (line 916) and §17.1 (line 10136).** The dialect says: "There is one classical memory (NS §3.2), shared by H, CC, R and the entry state of every L invocation". The North Star §3.2 contains no such statement. The nearest North Star text is the shared layout model (lines 214–227) and §3.1's "classical memory whose contents happen to be superposed" (line 148). The support is loose.
4. **Dialect §18.4, line 10544.** The dialect says: "NS §4.2 requires 'allocations inside released clean or borrowed and restored'". The quoted words are in §3.1 (line 153, control row), not §4.2. The content exists; the section number is wrong.
5. **Dialect §19.8, line 11459.** The dialect says: "Every input MUST have one target datalayout and triple (NS §3.2)". The North Star (lines 266–268) mentions "one target datalayout" only as a description of a supported mixed link. It does not mention the triple and does not phrase it as a MUST.
6. **ISA 0.9 §0, entry E16.9 (line 138).** The ISA spec makes an out-of-range integer literal ill-formed, citing "NS §0 'never a silent wrong answer'". The North Star §0 (lines 57–61) is about executions outside the reference domain producing named faults, not about how literals are spelled.
7. **Dialect §25.1 BLK-4 (line 11970) and `BD-E-REVERSIBLE-MEASURE` (line 11883).** These cite "NS §4.2" for quantum allocation in, and measurement refusal for, reversible-tier code. The §4.2 reversible-tier row (line 478) states neither. Both are inferences from "deterministic forwards and backwards" (§1, decision 4).

Attribution by chain, not a misquote: dialect §5 (exact reference analysis) and §16 (byte-identical reference objects) carry no North Star citation. Their North Star authority runs only through dialect §9.4's citation of NS §4.2 (entry 1).
