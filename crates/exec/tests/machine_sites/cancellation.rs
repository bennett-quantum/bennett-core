//! Clean discard after cancellation and unequal paths.
use super::{Circuit, Options, Outcome, Site, assert_dense, common, matrix, prepare};

#[test]
fn zero_wire_discard_after_cancellation_preserves_later_interference_and_outer_reversal() {
    // The optional read is implemented by |a>|0> -> |a>|a>. Its inverse clears the
    // wire. H† on each separated a=0/1 part then has a=1 support, with opposite
    // amplitudes: only their SUM is |0>. Allocation/unpromotion must wait for that
    // cancellation. Erasing the read from the source is an independent control.
    for input in 0..6 {
        for memory in [false, true] {
            for read in [false, true] {
                for outer in [false, true] {
                    let mut dense = Circuit::new(5);
                    let prep = prepare(&mut dense, input);
                    if !outer {
                        dense.gate(matrix("x"), 2, 0);
                    }
                    dense.gate(matrix("h"), 0, 0);
                    dense.gate(matrix("h"), 1, 0);
                    let target = if memory { "[24:0]" } else { "%a" };
                    let alloc = if memory { "" } else { "%a = qalloc 1\n" };
                    let access = if !read {
                        ""
                    } else if memory {
                        "%v = load i8 24\n"
                    } else {
                        "%v = call @id1(%a)\n"
                    };
                    let text = format!(
                        "memory 2048
func @id1(i1 %a) -> i1 {{
entry:
ret %a
}}
func @id2(i2 %c) -> i2 {{
entry:
ret %c
}}
func @main() -> i8 {{
entry:
%c = qalloc 2
{prep}%k = qalloc 1
{outer_mark}%m = mark budget 100
%split = call @id2(%c)
{alloc}h {target}
s {target}
{access}h {target}
rewind %m keep (i1 1 -> %k)
{outer_rewind}h %c[0]
h %c[1]
ret 7
}}",
                        outer_mark = if outer { "%outer = mark budget 400\n" } else { "" },
                        outer_rewind = if outer { "rewind %outer\n" } else { "" }
                    );
                    let module = common::load(&text);
                    for machine_meaning in [false, true] {
                        let mut e = common::executor(
                            &module,
                            "main",
                            &[],
                            Options { machine_meaning, reversible: true, ..common::bounded() },
                        );
                        let start = e.snapshot();
                        let context = format!(
                            "zero discard input={input} cell={memory} read={read} outer={outer} machine={machine_meaning}"
                        );
                        assert_eq!(e.run(), Outcome::Returned(Some(7)), "{context}");
                        assert_dense(&e, &dense, Site::Allocation, &context);
                        assert_eq!(e.stats().leftover_qubits, 0, "{context}");
                        assert_eq!(e.snapshot().quantum.registers.len(), 2, "{context}: all clean wires removed");
                        e.reverse().unwrap_or_else(|err| panic!("{context}: {err:?}"));
                        common::assert_same_state(&e.snapshot(), &start, &context);
                    }
                }
            }
        }
    }
}

#[test]
fn read_wires_zero_only_by_cancellation_are_discarded_after_the_worlds_sum() {
    // H(a); COPY(a,w1); H(a); COPY(a,w2). Inverting the last copy clears w2.
    // H† then gives each world both a values; inverse COPY(a,w1) leaves nonzero
    // w1 in each part. Their w1=1 amplitudes cancel across the w2 outcomes. The
    // final H† clears a too. This computes U† (X_k) U = X_k exactly, including
    // the hidden wires. Omitting both reads is the same unitary control case.
    for input in 0..6 {
        for reads in [false, true] {
            for memory in [false, true] {
                let mut dense = Circuit::new(5);
                let prep = prepare(&mut dense, input);
                dense.gate(matrix("x"), 2, 0);
                dense.gate(matrix("h"), 0, 0);
                dense.gate(matrix("h"), 1, 0);
                let (alloc, target, read1, read2) = if memory {
                    ("", "[24:0]", "%v1 = load i8 24\n", "%v2 = load i8 24\n")
                } else {
                    ("%a = qalloc 1\n", "%a", "%v1 = call @id(%a)\n", "%v2 = call @id(%a)\n")
                };
                let text = format!(
                    "memory 2048
func @id(i1 %a) -> i1 {{
entry:
ret %a
}}
func @main() -> i8 {{
entry:
%c = qalloc 2
{prep}%k = qalloc 1
%m = mark budget 100
{alloc}h {target}
{r1}h {target}
{r2}s {target}
rewind %m keep (i1 1 -> %k)
h %c[0]
h %c[1]
ret 7
}}",
                    r1 = if reads { read1 } else { "" },
                    r2 = if reads { read2 } else { "" }
                );
                let module = common::load(&text);
                for machine_meaning in [false, true] {
                    let mut e = common::executor(
                        &module,
                        "main",
                        &[],
                        Options { machine_meaning, reversible: true, ..common::bounded() },
                    );
                    let start = e.snapshot();
                    assert_eq!(
                        e.run(),
                        Outcome::Returned(Some(7)),
                        "cell={memory} reads={reads} input={input} machine={machine_meaning}"
                    );
                    assert_dense(&e, &dense, Site::Read, "cancelling read wire");
                    assert_eq!(e.stats().leftover_qubits, 0);
                    assert_eq!(e.snapshot().quantum.registers.len(), 2);
                    e.reverse().expect("cancelling read wire reverses");
                    common::assert_same_state(&e.snapshot(), &start, "cancelling read wire");
                }
            }
        }
    }
}

#[test]
fn unequal_clean_read_counts_rejoin_before_later_interference() {
    // Reading a qubit, using it only to choose a count of clean reads, then rewinding
    // erases the path: U†U=I. Two branches temporarily have different wire tables.
    for extra in [1, 2, 3] {
        let reads: String = (0..extra).map(|n| format!("%extra{n} = call @id(%a)\nh %a\n")).collect();
        let text = format!(
            "memory 2048
func @id(i1 %a) -> i1 {{
entry:
ret %a
}}
func @main() -> i8 {{
entry:
%c = qalloc 1
h %c
%a = qalloc 1
h %a
s %a
%m = mark budget 100
%v = call @id(%c)
br %v, longer, done
longer:
{reads}br done
done:
rewind %m
h %c
%m0 = measure %c
%r = zext i1 %m0 to i8
ret %r
}}"
        );
        let module = common::load(&text);
        for machine_meaning in [false, true] {
            for seed in 0..16 {
                let mut e = common::executor(
                    &module,
                    "main",
                    &[],
                    Options { machine_meaning, seed, reversible: true, ..common::bounded() },
                );
                assert_eq!(e.run(), Outcome::Returned(Some(0)), "extra={extra} machine={machine_meaning} seed={seed}");
                assert_eq!(e.stats().leftover_qubits, 0);
                assert_eq!(e.stats().observations, 0, "no path observation");
            }
        }
    }
}
