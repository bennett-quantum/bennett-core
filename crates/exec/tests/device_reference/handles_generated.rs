//! Seeded programs mixing literal registers, `qalloc i64`, `i64` handles, run-time positions,
//! released handles, boundary positions (0, width-1, width, 2^32), fault precedence, and undo.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use bennett_ir::Fault;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

use super::{BUDGET, Finish, module, observe};

const DEFAULT_PROGRAMS: u64 = 96;
const HEAVY_PROGRAMS: u64 = 2_048;
const FIRST_SEED: u64 = 0x5c3a_91e0_44d2_8b10;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Scenario {
    MixedMeasured,
    MixedReversible,
    BadWidth,
    DanglingReleased,
    DanglingZeroOrFuture,
    BadRangeBoundary,
    DynamicAlias,
    BadAngleOrder,
    FaultOrderCollision,
    ApplyQuantumInRegion,
}

const SCENARIOS: [Scenario; 10] = [
    Scenario::MixedMeasured,
    Scenario::MixedReversible,
    Scenario::BadWidth,
    Scenario::DanglingReleased,
    Scenario::DanglingZeroOrFuture,
    Scenario::BadRangeBoundary,
    Scenario::DynamicAlias,
    Scenario::BadAngleOrder,
    Scenario::FaultOrderCollision,
    Scenario::ApplyQuantumInRegion,
];

fn random_gate(rng: &mut ChaCha8Rng) -> String {
    match rng.random_range(0..12) {
        0 => "x".into(),
        1 => "h".into(),
        2 => "z".into(),
        3 => "s".into(),
        4 => "sdg".into(),
        5 => "t".into(),
        6 => "tdg".into(),
        7 => format!("rx({:.6})", rng.random_range(-3.0..=3.0)),
        8 => format!("ry({:.6})", rng.random_range(-3.0..=3.0)),
        9 => format!("rz({:.6})", rng.random_range(-3.0..=3.0)),
        10 => format!("p({:.6})", rng.random_range(-3.0..=3.0)),
        _ => {
            let op = ["rx", "ry", "rz", "p"][rng.random_range(0..4)];
            format!("{op}(%angle)")
        }
    }
}

struct Case {
    text: String,
    args: Vec<u64>,
    reversible: bool,
    scenario: Scenario,
}

fn generate(seed: u64) -> Case {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let idx = (seed - FIRST_SEED) as usize;
    let scenario = SCENARIOS[idx % SCENARIOS.len()];

    let q_width: u64 = [2, 3, 4, 65][rng.random_range(0..4)];
    let h_width: u64 = [1, 2, 3, 4, 65, 70][rng.random_range(0..6)];
    let k_width: u64 = [1, 2, 3, 66][rng.random_range(0..4)];
    let theta: f64 = rng.random_range(-3.1..=3.1);
    let mem_byte: u8 = rng.random();

    // Common prelude:
    //   handle 1: %q = qalloc q_width
    //   handle 2: %dead_q = qalloc 2       (freed via qfree %dead_q)
    //   handle 3: %h = qalloc i64 %hw
    //   handle 4: %dead_h = qalloc i64 2   (freed via qfree %dead_h)
    //   handle 5: %k = qalloc i64 %kw      (allocated AFTER releases; never reuses 2 or 4)
    //   %q_id = 1, %dead_q_id = 2
    //   Positions: %z = 0, %q_top = q_width - 1, %h_top = h_width - 1, %k_top = k_width - 1
    let mut text = format!(
        "memory 512
data 32 = i8 {mem_byte}, i8 0x5a
func @main(i64 %hw, i64 %kw, i64 %extra, i64 %angle) -> i64 {{
entry:
 %q = qalloc {q_width}
 %dead_q = qalloc 2
 h %dead_q[0]
 %h = qalloc i64 %hw
 %dead_h = qalloc i64 2
 x %dead_h[1] ctrl %dead_q[0]
 qfree %dead_q
 qfree %dead_h
 %k = qalloc i64 %kw
 store i64 %k, 48
 %q_id = add i64 0, 1
 %dead_q_id = add i64 0, 2
 %z = add i64 0, 0
 %q_top = add i64 0, {}
 %h_top = sub i64 %hw, 1
 %k_top = sub i64 %kw, 1
 %k_loaded = load i64 48
",
        q_width - 1
    );

    // Distinct qubit expressions across qN literal, qN indexed, i64 literal, i64 indexed, and memory.
    let q_slot_0 = if rng.random() { "%q[0]" } else { "%q[%z]" };
    let q_slot_top = if rng.random() { "%q_id[%q_top]" } else { "%q[%q_top]" };
    let h_slot_0 = if rng.random() { "%h[0]" } else { "%h[%z]" };
    let h_slot_top = if h_width == 1 {
        if rng.random() { "%h[0]" } else { "%h[%h_top]" }
    } else if rng.random() {
        "%h[%h_top]"
    } else {
        // literal position on i64 handle (checked at run time)
        "%h[1]"
    };
    let k_slot = if rng.random() { "%k_loaded[%k_top]" } else { "%k[0]" };

    // Build a pool of guaranteed-distinct live qubits for a randomized gate layer.
    let mut pool: Vec<(&str, (u64, u64))> = vec![
        (q_slot_0, (1, 0)),
        (q_slot_top, (1, q_width - 1)),
        (h_slot_0, (3, 0)),
        (k_slot, (5, if k_slot.contains("[%k_top]") { k_width - 1 } else { 0 })),
        ("[32:0]", (0, 32 * 8)),
    ];
    if h_width > 1 {
        let bit = if h_slot_top == "%h[1]" { 1 } else { h_width - 1 };
        pool.push((h_slot_top, (3, bit)));
    }
    // Prepare superposition on two wires.
    writeln!(text, " h {}", pool[0].0).expect("write h");
    writeln!(text, " h {}", pool[2].0).expect("write h");
    for _ in 0..rng.random_range(3..=6) {
        let t = rng.random_range(0..pool.len());
        let gate = random_gate(&mut rng);
        let mut used = BTreeSet::from([pool[t].1]);
        let mut ctrls = Vec::new();
        for _ in 0..rng.random_range(0..=3) {
            let c = rng.random_range(0..pool.len());
            if used.insert(pool[c].1) {
                ctrls.push(pool[c].0);
            }
        }
        if ctrls.is_empty() {
            writeln!(text, " {gate} {}", pool[t].0).expect("write gate");
        } else {
            writeln!(text, " {gate} {} ctrl {}", pool[t].0, ctrls.join(", ")).expect("write ctrl gate");
        }
    }

    let mut args = vec![h_width, k_width, 0, theta.to_bits()];
    let mut reversible = false;

    match scenario {
        Scenario::MixedMeasured => {
            // Release %h via call taking i64 handle, measure memory and (if q_width <= 64) %q.
            text.push_str(
                " call @drop(%h)
 %m_mem = measure i8 [32]
 %m_wide = zext i8 %m_mem to i64
",
            );
            if q_width <= 64 {
                writeln!(
                    text,
                    " %mq = measure %q
 %mq64 = zext i{q_width} %mq to i64
 %ans = xor i64 %m_wide, %mq64
 %ans2 = add i64 %ans, %k_loaded
 ret %ans2"
                )
                .expect("write measure");
            } else {
                text.push_str(" %ans = add i64 %m_wide, %k_loaded\n ret %ans\n");
            }
        }
        Scenario::MixedReversible => {
            reversible = true;
            // Also pass handles through a helper call and release one register before returning.
            text.push_str(
                " call @touch(%q_id, %k_loaded, %z, %k_top)
 call @drop(%h)
 %sum = add i64 %h, %k_loaded
 ret %sum
",
            );
        }
        Scenario::BadWidth => {
            reversible = rng.random();
            let bad_w = [0_u64, 1 << 32, (1 << 32) + 7, u64::MAX][rng.random_range(0..4)];
            args[2] = bad_w;
            text.push_str(
                " %bad = qalloc i64 %extra
 ret %bad
",
            );
        }
        Scenario::DanglingReleased => {
            reversible = rng.random();
            let use_dead_q = rng.random();
            let dead = if use_dead_q { "%dead_q_id" } else { "%dead_h" };
            match rng.random_range(0..4) {
                0 => writeln!(text, " qfree {dead}").expect("write"),
                1 => writeln!(text, " x {dead}[0]").expect("write"),
                2 => writeln!(text, " x {dead}[%z] ctrl %h[0]").expect("write"),
                _ => writeln!(text, " p(%angle) %h[0] ctrl {dead}[%z]").expect("write"),
            }
            text.push_str(" ret 0\n");
        }
        Scenario::DanglingZeroOrFuture => {
            reversible = rng.random();
            let bad_h = [0_u64, 6, 99, u64::MAX][rng.random_range(0..4)];
            args[2] = bad_h;
            match rng.random_range(0..4) {
                0 => text.push_str(" qfree %extra\n"),
                1 => text.push_str(" x %extra[0]\n"),
                2 => text.push_str(" x %extra[0] ctrl %extra[0]\n"),
                _ => text.push_str(" x %h[0] ctrl %extra[%z]\n"),
            }
            text.push_str(" ret 0\n");
        }
        Scenario::BadRangeBoundary => {
            reversible = rng.random();
            let on_q = rng.random();
            let target_width = if on_q { q_width } else { h_width };
            let bad_pos = [target_width, target_width + 1, 1 << 32, (1 << 32) + 1, u64::MAX][rng.random_range(0..5)];
            args[2] = bad_pos;
            if on_q {
                if rng.random() {
                    text.push_str(" x %q[%extra]\n");
                } else {
                    text.push_str(" h %h[0] ctrl %q[%extra]\n");
                }
            } else {
                match rng.random_range(0..4) {
                    0 => writeln!(text, " x %h[{h_width}]").expect("write"),
                    1 => text.push_str(" x %h[4294967296]\n"),
                    2 => text.push_str(" x %h[%extra]\n"),
                    _ => text.push_str(" x [33:0] ctrl %h[%extra]\n"),
                }
            }
            text.push_str(" ret 0\n");
        }
        Scenario::DynamicAlias => {
            reversible = rng.random();
            match rng.random_range(0..4) {
                0 => text.push_str(" x %q[0] ctrl %q_id[0]\n"),
                1 => text.push_str(" x %q[%q_top] ctrl %q_id[%q_top]\n"),
                2 => text.push_str(" x %k[0] ctrl %k_loaded[%z]\n"),
                _ => text.push_str(" x %h[0] ctrl %q[%z], %q_id[0]\n"),
            }
            text.push_str(" ret 0\n");
        }
        Scenario::BadAngleOrder => {
            reversible = rng.random();
            let non_finite =
                [f64::INFINITY.to_bits(), f64::NEG_INFINITY.to_bits(), f64::NAN.to_bits(), 0x7ff0_0000_0000_0005]
                    [rng.random_range(0..4)];
            args[2] = non_finite;
            let op = ["rx", "ry", "rz", "p"][rng.random_range(0..4)];
            // Target and control are also invalid (dangling/bad_range/OOB), testing bad_angle wins.
            writeln!(text, " {op}(%extra) %dead_h[4294967296] ctrl [512:0]").expect("write");
            text.push_str(" ret 0\n");
        }
        Scenario::FaultOrderCollision => {
            reversible = rng.random();
            match rng.random_range(0..5) {
                0 => {
                    // Target bad_range beats dangling control.
                    writeln!(text, " x %h[{h_width}] ctrl %dead_h[0]").expect("write");
                }
                1 => {
                    // Target dangling handle beats target bad_range.
                    text.push_str(" x %dead_h[4294967296]\n");
                }
                2 => {
                    // Control dangling beats earlier alias.
                    text.push_str(" x %h[0] ctrl %h[0], %dead_q_id[0]\n");
                }
                3 => {
                    // Control bad_range beats earlier alias.
                    writeln!(text, " x %h[0] ctrl %h[0], %h[{h_width}]").expect("write");
                }
                _ => {
                    // Target OOB beats dangling control.
                    text.push_str(" x [512:0] ctrl %dead_h[0]\n");
                }
            }
            text.push_str(" ret 0\n");
        }
        Scenario::ApplyQuantumInRegion => {
            let choice = rng.random_range(0..3);
            args[2] = choice;
            text.push_str(
                " %out = qalloc 1
 %flag = qalloc 5
 apply @in_region(%extra) -> %out, %flag budget 10
 %code = measure %flag
 %code64 = zext i5 %code to i64
 ret %code64
",
            );
        }
    }

    text.push_str(
        "}
func @drop(i64 %id) {
entry:
 qfree %id
 ret
}
func @touch(i64 %a, i64 %b, i64 %i, i64 %j) {
entry:
 x %b[%j] ctrl %a[%i]
 ret
}
func @in_region(i64 %which) -> i1 {
entry:
 switch i64 %which, 0: alloc_bad, 1: gate_bad, default: free_bad
alloc_bad:
 %r = qalloc i64 0
 ret 0
gate_bad:
 x %which[4294967296]
 ret 0
free_bad:
 qfree %which
 ret 0
}",
    );

    Case { text, args, reversible, scenario }
}

fn sweep(programs: u64) {
    let mut totals = observe::Counts::default();
    let mut finishes = BTreeSet::new();
    let mut reversible_runs = 0u64;
    let mut region_quantum_faults = 0u64;

    for seed in FIRST_SEED..FIRST_SEED + programs {
        let case = generate(seed);
        let result = std::panic::catch_unwind(|| {
            let program = module(&case.text);
            let run = if case.reversible {
                observe::run_and_reverse(&program, "main", &case.args, seed, BUDGET)
            } else {
                observe::run(&program, "main", &case.args, seed, BUDGET)
            };
            let finish = run.reference.finish.expect("completed");
            match case.scenario {
                Scenario::MixedMeasured | Scenario::MixedReversible => {
                    assert!(matches!(finish, Finish::Returned(Some(_))), "{:?}: {finish:?}", case.scenario);
                }
                Scenario::BadWidth => assert_eq!(finish, Finish::Faulted(Fault::BadWidth)),
                Scenario::DanglingReleased | Scenario::DanglingZeroOrFuture => {
                    assert_eq!(finish, Finish::Faulted(Fault::DanglingRegister));
                }
                Scenario::BadRangeBoundary => assert_eq!(finish, Finish::Faulted(Fault::BadRange)),
                Scenario::DynamicAlias => assert_eq!(finish, Finish::Faulted(Fault::QubitAlias)),
                Scenario::BadAngleOrder => assert_eq!(finish, Finish::Faulted(Fault::BadAngle)),
                Scenario::FaultOrderCollision => {
                    assert!(
                        matches!(
                            finish,
                            Finish::Faulted(Fault::BadRange | Fault::DanglingRegister | Fault::OutOfBounds)
                        ),
                        "{finish:?}"
                    );
                }
                Scenario::ApplyQuantumInRegion => {
                    assert_eq!(finish, Finish::Returned(Some(u128::from(Fault::QuantumInRegion.code()))));
                }
            }
            (finish, run.counts)
        })
        .unwrap_or_else(|failure| {
            eprintln!(
                "handle seed {seed:#018x} ({:?}, reversible={}) args={:?}\n{}",
                case.scenario, case.reversible, case.args, case.text
            );
            std::panic::resume_unwind(failure)
        });
        let (finish, counts) = result;
        finishes.insert(match finish {
            Finish::Returned(_) => "returned",
            Finish::Faulted(Fault::BadWidth) => "bad_width",
            Finish::Faulted(Fault::DanglingRegister) => "dangling_register",
            Finish::Faulted(Fault::BadRange) => "bad_range",
            Finish::Faulted(Fault::QubitAlias) => "qubit_alias",
            Finish::Faulted(Fault::BadAngle) => "bad_angle",
            Finish::Faulted(Fault::OutOfBounds) => "out_of_bounds",
            other => panic!("unexpected generated handle finish: {other:?}"),
        });
        if case.reversible {
            reversible_runs += 1;
        }
        if case.scenario == Scenario::ApplyQuantumInRegion {
            region_quantum_faults += 1;
        }
        totals.add(&counts);
    }

    assert_eq!(
        finishes,
        BTreeSet::from([
            "returned",
            "bad_width",
            "dangling_register",
            "bad_range",
            "qubit_alias",
            "bad_angle",
            "out_of_bounds",
        ])
    );
    assert!(reversible_runs > 0, "generated reversible handle runs");
    assert!(region_quantum_faults > 0, "generated apply quantum_in_region runs");
    println!(
        "{} handle programs ({} reversed step-by-step), {} steps, {} checkpoints, {} density entries, {} measurements",
        totals.programs, reversible_runs, totals.operations, totals.states, totals.density_entries, totals.measurements
    );
}

#[test]
fn seeded_handle_programs() {
    sweep(DEFAULT_PROGRAMS);
}

#[test]
#[ignore = "2,048 programs; cargo test -p bennett-exec --test device_reference handles_generated::seeded_handle_programs_heavy -- --ignored --exact --nocapture"]
fn seeded_handle_programs_heavy() {
    sweep(HEAVY_PROGRAMS);
}
