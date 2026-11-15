//! Un-running through gates across worlds equals the conjugated operation, against an independent dense
//! calculation (`oracle_support`, nalgebra; `docs/DESIGN.md`, "Worlds").
//!
//! The program prepares q (2 qubits), r and o, marks, reads q as an integer (one world per value v of q),
//! applies gates G, and rewinds keeping f(v) into o. Its meaning, written out densely: the part of the
//! state where q = v becomes G† X_o^{f(v)} G of itself, and the parts add. Un-running the read is faithful
//! only if that part still has q = v; where G carries o into q it does not, and the executor must fault
//! with `bad_rewind` instead of adding amplitudes that no longer belong together.

#[allow(dead_code, reason = "this test uses the dense calculation and the whole-world reference only")]
mod oracle_support;

use bennett_exec::{Executor, Options, Outcome, RunError};
use bennett_ir::{Fault, Inst, ValidModule};
use nalgebra::DVector;
use num_complex::Complex64 as C;
use oracle_support::dense::{Dense, EPS, gate_matrix};

/// Gates G between the read and the rewind; the ones marked carry o into q.
const MIDDLE: [&str; 9] = [
    "h %r",
    "ry(0.4) %r",
    "x %r ctrl %q[0]",
    "x %o[0] ctrl %r",
    "h %o[1]",
    "x %q[1] ctrl %o[0]", // carries o into q
    "s %q[0]",
    "h %q[1]",
    "x %q[0] ctrl %r",
];

/// The program with gates `middle`; with `memory`, each world also writes memory between the read and
/// the rewind (the callee at an address that depends on the value read, the caller the value), so the
/// worlds' overlays differ and the rewind must restore them for the worlds to merge.
fn program(middle: &[&str], memory: bool) -> String {
    let middle: String = middle.iter().map(|gate| format!("    {gate}\n")).collect();
    let (callee_store, caller_store) = if memory {
        (
            "    %w = zext i2 %z to i8\n    %a = zext i2 %x to i64\n    %p = add i64 %a, 8\n    store i8 %w, %p\n",
            "    %yb = zext i2 %y to i8\n    store i8 %yb, 32\n",
        )
    } else {
        ("", "")
    };
    format!(
        "memory 256
data 8 = i8 7, i8 7, i8 7, i8 7
func @f(i2 %x) -> i2 {{
entry:
    %y = mul i2 %x, 3
    %z = add i2 %y, 1
{callee_store}    ret %z
}}
func @main() {{
entry:
    %q = qalloc 2
    %r = qalloc 1
    %o = qalloc 2
    h %q[0]
    ry(0.7) %q[1]
    h %r
    rx(0.3) %o[0]
    %m = mark
    %y = call @f(%q)
{caller_store}{middle}    rewind %m keep (%y -> %o)
    h %q[0]
    ret
}}
"
    )
}

fn f(v: u64) -> u64 {
    (3 * v + 1) % 4
}

/// The gates of `@main` in order, split at the mark, the call and the rewind: (before, middle, after).
type Gates = Vec<(bennett_ir::Gate, (usize, u8), Vec<(usize, u8)>)>;

fn gates(module: &ValidModule) -> [Gates; 3] {
    let main = module.function(module.function_named("main").expect("@main"));
    let registers: Vec<bennett_ir::Slot> = main.blocks[0]
        .insts
        .iter()
        .filter(|inst| matches!(inst, Inst::Qalloc { .. }))
        .filter_map(Inst::result)
        .collect();
    let column = |slot| registers.iter().position(|&r| r == slot).expect("a register");
    let mut phases: [Gates; 3] = Default::default();
    let mut phase = 0;
    for inst in &main.blocks[0].insts {
        match inst {
            Inst::Call { .. } | Inst::Rewind { .. } => phase += 1,
            Inst::Gate { gate, target, controls } => phases[phase.min(2)].push((
                *gate,
                (column(target.register), target.bit),
                controls.iter().map(|c| (column(c.register), c.bit)).collect(),
            )),
            _ => {}
        }
    }
    phases
}

fn apply(dense: &mut Dense, gates: &Gates, adjoint: bool) {
    let mut order: Vec<_> = gates.iter().collect();
    if adjoint {
        order.reverse();
    }
    for (gate, target, controls) in order {
        let matrix = gate_matrix(*gate);
        dense.gate(if adjoint { matrix.adjoint() } else { matrix }, *target, controls);
    }
}

/// The expected state, or `None` if some world's un-run of the read would not be faithful.
fn expected(module: &ValidModule) -> Option<Dense> {
    let [before, middle, after] = gates(module);
    let mut state = DVector::from_element(32, C::new(0.0, 0.0));
    state[0] = C::new(1.0, 0.0);
    let mut prepared = Dense { widths: vec![2, 1, 2], state };
    apply(&mut prepared, &before, false);
    let mut total = Dense { widths: prepared.widths.clone(), state: DVector::zeros(32) };
    for v in 0..4u64 {
        let mut part = prepared.clone();
        for basis in 0..32 {
            if part.config(basis)[0] != v {
                part.state[basis] = C::new(0.0, 0.0);
            }
        }
        if part.state.norm() <= EPS {
            continue;
        }
        apply(&mut part, &middle, false);
        let widths = part.widths.clone();
        part.permute(|basis| {
            let o_offset = widths[0] + widths[1];
            basis ^ ((f(v) as usize) << o_offset)
        });
        apply(&mut part, &middle, true);
        let faithful = (0..32).all(|basis| part.config(basis)[0] == v || part.state[basis].norm() <= EPS);
        if !faithful {
            return None;
        }
        total.state += &part.state;
    }
    apply(&mut total, &after, false);
    Some(total)
}

/// No gates, each gate, and each pair, without and with memory writes in the worlds.
fn cases() -> Vec<(Vec<&'static str>, bool)> {
    let mut middles: Vec<Vec<&str>> = vec![Vec::new()];
    middles.extend(MIDDLE.iter().map(|&g| vec![g]));
    middles.extend(MIDDLE.iter().flat_map(|&a| MIDDLE.iter().map(move |&b| vec![a, b])));
    [false, true].into_iter().flat_map(|memory| middles.iter().map(move |middle| (middle.clone(), memory))).collect()
}

fn run(module: &ValidModule, options: Options) -> (Outcome, Executor<'_>) {
    let mut executor =
        Executor::new(module, module.function_named("main").expect("@main"), &[], options).expect("starts");
    (executor.run(), executor)
}

#[test]
fn un_running_through_gates_across_worlds_is_the_conjugated_xor() {
    let cases = cases();
    let (mut matched, mut refused) = (0, 0);
    let options = Options { budget: Some(10_000), max_configurations: Some(1 << 10), ..Options::default() };
    for (middle, memory) in &cases {
        let module = bennett_ir::load(&program(middle, *memory)).expect("valid");
        let (outcome, executor) = run(&module, options);
        match expected(&module) {
            Some(dense) => {
                assert_eq!(outcome, Outcome::Returned(None), "{middle:?}");
                let snapshot = executor.snapshot();
                assert!(snapshot.worlds.is_empty(), "{middle:?}: the worlds merged");
                assert_eq!(&snapshot.classical.memory[8..12], &[7; 4], "{middle:?}: memory restored");
                assert_eq!(snapshot.classical.memory[32], 0, "{middle:?}: memory restored");
                let stats = executor.stats();
                assert_eq!(stats.overlay_bytes, 0, "{middle:?}: one world again, its overlay folded");
                if *memory {
                    // Every world wrote one byte in the callee (its value is never the base's 7) and the
                    // caller's byte where the value read is not 0.
                    assert!(stats.peak_overlay_bytes >= 4, "{middle:?}: {stats:?}");
                }
                for basis in 0..32 {
                    let found = snapshot.quantum.amplitude(&dense.config(basis));
                    assert!(
                        (found - dense.state[basis]).norm() <= 1e-10,
                        "{middle:?} basis {basis}: {found} vs {}",
                        dense.state[basis]
                    );
                }
                matched += 1;
                // Forward then back returns to the start.
                let mut back = Executor::new(
                    &module,
                    module.function_named("main").expect("@main"),
                    &[],
                    Options { reversible: true, ..options },
                )
                .expect("starts");
                let at_entry = back.snapshot();
                back.run();
                back.reverse().expect("reverses");
                assert_eq!(back.snapshot().classical, at_entry.classical, "{middle:?}");
                assert!(back.snapshot().quantum.distance(&at_entry.quantum).is_some_and(|d| d <= 1e-10), "{middle:?}");
            }
            None => {
                assert_eq!(
                    outcome,
                    Outcome::Stopped(RunError::WorldFault { fault: Fault::BadRewind, worlds: 4 }),
                    "{middle:?}"
                );
                refused += 1;
            }
        }
    }
    // 91 cases, without and with memory writes. 18 contain the gate that carries o into q; the dense calculation finds 16 of them
    // unfaithful, and the executor refuses exactly those. The other two: the gate twice is the identity,
    // and after `h %q[1]` the carried X conjugates into a phase Z on q[1], which leaves q's value alone.
    assert_eq!((matched, refused), (150, 32));
}

/// The same programs against the independent whole-world reference (`oracle_support::worlds`, written
/// from the documents without reading this executor), which steps every world in lockstep. Its read of
/// a register passed to an integer parameter and its `bad_rewind` for an un-run its history does not
/// describe are the documents' meaning; here every world takes the same number of steps, so its lockstep
/// and this executor's parking at rewinds coincide.
#[test]
fn the_whole_world_reference_agrees_where_worlds_stay_in_step() {
    use oracle_support::worlds::{Finish, Model, basis};
    use oracle_support::worlds_executor::assert_snapshot;
    let cases = cases();
    let (mut agreed, mut both_refused) = (0, 0);
    let options =
        Options { budget: Some(10_000), max_configurations: Some(1 << 10), reversible: true, ..Options::default() };
    for (middle, memory) in &cases {
        let module = bennett_ir::load(&program(middle, *memory)).expect("valid");
        let model = Model::new(&module, "main");
        let reference = model.run(basis(model.initial("main", &[])), 1_000);
        let (outcome, executor) = run(&module, options);
        let refused = reference.keys().any(|w| w.classical.finish == Some(Finish::Faulted(Fault::BadRewind)));
        if refused {
            assert_eq!(
                outcome,
                Outcome::Stopped(RunError::WorldFault { fault: Fault::BadRewind, worlds: 4 }),
                "{middle:?}"
            );
            both_refused += 1;
        } else {
            assert_eq!(outcome, Outcome::Returned(None), "{middle:?}");
            assert_snapshot(&model, &reference, &executor);
            agreed += 1;
        }
    }
    assert_eq!((agreed, both_refused), (150, 32));
}

/// cargo test -p bennett-exec --test worlds_oracle three_gate -- --ignored --nocapture
#[test]
#[ignore = "729 dense conjugation probes beyond the fast two-gate pass"]
fn three_gate_read_mismatches_match_the_independent_dense_operator() {
    let mut counts = [0, 0];
    for a in MIDDLE {
        for b in MIDDLE {
            for c in MIDDLE {
                let middle = [a, b, c];
                let module = bennett_ir::load(&program(&middle)).expect("valid");
                let options = Options { budget: Some(10000), reversible: true, ..Options::default() };
                let (outcome, executor) = run(&module, options);
                if let Some(dense) = expected(&module) {
                    assert_eq!(outcome, Outcome::Returned(None), "{middle:?}");
                    assert!(executor.snapshot().worlds.is_empty());
                    for basis in 0..32 {
                        assert!(
                            (executor.snapshot().quantum.amplitude(&dense.config(basis)) - dense.state[basis]).norm()
                                < 1e-10,
                            "{middle:?}, basis {basis}"
                        );
                    }
                    counts[0] += 1;
                } else {
                    assert_eq!(
                        outcome,
                        Outcome::Stopped(RunError::WorldFault { fault: Fault::BadRewind, worlds: 4 }),
                        "{middle:?}"
                    );
                    counts[1] += 1;
                }
            }
        }
    }
    println!("729 dense three-gate cases: {} faithful, {} refused", counts[0], counts[1]);
}
