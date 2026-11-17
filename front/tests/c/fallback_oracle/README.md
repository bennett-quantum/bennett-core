# Native basis oracle for the front fallback

`../../tests/fallback_oracle.rs` was written from DESIGN.md before the fallback.
It does not read front lowering or executor arithmetic, gate, region or history
implementations. It imports the original independent dense oracle from
`crates/exec/tests/oracle_support/dense.rs` read-only as a second reference.

Every fixture has a shared `update(q, r, &y)` body, included in both builds. Host clang
compiles it at `-O0` and `-O2`. A **fresh native process per basis input** executes
that body once and prints q, r, y and the whole twelve-byte memory image; there are
eight basis inputs per program. The four `cells` the bodies write lie in the middle
of `image` (`fixture.h`), between four guard bytes on each side (`a5 5a c3 3c` and
`96 69 f0 0f`), and every comparison below takes the whole image, so a stray write
just before or just past the cells is a wrong result on either side, not unseen. There are no device runs in this reference calculation and no expected values
extracted from lowered IR. The controlled-X baseline gives X its ordinary C basis
action (Boolean toggle); native runs do not execute preparation or phase gates.
Unsigned byte operations and bounded, in-bounds loops avoid C undefined behavior.

The common device driver prepares each basis column, a complex product state
(preparation 8: Ry(0.7) q, S H r, T H y) or a real one whose eight Born weights all
differ (preparation 9: Ry(0.7) q, Ry(1.3) r, Ry(2.2) y), runs the same body, then
measures q/r/y and returns those bits together with the four cells; the harness
reads the whole image from the finished machine's memory and checks the returned
cells against it. The reference tabulates the preparation
analytically. Its coherent operation is a native basis permutation applied to
input amplitudes; its round trip is U-adjoint Z U then H q, with an explicit check
that repeating the fixture's U implements its inverse. No global-phase quotient:
complex amplitudes and signs must agree. The device adapter checks every extra
register is zero before projecting to named C variables.

DESIGN.md, “What is promised when history is left behind”, permits full state
assertions only for the documented coherent class or after an un-run. Therefore:

- `coherent_gate` exercises a documented controlled gate. It runs
  as a direct circuit and as a phase round trip, over all eight columns and the
  complex input, with O0, O2 and `--taint-all`. Native expectations are compared
  with the full C device state, and the fixture's whole image must still hold its
  initial bytes there (the coherent class writes no ordinary memory, so a leaked
  write or a write whose un-run was missed is a wrong result). Every coherent
  program also gets the measured checks below, in every mode. The front allocates three byte registers plus
  three cleanup-fault registers (36 qubits), beyond the existing dense oracle's
  13-qubit limit. Independently authored three-bit versions of these circuits
  are therefore run by both the original dense *interpreter* and the device,
  and compared with the same native expectations. No circuit is extracted from
  front output; no change to the original oracle or its allocation limit is
  needed. Actual C snapshots still check every high bit and extra register.
- `store`, `memcpy`, `memset`, `user_write`, `nested`, `loop` retain writes in
  ordinary memory. `outside_class` deliberately erases y; it passes since the front
  keeps an old value it cannot erase in a register released at once (2026-11-17), and
  is no longer ignored. Only immediate Born
  probabilities and exact basis results are asserted; phases/history layout are
  unspecified. The expected distribution sums squared input amplitudes by native
  measured result, rather than adding amplitudes across hidden histories. The
  device's distribution is read **exactly** from its state just before the
  measurements, for every column and both superposed preparations: the weight of
  each configuration of q/r/y, keyed by the image, other registers summed over. Then 512 seeded runs check the
  measurement path itself, and the eight columns are run to their exact results. Once
  accepted, each case must emit a history/coherence warning. Diagnostics from
  successful lowering are retained and tested, rather than lost by the usual
  lower-text helper. Values are checked **before** checking warnings.
- `native_c_regions_under_apply_match_native_and_the_dense_oracle` lowers the six
  ordinary memory-writing bodies without quantum markers and runs them in
  seven-qubit `apply` envelopes under both the original dense interpreter and the
  device, compared to native results (with S y around the `apply`, the expected
  operation is S-adjoint X^f S, cross-checked by the dense matrix gates and
  permutations), and the whole twelve-byte image, guard bytes included, runs
  through the dense interpreter four bits at a time (24 nibbles) with q in uniform
  superposition (its 13-qubit cap). This validates the oracle/adapter now; it does
  not claim the front accepts these C programs under qubit control. (Until
  2026-11-17 they also ran under a test-only `mark`/`rewind` envelope, removed with
  those instructions.)

`coherent_xor` and `coherent_nested` preserve measured refusals even inside the
documented class: Boolean `y ^= q && !r` and nested `if (q) if (r) y ^= 1` are
refused as updates without a known inverse (the nested form works at O2 and is
checked there, but refuses at O0 and with `--taint-all`). If either
starts compiling, the inventory checks its full complex vector without requiring
a history warning. These refusals are debts, not wrong results.

The regular inventory always attempts all nine waiting fixtures in all three
modes, prints `WAITING <name>/<mode>: <named refusal>`, and checks every program
that succeeds. Only a `not handled yet:` refusal is an allowed waiting outcome;
a crash, invalid device program, fault or wrong value fails. The nine individually
ignored acceptance tests fail with the refusal when explicitly run. Unignore each
as fallback support lands; the regular inventory ensures acceptance cannot silently
stop being checked in the meantime.

Run after building the front:

```sh
front/test.sh --test fallback_oracle -- --nocapture --test-threads=2
front/test.sh --test fallback_oracle waiting_for_fallback -- --ignored --nocapture --test-threads=2
```

The second command is expected to fail while those features are waiting. The
reference-only hand checks need host clang but no front executable:

```sh
cargo test -p bennett-front-tests --test fallback_oracle native_oracle_has_hand -- --nocapture
```

The reference's risks are extraction/packing mistakes, an incorrectly chosen
coherence guarantee, and undefined native behavior. Small hand calculations cover
entanglement, kickback and both S-adjoint X S phase signs; memory examples are also
checked by hand, host O0/O2 tables must agree, and the existing dense oracle checks independently authored compact versions
of the working circuits (also run on the device). A wrong correspondence between
a compact witness and its C program is an additional adapter risk; both are
compared against the independently executed native table. These checks do not prove arbitrary native C correct, model
unspecified surviving coherence, or pin the private history encoding. Outside-class
sampling uses 512 fixed seeds and an absolute tolerance of 0.075 per bin; exact
basis runs catch wrong results independently of that statistical check.

Native side: `native_reference_is_free_of_undefined_behaviour` builds every
fixture with ASan and UBSan and requires the same eight images and no diagnostic.
Native and device use the same clang, target, include path and fixture body; the
native build only adds `-DNATIVE` (the `main` driver, and X as a Boolean toggle in
`coherent_gate`).

**Seeded wrong results** (`mod seeded`): mutants of the lowered programs must be
rejected: a leftover ordinary write in a coherent program in all three modes, a
wrong phase, a swapped state and a global phase in a coherent program, and a wrong
r-dependent write or a stray write beside the cells in an applied region. The
seeds on a hand-written, fallback-shaped `store.c` (a call read q) were removed on
2026-11-17 with the reading of registers by ordinary code; they return with the
route that replaces the fallback, on a program the model expresses. An
independent testing pass found that the first version of this harness accepted
four of them: the 512-sample tolerance (0.075) exceeds every q = 1 branch's weight
(0.0296), preparation 8 gives y equal weights so a swapped y is invisible to any
measurement, the coherent projection ignored ordinary memory (and the measured
check ran at O0 only), and the region kept one bit, the same for both values of r
in four of the six regions. Each is now a regression test.

Open integration details for the route that replaces the fallback (`HANDOFF.md`,
"Next steps", 4): the exact source and wording of the warning are not frozen. The current acceptance
assertion requires a successful front diagnostic containing `warning` and
`history`, `entangl`, or `coherence`. If the public warning is delivered at runtime,
connect the CLI's actual diagnostic here; executor observation counters alone
must not be mistaken for a user-facing warning. If the fallback places C variables
in memory, the coherent-state adapter must map their named addresses to snapshot
cells; it deliberately fails rather than quietly dropping them today.

Measured after the testing pass: ten native programs / 160 host basis runs plus 80
sanitized; 54 coherent baseline vectors plus nine accepted nested-O2 vectors; four
programs (coherent_gate in three modes, nested O2) each with 8 exact columns, 10
exact Born tables and 512 sampled outcomes; 54 ordinary-C region vectors with the
whole image kept, each checked under mark/rewind, under apply in the original
dense interpreter and device, and by dense conjugation, plus 48 dense nibble
runs (144 since the guard bytes: 24 nibbles of the twelve-byte image). The default
oracle suite has twelve passing tests (seven of them seeded wrong results;
fourteen and nine since the guard bytes) and nine intentionally ignored acceptance tests; inventory has
one accepted mode and 26 named refusals. No
accepted wrong result was found. Warning checks remain waiting with the refused
outside-class programs; none has silently been counted as a tested warning.

Required checks passed after the testing pass:
`cargo test` (292 passed, 19 ignored),
`cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`, and
`front/test.sh` (170 passed, 21 ignored). Front-specific clippy also passed.
