//! Independent testing of DESIGN.md “Worlds”; production code is never changed by these probes.
#[allow(dead_code, reason = "shared independent oracle helpers")]
mod oracle_support;

use bennett_exec::{Executor, Options, Outcome, ReverseError, RunError};
use bennett_ir::{Fault, ValidModule};
use oracle_support::worlds::{Finish, Model, Superposition, basis};
use oracle_support::worlds_executor::assert_worlds;
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
                && w.classical.tape == snapshot.classical.tape
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
    let repeat = if k & 8 == 0 { "" } else { "%n = mark\n%w = call @work(%q, %b, %acc)\nrewind %n keep (%w -> %out)" };
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
entry:\n%m = mark\n%r = call @id(%b)\nrewind %m keep (%r -> %a)\nret\n}}
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
%outer = mark
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
%m = mark
%v = call @id1(%q)
rewind %m
ret
}
func @unaligned() {
entry:
%q = qalloc 1
h %q
%m = mark
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

/// cargo test -p bennett-exec --test worlds_independent shared_history_read -- --ignored --nocapture
#[test]
#[ignore = "F1: a later split hides a changed earlier read; returns success instead of bad_rewind"]
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
%m = mark
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
                    "func @main() {{\nentry:\n%a = qalloc 2\n%b = qalloc 2\n%out = qalloc 2\n%fault = qalloc 4\n{}{}{}{}\n{prep}\nOP\nret\n}}\n",
                    if a & 1 != 0 { "x %a[0]\n" } else { "" },
                    if a & 2 != 0 { "x %a[1]\n" } else { "" },
                    if b & 1 != 0 { "x %b[0]\n" } else { "" },
                    if b & 2 != 0 { "x %b[1]\n" } else { "" }
                );
                let apply = format!("{body}{}", entry.replace("OP", "apply @f(%a, %b) -> %out, %fault budget 100"));
                let idiom = format!(
                    "{body}{}",
                    entry.replace("OP", "%m = mark\n%v = call @f(%a, %b)\nrewind %m keep (%v -> %out)")
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
