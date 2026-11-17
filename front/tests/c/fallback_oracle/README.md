# Native basis oracle for the front fallback

The Rust harness in `../../tests/fallback_oracle.rs` compares the front's output
with native C and uses the independent dense oracle in
`crates/exec/tests/oracle_support/dense.rs` for small circuits.

Every fixture shares its `update(q, r, &y)` body between the native and device
builds. For each of the eight definite inputs, a fresh native process runs that
body at O0 and O2. It prints q, r, y and the whole twelve-byte image, including
four guard bytes on each side of the four result cells. Native C models
controlled X as a Boolean toggle. The harness calculates preparation amplitudes
analytically. A separate ASan/UBSan build of each fixture checks the same eight
native images and requires an empty diagnostic stream.

## Measurements and exact weights

The device driver prepares one of eight definite inputs or one of two product
states. Preparation 8 applies Ry(0.7) on q, H then S on r, and H then T on y.
Preparation 9 applies Ry(0.7), Ry(1.3) and Ry(2.2) on q, r and y, giving
eight distinct weights. The driver runs the shared body and returns the
measured q, r, y and four result cells.

The default device builds of `store`, `memset`, `nested`, `memcpy` and `loop`
measure the cells with `bennett_measure_bytes`. A write under qubit control
makes the bytes hold qubits, so the programmer must measure them before
ordinary C can read the result. The driver measures the cells first so the
snapshot just before the first measurement contains the updated bytes at their
addresses.

For each basis configuration in that snapshot, the adapter reads the ordinary
bytes and fills in each byte holding qubits from the register identified by the
snapshot's address-to-register table. It adds squared amplitude magnitudes for
configurations with the same q, r, y and full image. The reference calculates
the expected Born weights from the native images and the analytic preparation.
The harness compares every result weight with an absolute tolerance of 1e-9.
It uses the same byte decoding after execution and requires all remaining
configurations to agree on the image, including any guard bytes that still hold
qubits. The returned cells must agree with the image.

Each of the five fixtures checks eight exact basis outcomes, ten exact Born
tables and 512 seeded outcomes at O0, O2 and O0 with `--taint-all`. That is 120
basis outcomes, 150 exact tables and 7,680 sampled outcomes across the 15
fixture and mode combinations. The sample check rejects impossible results and
uses an absolute tolerance of 0.075 per result. The exact table check catches
changes too small for that tolerance. Preparation 9 distinguishes basis
inputs whose weights happen to agree in preparation 8. The harness checks the
front's coherence warning after checking the values. `outside_class` runs the
same checks for an overwrite that erases y and releases its old value.

## Complex amplitudes and applied functions

- `coherent_gate` exercises a controlled X directly and in a phase round trip
  (U† Z U, then H on q) at O0, O2 and with `--taint-all`. The native reference
  checks that repeating U implements U†. All complex amplitudes, including
  their signs and global phase, must agree. The lowered snapshots must keep
  the whole image at its initial values and every extra register at zero.
  Independently authored three-bit circuits fit the dense interpreter's
  13-qubit cap and are compared with the same native expectations. The direct
  circuit also gets the measured checks in all three modes.
- `coherent_xor` and `coherent_nested` check conditional Boolean XOR updates
  with the same complex-vector and measured checks in all three modes.
- `native_c_regions_under_apply_match_native_and_the_dense_oracle` runs the
  six ordinary memory-writing bodies in seven-qubit `apply` circuits under
  both the dense interpreter and the device. Dense matrix gates and
  permutations cross-check the S† X^f S calculation. The dense interpreter
  also checks the full twelve-byte image returned by the applied function
  four bits at a time, for all four values of q and r.

The seeded tests reject a leaked ordinary write in a coherent circuit, wrong
relative or global phase, an incorrect basis state, a wrong r-dependent write
inside an applied function, and writes into either guard. The hand calculations
check both signs of phase kickback and selected native memory images.

## Running the oracle

```sh
front/test.sh --test fallback_oracle -- --nocapture --test-threads=2
```

The suite passes 16 tests and ignores `user_write`, which calls a
memory-writing function under qubit control. The inventory checks 24 accepted
fixture and mode combinations and reports three named refusals for that case.
Run the ignored case explicitly to see the refusal.

```sh
front/test.sh --test fallback_oracle waiting_for_fallback::user_write -- --ignored --exact --nocapture
```

The reference-only hand checks need host clang and run independently of the
front executable.

```sh
cargo test -p bennett-front-tests --test fallback_oracle native_oracle_has_hand -- --nocapture
```
