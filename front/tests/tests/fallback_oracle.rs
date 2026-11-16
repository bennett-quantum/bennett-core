//! Oracle written before the C fallback: native C basis images, never device
//! arithmetic/regions as the expected answer. See c/fallback_oracle/README.md.
#[allow(dead_code, reason = "reuse the original independent dense oracle, not every entry point")]
#[path = "../../../crates/exec/tests/oracle_support/dense.rs"]
mod dense;
#[path = "fallback_support/native.rs"]
mod native;

use std::collections::BTreeMap;
use std::process::Command;
use std::sync::OnceLock;

use bennett_exec::{Executor, Outcome};
use bennett_front_tests::{CLANG_FLAGS, CLANG_O2_FLAGS, c_dir, entry, options, out_dir, root};
use bennett_ir::{Inst, ValidModule};
use native::{Native, Vector};
use num_complex::Complex64 as C;

const CASES: &[(&str, bool)] = &[
    ("coherent_gate", true),
    ("coherent_xor", true),
    ("coherent_nested", true),
    ("store", false),
    ("memcpy", false),
    ("memset", false),
    ("user_write", false),
    ("nested", false),
    ("loop", false),
    ("outside_class", false),
];
const MODES: &[(&str, &[&str], &[&str])] =
    &[("o0", CLANG_FLAGS, &[]), ("o2", CLANG_O2_FLAGS, &[]), ("taint", CLANG_FLAGS, &["--taint-all"])];

fn natives() -> &'static BTreeMap<&'static str, Native> {
    static NATIVE: OnceLock<BTreeMap<&'static str, Native>> = OnceLock::new();
    NATIVE.get_or_init(|| {
        CASES
            .iter()
            .map(|&(name, _)| {
                let native = Native::compile(name, "-O0");
                assert_eq!(native.images, Native::compile(name, "-O2").images, "{name}: native optimizer disagreement");
                (name, native)
            })
            .collect()
    })
}

struct Lowered {
    text: String,
    diagnostics: String,
}

/// Unlike the general front harness, retain diagnostics on successful lowering:
/// the warning is part of the fallback contract, not an expected compilation error.
fn lower(name: &str, tag: &str, flags: &[&str], extra: &[&str], defines: &[&str]) -> Result<Lowered, String> {
    let clang = std::env::var_os("CLANG").unwrap_or_else(|| "clang".into());
    let front = std::env::var_os("BENNETT_FRONT").map_or_else(|| root().join("target/front/bennett-front"), Into::into);
    let bc = out_dir().join(format!("fallback.{name}.{tag}.bc"));
    let bn = bc.with_extension("bn");
    let output = Command::new(clang)
        .args(flags)
        .args(defines)
        .arg("-I")
        .arg(root().join("front/include"))
        .args(["-emit-llvm", "-c"])
        .arg(c_dir().join(format!("fallback_oracle/{name}.c")))
        .arg("-o")
        .arg(&bc)
        .output()
        .expect("clang");
    assert!(output.status.success(), "clang {name}: {}", String::from_utf8_lossy(&output.stderr));
    let output = Command::new(front)
        .arg(&bc)
        .arg("-o")
        .arg(&bn)
        .args(["--stack-bytes=65536", "--region-budget=100000"])
        .args(extra)
        .output()
        .expect("bennett-front");
    let diagnostics = String::from_utf8(output.stderr).expect("front diagnostics");
    if !output.status.success() {
        return Err(diagnostics);
    }
    Ok(Lowered { text: std::fs::read_to_string(bn).expect("device text"), diagnostics })
}

fn assert_vector(actual: &Vector, expected: &Vector, context: &str) {
    for b in 0..8 {
        assert!((actual[b] - expected[b]).norm() < 1e-11, "{context}, basis {b}: {} != {}", actual[b], expected[b]);
    }
}

/// Project only explicitly named C variables. Every other live register must be
/// zero, and there must be one world: discarded garbage cannot hide bad coherence.
fn projected(module: &ValidModule, basis: usize) -> Vector {
    let mut exec = Executor::new(module, entry(module, "experiment"), &[basis as u64], options(0)).expect("starts");
    loop {
        let snapshot = exec.snapshot();
        let frame = snapshot.classical.frames.last().expect("frame before measurement");
        if matches!(module.function(frame.func).block(frame.block).insts.get(frame.inst), Some(Inst::Measure { .. })) {
            assert!(snapshot.worlds.is_empty(), "coherent class must have no un-run history worlds at measurement");
            assert!(snapshot.quantum.cells.is_empty(), "adapter needs a named-address mapping for memory qubits");
            let f = module.function(entry(module, "experiment"));
            // Qalloc handles are live slot values, not allocation-order guesses.
            let columns: Vec<_> = ["q", "r", "y"]
                .map(|name| {
                    let slot = f
                        .slot_names
                        .iter()
                        .position(|s| s == name)
                        .unwrap_or_else(|| panic!("no %{name}: {:?}", f.slot_names));
                    usize::try_from(snapshot.classical.slots[frame.base + slot] & 0xffff_ffff).expect("register column")
                })
                .into();
            let mut result = [C::new(0.0, 0.0); 8];
            for (config, amplitude) in snapshot.quantum.amplitudes {
                let mut b = 0;
                for (i, &col) in columns.iter().enumerate() {
                    assert!(config[col] <= 1);
                    b |= (config[col] as usize) << i;
                }
                assert!(
                    config.iter().enumerate().all(|(col, v)| columns.contains(&col) || *v == 0),
                    "unclean workspace: {config:?}"
                );
                result[b] += amplitude;
            }
            return result;
        }
        if let Some(outcome) = exec.step() {
            panic!("experiment ended before measurement: {outcome:?}");
        }
    }
}

/// Independently authored compact witnesses of the C circuits. The front uses
/// three byte registers plus cleanup fault registers (36 qubits), even though
/// only three bits vary. The original dense interpreter caps allocation at 13.
/// Keep its code unchanged and run the same logical circuit on three bits;
/// compare the compact program on the device too, never infer a circuit from
/// the front's emitted gates or drop live bits in its actual snapshot.
fn dense_projected(name: &str, basis: usize, round_trip: bool) -> Vector {
    let preparation = if basis < 8 {
        ["q", "r", "y"]
            .iter()
            .enumerate()
            .filter(|(bit, _)| basis & (1 << bit) != 0)
            .map(|(_, name)| format!("x %{name}\n"))
            .collect::<String>()
    } else {
        "ry(0.7) %q\nh %r\ns %r\nh %y\nt %y\n".into()
    };
    let update = match name {
        "coherent_gate" | "coherent_xor" => "x %r\nx %y ctrl %q, %r\nx %r\n",
        "coherent_nested" => "x %y ctrl %q, %r\n",
        other => panic!("no independently authored dense witness for {other}"),
    };
    let tail = if round_trip { format!("z %y\n{update}h %q\n") } else { String::new() };
    let text = format!(
        "memory 256\nfunc @experiment(i32 %basis) {{\nentry:\n%q = qalloc 1\n%r = qalloc 1\n%y = qalloc 1\n{preparation}{update}{tail}%m = measure %q\nret\n}}\n"
    );
    let module = bennett_ir::load(&text).expect("compact dense witness");
    let mut reference = dense::Oracle::new(&module);
    assert_eq!(reference.prefix("experiment", &[basis as u64]), None);
    let result: Vector = reference.dense.state.as_slice().try_into().expect("three logical qubits");
    assert_vector(&projected(&module, basis), &result, "compact device/dense witness");
    result
}

#[test]
fn native_oracle_has_hand_computed_images_amplitudes_and_phase_signs() {
    let n = natives();
    let h = std::f64::consts::FRAC_1_SQRT_2;
    let mut plus = [C::new(0.0, 0.0); 8];
    plus[0] = C::new(h, 0.0);
    plus[1] = C::new(h, 0.0);
    let mut bell = [C::new(0.0, 0.0); 8];
    bell[0] = C::new(h, 0.0);
    bell[5] = C::new(h, 0.0);
    assert_vector(&n["coherent_xor"].coherent(plus), &bell, "native CNOT");
    let mut phase = [C::new(0.0, 0.0); 8];
    phase[1] = C::new(1.0, 0.0);
    assert_vector(&n["coherent_xor"].phase_round_trip(plus), &phase, "native kickback sign");
    let kept = n["store"].kept();
    assert_eq!(kept, [0, 1, 0, 1]);
    let mut conjugated = [C::new(0.0, 0.0); 8];
    conjugated[0] = C::new(h, 0.0);
    conjugated[5] = C::new(0.0, -h);
    assert_vector(&native::unrun_keep(plus, kept), &conjugated, "S† X S has -i on zero input");
    let one = native::prepared(4);
    let mut inverse_sign = [C::new(0.0, 0.0); 8];
    inverse_sign[0] = C::new(0.0, 1.0);
    assert_vector(&native::unrun_keep(one, [1; 4]), &inverse_sign, "S† X S has +i on one input");
    assert_eq!(n["coherent_xor"].images[0].measured(), 1_576_968, "hand-packed output bytes");
    assert_eq!(n["memcpy"].images[3].memory, [7, 6, 5, 4]);
    assert_eq!(n["memset"].images[1].memory, [6; 4]);
    assert_eq!(n["user_write"].images[3].memory, [7, 5, 3, 0]);
    assert_eq!(n["nested"].images[1].memory, [3, 2, 4, 0]);
    assert_eq!(n["loop"].images[3].memory, [2, 4, 6, 0]);
    let probabilities = n["outside_class"].measured(&plus);
    assert_eq!(probabilities.len(), 2);
    assert!(probabilities.values().all(|p| (*p - 0.5).abs() < 1e-12));
    let lost = n["outside_class"].measured(&native::prepared(8));
    assert!((lost.values().sum::<f64>() - 1.0).abs() < 1e-12);
    assert!(lost.keys().all(|key| key & 4 == 0));
    println!(
        "10 native programs, 160 native basis runs (-O0/-O2); hand amplitudes, memory and both conjugation phase signs agree"
    );
}

fn check_coherent(module: &ValidModule, native: &Native, round_trip: bool, name: &str, context: &str) {
    for basis in 0..=8 {
        let expected = if round_trip {
            native.phase_round_trip(native::prepared(basis))
        } else {
            native.coherent(native::prepared(basis))
        };
        assert_vector(&projected(module, basis), &expected, context);
        assert_vector(&dense_projected(name, basis, round_trip), &expected, &format!("dense {context}"));
    }
}

#[test]
fn coherent_front_programs_match_native_and_existing_dense_oracle() {
    let name = "coherent_gate";
    for &(mode, flags, extra) in MODES {
        for round_trip in [false, true] {
            let tag = format!("{mode}.{round_trip}");
            let defines = if round_trip { &["-DROUND_TRIP"][..] } else { &[] };
            let lowered = lower(name, &tag, flags, extra, defines).unwrap_or_else(|e| panic!("{name}: {e}"));
            let module = bennett_ir::load(&lowered.text).expect("valid coherent program");
            check_coherent(&module, &natives()[name], round_trip, name, &format!("{name} {tag}"));
            if !round_trip && mode == "o0" {
                check_measured(&module, &natives()[name], name);
            }
        }
    }
    println!(
        "54 complex vectors agree: native C, original dense oracle and device (1 program, 3 modes, 2 circuits, 9 inputs); 520 measured outcomes also checked"
    );
}

fn check_measured(module: &ValidModule, native: &Native, context: &str) {
    for basis in 0..8 {
        let mut e = Executor::new(module, entry(module, "experiment"), &[basis], options(0)).expect("starts");
        assert_eq!(
            e.run(),
            Outcome::Returned(Some(native.images[basis as usize].measured().into())),
            "WRONG RESULT {context}, basis {basis}"
        );
    }
    let expected = native.measured(&native::prepared(8));
    let mut observed = BTreeMap::<u64, usize>::new();
    const SAMPLES: u64 = 512;
    for seed in 0..SAMPLES {
        let mut e = Executor::new(module, entry(module, "experiment"), &[8], options(seed)).expect("starts");
        let Outcome::Returned(Some(value)) = e.run() else {
            panic!("{context}: failed measurement run");
        };
        let value = u64::try_from(value).expect("measurement result");
        assert!(expected.contains_key(&value), "WRONG RESULT {context}: impossible measurement {value}");
        *observed.entry(value).or_default() += 1;
    }
    for (value, p) in expected {
        let actual = *observed.get(&value).unwrap_or(&0) as f64 / SAMPLES as f64;
        assert!((actual - p).abs() < 0.075, "{context}: measurement {value}: {actual} vs {p}");
    }
}

fn history_warning(diagnostics: &str) -> bool {
    let text = diagnostics.to_ascii_lowercase();
    text.contains("warning") && (text.contains("history") || text.contains("entangl") || text.contains("coherence"))
}

/// This regular test always probes every waiting case. An accepted program is
/// immediately checked for wrong results BEFORE checking its warning. A named
/// refusal is printed rather than being mistaken for an oracle pass.
#[test]
fn fallback_inventory_checks_every_accepted_program_and_names_each_refusal() {
    let mut waiting = 0;
    let mut accepted = 0;
    let mut missing_warnings = Vec::new();
    for &(name, coherent) in CASES.iter().filter(|(name, _)| *name != "coherent_gate") {
        for &(mode, flags, extra) in MODES {
            match lower(name, mode, flags, extra, &[]) {
                Err(reason) => {
                    assert!(reason.starts_with("not handled yet:"), "{name} {mode}: unexpected failure: {reason}");
                    eprintln!("WAITING {name}/{mode}: {}", reason.trim());
                    waiting += 1;
                }
                Ok(lowered) => {
                    let module = bennett_ir::load(&lowered.text).expect("valid fallback program");
                    if coherent {
                        check_coherent(&module, &natives()[name], false, name, name);
                    } else {
                        check_measured(&module, &natives()[name], name);
                    }
                    if !coherent && !history_warning(&lowered.diagnostics) {
                        missing_warnings.push(format!("{name}/{mode}: {}", lowered.diagnostics));
                    }
                    accepted += 1;
                }
            }
        }
    }
    println!("fallback inventory: {accepted} checked, {waiting} waiting by name; no accepted case skipped");
    assert!(missing_warnings.is_empty(), "missing documented history/coherence warnings: {missing_warnings:?}");
}

fn waiting_case(name: &str) {
    for &(mode, flags, extra) in MODES {
        let lowered = lower(name, &format!("required.{mode}"), flags, extra, &[])
            .unwrap_or_else(|reason| panic!("WAITING {name}/{mode}: {reason}"));
        let module = bennett_ir::load(&lowered.text).expect("valid fallback program");
        if CASES.iter().any(|&(case, coherent)| case == name && coherent) {
            check_coherent(&module, &natives()[name], false, name, name);
        } else {
            check_measured(&module, &natives()[name], name);
            assert!(history_warning(&lowered.diagnostics), "{name}/{mode}: missing history/coherence warning");
        }
    }
}

macro_rules! waiting {
    ($($name:ident),* $(,)?) => {$(
        #[test]
        #[ignore = "waiting for HANDOFF step 4: controlled stores/writing calls or destructive updates are refused; inventory prints the measured refusal"]
        fn $name() { waiting_case(stringify!($name)); }
    )*};
}
mod waiting_for_fallback {
    use super::waiting_case;
    waiting!(store, memcpy, memset, user_write, nested, outside_class);
    #[test]
    #[ignore = "documented Boolean XOR class still refuses y ^= q && !r; inventory prints the refusal"]
    fn coherent_xor() {
        waiting_case("coherent_xor");
    }
    #[test]
    #[ignore = "nested documented Boolean XOR still refuses; inventory prints the refusal"]
    fn coherent_nested() {
        waiting_case("coherent_nested");
    }
    #[test]
    #[ignore = "waiting for HANDOFF step 4: loop trip count depends on a qubit and writes memory"]
    fn variable_trip_loop() {
        waiting_case("loop");
    }
}

/// Test-only integration adapter, NOT a claim that C can express an un-run yet.
/// Lower only ordinary C, surround it with the documented device history idiom,
/// and compare to native output values with private memory restored.
#[test]
fn native_c_regions_under_marks_match_keep_and_restore_memory() {
    use bennett_ir::{FixedGate, Gate};
    for name in ["store", "memcpy", "memset", "user_write", "nested", "loop"] {
        let lowered = lower(name, "region", CLANG_FLAGS, &[], &["-DCLASSICAL_REGION"]).expect("ordinary C lowers");
        for basis in 0..=8 {
            let prep = if basis < 8 {
                (0..3)
                    .filter(|bit| basis & (1 << bit) != 0)
                    .map(|bit| if bit == 2 { "x %y\n".into() } else { format!("x %q[{bit}]\n") })
                    .collect::<String>()
            } else {
                "ry(0.7) %q[0]\nh %q[1]\ns %q[1]\nh %y\nt %y\n".into()
            };
            let text = format!(
                "{}\nfunc @oracle_adapter() {{\nentry:\n%q = qalloc 2\n%y = qalloc 1\n{prep}%m = mark\n%v = call @result(%q)\ns %y\nrewind %m keep (%v -> %y)\nret\n}}\n",
                lowered.text
            );
            let module = bennett_ir::load(&text).expect("marked adapter valid");
            let mut e = Executor::new(&module, entry(&module, "oracle_adapter"), &[], options(0)).expect("starts");
            let before = e.snapshot();
            assert_eq!(e.run(), Outcome::Returned(None), "{name} basis {basis}");
            let snapshot = e.snapshot();
            assert!(snapshot.worlds.is_empty(), "{name}: worlds must merge after un-run");
            assert_eq!(snapshot.classical.memory, before.classical.memory, "{name}: region memory restored");
            let expected = native::unrun_keep(native::prepared(basis), natives()[name].kept());
            // The existing dense interpreter can also execute these exact
            // lowered C bodies via apply (seven allocated qubits total).
            // This independently checks native C against its scalar evaluator,
            // as well as checking the conjugation with matrix primitives below.
            let apply_text = format!(
                "{}\nfunc @dense_c_adapter() {{\nentry:\n%q = qalloc 2\n%y = qalloc 1\n%e = qalloc 4\n{prep}s %y\napply @result(%q) -> %y, %e budget 100000\nsdg %y\n%m = measure %q\nret\n}}\n",
                lowered.text
            );
            let apply_module = bennett_ir::load(&apply_text).expect("dense C apply adapter");
            let mut interpreted = dense::Oracle::new(&apply_module);
            assert_eq!(interpreted.prefix("dense_c_adapter", &[]), None);
            let mut interpreted_vector = [C::new(0.0, 0.0); 8];
            for (b, &amplitude) in interpreted.dense.state.iter().enumerate() {
                if amplitude.norm() < 1e-14 {
                    continue;
                }
                let config = interpreted.dense.config(b);
                assert_eq!(config[2], 0, "{name}: dense C region fault");
                interpreted_vector[(config[0] | config[1] << 2) as usize] += amplitude;
            }
            assert_vector(&interpreted_vector, &expected, &format!("dense C interpreter {name}"));
            let applied = bennett_front_tests::state_before_first_measurement(&apply_module, "dense_c_adapter", &[]);
            assert!(applied.worlds.is_empty());
            assert!(applied.quantum.amplitudes.iter().all(|(config, _)| config[2] == 0));
            let applied_vector =
                std::array::from_fn(|b| applied.quantum.amplitude(&[(b & 3) as u64, (b >> 2) as u64, 0]));
            assert_vector(&applied_vector, &expected, &format!("device C apply {name}"));
            let actual = std::array::from_fn(|b| snapshot.quantum.amplitude(&[(b & 3) as u64, (b >> 2) as u64]));
            assert_vector(&actual, &expected, &format!("marked {name} basis {basis}"));
            // The old dense oracle's gate/permutation backend independently
            // computes the same conjugation, with a different vector layout.
            let mut reference = dense::Dense {
                widths: vec![2, 1],
                state: nalgebra::DVector::from_vec(native::prepared(basis).to_vec()),
            };
            reference.gate(dense::gate_matrix(Gate::Fixed(FixedGate::S)), (1, 0), &[]);
            let kept = natives()[name].kept();
            reference.permute(|b| b ^ (kept[b & 3] << 2));
            reference.gate(dense::gate_matrix(Gate::Fixed(FixedGate::Sdg)), (1, 0), &[]);
            assert_vector(
                &reference.state.as_slice().try_into().expect("8 amplitudes"),
                &expected,
                "dense conjugated keep",
            );
        }
    }
    println!(
        "54 ordinary-C marked regions agree with native keep + un-run and dense conjugation; memory restored exactly"
    );
}
