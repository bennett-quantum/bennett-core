//! Applied calls (`docs/DESIGN.md`, "Applied calls"): a call whose arguments depend on a qubit, to an ordinary
//! function, is applied whole with one budget, whatever memory it writes, allocates or frees and whatever
//! loops and calls it contains. Its writes are undone when it ends; only its result survives. Each argument
//! value's result is compared with a native run of the same C in a fresh process.
use bennett_exec::{Executor, Options, Outcome, RunError, Snapshot};
use bennett_front_tests::{
    CLANG_FLAGS, CLANG_O2_FLAGS, c_dir, lower_text, native_build, options, out_dir, registers, root,
    state_around_first_apply, write_c,
};
use bennett_ir::{Fault, ValidModule};
use num_complex::Complex64;
use std::path::Path;
use std::process::Command;

/// The step budget of every `apply` in the musl fixture: far above any argument value's need.
const WORK_BUDGET: u64 = 1_000_000;
/// The argument register of the musl fixture is three qubits in uniform superposition.
const VALUES: u64 = 8;

/// `source` linked with musl by `rt/musl/link.py` (the front included); the device text and the front's
/// diagnostics.
fn musl_link(source: &Path, name: &str, region_budget: u64) -> (String, String) {
    let workspace = root().canonicalize().expect("workspace");
    let build = Command::new("python3").arg(workspace.join("rt/musl/build.py")).output().expect("musl build");
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    let file = out_dir().join(format!("{name}.bn"));
    let link = Command::new("python3")
        .arg(workspace.join("rt/musl/link.py"))
        .arg(source)
        .arg("-o")
        .arg(&file)
        .arg(format!("--region-budget={region_budget}"))
        .arg(format!("--clang-flag=-I{}", workspace.join("front/include").display()))
        .arg("--clang-flag=-fno-discard-value-names")
        .output()
        .expect("musl link");
    let diagnostics = String::from_utf8_lossy(&link.stderr).into_owned();
    assert!(link.status.success(), "{diagnostics}");
    (std::fs::read_to_string(&file).expect("device text"), diagnostics)
}

/// The native result for each argument value, each from a fresh process in an empty directory of its own
/// (the fixture creates a file in its working directory).
fn native_results(source: &Path, name: &str) -> Vec<u64> {
    let executable = native_build(source, name, &["-DNATIVE"]);
    (0..VALUES)
        .map(|v| {
            let dir = out_dir().join(format!("{name}.native-run-{v}"));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("native run directory");
            let output = Command::new(&executable).arg(v.to_string()).current_dir(&dir).output().expect("native run");
            assert!(output.status.success(), "native {name}({v}): {output:?}");
            String::from_utf8(output.stdout).expect("native output").trim().parse().expect("native result")
        })
        .collect()
}

/// The columns of `entry`'s argument register `v`, result register `r` and the fault register of `r`'s
/// first `apply`.
fn columns(module: &ValidModule, entry: &str) -> (usize, usize, usize) {
    let names = registers(module, entry);
    let at = |name: &str| names.iter().position(|n| n == name).unwrap_or_else(|| panic!("no %{name} in {names:?}"));
    (at("v"), at("r"), at("r.fault"))
}

fn work_source() -> std::path::PathBuf {
    c_dir().join("applied/work.c")
}

/// Exactly the eight configurations (v, result of v, no fault), each with amplitude 1/sqrt(8), where
/// `result(v)` is `None` for a value that faulted with `fault`.
fn assert_applied(
    module: &ValidModule,
    state: &Snapshot,
    expected: &[(u64, Option<u64>)],
    fault: Fault,
    context: &str,
) {
    assert_applied_in(module, "applied_entry", state, expected, fault, context);
}

fn assert_applied_in(
    module: &ValidModule,
    entry: &str,
    state: &Snapshot,
    expected: &[(u64, Option<u64>)],
    fault: Fault,
    context: &str,
) {
    let (v, r, f) = columns(module, entry);
    assert_eq!(state.quantum.amplitudes.len(), expected.len(), "{context}: {:?}", state.quantum.amplitudes);
    let weight = 1.0 / (VALUES as f64).sqrt();
    for &(value, result) in expected {
        let config = state
            .quantum
            .amplitudes
            .iter()
            .find(|(config, _)| config[v] == value)
            .unwrap_or_else(|| panic!("{context}: no configuration with v = {value}"));
        let (want_r, want_fault) = result.map_or((0, fault.code()), |r| (r, 0));
        assert_eq!((config.0[r], config.0[f]), (want_r, want_fault), "{context}, v = {value}");
        assert!((config.1 - Complex64::new(weight, 0.0)).norm() < 1e-12, "{context}, v = {value}: {}", config.1);
    }
}

#[test]
fn a_call_that_allocates_writes_files_and_loops_is_applied_whole_and_matches_native_for_each_value() {
    let source = work_source();
    let native = native_results(&source, "applied_work");
    let (text, diagnostics) = musl_link(&source, "applied_work", WORK_BUDGET);
    assert!(!diagnostics.contains("warning"), "nothing after the call reads what it writes: {diagnostics}");
    let module = bennett_ir::load(&text).expect("valid device program");
    let (before, after) = state_around_first_apply(&module, "applied_entry", options(0));
    // Its heap, static buffer and file were written on the private memory of each run, and undone.
    assert_eq!(after.classical.memory, before.classical.memory, "the applied call left a write behind");
    assert_eq!(after.classical.tape, before.classical.tape);
    let expected: Vec<(u64, Option<u64>)> = (0..VALUES).map(|v| (v, Some(native[v as usize]))).collect();
    assert_applied(&module, &after, &expected, Fault::Budget, "applied work");
    // The whole program runs in strict mode to its end: no value faulted.
    let mut executor =
        Executor::new(&module, module.function_named("applied_entry").expect("entry"), &[], options(3)).expect("start");
    assert!(matches!(executor.run(), Outcome::Returned(Some(_))));
}

/// The one budget covers the whole applied call, its nested loops, allocation and file operations included.
/// At the smallest budget that suffices for every value nothing faults; one step less faults exactly the
/// values that need it all, with their result register untouched, and the others keep their native result.
#[test]
fn a_budget_too_small_for_some_values_faults_those_values_only() {
    let source = work_source();
    let native = native_results(&source, "applied_budget");
    let (text, _) = musl_link(&source, "applied_budget", WORK_BUDGET);
    // The device text with the applied call's budget replaced by `budget`.
    let with_budget = |budget: u64| {
        let line = text.lines().find(|l| l.contains("apply @applied_entry.apply(")).expect("the applied call");
        let changed = line.replace(&format!("budget {WORK_BUDGET}"), &format!("budget {budget}"));
        bennett_ir::load(&text.replacen(line, &changed, 1)).expect("valid device program")
    };
    let lenient = Options { allow_region_faults: true, ..options(0) };
    let faulting = |module: &ValidModule| {
        let (_, after) = state_around_first_apply(module, "applied_entry", lenient);
        let (v, _, f) = columns(module, "applied_entry");
        after.quantum.amplitudes.iter().filter(|(c, _)| c[f] != 0).map(|(c, _)| c[v]).collect::<Vec<u64>>()
    };
    let (mut low, mut high) = (0, WORK_BUDGET);
    assert!(faulting(&with_budget(high)).is_empty());
    while high - low > 1 {
        let middle = (low + high) / 2;
        if faulting(&with_budget(middle)).is_empty() {
            high = middle;
        } else {
            low = middle;
        }
    }
    let short = with_budget(high - 1);
    let faulted = faulting(&short);
    assert!(!faulted.is_empty() && (faulted.len() as u64) < VALUES, "budget {}: {faulted:?}", high - 1);
    let (_, after) = state_around_first_apply(&short, "applied_entry", lenient);
    let expected: Vec<(u64, Option<u64>)> =
        (0..VALUES).map(|v| (v, (!faulted.contains(&v)).then(|| native[v as usize]))).collect();
    assert_applied(&short, &after, &expected, Fault::Budget, &format!("budget {}", high - 1));
    // In the simulator's strict mode the same apply stops the run, naming the fault and how many values had it.
    let mut strict =
        Executor::new(&short, short.function_named("applied_entry").expect("entry"), &[], options(0)).expect("start");
    match strict.run() {
        Outcome::Stopped(RunError::RegionFault { fault, faulting, configurations, .. }) => {
            assert_eq!((fault, faulting, configurations), (Fault::Budget, faulted.len(), VALUES as usize));
        }
        other => panic!("strict run at budget {}: {other:?}", high - 1),
    }
    println!("smallest budget for every value: {high} steps; at {} values {faulted:?} fault", high - 1);
}

const MODES: [(&str, &[&str], &[&str]); 3] =
    [("o0", CLANG_FLAGS, &[]), ("o2", CLANG_O2_FLAGS, &[]), ("taint", CLANG_FLAGS, &["--taint-all"])];

fn lowered(name: &str, source: &str, mode: (&str, &[&str], &[&str])) -> Result<String, String> {
    let path = write_c(&format!("{name}.{}", mode.0), source);
    lower_text(&path, &format!("{name}.{}", mode.0), mode.1, mode.2)
}

/// A console write inside an applied call is the named fault `effect_in_region`, for every value.
#[test]
fn a_console_write_inside_an_applied_call_is_the_named_fault() {
    let source = r#"#include "bennett.h"
__attribute__((noinline)) unsigned talk(unsigned v) { __bennett_effect('x'); return v + 1; }
int applied_entry(void) {
  unsigned char v = 0;
  unsigned r = 0;
  bennett_h(bennett_bit(&v, 0));
  bennett_h(bennett_bit(&v, 1));
  bennett_h(bennett_bit(&v, 2));
  r ^= talk(v);
  return (int)(bennett_measure(&v) + bennett_measure(&r));
}
"#;
    for mode in MODES {
        let text = lowered("applied_console", source, mode).unwrap_or_else(|e| panic!("{}: {e}", mode.0));
        let module = bennett_ir::load(&text).expect("valid device program");
        let lenient = Options { allow_region_faults: true, ..options(0) };
        let (_, after) = state_around_first_apply(&module, "applied_entry", lenient);
        assert!(after.classical.tape.is_empty(), "{}: the console write happened", mode.0);
        let expected: Vec<(u64, Option<u64>)> = (0..VALUES).map(|v| (v, None)).collect();
        assert_applied(&module, &after, &expected, Fault::EffectInRegion, mode.0);
    }
}

/// musl's `printf` inside an applied call: a line is written out to the console, the named fault for every
/// value; without a newline the output stays in musl's buffer, which is undone with the call's other writes.
#[test]
fn printf_of_a_line_inside_an_applied_call_faults_and_buffered_output_is_undone() {
    let (text, _) = musl_link(&c_dir().join("applied/speak.c"), "applied_speak", WORK_BUDGET);
    let module = bennett_ir::load(&text).expect("valid device program");
    let lenient = Options { allow_region_faults: true, ..options(0) };
    let (_, after) = state_around_first_apply(&module, "line_entry", lenient);
    assert!(after.classical.tape.is_empty());
    let faulted: Vec<(u64, Option<u64>)> = (0..VALUES).map(|v| (v, None)).collect();
    assert_applied_in(&module, "line_entry", &after, &faulted, Fault::EffectInRegion, "printf of a line");
    let (before, after) = state_around_first_apply(&module, "partial_entry", options(0));
    assert_eq!(after.classical.memory, before.classical.memory);
    assert_eq!(after.classical.tape, before.classical.tape);
    let results: Vec<(u64, Option<u64>)> = (0..VALUES).map(|v| (v, Some(5 * v))).collect();
    assert_applied_in(&module, "partial_entry", &after, &results, Fault::Budget, "buffered printf");
}

/// A plain applied call that writes a global the caller reads afterwards: the program is lowered, the
/// result is the applied function's for each value, the global keeps its value, and the front warns.
#[test]
fn an_applied_call_whose_writes_later_code_reads_is_warned_about_and_undone() {
    let source = r#"#include "bennett.h"
unsigned counter = 5;
__attribute__((noinline)) unsigned bump(unsigned v) { counter += v; return counter * 3 + v; }
int applied_entry(void) {
  unsigned char v = 0;
  unsigned r = 0;
  bennett_h(bennett_bit(&v, 0));
  bennett_h(bennett_bit(&v, 1));
  bennett_h(bennett_bit(&v, 2));
  r ^= bump(v);
  bennett_measure(&v);
  bennett_measure(&r);
  return (int)counter;
}
"#;
    for mode in MODES {
        let name = format!("applied_warning.{}", mode.0);
        let path = write_c(&name, source);
        let include = format!("-I{}", root().join("front/include").display());
        let flags: Vec<&str> = mode.1.iter().copied().chain([include.as_str()]).collect();
        let bitcode = bennett_front_tests::bitcode(&path, &name, &flags);
        let program = out_dir().join(format!("{name}.bn"));
        let output = Command::new(bennett_front_tests::front())
            .arg(&bitcode)
            .arg("-o")
            .arg(&program)
            .arg("--region-budget=100000")
            .arg("--stack-bytes=65536")
            .args(mode.2)
            .output()
            .expect("front");
        let diagnostics = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{}: {diagnostics}", mode.0);
        assert!(
            diagnostics.contains("warning: the call of @bump depends on a qubit and is applied whole")
                && diagnostics.contains("@counter"),
            "{}: {diagnostics}",
            mode.0
        );
        let module = bennett_ir::load(&std::fs::read_to_string(&program).expect("text")).expect("valid");
        let (_, after) = state_around_first_apply(&module, "applied_entry", options(0));
        let expected: Vec<(u64, Option<u64>)> =
            (0..VALUES).map(|v| (v, Some(((5 + v) * 3 + v) & 0xffff_ffff))).collect();
        assert_applied(&module, &after, &expected, Fault::Budget, mode.0);
        let mut executor =
            Executor::new(&module, module.function_named("applied_entry").expect("entry"), &[], options(1))
                .expect("start");
        assert_eq!(executor.run(), Outcome::Returned(Some(5)), "{}: the write was not undone", mode.0);
    }
}

/// Refused by name: a call with qubit-dependent arguments whose result nothing uses (applied, it would do
/// nothing), and two such calls in one computation when one may write memory (the second would see the
/// first's writes, but each applied call starts from the memory as it was).
#[test]
fn calls_the_rule_cannot_express_are_refused_by_name() {
    let unused = r#"#include "bennett.h"
unsigned counter;
__attribute__((noinline)) unsigned note(unsigned v) { counter += v; return counter; }
int applied_entry(void) {
  unsigned char v = 0;
  bennett_h(&v);
  note(v);
  return (int)bennett_measure(&v) + (int)counter;
}
"#;
    let two = r#"#include "bennett.h"
unsigned counter;
__attribute__((noinline)) unsigned bump(unsigned v) { counter += v; return counter; }
__attribute__((noinline)) unsigned look(unsigned v) { return counter + v; }
int applied_entry(void) {
  unsigned char v = 0;
  unsigned r = 0;
  bennett_h(&v);
  r ^= bump(v) + look(v);
  return (int)(bennett_measure(&v) + bennett_measure(&r));
}
"#;
    for mode in [MODES[0], MODES[2]] {
        let refused = lowered("applied_unused", unused, mode).expect_err("an unused applied call");
        assert!(refused.contains("whose result is not used, to a function that may write memory"), "{refused}");
        let refused = lowered("applied_two", two, mode).expect_err("two calls, one writing");
        assert!(refused.contains("two calls that depend on a qubit in one computation"), "{refused}");
    }
}

/// `--taint-all` leaves an ordinary call with ordinary arguments in the control program, where its writes
/// survive, even though its result is combined with a qubit: the mode changes no result.
#[test]
fn an_ordinary_writing_call_stays_ordinary_with_taint_all() {
    let source = r#"#include "bennett.h"
unsigned counter;
__attribute__((noinline)) unsigned bump(unsigned n) { counter += n; return counter; }
int applied_entry(void) {
  _Bool q = 0, y = 0;
  bennett_x(&q);
  y ^= q & bump(3);
  bennett_measure(&q);
  return (int)bennett_measure(&y) + 2 * (int)counter;
}
"#;
    for mode in MODES {
        let text = lowered("applied_ordinary", source, mode).unwrap_or_else(|e| panic!("{}: {e}", mode.0));
        let module = bennett_ir::load(&text).expect("valid");
        let mut executor =
            Executor::new(&module, module.function_named("applied_entry").expect("entry"), &[], options(0))
                .expect("start");
        assert_eq!(executor.run(), Outcome::Returned(Some(7)), "{}", mode.0);
    }
}

/// The caller already owns an allocation and an open descriptor at apply entry. The applied function
/// reallocates and frees the object, then duplicates the descriptor, changes the file and closes both
/// descriptors. The caller can still use its original object and descriptor after the apply.
#[test]
fn apply_restores_an_existing_heap_object_file_and_shared_offset() {
    let source = c_dir().join("applied/rollback.c");
    let native = native_results(&source, "applied_rollback");
    let (text, _) = musl_link(&source, "applied_rollback", WORK_BUDGET);
    let module = bennett_ir::load(&text).expect("valid device program");
    let (before, after) = state_around_first_apply(&module, "applied_entry", options(0));
    assert!(before.classical.memory == after.classical.memory, "apply changed the working memory");
    assert_eq!(after.classical.stack_pointer, before.classical.stack_pointer);
    assert_eq!(after.classical.tape, before.classical.tape);

    let expected: Vec<_> = native.iter().copied().enumerate().map(|(n, result)| (n as u64, Some(result))).collect();
    assert_applied(&module, &after, &expected, Fault::Budget, "existing allocation and descriptor");
    let mut executor =
        Executor::new(&module, module.function_named("applied_entry").expect("entry"), &[], options(0)).expect("start");
    let outcome = executor.run();
    assert!(
        matches!(outcome, Outcome::Returned(Some(result)) if native.contains(&(result as u64))),
        "ordinary heap and descriptor probe: {outcome:?}"
    );
}
