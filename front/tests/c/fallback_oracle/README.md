# Native basis oracle for the front fallback

`../../tests/fallback_oracle.rs` was written from DESIGN.md before the fallback.
It does not read front lowering or executor arithmetic, gate, region or history
implementations. It imports the original independent dense oracle from
`crates/exec/tests/oracle_support/dense.rs` read-only as a second reference.

Every fixture has one `update(q, r, &y)` body, included in both builds. Host clang
compiles it at `-O0` and `-O2`. A **fresh native process per basis input** executes
that body once and prints q, r, y and four memory bytes; there are eight basis
inputs per program. There are no native marker implementations, no device runs
in this reference calculation, no expected values extracted from lowered IR.
Unsigned byte operations and bounded, in-bounds loops avoid C undefined behavior.

The common device driver prepares each basis column or a complex product state
(Ry(0.7) q, S H r, T H y), runs the same body, then measures q/r/y and returns those
bits together with the four bytes. The reference tabulates the preparation
analytically. Its coherent operation is a native basis permutation applied to
input amplitudes; its round trip is U-adjoint Z U then H q, with an explicit check
that repeating the fixture's U implements its inverse. No global-phase quotient:
complex amplitudes and signs must agree. The device adapter checks every extra
register is zero and there is one world before projecting to named C variables.

DESIGN.md, “What is promised when history is left behind”, permits full state
assertions only for the documented coherent class or after an un-run. Therefore:

- `coherent_xor` and `coherent_nested` exercise documented XOR updates. Each runs
  as a direct circuit and as a phase round trip, over all eight columns and the
  complex input, with O0, O2 and `--taint-all`. Both the device and the existing
  dense *interpreter* are compared with the native oracle.
- `store`, `memcpy`, `memset`, `user_write`, `nested`, `loop` retain writes in
  ordinary memory. `outside_class` deliberately erases y. Only immediate Born
  probabilities and exact basis results are asserted; phases/history layout are
  unspecified. The expected distribution sums squared input amplitudes by native
  measured result, rather than adding amplitudes across hidden histories. Once
  accepted, each case must emit a history/coherence warning. Diagnostics from
  successful lowering are retained and tested, rather than lost by the usual
  lower-text helper. Values are checked **before** checking warnings.
- `native_c_regions_under_marks_match_keep_and_restore_memory` lowers the six
  ordinary memory-writing bodies without quantum markers, then adds a **test-only
  device IR** mark/call/rewind-keep envelope. The native result is bit 1 of cell 0;
  q/r remain, this bit is XOR-kept into y, and everything in the ordinary C region
  is un-run. With S y inside the mark, the expected operation is S-adjoint X^f S,
  including its opposite imaginary signs on y=0 and y=1. Original dense matrix
  gates/permutations cross-check it, and all memory must be restored exactly.
  This validates the oracle/adapter now; it does not claim the front accepts
  these C programs under qubit control or exposes C mark/rewind markers.

The regular inventory always attempts all seven waiting fixtures in all three
modes, prints `WAITING <name>/<mode>: <named refusal>`, and checks every program
that succeeds. Only a `not handled yet:` refusal is an allowed waiting outcome;
a crash, invalid device program, fault or wrong value fails. The seven individually
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
checked by hand, host O0/O2 tables must agree, and the existing dense oracle checks
the working programs. These checks do not prove arbitrary native C correct, model
unspecified surviving coherence, or pin the private history encoding. Outside-class
sampling uses 512 fixed seeds and an absolute tolerance of 0.075 per bin; exact
basis runs catch wrong results independently of that statistical check.

Open integration details for step 4: no C mark/rewind marker is documented yet;
the exact source and wording of the warning are not frozen. The current acceptance
assertion requires a successful front diagnostic containing `warning` and
`history`, `entangl`, or `coherence`. If the public warning is delivered at runtime,
connect the CLI's actual diagnostic here; executor observation counters alone
must not be mistaken for a user-facing warning. If the fallback places C variables
in memory, the coherent-state adapter must map their named addresses to snapshot
cells; it deliberately fails rather than quietly dropping them today.
