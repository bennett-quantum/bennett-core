//! A rewind whose un-run meets a state its history does not describe (`docs/DESIGN.md`, "History: `mark`
//! and `rewind`", step 1), and undoing rewinds reached again in loops ("Undoing a rewind").

mod common;

use bennett_exec::{Executor, Options, Outcome};
use bennett_ir::{Fault, Inst, ValidModule};
use common::{TOLERANCE, assert_reverses, assert_same_state, bounded, executor, load};
use num_complex::Complex64 as C;

fn options() -> Options {
    Options { reversible: true, ..bounded() }
}

/// Steps until the next instruction is the `n`th `rewind` the run reaches, counting from 1.
fn at_rewind(e: &mut Executor<'_>, module: &ValidModule, n: usize) {
    let mut seen = 0;
    for _ in 0..10_000 {
        let s = e.snapshot();
        let f = s.classical.frames.last().expect("frame");
        if matches!(module.function(f.func).block(f.block).insts.get(f.inst), Some(Inst::Rewind { .. })) {
            seen += 1;
            if seen == n {
                return;
            }
        }
        assert_eq!(e.step(), None);
    }
    panic!("rewind {n} not reached");
}

/// The next step faults with `bad_rewind` and changes nothing: state, step count and journal size.
fn assert_bad_rewind_changes_nothing(e: &mut Executor<'_>, context: &str) {
    let before = e.snapshot();
    let bytes = e.stats().journal_bytes;
    assert_eq!(e.step(), Some(Outcome::Faulted(Fault::BadRewind)), "{context}");
    let mut after = e.snapshot();
    after.classical.outcome = None;
    assert_same_state(&after, &before, context);
    assert_eq!(e.stats().journal_bytes, bytes, "{context}: journal size");
}

/// The smallest form (jm-test1 F1): un-running the allocation of a temporary that read the kept register
/// would discard every configuration. Before the fix the run returned with norm 0.
#[test]
fn a_temporary_that_read_a_kept_register_makes_the_rewind_fault() {
    let module = load(
        "memory 256\nfunc @main() {\nentry:\n%q = qalloc 1\n%h = mark\n%tmp = qalloc 1\nx %tmp ctrl %q\nrewind %h keep (i1 1 -> %q)\nret\n}",
    );
    let mut e = executor(&module, "main", &[], options());
    at_rewind(&mut e, &module, 1);
    assert_bad_rewind_changes_nothing(&mut e, "F1");
    let norm: f64 = e.snapshot().quantum.amplitudes.iter().map(|(_, a)| a.norm_sqr()).sum();
    assert!((norm - 1.0).abs() < TOLERANCE, "norm {norm}");
    assert_reverses(&module, "main", &[], options());
}

/// A two-qubit state over (q, tmp), index q + 2 tmp: the independent calculation for the exhaustive
/// check below. It knows the gates' matrices and nothing of the executor.
type State = [C; 4];

#[derive(Clone, Copy, Debug)]
enum Op {
    H(usize),
    X(usize),
    T(usize),
    Ry(usize),
    /// A gate on the first qubit controlled by the second (0 is q, 1 is tmp).
    Cx(usize, usize),
    Ch(usize, usize),
}

const ANGLE: f64 = 0.47;

impl Op {
    const ALL: [Op; 10] = [
        Op::H(0),
        Op::X(0),
        Op::T(0),
        Op::Ry(0),
        Op::H(1),
        Op::X(1),
        Op::Cx(1, 0),
        Op::Cx(0, 1),
        Op::Ch(1, 0),
        Op::Ry(1),
    ];

    fn text(self) -> String {
        let name = |q: usize| if q == 0 { "%q" } else { "%tmp" };
        match self {
            Op::H(t) => format!("h {}", name(t)),
            Op::X(t) => format!("x {}", name(t)),
            Op::T(t) => format!("t {}", name(t)),
            Op::Ry(t) => format!("ry({ANGLE}) {}", name(t)),
            Op::Cx(t, c) => format!("x {} ctrl {}", name(t), name(c)),
            Op::Ch(t, c) => format!("h {} ctrl {}", name(t), name(c)),
        }
    }

    /// The 2x2 matrix, its target and its control.
    fn parts(self, inverse: bool) -> ([[C; 2]; 2], usize, Option<usize>) {
        let (z, o) = (C::new(0.0, 0.0), C::new(1.0, 0.0));
        let r = C::new(std::f64::consts::FRAC_1_SQRT_2, 0.0);
        let (c, s) = (C::new((ANGLE / 2.0).cos(), 0.0), C::new((ANGLE / 2.0).sin(), 0.0));
        let phase =
            C::from_polar(1.0, if inverse { -std::f64::consts::FRAC_PI_4 } else { std::f64::consts::FRAC_PI_4 });
        let ry = if inverse { [[c, s], [-s, c]] } else { [[c, -s], [s, c]] };
        match self {
            Op::H(t) => ([[r, r], [r, -r]], t, None),
            Op::X(t) => ([[z, o], [o, z]], t, None),
            Op::T(t) => ([[o, z], [z, phase]], t, None),
            Op::Ry(t) => (ry, t, None),
            Op::Cx(t, c) => ([[z, o], [o, z]], t, Some(c)),
            Op::Ch(t, c) => ([[r, r], [r, -r]], t, Some(c)),
        }
    }

    fn apply(self, state: &mut State, inverse: bool) {
        let (m, target, control) = self.parts(inverse);
        let mut next = [C::new(0.0, 0.0); 4];
        for (index, &amplitude) in state.iter().enumerate() {
            if control.is_some_and(|c| index >> c & 1 == 0) {
                next[index] += amplitude;
                continue;
            }
            let bit = index >> target & 1;
            let base = index & !(1 << target);
            next[base] += m[0][bit] * amplitude;
            next[base | 1 << target] += m[1][bit] * amplitude;
        }
        *state = next;
    }
}

fn tmp_weight(state: &State) -> f64 {
    state[2].norm_sqr() + state[3].norm_sqr()
}

/// Every program `prep q; h = mark; tmp = qalloc 1; ops; [qfree tmp]; rewind h keep (k -> q); ret` with
/// up to two operations from ten (gates on q, on tmp, and between them), four preparations, both keep
/// values, with and without the free: the rewind succeeds, with the conjugated state, exactly when the
/// un-run brings tmp back to zero; otherwise it faults with `bad_rewind` and changes nothing. Either way
/// the norm is kept and the run reverses to its entry.
#[test]
fn un_running_a_temporary_faults_exactly_when_it_is_not_back_to_zero() {
    let mut sequences = vec![vec![]];
    for a in Op::ALL {
        sequences.push(vec![a]);
        for b in Op::ALL {
            sequences.push(vec![a, b]);
        }
    }
    let preps: [Option<Op>; 4] = [None, Some(Op::X(0)), Some(Op::H(0)), Some(Op::Ry(0))];
    let (mut succeeded, mut faulted, mut dirty_forward) = (0, 0, 0);
    for prep in preps {
        for ops in &sequences {
            for free in [false, true] {
                for keep in [0u64, 1] {
                    let mut text = String::from("memory 256\nfunc @main() {\nentry:\n%q = qalloc 1\n");
                    if let Some(prep) = prep {
                        text += &format!("{}\n", prep.text());
                    }
                    text += "%h = mark\n%tmp = qalloc 1\n";
                    for op in ops {
                        text += &format!("{}\n", op.text());
                    }
                    if free {
                        text += "qfree %tmp\n";
                    }
                    text += &format!("rewind %h keep (i1 {keep} -> %q)\nret\n}}");
                    let context = format!("{prep:?} {ops:?} free {free} keep {keep}");
                    let module = load(&text);

                    let mut state: State = [C::new(1.0, 0.0), C::new(0.0, 0.0), C::new(0.0, 0.0), C::new(0.0, 0.0)];
                    if let Some(prep) = prep {
                        prep.apply(&mut state, false);
                    }
                    let at_mark = state;
                    for op in ops {
                        op.apply(&mut state, false);
                    }
                    if free && tmp_weight(&state) > TOLERANCE {
                        let mut e = executor(&module, "main", &[], options());
                        assert_eq!(e.run(), Outcome::Faulted(Fault::DirtyFree), "{context}");
                        dirty_forward += 1;
                        continue;
                    }
                    if keep == 1 {
                        state = [state[1], state[0], state[3], state[2]];
                    }
                    for op in ops.iter().rev() {
                        op.apply(&mut state, true);
                    }

                    let mut e = executor(&module, "main", &[], options());
                    at_rewind(&mut e, &module, 1);
                    if tmp_weight(&state) > TOLERANCE {
                        assert_bad_rewind_changes_nothing(&mut e, &context);
                        faulted += 1;
                    } else {
                        assert_eq!(e.run(), Outcome::Returned(None), "{context}");
                        let quantum = e.snapshot().quantum;
                        for (q, &expected) in state[..2].iter().enumerate() {
                            let got = quantum.amplitude(&[q as u64]);
                            assert!((got - expected).norm() <= TOLERANCE, "{context}: q={q}: {got} against {expected}");
                        }
                        succeeded += 1;
                    }
                    // A keep of 0 leaves the history matching: the state at the mark again.
                    if keep == 0 {
                        assert!(at_mark.iter().zip(&state).all(|(a, b)| (a - b).norm() <= TOLERANCE), "{context}");
                    }
                    assert_reverses(&module, "main", &[], options());
                }
            }
        }
    }
    assert_eq!((succeeded, faulted, dirty_forward), (1064, 156, 556), "counts");
}

/// A rewind inside the un-run region, run again under the outer keep, frees a register that is no longer
/// zero. Before the fix this panicked in the executor.
#[test]
fn a_freed_register_not_back_to_zero_when_an_inner_region_runs_again_faults() {
    let module = load(
        "memory 256\nfunc @main() {\nentry:\n%q = qalloc 1\nx %q\n%h = mark\n%i = mark\n%t = qalloc 1\nx %t ctrl %q\nx %t\nqfree %t\nrewind %i\nrewind %h keep (i1 1 -> %q)\nret\n}",
    );
    let mut e = executor(&module, "main", &[], options());
    at_rewind(&mut e, &module, 2);
    assert_bad_rewind_changes_nothing(&mut e, "dirty free in the run again");
    assert_reverses(&module, "main", &[], options());
}

/// An inner rewind, run again inside an un-run region under the outer keep, itself meets a temporary
/// that is not back to zero: the inner fault makes the outer rewind fault, and nothing changes.
#[test]
fn an_inner_rewind_that_faults_when_run_again_makes_the_outer_one_fault() {
    let module = load(
        "memory 256\nfunc @main() {\nentry:\n%q = qalloc 1\n%r = qalloc 1\n%h = mark\n%a = mark\n%i = mark\n%t = qalloc 1\nx %t ctrl %q, %r\nrewind %i keep (i1 1 -> %r)\nrewind %a\nrewind %h keep (i1 1 -> %q)\nret\n}",
    );
    let mut e = executor(&module, "main", &[], options());
    at_rewind(&mut e, &module, 3);
    assert_bad_rewind_changes_nothing(&mut e, "inner bad_rewind in the run again");
    assert_reverses(&module, "main", &[], options());
    // With q kept as 0 the history still matches and the same rewind runs.
    let module = load(
        "memory 256\nfunc @main() {\nentry:\n%q = qalloc 1\n%r = qalloc 1\n%h = mark\n%a = mark\n%i = mark\n%t = qalloc 1\nx %t ctrl %q, %r\nrewind %i keep (i1 1 -> %r)\nrewind %a\nrewind %h keep (i1 0 -> %q)\nret\n}",
    );
    let mut e = executor(&module, "main", &[], options());
    assert_eq!(e.run(), Outcome::Returned(None));
    assert_reverses(&module, "main", &[], options());
}

/// Runs with `budget`, keeping a snapshot after every step, then steps back through every one of them.
fn assert_every_step_reverses(module: &ValidModule, budget: u64) {
    let mut e = executor(module, "main", &[], Options { budget: Some(budget), ..options() });
    let mut snapshots = vec![e.snapshot()];
    while e.step().is_none() {
        snapshots.push(e.snapshot());
    }
    assert_eq!(e.stats().steps, budget, "the budget ends the loop");
    assert_eq!(e.outcome(), Some(Outcome::Faulted(Fault::Budget)));
    for (index, expected) in snapshots[..snapshots.len() - 1].iter().enumerate().rev() {
        assert_eq!(e.step_back(), Ok(true), "budget {budget}, step {index}");
        assert_same_state(&e.snapshot(), expected, &format!("budget {budget}, back to step {index}"));
    }
    assert_eq!(e.step_back(), Ok(false));
    assert_eq!(e.stats().steps, 0);
}

/// jm-test1 F2: undoing a rewind reached the second (or nth) time in a loop restores the visit being
/// undone, step by step, with gates and keeps, a mark per iteration nested in an outer one, and across a
/// call; at every budget, reversal returns to the entry with the step count 0.
#[test]
fn undoing_rewinds_reached_again_in_loops_restores_every_step() {
    let witness = "memory 256\nfunc @main() {\nentry:\n%q = qalloc 1\n%h = mark\nbr loop\nloop:\nrewind %h keep (i1 1 -> %q)\nbr loop\n}";
    let module = load(witness);
    let mut e = executor(&module, "main", &[], options());
    at_rewind(&mut e, &module, 2);
    let before = e.snapshot();
    assert_eq!(e.step(), None);
    assert_eq!(e.step_back(), Ok(true));
    assert_same_state(&e.snapshot(), &before, "undo of the second visit");

    let programs = [
        witness.to_string(),
        "memory 256\nfunc @main() {\nentry:\n%q = qalloc 1\n%h = mark\nbr loop\nloop:\nh %q\nrewind %h keep (i1 1 -> %q)\nbr loop\n}".into(),
        "memory 256\nfunc @main() {\nentry:\n%h = mark\nbr loop\nloop:\nrewind %h\nbr loop\n}".into(),
        "memory 256\nfunc @main() {\nentry:\n%q = qalloc 2\n%h = mark\nbr outer\nouter:\n%g = mark\nh %q[0]\nrewind %g keep (i2 1 -> %q)\nx %q[1] ctrl %q[0]\nrewind %h keep (i2 2 -> %q)\nbr outer\n}".into(),
        "memory 256\nfunc @f(q2 %q) {\nentry:\n%g = mark\nry(0.47) %q[1]\nrewind %g keep (i2 2 -> %q)\nret\n}\nfunc @main() {\nentry:\n%q = qalloc 2\n%h = mark\nbr loop\nloop:\ncall @f(%q)\nh %q[0]\nrewind %h keep (i2 1 -> %q)\nbr loop\n}".into(),
    ];
    for text in &programs {
        let module = load(text);
        for budget in 1..=16 {
            assert_every_step_reverses(&module, budget);
        }
    }
}

/// Rewinding one mark again un-runs the previous rewind of it, so a loop of rewinds of one mark doubles
/// the work per rewind (the same doubling as nested cleaning-up regions). Measured: n rewinds un-run
/// 3 * 2^(n-1) - 2 steps; reversal runs again as many steps as it un-runs, and returns to step 0.
#[test]
fn a_loop_of_rewinds_of_one_mark_doubles_the_work_per_rewind() {
    let module = load("memory 256\nfunc @main() {\nentry:\n%h = mark\nbr loop\nloop:\nrewind %h\nbr loop\n}");
    for budget in [1u64, 2, 3, 4, 8, 16, 32] {
        let mut e = executor(&module, "main", &[], Options { budget: Some(budget), ..options() });
        assert_eq!(e.run(), Outcome::Faulted(Fault::Budget));
        let rewinds = (budget - 1) / 2;
        let rewound = if rewinds == 0 { 0 } else { 3 * (1 << (rewinds - 1)) - 2 };
        assert_eq!(e.stats().rewound_steps, rewound, "budget {budget}");
        assert_eq!(e.reverse(), Ok(budget));
        let stats = e.stats();
        assert_eq!(stats.steps, 0);
        assert_eq!(stats.rerun_steps, stats.rewound_steps, "budget {budget}: every un-run step is run again");
    }
}
