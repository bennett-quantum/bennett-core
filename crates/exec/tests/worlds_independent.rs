//! Independent testing of DESIGN.md “Worlds”; production code is never changed by these probes.
#[allow(dead_code, reason = "shared independent oracle helpers")]
mod oracle_support;

use bennett_exec::{Executor, Options, Outcome, ReverseError, RunError};
use bennett_ir::{Fault, ValidModule};
use oracle_support::worlds::{Finish, Model, Superposition, basis};
use oracle_support::worlds_executor::{assert_worlds, observed_one_of};
use std::collections::BTreeMap;

fn options() -> Options {
    Options { reversible: true, budget: Some(10000), max_configurations: Some(8192), ..Options::default() }
}
fn executor<'a>(p: &'a ValidModule, entry: &str, seed: u64) -> Executor<'a> {
    Executor::new(p, p.function_named(entry).expect("entry"), &[], Options { seed, ..options() }).expect("start")
}
fn raw<'a>(text: &'a str, name: &str) -> &'a str {
    text.split_once(&format!("const {name}: &str = r#\"")).expect("fixture").1.split_once("\"#;").expect("end").0
}

fn assert_sample(model: &Model<'_>, state: &Superposition, actual: &Executor<'_>) {
    let mut groups = BTreeMap::<_, Superposition>::new();
    for (w, &a) in state {
        groups.entry((w.classical.clone(), w.history.clone())).or_default().insert(w.clone(), a);
    }
    if groups.len() == 1 {
        assert_worlds(model, state, &actual.snapshot());
        return;
    }
    assert!(actual.snapshot().worlds.is_empty(), "the host observes one world");
    let snapshot = actual.snapshot();
    let selected = groups
        .into_values()
        .find(|g| {
            let w = g.keys().next().expect("world");
            model.memory(w) == snapshot.classical.memory
                && w.classical.tape.iter().copied().eq(snapshot.classical.tape.iter().copied())
                && match (&w.classical.finish, actual.outcome()) {
                    (Some(Finish::Returned(x)), Some(Outcome::Returned(y))) => *x == y,
                    (Some(Finish::Faulted(x)), Some(Outcome::Faulted(y))) => *x == y,
                    _ => false,
                }
                && g.keys().any(|w| snapshot.quantum.amplitudes.iter().any(|(c, _)| w.quantum.starts_with(c)))
        })
        .expect("sample lies in independent positive-weight world support");
    let norm = selected.values().map(|a| a.norm_sqr()).sum::<f64>().sqrt();
    let selected = selected.into_iter().map(|(w, a)| (w, a / norm)).collect();
    assert_worlds(model, &selected, &snapshot);
}

fn coherent(text: &str, entry: &str) {
    let p = bennett_ir::load(text).expect("valid generated fixture");
    let m = Model::new(&p, entry);
    let reference = m.run_joined(basis(m.initial(entry, &[])), 2000);
    let mut e = executor(&p, entry, 0);
    let start = e.snapshot();
    let out = e.run();
    assert!(matches!(out, Outcome::Returned(_)), "{entry}: {out:?}\n{text}");
    assert_worlds(&m, &reference, &e.snapshot());
    e.reverse().expect("coherent history reverses");
    assert_eq!(e.snapshot().classical, start.classical, "all classical state and ticks restored");
    assert!(e.snapshot().worlds.is_empty());
    assert!(e.snapshot().quantum.distance(&start.quantum).is_some_and(|d| d < 1e-11));
}

#[test]
fn existing_worlds_programs_match_the_joined_reference() {
    let text = include_str!("worlds.rs");
    let p = format!("{}{}", raw(text, "WORLDS"), raw(text, "MORE"));
    for entry in ["copy_state", "nested_state", "alloc_inside_state", "jumps_state", "conjugated_match"] {
        coherent(&p, entry);
    }
}

fn generated(k: usize) -> String {
    let indirect = if k & 1 == 0 {
        "%v = call @work(%q, %b, %acc)"
    } else {
        "%fp = faddr @work\n%v = call_indirect fn(i2, q1, q1) -> i2 %fp(%q, %b, %acc)"
    };
    let inner = if k & 2 == 0 { "" } else { "call @copy(%b, %acc)" };
    let temp = if k & 4 == 0 { "" } else { "%tmp = qalloc 1\nh %tmp\n%ignored = call @id(%tmp)" };
    let repeat =
        if k & 8 == 0 { "" } else { "%n = mark budget -1\n%w = call @work(%q, %b, %acc)\nrewind %n keep (%w -> %out)" };
    let prep = [
        "h %q[0]\nh %q[1]",
        "ry(0.31) %q[0]\nrx(1.1) %q[1]",
        "h %q[0]\ns %q[0]\nh %q[1]",
        "rx(0.83) %q[0]\nry(2.3) %q[1]",
    ][(k / 16) % 4];
    let middle = ["s %out[0]", "h %out[0]", "ry(0.43) %out[1]", "x %acc ctrl %q[1]"][(k / 64) % 4];
    format!(
        "memory 1024
func @id(i1 %x)->i1 {{
entry:\nret %x\n}}
func @copy(q1 %b, q1 %a) {{
entry:\n%m = mark budget -1\n%r = call @id(%b)\nrewind %m keep (%r -> %a)\nret\n}}
func @work(i2 %x, q1 %b, q1 %acc)->i2 {{
entry:
%c = icmp ult i2 %x, 2
br %c, short, long
short:
store i2 %x, 16
ret %x
long:
%y = mul i2 %x, 3
store i2 %y, 17
%z = load i2 17
{inner}
ret %z
}}
func @main() {{
entry:
%q = qalloc 2
%b = qalloc 1
%acc = qalloc 1
%out = qalloc 2
{prep}
ry(0.7) %b
%outer = mark budget -1
{temp}
{indirect}
{middle}
rewind %outer keep (%v -> %out)
{repeat}
h %q[0]
h %q[1]
ret
}}"
    )
}

#[test]
fn generated_unequal_paths_nested_reads_and_private_memory_match_whole_worlds() {
    for k in (0..256).step_by(17) {
        coherent(&generated(k), "main");
    }
    println!("16 generated full-world comparisons and full reversals");
}

const SAMPLING: &str = r#"
memory 512
func @id(i2 %v)->i2 {
entry:
ret %v
}
func @main()->i2 {
entry:
%q = qalloc 2
ry(1.0471975511965976) %q[0]
ry(1.5707963267948966) %q[1]
%v = call @id(%q)
store i2 %v, 0
ret %v
}
func @measure()->i2 {
entry:
%q = qalloc 2
ry(1.0471975511965976) %q[0]
ry(1.5707963267948966) %q[1]
%v = call @id(%q)
%m = measure %q
ret %m
}
"#;

#[test]
fn sampling_uses_unequal_born_weights_at_measurement_and_at_host_return() {
    let p = bennett_ir::load(SAMPLING).expect("sampling");
    // N=4096, expected (.375,.125,.375,.125); tolerance .035 per bin
    // exceeds 4.6 sigma even for the highest variance bin. Fixed seeds make this reproducible.
    for entry in ["main", "measure"] {
        let mut bins = [0usize; 4];
        for seed in 0..4096 {
            let mut e = executor(&p, entry, seed);
            let Outcome::Returned(Some(n)) = e.run() else { panic!("sample returns") };
            bins[n as usize] += 1;
            assert_eq!(e.reverse(), Err(ReverseError::PastMeasurement));
            let weight = e.snapshot().quantum.amplitudes.iter().map(|(_, a)| a.norm_sqr()).sum::<f64>();
            assert!((weight - 1.0).abs() < 1e-12);
        }
        for (i, p) in [0.375, 0.125, 0.375, 0.125].into_iter().enumerate() {
            assert!((bins[i] as f64 / 4096.0 - p).abs() < 0.035, "{entry}: {bins:?}");
        }
        println!("{entry}: 4096 seeds, bins {bins:?}, absolute probability tolerance 0.035");
    }
    let model = Model::new(&p, "main");
    let reference = model.run_joined(basis(model.initial("main", &[])), 100);
    for seed in 0..16 {
        let mut e = executor(&p, "main", seed);
        e.run();
        assert_sample(&model, &reference, &e);
    }
}

const POLICIES: &str = r#"
memory 512
func @id1(i1 %x)->i1 {
entry:
ret %x
}
func @fault(i1 %x)->i1 {
entry:
br %x, one, zero
one:
ret 1
zero:
fault divide_by_zero
}
func @faulted() {
entry:
%q = qalloc 1
h %q
%v = call @fault(%q)
ret
}
func @uniform() {
entry:
%q = qalloc 1
h %q
%m = mark budget -1
%v = call @id1(%q)
rewind %m
ret
}
func @unaligned() {
entry:
%q = qalloc 1
h %q
%m = mark budget -1
%v = call @id1(%q)
br %v, one, zero
zero:
rewind %m
ret
one:
rewind %m
br zero
}
"#;

#[test]
fn documented_stops_fault_scope_and_tick_budget_boundaries() {
    let p = bennett_ir::load(POLICIES).expect("policy fixtures");
    let mut unaligned = executor(&p, "unaligned", 0);
    let start = unaligned.snapshot();
    assert_eq!(unaligned.run(), Outcome::Stopped(RunError::Unaligned));
    unaligned.reverse().expect("stop can be reversed");
    assert_eq!(unaligned.snapshot().classical, start.classical);
    assert!(unaligned.snapshot().quantum.distance(&start.quantum).is_some_and(|d| d < 1e-12));
    let mut strict = executor(&p, "faulted", 0);
    assert_eq!(strict.run(), Outcome::Stopped(RunError::WorldFault { fault: Fault::DivideByZero, worlds: 2 }));
    let mut support = std::collections::BTreeSet::new();
    for seed in 0..64 {
        let mut e = Executor::new(
            &p,
            p.function_named("faulted").expect("entry"),
            &[],
            Options { seed, allow_region_faults: true, ..options() },
        )
        .expect("start");
        support.insert(format!("{:?}", e.run()));
    }
    assert_eq!(
        support,
        std::collections::BTreeSet::from([
            format!("{:?}", Outcome::Faulted(Fault::DivideByZero)),
            format!("{:?}", Outcome::Returned(None))
        ])
    );
    let mut full = executor(&p, "uniform", 0);
    assert_eq!(full.run(), Outcome::Returned(None));
    let ticks = full.stats().steps;
    for budget in 0..=ticks + 1 {
        let mut e = Executor::new(
            &p,
            p.function_named("uniform").expect("entry"),
            &[],
            Options { budget: Some(budget), ..options() },
        )
        .expect("start");
        let start = e.snapshot();
        let result = e.run();
        assert_eq!(
            result,
            if budget < ticks { Outcome::Faulted(Fault::Budget) } else { Outcome::Returned(None) },
            "budget {budget}"
        );
        assert_eq!(e.stats().steps, budget.min(ticks));
        e.reverse().expect("budget termination reverses");
        assert_eq!(e.snapshot().classical, start.classical);
        assert!(e.snapshot().quantum.distance(&start.quantum).is_some_and(|d| d < 1e-12));
    }
}

/// F1 of the testing pass: a later split hid a changed earlier read, and the rewind returned (1, 1).
#[test]
fn shared_history_read_must_still_match_after_world_contributions_are_summed() {
    // DESIGN.md Worlds, “A read the state no longer matches”: bad_rewind changes nothing.
    let p = bennett_ir::load(
        r#"
memory 512
func @id(i1 %x)->i1 {
entry:
ret %x
}
func @main() {
entry:
%q = qalloc 1
%out = qalloc 1
%m = mark budget -1
call @id(%q)
x %q ctrl %out
h %q
call @id(%q)
rewind %m keep (i1 1 -> %out)
ret
}
"#,
    )
    .expect("witness");
    let model = Model::new(&p, "main");
    let expected = model.run_joined(basis(model.initial("main", &[])), 100);
    assert!(
        expected.keys().all(|w| w.classical.finish == Some(Finish::Faulted(Fault::BadRewind))),
        "independent full-state replay detects changed read"
    );
    let mut e = executor(&p, "main", 0);
    let got = e.run();
    eprintln!("shared-history read result {got:?}, state {:?}", e.snapshot().quantum);
    assert!(
        matches!(
            got,
            Outcome::Faulted(Fault::BadRewind) | Outcome::Stopped(RunError::WorldFault { fault: Fault::BadRewind, .. })
        ),
        "changed earlier read must be detected: {got:?}"
    );
}

/// cargo test -p bennett-exec --test worlds_independent generated_extended -- --ignored --nocapture
#[test]
#[ignore = "256 generated complete coherent programs including nested unequal paths"]
fn generated_extended() {
    for k in 0..256 {
        coherent(&generated(k), "main");
    }
    println!("256 generated complete-world comparisons and full reversals");
}

#[test]
fn apply_and_the_history_idiom_agree_on_generated_four_by_four_argument_states() {
    let body = r#"
memory 1024
data 16 = i8 7
func @f(i2 %a, i2 %b)->i2 {
entry:
%v = load i8 16
%w = zext i2 %a to i8
%sum = add i8 %v, %w
store i8 %sum, 16
%c = icmp ult i2 %a, %b
br %c, less, greater
less:
%x = add i2 %a, %b
ret %x
greater:
%y = mul i2 %a, %b
%z = xor i2 %y, 1
ret %z
}
"#;
    let mut cases = 0;
    for a in 0..4 {
        for b in 0..4 {
            for phase in 0..4 {
                let prep = if phase == 0 {
                    String::new()
                } else {
                    format!("ry({}) %a[0]\nrx({}) %b[1]\ns %b[0]", 0.31 * phase as f64, 0.7 * phase as f64)
                };
                let entry = format!(
                    "func @main() {{\nentry:\n%a = qalloc 2\n%b = qalloc 2\n%out = qalloc 2\n%fault = qalloc 5\n{}{}{}{}\n{prep}\nOP\nret\n}}\n",
                    if a & 1 != 0 { "x %a[0]\n" } else { "" },
                    if a & 2 != 0 { "x %a[1]\n" } else { "" },
                    if b & 1 != 0 { "x %b[0]\n" } else { "" },
                    if b & 2 != 0 { "x %b[1]\n" } else { "" }
                );
                let apply = format!("{body}{}", entry.replace("OP", "apply @f(%a, %b) -> %out, %fault budget 100"));
                let idiom = format!(
                    "{body}{}",
                    entry.replace("OP", "%m = mark budget -1\n%v = call @f(%a, %b)\nrewind %m keep (%v -> %out)")
                );
                let pa = bennett_ir::load(&apply).expect("apply");
                let pi = bennett_ir::load(&idiom).expect("idiom");
                let (mut ea, mut ei) = (executor(&pa, "main", 0), executor(&pi, "main", 0));
                assert_eq!(ea.run(), Outcome::Returned(None));
                assert_eq!(ei.run(), Outcome::Returned(None));
                assert_eq!(ea.snapshot().classical.memory, ei.snapshot().classical.memory);
                assert!(ea.snapshot().quantum.distance(&ei.snapshot().quantum).is_some_and(|d| d < 1e-11));
                let model = Model::new(&pi, "main");
                let reference = model.run_joined(basis(model.initial("main", &[])), 200);
                assert_worlds(&model, &reference, &ei.snapshot());
                cases += 1;
            }
        }
    }
    assert_eq!(cases, 64);
    println!("64 apply/idiom/full-world comparisons (16 argument basis states x 4 preparations)");
}

#[test]
fn all_finite_world_fixtures_match_independent_outcome_instruments() {
    use oracle_support::worlds_executor::{assert_observed, observed_reference};
    let text = include_str!("worlds.rs");
    let p = bennett_ir::load(&format!("{}{}", raw(text, "WORLDS"), raw(text, "MORE"))).expect("world fixtures");
    let mut runs = 0;
    for entry in [
        "split",
        "interfere",
        "copy_state",
        "copy_twice",
        "conjugated_mismatch",
        "conjugated_match",
        "alloc_inside_state",
        "nested_state",
        "measure_apart",
        "fault_in_one",
        "jumps_state",
    ] {
        let model = Model::new(&p, entry);
        let reference = observed_reference(&model, basis(model.initial(entry, &[])), 2000);
        assert!((reference.values().map(|a| a.norm_sqr()).sum::<f64>() - 1.0).abs() < 1e-11, "{entry}");
        for seed in 0..16 {
            let mut e = Executor::new(
                &p,
                p.function_named(entry).expect("entry"),
                &[],
                Options { seed, allow_region_faults: true, ..options() },
            )
            .expect("start");
            e.run();
            assert_observed(&model, &reference, &e);
            runs += 1;
        }
    }
    println!("{runs} finite worlds-fixture runs match full independent outcome/measurement instruments");
}

/// cargo test -p bennett-exec --test worlds_independent existing_executor_programs_extended -- --ignored --nocapture
#[test]
#[ignore = "all executor binaries with independent reference companions; finite prefixes for long workloads"]
fn existing_executor_programs_extended() {
    let status = std::process::Command::new("cargo")
        .args(["test", "-p", "bennett-exec", "--tests", "--", "--nocapture"])
        .env("CARGO_BUILD_JOBS", "3")
        .env("RUST_TEST_THREADS", "3")
        .env("BENNETT_WORLDS_REFERENCE", "1")
        .status()
        .expect("extended executor/reference pass");
    assert!(status.success());
}

/// A read whose worlds take paths of different lengths: the world that read 1 rewinds later.
const SLOW: &str = "func @slow(i1 %x)->i1 {\nentry:\nbr %x, long, short\nlong:\n%a = add i1 %x, 1\n%b = add i1 %a, 1\nret %x\nshort:\nret %x\n}\n";

/// The class of F1: reads of `%q` (one world per value where they split; `S` reads through a callee whose
/// length depends on the value), gates between them (one carries a
/// keep into `%o` over to `%q`), a temporary allocated and entangled inside the region, and nested rewinds,
/// in every order up to `events` long, under four preparations and four keeps. Every case is compared with
/// the whole-world reference; cases without nesting also with an independent dense calculation of the
/// documents' meaning (`dense_meaning`). Returns (faithful, refused, nested) counts.
fn shared_history_class(events: usize, stride: usize) -> (usize, usize, usize) {
    const EVENTS: [&str; 10] = [
        "R",
        "S",
        "h %q",
        "x %q ctrl %o",
        "x %o ctrl %q",
        "ry(0.4) %q",
        "h %o",
        "x %q ctrl %r",
        "%t = qalloc 1\nx %t ctrl %o",
        "N",
    ];
    const PREPARATIONS: [&str; 4] = ["h %q", "ry(1.1) %q\nrx(0.3) %o", "", "h %q\nh %r"];
    let mut sequences: Vec<Vec<usize>> = vec![Vec::new()];
    for length in 1..=events {
        let mut next = Vec::new();
        for sequence in sequences.iter().filter(|s| s.len() == length - 1) {
            for event in 0..EVENTS.len() {
                let mut longer = sequence.clone();
                longer.push(event);
                next.push(longer);
            }
        }
        sequences.extend(next);
    }
    // One temporary per region: the text names it `%t`.
    sequences.retain(|s| s.iter().filter(|&&e| e == 8).count() <= 1);
    let (mut faithful, mut refused, mut nested) = (0, 0, 0);
    for sequence in sequences.iter().step_by(stride) {
        let reads = sequence.iter().filter(|&&e| e <= 1).count();
        let mut keeps = vec![String::new(), " keep (i1 1 -> %o)".to_string()];
        if reads > 0 {
            keeps.push(" keep (%v1 -> %o)".to_string());
            keeps.push(format!(" keep (%v{reads} -> %r)"));
        }
        for prep in PREPARATIONS {
            for keep in &keeps {
                let (mut read, mut inner) = (0, 0);
                let body: Vec<String> = sequence
                    .iter()
                    .map(|&e| match EVENTS[e] {
                        "R" | "S" => {
                            read += 1;
                            let callee = if EVENTS[e] == "R" { "id" } else { "slow" };
                            format!("%v{read} = call @{callee}(%q)")
                        }
                        "N" => {
                            inner += 1;
                            format!("%n{inner} = mark budget -1\n%w{inner} = call @id(%q)\nh %q\nrewind %n{inner} keep (%w{inner} -> %r)")
                        }
                        gate => gate.to_string(),
                    })
                    .collect();
                let text = format!(
                    "memory 512\nfunc @id(i1 %x)->i1 {{\nentry:\nret %x\n}}\n{SLOW}func @main() {{\nentry:\n%q = qalloc 1\n%o = qalloc 1\n%r = qalloc 1\n{prep}\n%m = mark budget -1\n{}\nrewind %m{keep}\nret\n}}\n",
                    body.join("\n")
                );
                let p = bennett_ir::load(&text).expect("valid generated class program");
                let model = Model::new(&p, "main");
                let reference = model.run_joined(basis(model.initial("main", &[])), 4000);
                let reference_refuses =
                    reference.keys().any(|w| w.classical.finish == Some(Finish::Faulted(Fault::BadRewind)));
                let mut e = executor(&p, "main", 0);
                let start = e.snapshot();
                let out = e.run();
                let dense = (!sequence.contains(&9)).then(|| dense_meaning(sequence, prep, keep));
                if let Some(dense) = &dense {
                    assert_eq!(dense.is_none(), reference_refuses, "dense and reference disagree:\n{text}");
                } else {
                    nested += 1;
                }
                if reference_refuses {
                    assert!(
                        matches!(
                            out,
                            Outcome::Faulted(Fault::BadRewind)
                                | Outcome::Stopped(RunError::WorldFault { fault: Fault::BadRewind, .. })
                        ),
                        "must refuse: {out:?}\n{text}"
                    );
                    if out == Outcome::Faulted(Fault::BadRewind) {
                        // Changed nothing: the world observed is one the reference keeps before the rewind.
                        assert!(observed_one_of(&model, &reference, &e.snapshot()), "{:?}\n{text}", e.snapshot());
                    }
                    refused += 1;
                } else {
                    assert_eq!(out, Outcome::Returned(None), "must run:\n{text}");
                    assert_worlds(&model, &reference, &e.snapshot());
                    if let Some(Some(dense)) = &dense {
                        let snapshot = e.snapshot();
                        for (basis, &want) in dense.iter().enumerate() {
                            let basis = basis as u64;
                            let got = snapshot.quantum.amplitude(&[basis & 1, basis >> 1 & 1, basis >> 2 & 1]);
                            assert!((got - want).norm() < 1e-10, "basis {basis}: {got} {want}\n{text}");
                        }
                    }
                    e.reverse().expect("a faithful rewind reverses");
                    assert_eq!(e.snapshot().classical, start.classical, "{text}");
                    assert!(e.snapshot().quantum.distance(&start.quantum).is_some_and(|d| d < 1e-11), "{text}");
                    faithful += 1;
                }
            }
        }
    }
    (faithful, refused, nested)
}

/// The documents' meaning of `prep; mark; events; rewind keep`, written densely over q, o, r and the
/// temporary t (bits 0..3), without nesting: going forward, each read splits every part by q's value;
/// the keep XORs into each part; un-running goes back event by event, and at a read the parts that differ
/// only in later reads are summed and must hold the value read, at the temporary's allocation the sum must
/// have t = 0. `None` where a check fails (the rewind refuses); else the state over q, o, r after it.
fn dense_meaning(sequence: &[usize], prep: &str, keep: &str) -> Option<Vec<num_complex::Complex64>> {
    use num_complex::Complex64 as C;
    use oracle_support::dense::gate_matrix;
    type State = Vec<C>;
    let matrix = |name: &str| {
        use bennett_ir::{FixedGate, Gate, Rotation};
        gate_matrix(match name {
            "h" => Gate::Fixed(FixedGate::H),
            "x" => Gate::Fixed(FixedGate::X),
            "ry(0.4)" => Gate::Rotate(Rotation::Ry, 0.4),
            "ry(1.1)" => Gate::Rotate(Rotation::Ry, 1.1),
            "rx(0.3)" => Gate::Rotate(Rotation::Rx, 0.3),
            other => panic!("gate {other}"),
        })
    };
    let bit = |register: &str| match register {
        "%q" => 0,
        "%o" => 1,
        "%r" => 2,
        "%t" => 3,
        other => panic!("register {other}"),
    };
    // A gate line: name, target, optional control.
    let gate = |state: &mut State, line: &str, adjoint: bool| {
        let words: Vec<&str> = line.split_whitespace().collect();
        let m = if adjoint { matrix(words[0]).adjoint() } else { matrix(words[0]) };
        let target = 1usize << bit(words[1]);
        let control = if words.len() == 4 { 1usize << bit(words[3]) } else { 0 };
        for low in 0..16 {
            if low & target == 0 && low & control == control {
                let (a, b) = (state[low], state[low | target]);
                state[low] = m[(0, 0)] * a + m[(0, 1)] * b;
                state[low | target] = m[(1, 0)] * a + m[(1, 1)] * b;
            }
        }
    };
    let weight = |state: &State, wrong: &dyn Fn(usize) -> bool| -> f64 {
        (0..16).filter(|&b| wrong(b)).map(|b| state[b].norm_sqr()).sum()
    };
    let mut start = vec![C::new(0.0, 0.0); 16];
    start[0] = C::new(1.0, 0.0);
    for line in prep.lines() {
        gate(&mut start, line, false);
    }
    const EVENTS: [&str; 9] =
        ["R", "R", "h %q", "x %q ctrl %o", "x %o ctrl %q", "ry(0.4) %q", "h %o", "x %q ctrl %r", "ALLOC"];
    let mut parts: BTreeMap<Vec<u64>, State> = BTreeMap::from([(Vec::new(), start)]);
    for &e in sequence {
        match EVENTS[e] {
            "R" => {
                let mut next = BTreeMap::new();
                for (label, state) in parts {
                    for value in 0..2u64 {
                        let part: State = (0..16)
                            .map(|b| if (b & 1) as u64 == value { state[b] } else { C::new(0.0, 0.0) })
                            .collect();
                        if part.iter().map(|a| a.norm_sqr()).sum::<f64>() > 1e-24 {
                            let mut longer = label.clone();
                            longer.push(value);
                            next.insert(longer, part);
                        }
                    }
                }
                parts = next;
            }
            "ALLOC" => parts.values_mut().for_each(|s| gate(s, "x %t ctrl %o", false)),
            line => parts.values_mut().for_each(|s| gate(s, line, false)),
        }
    }
    for (label, state) in parts.iter_mut() {
        let (value, register) = match keep {
            "" => (0, 0),
            " keep (i1 1 -> %o)" => (1, 1),
            " keep (%v1 -> %o)" => (label[0], 1),
            _ => (*label.last().expect("a read"), 2),
        };
        if value == 1 {
            let flip = 1usize << register;
            let old = state.clone();
            for b in 0..16 {
                state[b ^ flip] = old[b];
            }
        }
    }
    for &e in sequence.iter().rev() {
        match EVENTS[e] {
            "R" => {
                let mut merged = BTreeMap::<Vec<u64>, State>::new();
                for (mut label, state) in parts {
                    let value = label.pop().expect("a read's value") as usize;
                    if weight(&state, &|b| b & 1 != value) > 1e-20 {
                        return None;
                    }
                    let sum = merged.entry(label).or_insert_with(|| vec![C::new(0.0, 0.0); 16]);
                    for b in 0..16 {
                        sum[b] += state[b];
                    }
                }
                parts = merged;
            }
            "ALLOC" => {
                for state in parts.values_mut() {
                    gate(state, "x %t ctrl %o", true);
                    if weight(state, &|b| b & 8 != 0) > 1e-20 {
                        return None;
                    }
                }
            }
            line => parts.values_mut().for_each(|s| gate(s, line, true)),
        }
    }
    let state = parts.remove(&Vec::new()).expect("every part is back at the mark");
    Some(state[..8].to_vec())
}

#[test]
fn reads_keeps_gates_temporaries_and_nesting_in_every_order_match_dense_and_reference() {
    let (faithful, refused, nested) = shared_history_class(2, 1);
    println!("{faithful} faithful, {refused} refused ({nested} nested, reference only)");
    assert!(faithful > 0 && refused > 0 && nested > 0);
}

/// cargo test --release -p bennett-exec --test worlds_independent shared_history_class_extended -- --ignored --nocapture
/// (`CLASS_EVENTS=4` for every order of four events; `CLASS_STRIDE=n` takes every n-th order.)
#[test]
#[ignore = "every order of three events: 12,816 programs against dense and reference"]
fn shared_history_class_extended() {
    let number = |name: &str, default: usize| std::env::var(name).map_or(default, |n| n.parse().expect("a number"));
    let (faithful, refused, nested) = shared_history_class(number("CLASS_EVENTS", 3), number("CLASS_STRIDE", 1));
    println!("{faithful} faithful, {refused} refused ({nested} nested, reference only)");
}

/// A budget that stops the run while some worlds have rewound and wait for the checks they deferred, and
/// others still run (`@slow` takes longer where it read 1): reversal from every stop gets back to the start,
/// for a rewind whose parts sum to what their history describes and for two whose sums do not (the keep
/// into `%o` is carried into `%q`, which the first read read, or into a temporary allocated since the mark).
#[test]
fn a_stop_before_deferred_checks_are_made_reverses_exactly() {
    for (gate, keep, refuses) in [
        ("x %o ctrl %q", "", false),
        ("x %q ctrl %o", " keep (i1 1 -> %o)", true),
        ("%t = qalloc 1\nx %t ctrl %o", " keep (i1 1 -> %o)", true),
    ] {
        let ret = 9 + gate.lines().count() - 1;
        let text = format!(
            "memory 512\nfunc @id(i1 %x)->i1 {{\nentry:\nret %x\n}}\n{SLOW}func @main() {{\nentry:\n%q = qalloc 1\n%o = qalloc 1\nh %q\n%m = mark budget -1\n%v1 = call @id(%q)\n{gate}\nh %q\n%v2 = call @slow(%q)\nrewind %m{keep}\nret\n}}\n"
        );
        let p = bennett_ir::load(&text).expect("valid");
        let mut full = executor(&p, "main", 0);
        let end = full.run();
        assert_eq!(end, if refuses { Outcome::Faulted(Fault::BadRewind) } else { Outcome::Returned(None) });
        let ticks = full.stats().steps;
        let mut parked = 0;
        for budget in 0..ticks {
            let mut e = Executor::new(
                &p,
                p.function_named("main").expect("entry"),
                &[],
                Options { budget: Some(budget), ..options() },
            )
            .expect("start");
            let start = e.snapshot();
            assert_eq!(e.run(), Outcome::Faulted(Fault::Budget));
            // Some worlds have finished their rewind (at `ret`) while others have not.
            let snapshot = e.snapshot();
            let at_ret = std::iter::once(&snapshot)
                .chain(&snapshot.worlds)
                .filter(|s| s.classical.frames.last().is_some_and(|f| f.inst == ret))
                .count();
            parked += usize::from(at_ret > 0 && !snapshot.worlds.is_empty());
            e.reverse().expect("a budget stop reverses");
            assert_eq!(e.snapshot().classical, start.classical, "budget {budget}\n{text}");
            assert!(
                e.snapshot().quantum.distance(&start.quantum).is_some_and(|d| d < 1e-12),
                "budget {budget}\n{text}"
            );
        }
        assert!(parked > 0, "some stop falls while checks wait");
    }
}

/// Found by the four-event class pass: the world that read 0 rewinds the inner mark early and waits; the
/// one that read 1 (`@slow`) rewinds both marks first, and its parts of the outer rewind, emptied by the
/// temporary its keep dirtied, must wait for their own checks while the first world's inner checks pass.
#[test]
fn emptied_parts_wait_for_their_own_checks_while_a_deeper_rewind_passes() {
    let text = format!(
        "memory 512\nfunc @id(i1 %x)->i1 {{\nentry:\nret %x\n}}\n{SLOW}func @main() {{\nentry:\n%q = qalloc 1\n%o = qalloc 1\n%r = qalloc 1\nh %q\n%m = mark budget -1\n%v1 = call @slow(%q)\nh %q\n%t = qalloc 1\nx %t ctrl %o\n%n1 = mark budget -1\n%w1 = call @id(%q)\nh %q\nrewind %n1 keep (%w1 -> %r)\nrewind %m keep (i1 1 -> %o)\nret\n}}\n"
    );
    let p = bennett_ir::load(&text).expect("valid");
    let model = Model::new(&p, "main");
    let reference = model.run_joined(basis(model.initial("main", &[])), 4000);
    assert!(reference.keys().all(|w| w.classical.finish == Some(Finish::Faulted(Fault::BadRewind))));
    let mut e = executor(&p, "main", 0);
    assert_eq!(e.run(), Outcome::Faulted(Fault::BadRewind));
    assert!(observed_one_of(&model, &reference, &e.snapshot()));
}
