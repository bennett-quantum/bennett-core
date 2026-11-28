//! Seeded programs mixing every remaining handle form: Pauli `y`, run-time gate/rotation codes
//! (`gate(%k)`, `rotate(%k, %a)`), constants as interface operands, `qwidth`, slice measurement
//! (`measure i64 %r[%low, %bits]`), control lists from memory (with repeated entries), `apply`
//! step reports, block-form `apply` (superposed inputs, controls, private faults, placement
//! failures, strict mode), fault precedence, and step-by-step undo.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use bennett_ir::Fault;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

use super::{BUDGET, Finish, module, observe};

const DEFAULT_PROGRAMS: u64 = 120;
const HEAVY_PROGRAMS: u64 = 1_024;
const FIRST_SEED: u64 = 0x71d4_08b2_a93e_6201;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Scenario {
    MixedMeasured,
    MixedReversible,
    BlockApplySuperposedStrict,
    BlockApplyPlacementFailure,
    BadGatePrecedence,
    ControlListFaults,
    SliceMeasureFaults,
    BlockApplyOuterFaults,
    QwidthAndConstHandleFaults,
    InRegionNewForms,
}

const SCENARIOS: [Scenario; 10] = [
    Scenario::MixedMeasured,
    Scenario::MixedReversible,
    Scenario::BlockApplySuperposedStrict,
    Scenario::BlockApplyPlacementFailure,
    Scenario::BadGatePrecedence,
    Scenario::ControlListFaults,
    Scenario::SliceMeasureFaults,
    Scenario::BlockApplyOuterFaults,
    Scenario::QwidthAndConstHandleFaults,
    Scenario::InRegionNewForms,
];

struct Case {
    text: String,
    args: Vec<u64>,
    reversible: bool,
    strict: bool,
    scenario: Scenario,
}

fn generate(seed: u64) -> Case {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let idx = (seed - FIRST_SEED) as usize;
    let scenario = SCENARIOS[idx % SCENARIOS.len()];

    let x_width: u64 = [2, 3, 4, 70][rng.random_range(0..4)];
    let out_width: u64 = [4, 5, 8][rng.random_range(0..3)];
    let fault_width: u64 = [5, 6, 8][rng.random_range(0..3)];
    let theta: f64 = rng.random_range(-3.0..=3.0);
    let theta_bits = theta.to_bits();
    let img_seed: u8 = rng.random_range(1..=15);
    let apply_budget: u64 = [0, 3, 6, 12, 24][rng.random_range(0..5)];
    let x_low: u64 = if x_width > 2 { rng.random_range(0..=x_width - 2) } else { 0 };
    let out_low: u64 = if out_width > 4 { rng.random_range(0..=out_width - 4) } else { 0 };

    // Handles created in prelude:
    //   1: %dead (width 2, freed via constant handle `qfree 1`)
    //   2: %c    (width 2, control register)
    //   3: %x    (width x_width, input register)
    //   4: %out  (width out_width, output register)
    //   5: %flt  (width fault_width, fault register)
    //
    // Static memory layout (memory 1024):
    //   16: ordinary byte (may be promoted to qubit cell in fault tests)
    //   32..48: image (16 bytes; byte 0 has nonzero high bits 0xe0, byte 4 has img_seed)
    //   64..144: two valid fields:
    //     field 0: handle 3 (%x), low x_low, bits 2, offset 0, dir 1 (IN)
    //     field 1: handle 4 (%out), low out_low, bits 4, offset 8, dir 2 (OUT)
    //   160..208: valid control list (3 entries, with repeat): (2, 0), (2, 0), (2, 1)
    //   224..256: control list with dangling entry: (3, 0), (1, 0)
    //   256..288: control list with out-of-range pos: (2, 0), (2, 9)
    //   288..320: control list aliasing handle 3 bit 0: (2, 0), (3, 0)
    //   320..360: malformed field (bad dir 3)
    //   360..400: field with dangling handle 1
    //   400..440: field overlapping fault[0..5]: handle 5, low 3, bits 2, offset 0, dir 1
    //   440..520: two fields with overlapping byte offsets (offset 0 and offset 0)
    let mut text = format!(
        "memory 1024
data 16 = i8 0x55
data 32 = i8 0xe0, i8 0x21, i8 0x00, i8 0x00, i8 {img_seed}, zero 11
data 64 = i64 3, i64 {x_low}, i64 2, i64 0, i64 1, i64 4, i64 {out_low}, i64 4, i64 8, i64 2
data 160 = i64 2, i64 0, i64 2, i64 0, i64 2, i64 1
data 224 = i64 3, i64 0, i64 1, i64 0
data 256 = i64 2, i64 0, i64 2, i64 9
data 288 = i64 2, i64 0, i64 3, i64 0
data 320 = i64 3, i64 0, i64 1, i64 0, i64 3
data 360 = i64 1, i64 0, i64 1, i64 0, i64 1
data 400 = i64 5, i64 3, i64 2, i64 0, i64 1
data 440 = i64 3, i64 0, i64 1, i64 0, i64 1, i64 3, i64 1, i64 1, i64 0, i64 1

applied func @kernel(i64 %b) {{
entry:
 %x = load i8 %b
 %p_s = add i64 %b, 4
 %s = load i8 %p_s
 switch i8 %x, 0: c0, 1: c1, 2: c2, default: c3
c0:
 %v0 = add i8 %s, 5
 %o0 = add i64 %b, 8
 store i8 %v0, %o0
 ret
c1:
 %v1 = xor i8 %s, 0x0f
 %o1 = add i64 %b, 8
 store i8 %v1, %o1
 ret
c2:
 %boom = udiv i8 1, 0
 ret
c3:
 br spin(i64 6)
spin(i64 %k):
 %z = icmp eq i64 %k, 0
 br %z, done, more
more:
 %k1 = sub i64 %k, 1
 br spin(%k1)
done:
 %o3 = add i64 %b, 8
 store i8 9, %o3
 ret
}}

func @plain_target(i64 %b) {{
entry:
 ret
}}

applied func @in_region_probe(i64 %b) {{
entry:
 %tag = load i8 %b
 switch i8 %tag, 0: p_qw, 1: p_ms, 2: p_gb, 3: p_rb, 4: p_ab, 5: p_y, default: p_qf
p_qw:
 %w = qwidth 0
 ret
p_ms:
 %m = measure i64 0[0, 0]
 ret
p_gb:
 gate(0) 0[99] ctrl list(4096, 1)
 ret
p_rb:
 rotate(0, 0) 0[99] ctrl list(4096, 1)
 ret
p_ab:
 apply 0 budget 0, fault 0, image(4096, 0), fields(4096, 1), ctrl list(4096, 1)
 ret
p_y:
 y 0[99]
 ret
p_qf:
 qfree 0
 ret
}}

func @main(i64 %angle, i64 %bgt, i64 %extra) -> i64 {{
entry:
 %dead = qalloc 2
 qfree 1
 %c = qalloc 2
 %x = qalloc i64 {x_width}
 %out = qalloc i64 {out_width}
 %flt = qalloc i64 {fault_width}
 %wx = qwidth %x
 %w_const = qwidth 4
 h 2[0]
 y %c[1] ctrl 2[0]
 h %x[{x_low}]
 h 3[{}]
",
        x_low + 1
    );

    // Emit 2..4 randomized gates using y, gate(%k), rotate(%k, %a), written and list controls.
    for _ in 0..rng.random_range(2..=4) {
        let choice = rng.random_range(0..6);
        let target = if rng.random() { format!("4[{out_low}]") } else { format!("%out[{}]", out_low + 1) };
        match choice {
            0 => writeln!(text, " y {target}").expect("write y"),
            1 => writeln!(text, " y {target} ctrl 2[0]").expect("write cy"),
            2 => {
                let code = rng.random_range(1..=8);
                writeln!(text, " gate({code}) {target} ctrl list(160, 2)").expect("write gate list");
            }
            3 => {
                let code = rng.random_range(1..=8);
                writeln!(text, " gate({code}) {target} ctrl %c[0]").expect("write gate written");
            }
            4 => {
                let code = rng.random_range(9..=12);
                writeln!(text, " rotate({code}, %angle) {target} ctrl list(160, 3)").expect("write rot list");
            }
            _ => {
                let code = rng.random_range(9..=12);
                writeln!(text, " rotate({code}, {theta_bits}) {target} ctrl 2[1]").expect("write rot const");
            }
        }
    }

    let mut args = vec![theta_bits, apply_budget, 0];
    let mut reversible = false;
    let mut strict = false;

    match scenario {
        Scenario::MixedMeasured => {
            let ctrl_count = rng.random_range(0..=3);
            let ctrl_addr = if ctrl_count == 0 { 4096 } else { 160 };
            writeln!(
                text,
                " %fn = faddr @kernel
 apply %fn budget %bgt, fault 5, image(32, 16), fields(64, 2), ctrl list({ctrl_addr}, {ctrl_count})
 %mx = measure i64 3[{x_low}, 2]
 %mo = measure i64 %out[{out_low}, 4]
 %mf = measure i64 5[0, 5]
 %r1 = xor i64 %mx, %mo
 %r2 = xor i64 %r1, %mf
 %r3 = add i64 %r2, %wx
 %r4 = add i64 %r3, %w_const
 ret %r4"
            )
            .expect("write measured");
        }
        Scenario::MixedReversible => {
            reversible = true;
            let ctrl_count = rng.random_range(0..=3);
            let ctrl_addr = if ctrl_count == 0 { 4096 } else { 160 };
            writeln!(
                text,
                " %fn = faddr @kernel
 apply %fn budget %bgt, fault %flt, image(32, 16), fields(64, 2), ctrl list({ctrl_addr}, {ctrl_count})
 %sum = add i64 %wx, %w_const
 ret %sum"
            )
            .expect("write reversible");
        }
        Scenario::BlockApplySuperposedStrict => {
            strict = true;
            let ctrl_count = rng.random_range(0..=2);
            let ctrl_addr = if ctrl_count == 0 { 0 } else { 160 };
            writeln!(
                text,
                " %fn = faddr @kernel
 apply %fn budget %bgt, fault 5, image(32, 16), fields(64, 2), ctrl list({ctrl_addr}, {ctrl_count})
 %mf = measure i64 %flt[0, 5]
 %mo = measure i64 4[{out_low}, 4]
 %ans = xor i64 %mf, %mo
 ret %ans"
            )
            .expect("write strict");
        }
        Scenario::BlockApplyPlacementFailure => {
            reversible = rng.random();
            strict = !reversible;
            // Either fail placement via a huge block (hits static_end 520) or via a promoted qubit byte at base.
            if rng.random() {
                writeln!(
                    text,
                    " %fn = faddr @kernel
 apply %fn budget %bgt, fault 5, image(32, 480), fields(64, 2), ctrl list(160, 1)
 ret %wx"
                )
                .expect("write oom size");
            } else {
                // Promote a byte near top of free space so block placement collides with held qubits.
                // Free space is below ~920..880; place a 128-byte block after promoting byte 720.
                writeln!(
                    text,
                    " x [720:0]
 %fn = faddr @kernel
 apply %fn budget %bgt, fault 5, image(32, 256), fields(64, 2), ctrl list(0, 0)
 ret %wx"
                )
                .expect("write oom qubit");
            }
        }
        Scenario::BadGatePrecedence => {
            reversible = rng.random();
            let bad_code = [0u64, 9, 12, 13, u64::MAX][rng.random_range(0..5)];
            args[2] = bad_code;
            match rng.random_range(0..4) {
                0 => text.push_str(" gate(%extra) 1[99] ctrl list(4096, 1)\n"),
                1 => {
                    let bad_rot = [0u64, 1, 8, 13, u64::MAX][rng.random_range(0..5)];
                    args[2] = bad_rot;
                    text.push_str(" rotate(%extra, 9221120237041090560) 1[99] ctrl list(4096, 1)\n");
                }
                2 => text.push_str(" rotate(12, 9221120237041090560) 1[99] ctrl list(4096, 1)\n"),
                _ => text.push_str(" gate(2) 1[99] ctrl list(4096, 1)\n"),
            }
            text.push_str(" ret 0\n");
        }
        Scenario::ControlListFaults => {
            reversible = rng.random();
            match rng.random_range(0..6) {
                0 => {
                    args[2] = (u64::MAX / 16) + 1;
                    text.push_str(" gate(1) 3[0] ctrl list(160, %extra)\n");
                }
                1 => text.push_str(" gate(1) 3[0] ctrl list(1016, 1)\n"),
                2 => text.push_str(" x [16:0]\n gate(1) 3[0] ctrl list(16, 1)\n"),
                3 => text.push_str(" gate(1) 3[0] ctrl list(224, 2)\n"),
                4 => text.push_str(" gate(1) 3[0] ctrl list(256, 2)\n"),
                _ => text.push_str(" gate(1) 3[0] ctrl list(288, 2)\n"),
            }
            text.push_str(" ret 0\n");
        }
        Scenario::SliceMeasureFaults => {
            reversible = rng.random();
            match rng.random_range(0..6) {
                0 => text.push_str(" %m = measure i64 1[0, 0]\n"),
                1 => text.push_str(" %m = measure i64 0[0, 1]\n"),
                2 => text.push_str(" %m = measure i64 3[0, 0]\n"),
                3 => text.push_str(" %m = measure i64 3[0, 65]\n"),
                4 => {
                    args[2] = u64::MAX;
                    text.push_str(" %m = measure i64 3[%extra, 1]\n");
                }
                _ => writeln!(text, " %m = measure i64 3[{x_width}, 1]").expect("write bad slice"),
            }
            text.push_str(" ret 0\n");
        }
        Scenario::BlockApplyOuterFaults => {
            reversible = rng.random();
            text.push_str(" %app = faddr @kernel\n %plain = faddr @plain_target\n");
            match rng.random_range(0..8) {
                0 => text.push_str(" apply %plain budget 10, fault 5, image(32, 16), fields(64, 2), ctrl list(0, 0)\n"),
                1 => text.push_str(" apply %app budget 10, fault 1, image(32, 16), fields(64, 2), ctrl list(0, 0)\n"),
                2 => text.push_str(" apply %app budget 10, fault 2, image(32, 16), fields(64, 2), ctrl list(0, 0)\n"),
                3 => text.push_str(" apply %app budget 10, fault 5, image(32, 0), fields(64, 2), ctrl list(224, 2)\n"),
                4 => {
                    text.push_str(" apply %app budget 10, fault 5, image(32, 16), fields(320, 1), ctrl list(224, 2)\n")
                }
                5 => {
                    text.push_str(" apply %app budget 10, fault 5, image(32, 16), fields(320, 1), ctrl list(256, 2)\n")
                }
                6 => text.push_str(" apply %app budget 10, fault 5, image(32, 16), fields(360, 1), ctrl list(0, 0)\n"),
                _ => {
                    let faddr = [400, 440][rng.random_range(0..2)];
                    let cnt = if faddr == 440 { 2 } else { 1 };
                    writeln!(
                        text,
                        " apply %app budget 10, fault 5, image(32, 16), fields({faddr}, {cnt}), ctrl list(0, 0)"
                    )
                    .expect("write overlap");
                }
            }
            text.push_str(" ret 0\n");
        }
        Scenario::QwidthAndConstHandleFaults => {
            reversible = rng.random();
            match rng.random_range(0..4) {
                0 => text.push_str(" %w = qwidth 1\n"),
                1 => text.push_str(" %w = qwidth 0\n"),
                2 => text.push_str(" qfree 1\n"),
                _ => text.push_str(" y 1[0]\n"),
            }
            text.push_str(" ret 0\n");
        }
        Scenario::InRegionNewForms => {
            strict = true;
            let tag = rng.random_range(0..7u64);
            writeln!(
                text,
                " store i8 {tag}, 32
 %probe = faddr @in_region_probe
 apply %probe budget 5, fault 5, image(32, 16), fields(0, 0), ctrl list(0, 0)
 %code = measure i64 5[0, 5]
 ret %code"
            )
            .expect("write probe");
        }
    }

    text.push_str("}\n");
    Case { text, args, reversible, strict, scenario }
}

fn sweep(programs: u64) {
    let mut totals = observe::Counts::default();
    let mut finishes = BTreeSet::new();
    let mut reversible_runs = 0u64;
    let mut strict_runs = 0u64;
    let mut apply_reports = 0usize;

    for seed in FIRST_SEED..FIRST_SEED + programs {
        let case = generate(seed);
        let result = std::panic::catch_unwind(|| {
            let program = module(&case.text);
            let run = if case.reversible {
                observe::run_and_reverse(&program, "main", &case.args, seed, BUDGET)
            } else if case.strict {
                observe::run_strict(&program, "main", &case.args, seed, BUDGET)
            } else {
                observe::run(&program, "main", &case.args, seed, BUDGET)
            };
            let finish = run.reference.finish.expect("completed");
            match case.scenario {
                Scenario::MixedMeasured | Scenario::MixedReversible | Scenario::BlockApplySuperposedStrict => {
                    assert!(matches!(finish, Finish::Returned(Some(_))), "{:?}: {finish:?}", case.scenario);
                }
                Scenario::BlockApplyPlacementFailure => {
                    assert!(matches!(finish, Finish::Returned(Some(_))), "{:?}: {finish:?}", case.scenario);
                    let report = run.apply_reports.last().expect("placement failure report");
                    assert_eq!((report.largest, report.at_budget), (Some(0), 0), "placement failure has 0 steps");
                }
                Scenario::InRegionNewForms => {
                    assert_eq!(finish, Finish::Returned(Some(u128::from(Fault::QuantumInRegion.code()))));
                }
                Scenario::BadGatePrecedence => {
                    assert!(
                        matches!(finish, Finish::Faulted(Fault::BadGate | Fault::BadAngle | Fault::DanglingRegister)),
                        "{finish:?}"
                    );
                }
                Scenario::ControlListFaults => {
                    assert!(
                        matches!(
                            finish,
                            Finish::Faulted(
                                Fault::BadArgument | Fault::DanglingRegister | Fault::BadRange | Fault::QubitAlias
                            )
                        ),
                        "{finish:?}"
                    );
                }
                Scenario::SliceMeasureFaults => {
                    assert!(matches!(finish, Finish::Faulted(Fault::DanglingRegister | Fault::BadRange)), "{finish:?}");
                }
                Scenario::BlockApplyOuterFaults => {
                    assert!(
                        matches!(
                            finish,
                            Finish::Faulted(
                                Fault::BadIndirect | Fault::DanglingRegister | Fault::BadRange | Fault::BadArgument
                            )
                        ),
                        "{finish:?}"
                    );
                }
                Scenario::QwidthAndConstHandleFaults => {
                    assert_eq!(finish, Finish::Faulted(Fault::DanglingRegister));
                }
            }
            (finish, run.counts, run.apply_reports.len())
        })
        .unwrap_or_else(|failure| {
            eprintln!(
                "forms seed {seed:#018x} ({:?}, rev={}, strict={}) args={:?}\n{}",
                case.scenario, case.reversible, case.strict, case.args, case.text
            );
            std::panic::resume_unwind(failure)
        });
        let (finish, counts, reports) = result;
        finishes.insert(match finish {
            Finish::Returned(_) => "returned",
            Finish::Faulted(Fault::BadGate) => "bad_gate",
            Finish::Faulted(Fault::BadAngle) => "bad_angle",
            Finish::Faulted(Fault::DanglingRegister) => "dangling_register",
            Finish::Faulted(Fault::BadRange) => "bad_range",
            Finish::Faulted(Fault::BadArgument) => "bad_argument",
            Finish::Faulted(Fault::QubitAlias) => "qubit_alias",
            Finish::Faulted(Fault::BadIndirect) => "bad_indirect",
            other => panic!("unexpected generated forms finish: {other:?}"),
        });
        if case.reversible {
            reversible_runs += 1;
        }
        if case.strict {
            strict_runs += 1;
        }
        apply_reports += reports;
        totals.add(&counts);
    }

    assert_eq!(
        finishes,
        BTreeSet::from([
            "returned",
            "bad_gate",
            "bad_angle",
            "dangling_register",
            "bad_range",
            "bad_argument",
            "qubit_alias",
            "bad_indirect",
        ])
    );
    assert!(reversible_runs > 0, "generated reversible runs");
    assert!(strict_runs > 0, "generated strict block-apply runs");
    assert!(apply_reports > 0, "generated apply step reports");
    println!(
        "{} remaining-form programs ({} reversed, {} strict), {} steps, {} checkpoints, {} density entries, {} measurements, {} apply reports",
        totals.programs,
        reversible_runs,
        strict_runs,
        totals.operations,
        totals.states,
        totals.density_entries,
        totals.measurements,
        apply_reports,
    );
}

#[test]
fn seeded_remaining_forms() {
    sweep(DEFAULT_PROGRAMS);
}

#[test]
#[ignore = "1,024 programs; cargo test -p bennett-exec --test device_reference forms_generated::seeded_remaining_forms_heavy -- --ignored --exact --nocapture"]
fn seeded_remaining_forms_heavy() {
    sweep(HEAVY_PROGRAMS);
}
