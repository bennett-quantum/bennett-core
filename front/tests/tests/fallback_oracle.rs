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

use bennett_exec::{Executor, Outcome, Snapshot};
use bennett_front_tests::{CLANG_FLAGS, CLANG_O2_FLAGS, c_dir, entry, options, out_dir, root};
use bennett_ir::{Inst, ValidModule};
use native::{INITIAL_IMAGE, Native, Vector};
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
        .arg("--stack-bytes=65536")
        .arg(format!("--region-budget={REGION_BUDGET}"))
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

/// The step budget of every region the front writes for these fixtures, and of the test's own `apply`s.
const REGION_BUDGET: u64 = 100_000;

/// A device program, with the address of the fixtures' whole twelve-byte image
/// (guard bytes, cells, guard bytes) when it is a lowered fixture (found by the
/// front's `; @image` data comment).
struct Device {
    module: ValidModule,
    image: Option<usize>,
}

impl Device {
    fn lowered(text: &str) -> Self {
        let image = image_address(text).expect("a lowered fixture has `; @image` data");
        Self { module: bennett_ir::load(text).expect("valid device program"), image: Some(image) }
    }

    fn compact(text: &str) -> Self {
        Self { module: bennett_ir::load(text).expect("valid compact program"), image: None }
    }

    /// The whole image in the snapshot's memory: a write anywhere in it is seen.
    fn image(&self, state: &Snapshot) -> [u8; 12] {
        let a = self.image.expect("this check needs the fixture's image");
        state.classical.memory[a..a + INITIAL_IMAGE.len()].try_into().expect("the whole image")
    }

    /// The packed result of a finished run: the q, r, y bits `experiment`
    /// returned and the whole image from the machine's memory. The cells it
    /// returned must be those of the memory.
    fn finished(&self, returned: u64, state: &Snapshot, context: &str) -> u128 {
        let image = native::Image { bits: (returned & 7) as usize, memory: self.image(state) };
        assert_eq!(returned >> 3, u64::from(u32::from_le_bytes(image.cells())), "{context}: returned cells");
        image.measured()
    }
}

fn image_address(text: &str) -> Option<usize> {
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        if line.trim() == "; @image" {
            return lines.next()?.trim().strip_prefix("data ")?.split_whitespace().next()?.parse().ok();
        }
    }
    None
}

fn at_measure(module: &ValidModule, state: &Snapshot) -> bool {
    state.classical.frames.last().is_some_and(|frame| {
        matches!(module.function(frame.func).block(frame.block).insts.get(frame.inst), Some(Inst::Measure { .. }))
    })
}

/// Runs `experiment(basis)` until it stands at a measurement, none taken.
fn run_to_measure(device: &Device, basis: usize) -> Snapshot {
    let module = &device.module;
    let mut exec = Executor::new(module, entry(module, "experiment"), &[basis as u64], options(0)).expect("starts");
    loop {
        let snapshot = exec.snapshot();
        if at_measure(module, &snapshot) {
            return snapshot;
        }
        if let Some(outcome) = exec.step() {
            panic!("experiment ended before measurement: {outcome:?}");
        }
    }
}

/// The register columns of the C variables q, r, y in a snapshot standing in `experiment`.
fn columns(module: &ValidModule, state: &Snapshot) -> [usize; 3] {
    let frame = state.classical.frames.last().expect("frame before measurement");
    let f = module.function(frame.func);
    // Qalloc handles are live slot values, not allocation-order guesses.
    ["q", "r", "y"].map(|name| {
        let slot =
            f.slot_names.iter().position(|s| s == name).unwrap_or_else(|| panic!("no %{name}: {:?}", f.slot_names));
        usize::try_from(state.classical.slots[frame.base + slot] & 0xffff_ffff).expect("register column")
    })
}

fn named_bits(config: &[u64], columns: [usize; 3]) -> usize {
    columns.iter().enumerate().fold(0, |b, (i, &col)| {
        assert!(config[col] <= 1, "a C _Bool holds {}", config[col]);
        b | (config[col] as usize) << i
    })
}

/// Project only explicitly named C variables. Every other live register must be
/// zero: discarded garbage cannot hide bad coherence.
/// The coherent class writes no ordinary memory (the native images keep it), so
/// a lowered fixture's whole image must still hold its initial bytes: a leaked
/// write, also one just outside the cells, or a write whose un-run was missed,
/// is a wrong result here.
fn projected(device: &Device, basis: usize) -> Vector {
    let snapshot = run_to_measure(device, basis);
    assert!(snapshot.quantum.cells.is_empty(), "adapter needs a named-address mapping for memory qubits");
    if device.image.is_some() {
        assert_eq!(device.image(&snapshot), INITIAL_IMAGE, "WRONG RESULT basis {basis}: ordinary memory changed");
    }
    let columns = columns(&device.module, &snapshot);
    let mut result = [C::new(0.0, 0.0); 8];
    for (config, amplitude) in snapshot.quantum.amplitudes {
        let b = named_bits(&config, columns);
        assert!(
            config.iter().enumerate().all(|(col, v)| columns.contains(&col) || *v == 0),
            "unclean workspace: {config:?}"
        );
        result[b] += amplitude;
    }
    result
}

/// The exact probability of each measured result (q, r, y and the whole image,
/// packed as `native::Image::measured` packs them), from the device state just
/// before the measurements: every configuration's weight, keyed by the image.
/// Other registers (history the toolchain kept) are summed over.
fn born(device: &Device, basis: usize) -> BTreeMap<u128, f64> {
    let snapshot = run_to_measure(device, basis);
    let mut probabilities = BTreeMap::new();
    assert!(snapshot.quantum.cells.is_empty(), "adapter needs a named-address mapping for memory qubits");
    let columns = columns(&device.module, &snapshot);
    let memory = native::Image { bits: 0, memory: device.image(&snapshot) }.measured();
    for (config, amplitude) in &snapshot.quantum.amplitudes {
        *probabilities.entry(memory | named_bits(config, columns) as u128).or_default() += amplitude.norm_sqr();
    }
    probabilities.retain(|_, p: &mut f64| *p > 1e-14);
    probabilities
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
    let device = Device::compact(&text);
    let mut reference = dense::Oracle::new(&device.module);
    assert_eq!(reference.prefix("experiment", &[basis as u64]), None);
    let result: Vector = reference.dense.state.as_slice().try_into().expect("three logical qubits");
    assert_vector(&projected(&device, basis), &result, "compact device/dense witness");
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
    // Basis 0 of coherent_xor, packed by hand: q, r, y = 0, then the twelve
    // image bytes from bit 3 on (guards a5 5a c3 3c, cells 1 2 3 0, guards 96 69 f0 0f).
    let packed: u128 = 0xa5 << 3
        | 0x5a << 11
        | 0xc3 << 19
        | 0x3c << 27
        | 1 << 35
        | 2 << 43
        | 3 << 51
        | 0x96 << 67
        | 0x69 << 75
        | 0xf0 << 83
        | 0x0f << 91;
    assert_eq!(n["coherent_xor"].images[0].measured(), packed, "hand-packed output bytes");
    assert_eq!(n["memcpy"].images[3].cells(), [7, 6, 5, 4]);
    assert_eq!(n["memset"].images[1].cells(), [6; 4]);
    assert_eq!(n["user_write"].images[3].cells(), [7, 5, 3, 0]);
    assert_eq!(n["nested"].images[1].cells(), [3, 2, 4, 0]);
    assert_eq!(n["loop"].images[3].cells(), [2, 4, 6, 0]);
    // No fixture writes a guard byte: they hold their initial bytes in every image.
    for (name, native) in n {
        for image in &native.images {
            assert_eq!(image.memory[..4], INITIAL_IMAGE[..4], "{name}: low guard bytes");
            assert_eq!(image.memory[8..], INITIAL_IMAGE[8..], "{name}: high guard bytes");
        }
    }
    let probabilities = n["outside_class"].measured(&plus);
    assert_eq!(probabilities.len(), 2);
    assert!(probabilities.values().all(|p| (*p - 0.5).abs() < 1e-12));
    // Preparation 9 by hand: Ry(0.7) q, Ry(1.3) r, Ry(2.2) y on |000>, then
    // `if (q) cells[0] = 7`: eight results, eight different weights.
    let weights = n["store"].measured(&native::prepared(9));
    let (c, s) = (|t: f64| (t / 2.0).cos().powi(2), |t: f64| (t / 2.0).sin().powi(2));
    let initial = native::Image { bits: 0, memory: INITIAL_IMAGE }.measured();
    let mut seven = INITIAL_IMAGE;
    seven[native::CELLS.start] = 7;
    let written = native::Image { bits: 0, memory: seven }.measured();
    assert!((weights[&initial] - c(0.7) * c(1.3) * c(2.2)).abs() < 1e-15);
    assert!((weights[&(written | 0b101)] - s(0.7) * c(1.3) * s(2.2)).abs() < 1e-15);
    assert_eq!(weights.len(), 8);
    let mut sorted: Vec<f64> = weights.values().copied().collect();
    sorted.sort_by(f64::total_cmp);
    assert!(sorted.windows(2).all(|w| w[1] - w[0] > 1e-3), "preparation 9 weighs every basis state apart: {sorted:?}");
    let lost = n["outside_class"].measured(&native::prepared(8));
    assert!((lost.values().sum::<f64>() - 1.0).abs() < 1e-12);
    assert!(lost.keys().all(|key| key & 4 == 0));
    println!(
        "10 native programs, 160 native basis runs (-O0/-O2); hand amplitudes, memory and both conjugation phase signs agree"
    );
}

fn check_coherent(device: &Device, native: &Native, round_trip: bool, name: &str, context: &str) {
    for basis in 0..=8 {
        let expected = if round_trip {
            native.phase_round_trip(native::prepared(basis))
        } else {
            native.coherent(native::prepared(basis))
        };
        assert_vector(&projected(device, basis), &expected, context);
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
            let device = Device::lowered(&lowered.text);
            check_coherent(&device, &natives()[name], round_trip, name, &format!("{name} {tag}"));
            if !round_trip {
                check_measured(&device, &natives()[name], &format!("{name} {tag}"));
            }
        }
    }
    println!(
        "54 complex vectors agree: native C, original dense oracle and device (1 program, 3 modes, 2 circuits, 9 inputs); 3 x 520 measured outcomes and 3 x 10 exact Born tables also checked"
    );
}

/// Runs `experiment(basis)` with `seed` to its end: the packed result with the
/// whole image read from the finished machine.
fn run_finished(device: &Device, basis: u64, seed: u64, context: &str) -> u128 {
    let module = &device.module;
    let mut e = Executor::new(module, entry(module, "experiment"), &[basis], options(seed)).expect("starts");
    let Outcome::Returned(Some(value)) = e.run() else {
        panic!("{context}: failed measurement run, basis {basis}, seed {seed}");
    };
    device.finished(u64::try_from(value).expect("measurement result"), &e.snapshot(), context)
}

fn check_measured(device: &Device, native: &Native, context: &str) {
    for basis in 0..8 {
        assert_eq!(
            run_finished(device, basis, 0, context),
            native.images[basis as usize].measured(),
            "WRONG RESULT {context}, basis {basis}"
        );
    }
    // Exact weights first: sampling alone cannot see a wrong weight on a branch
    // lighter than its tolerance (every q = 1 branch of preparation 8 is), and
    // preparation 8 gives y equal weights, so a y swapped on one branch is
    // invisible to any measurement of it; preparation 9 weighs all eight apart.
    for basis in 0..=9 {
        let expected = native.measured(&native::prepared(basis));
        let actual = born(device, basis);
        for key in expected.keys().chain(actual.keys()) {
            let (e, a) = (expected.get(key).copied().unwrap_or(0.0), actual.get(key).copied().unwrap_or(0.0));
            assert!(
                (e - a).abs() < 1e-9,
                "WRONG RESULT {context}: preparation {basis}, result {key}: weight {a}, not {e}"
            );
        }
    }
    let expected = native.measured(&native::prepared(8));
    let mut observed = BTreeMap::<u128, usize>::new();
    const SAMPLES: u64 = 512;
    for seed in 0..SAMPLES {
        let value = run_finished(device, 8, seed, context);
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

/// Checks a program the front accepted: values first, then the warning. A
/// coherent fixture gets its full complex vector and the measured checks; any
/// other gets the measured checks and must carry the history warning.
fn check_accepted(name: &str, coherent: bool, lowered: &Lowered) -> Result<(), String> {
    let device = Device::lowered(&lowered.text);
    if coherent {
        check_coherent(&device, &natives()[name], false, name, name);
    }
    check_measured(&device, &natives()[name], name);
    if !coherent && !history_warning(&lowered.diagnostics) {
        return Err(format!("{name}: missing history/coherence warning: {}", lowered.diagnostics));
    }
    Ok(())
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
                    if let Err(missing) = check_accepted(name, coherent, &lowered) {
                        missing_warnings.push(format!("{mode}: {missing}"));
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
    let coherent = CASES.iter().any(|&(case, coherent)| case == name && coherent);
    for &(mode, flags, extra) in MODES {
        let lowered = lower(name, &format!("required.{mode}"), flags, extra, &[])
            .unwrap_or_else(|reason| panic!("WAITING {name}/{mode}: {reason}"));
        check_accepted(name, coherent, &lowered).unwrap_or_else(|missing| panic!("{mode}: {missing}"));
    }
}

macro_rules! waiting {
    ($($name:ident),* $(,)?) => {$(
        #[test]
        #[ignore = "waiting for the route that replaces the fallback (HANDOFF, Next steps 4): controlled stores/writing calls or destructive updates are refused; inventory prints the measured refusal"]
        fn $name() { waiting_case(stringify!($name)); }
    )*};
}
mod waiting_for_fallback {
    use super::waiting_case;
    waiting!(store, memcpy, memset, user_write, nested);
    /// `*y = 0` on a y of unknown value: the old value is kept in a register released at once, with the
    /// coherence warning (docs/DESIGN.md, "Overwrites that keep the old value").
    #[test]
    fn outside_class() {
        waiting_case("outside_class");
    }
    /// The documented Boolean XOR `y ^= q && !r`, coherent in every mode since the
    /// front proves a one-bit flip case by case when the simplifier cannot fold it.
    #[test]
    fn coherent_xor() {
        waiting_case("coherent_xor");
    }
    /// The nested `if (q) if (r) y ^= 1`, also at O0 and with `--taint-all`.
    #[test]
    fn coherent_nested() {
        waiting_case("coherent_nested");
    }
    #[test]
    #[ignore = "waiting for the route that replaces the fallback (HANDOFF, Next steps 4): loop trip count depends on a qubit and writes memory"]
    fn variable_trip_loop() {
        waiting_case("loop");
    }
}

/// Ordinary C regions (the fixtures' CLASSICAL_REGION builds) run under `apply`, in the device and in the
/// independent dense interpreter, against native results; not a claim that the front emits them.
#[test]
fn native_c_regions_under_apply_match_native_and_the_dense_oracle() {
    for name in REGIONS {
        let lowered = lower(name, "region", CLANG_FLAGS, &[], &["-DCLASSICAL_REGION"]).expect("ordinary C lowers");
        check_region(name, &lowered.text);
    }
}

const REGIONS: [&str; 6] = ["store", "memcpy", "memset", "user_write", "nested", "loop"];

/// `region` is the lowered CLASSICAL_REGION build of fixture `name`.
fn check_region(name: &str, region: &str) {
    use bennett_ir::{FixedGate, Gate};
    let native = &natives()[name];
    let image = native.region_image();
    for basis in 0..=8 {
        let prep = if basis < 8 {
            (0..3)
                .filter(|bit| basis & (1 << bit) != 0)
                .map(|bit| if bit == 2 { "x %y\n".into() } else { format!("x %q[{bit}]\n") })
                .collect::<String>()
        } else {
            "ry(0.7) %q[0]\nh %q[1]\ns %q[1]\nh %y\nt %y\n".into()
        };
        let expected = native::unrun_keep(native::prepared(basis), native.kept());
        // The existing dense interpreter can also execute these exact
        // lowered C bodies via apply (seven allocated qubits total).
        // This independently checks native C against its scalar evaluator,
        // as well as checking the conjugation with matrix primitives below.
        let apply_text = format!(
            "{region}\nfunc @dense_c_adapter() {{\nentry:\n%q = qalloc 2\n%y = qalloc 1\n%e = qalloc 5\n{prep}s %y\napply @bit(%q) -> %y, %e budget {REGION_BUDGET}\nsdg %y\n%m = measure %q\nret\n}}\n"
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
        assert!(applied.quantum.amplitudes.iter().all(|(config, _)| config[2] == 0));
        let applied_vector = std::array::from_fn(|b| applied.quantum.amplitude(&[(b & 3) as u64, (b >> 2) as u64, 0]));
        assert_vector(&applied_vector, &expected, &format!("device C apply {name}"));
        // The old dense oracle's gate/permutation backend independently
        // computes the same conjugation, with a different vector layout.
        let mut reference =
            dense::Dense { widths: vec![2, 1], state: nalgebra::DVector::from_vec(native::prepared(basis).to_vec()) };
        reference.gate(dense::gate_matrix(Gate::Fixed(FixedGate::S)), (1, 0), &[]);
        let kept = native.kept();
        reference.permute(|b| b ^ (kept[b & 3] << 2));
        reference.gate(dense::gate_matrix(Gate::Fixed(FixedGate::Sdg)), (1, 0), &[]);
        assert_vector(
            &reference.state.as_slice().try_into().expect("8 amplitudes"),
            &expected,
            "dense conjugated keep",
        );
    }
    // The whole image in the dense interpreter, four bits at a time (its
    // 13-qubit cap): q in uniform superposition, each nibble kept into o.
    for which in 0..2 * INITIAL_IMAGE.len() {
        let text = format!(
            "{region}\nfunc @dense_nibble() {{\nentry:\n%q = qalloc 2\n%o = qalloc 4\n%e = qalloc 5\nh %q[0]\nh %q[1]\napply @nibble(%q, i5 {which}) -> %o, %e budget {REGION_BUDGET}\n%m = measure %q\nret\n}}\n"
        );
        let module = bennett_ir::load(&text).expect("dense nibble adapter");
        let mut interpreted = dense::Oracle::new(&module);
        assert_eq!(interpreted.prefix("dense_nibble", &[]), None);
        let mut weight = 0.0;
        for (b, &amplitude) in interpreted.dense.state.iter().enumerate() {
            if amplitude.norm() < 1e-14 {
                continue;
            }
            let config = interpreted.dense.config(b);
            let nibble = ((image[config[0] as usize] >> (4 * which)) & 15) as u64;
            assert_eq!(
                (config[1], config[2]),
                (nibble, 0),
                "WRONG RESULT dense nibble {which} of {name}, q {}",
                config[0]
            );
            weight += amplitude.norm_sqr();
        }
        assert!((weight - 1.0).abs() < 1e-12, "dense nibble {which} of {name}: weight {weight}");
    }
}

/// The ASan/UBSan build of every fixture gives the same basis images, with no
/// diagnostic: the native reference does not rest on undefined behaviour.
#[test]
fn native_reference_is_free_of_undefined_behaviour() {
    for &(name, _) in CASES {
        let checked = Native::compile_with(
            name,
            "sanitized",
            &["-O1", "-g", "-fsanitize=address,undefined", "-fno-sanitize-recover=all", "-fno-omit-frame-pointer"],
        );
        assert_eq!(checked.images, natives()[name].images, "{name}: sanitized native build disagrees");
    }
    println!("10 native programs x 8 basis runs under ASan/UBSan: no diagnostic, same images");
}

/// Seeded wrong device results. Each must be rejected by the harness; the
/// correct hand-written fallback-shaped program must be accepted, so the
/// harness is not stricter than DESIGN.md either.
mod seeded {
    use super::*;

    fn rejected(f: impl FnOnce()) -> String {
        let error = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).expect_err("the harness accepted it");
        error
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| error.downcast_ref::<&str>().map(|s| (*s).to_owned()))
            .unwrap_or_default()
    }

    fn gate_text(mode: &str) -> String {
        let (flags, extra) = match mode {
            "o2" => (CLANG_O2_FLAGS, &[][..]),
            "taint" => (CLANG_FLAGS, &["--taint-all"][..]),
            _ => (CLANG_FLAGS, &[][..]),
        };
        lower("coherent_gate", &format!("seeded.{mode}"), flags, extra, &[]).expect("coherent gate lowers").text
    }

    fn before_measuring(text: &str, seeded: &str) -> Device {
        let marker = "%q.measured = measure %q";
        assert_eq!(text.matches(marker).count(), 1, "one measurement of q");
        Device::lowered(&text.replacen(marker, &format!("{seeded}\n    {marker}"), 1))
    }

    /// Found by this pass: the coherent projection ignored ordinary memory and
    /// the measured check ran at O0 only, so in O2, `--taint-all` and every
    /// accepted inventory case a leftover write (a leaked byte, or a store whose
    /// un-run was missed) passed.
    #[test]
    fn a_coherent_program_that_leaves_an_ordinary_write_is_rejected() {
        let native = &natives()["coherent_gate"];
        for mode in ["o0", "o2", "taint"] {
            let text = gate_text(mode);
            let cells = image_address(&text).expect("image") + native::CELLS.start;
            // In a cell, and (the guard bytes) just before and just past them.
            for address in [cells + 1, cells - 1, cells + 4] {
                let leaky = before_measuring(&text, &format!("store i8 9, {address}"));
                let message = rejected(|| check_coherent(&leaky, native, false, "coherent_gate", mode));
                assert!(message.contains("ordinary memory changed"), "{mode} {address}: {message}");
                let message = rejected(|| check_measured(&leaky, native, mode));
                assert!(message.contains("WRONG RESULT"), "{mode} {address}: {message}");
            }
        }
    }

    #[test]
    fn a_wrong_phase_or_swapped_state_in_a_coherent_program_is_rejected() {
        let native = &natives()["coherent_gate"];
        let text = gate_text("o2");
        for seeded in ["s %y[0]", "z %q[0]", "x %r[0] ctrl %q[0]"] {
            let wrong = before_measuring(&text, seeded);
            let message = rejected(|| check_coherent(&wrong, native, false, "coherent_gate", seeded));
            assert!(message.contains(seeded), "{seeded}: {message}");
        }
        // A global phase is not observable, but no global-phase quotient is
        // taken: a uniform -1 is still a different state vector from native's.
        let message = rejected(|| {
            check_coherent(
                &before_measuring(&text, "x %q[0]\n    z %q[0]\n    x %q[0]\n    z %q[0]"),
                native,
                false,
                "coherent_gate",
                "global",
            )
        });
        assert!(message.contains("global"), "{message}");
    }

    /// A wrong write in an applied region, or a stray write beside the cells: the region's whole image,
    /// guard bytes included, is compared nibble by nibble in the dense interpreter, so each is seen.
    #[test]
    fn a_wrong_write_in_an_applied_region_is_rejected() {
        let lowered = |name: &str| {
            lower(name, "seeded.region", CLANG_FLAGS, &[], &["-DCLASSICAL_REGION"]).expect("ordinary C lowers").text
        };
        let nested = lowered("nested");
        let wrong = nested.replacen("xor i8 %v.1, 7", "xor i8 %v.1, 6", 1);
        assert_ne!(wrong, nested, "nested's r-dependent write is where the seed expects it");
        let message = rejected(|| check_region("nested", &wrong));
        assert!(message.contains("WRONG RESULT"), "{message}");
        let store = lowered("store");
        let cells = image_address(&store).expect("image") + native::CELLS.start;
        let entry = "func @result(i2 %basis) -> i128 {\nentry:\n";
        assert_eq!(store.matches(entry).count(), 1, "the region's entry is where the seed expects it");
        for address in [cells - 1, cells + 4] {
            let stray = store.replacen(entry, &format!("{entry}    store i8 9, {address}\n"), 1);
            let message = rejected(|| check_region("store", &stray));
            assert!(message.contains("WRONG RESULT"), "{address}: {message}");
        }
    }
}
