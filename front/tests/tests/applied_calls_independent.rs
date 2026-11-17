//! Independent C-to-device checks for applied calls. Native executions supply the values of the applied
//! functions. The executor's snapshots expose both the register amplitudes and the working memory.
use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

use bennett_exec::{Executor, Options, Outcome, RunError, Snapshot};
use bennett_front_tests::{
    CLANG_FLAGS, CLANG_O2_FLAGS, REGION_BUDGET, STACK_BYTES, bitcode, c_dir, column, entry, front, lower, lower_text,
    lower_text_with_budget, native_build, options, out_dir, registers, root, run, state_around_first_apply,
    state_before_first_measurement, write_c,
};
use bennett_ir::{Const, Fault, FixedGate, FuncId, Gate, Inst, Operand, Place, ValidModule, Width, validate};

const MODES: [(&str, &[&str], &[&str]); 3] =
    [("o0", CLANG_FLAGS, &[]), ("o2", CLANG_O2_FLAGS, &[]), ("taint", CLANG_FLAGS, &["--taint-all"])];
const VALUES: usize = 8;

fn source() -> std::path::PathBuf {
    c_dir().join("applied/independent.c")
}

fn native_table() -> [[u64; 3]; VALUES] {
    static TABLE: OnceLock<[[u64; 3]; VALUES]> = OnceLock::new();
    *TABLE.get_or_init(|| {
        let executable = native_build(&source(), "apply-independent", &["-DNATIVE"]);
        std::array::from_fn(|v| {
            let output = Command::new(&executable).arg(v.to_string()).output().expect("native run");
            assert!(output.status.success(), "native input {v}: {output:?}");
            String::from_utf8(output.stdout)
                .expect("native output")
                .split_whitespace()
                .map(|word| word.parse().expect("native result"))
                .collect::<Vec<_>>()
                .try_into()
                .expect("three native results")
        })
    })
}

fn compare_memory(before: &Snapshot, after: &Snapshot, context: &str) {
    let before_memory = &before.classical.memory;
    let after_memory = &after.classical.memory;
    assert_eq!(before_memory.len(), after_memory.len(), "{context}: memory size");
    assert_eq!(before_memory.iter().zip(after_memory).position(|(a, b)| a != b), None, "{context}: first byte changed");
    assert_eq!(after.classical.stack_pointer, before.classical.stack_pointer, "{context}: stack pointer");
    assert_eq!(after.classical.tape, before.classical.tape, "{context}: console tape");
    assert_eq!(after.quantum.registers, before.quantum.registers, "{context}: register table");
    assert_eq!(after.quantum.cells, before.quantum.cells, "{context}: qubit bytes");
}

fn compare_apply(module: &ValidModule, name: &str, expected: &[Result<u64, Fault>], initial: u64, opts: Options) {
    let (before, after) = state_around_first_apply(module, name, opts);
    compare_memory(&before, &after, name);
    let columns = registers(module, name);
    let (v, r, f) = (column(&columns, "v"), column(&columns, "r"), column(&columns, "r.fault"));
    assert_eq!(before.quantum.amplitudes.len(), expected.len(), "@{name}: input configurations");
    assert_eq!(after.quantum.amplitudes.len(), expected.len(), "@{name}: output configurations");
    let uniform = 1.0 / (expected.len() as f64).sqrt();
    for (value, expected) in expected.iter().copied().enumerate() {
        let (config, amplitude) = before
            .quantum
            .amplitudes
            .iter()
            .find(|(config, _)| config[v] == value as u64)
            .unwrap_or_else(|| panic!("@{name}: missing input v={value}"));
        let (actual, actual_amplitude) = after
            .quantum
            .amplitudes
            .iter()
            .find(|(out, _)| out[v] == config[v])
            .unwrap_or_else(|| panic!("@{name}: missing v={value}"));
        assert_eq!((config[r], config[f]), (initial, 0), "@{name}, v={value}: initial targets");
        let (result, fault) = expected.map_or_else(|fault| (initial, fault.code()), |result| (initial ^ result, 0));
        assert_eq!((actual[r], actual[f]), (result, fault), "@{name}, v={value}: result and fault");
        let mut wanted = config.clone();
        wanted[r] = result;
        wanted[f] = fault;
        assert_eq!(*actual, wanted, "@{name}, v={value}: full register configuration");
        assert!((*amplitude - uniform).norm() < 1e-12, "@{name}, v={value}: input amplitude {amplitude}");
        assert!((*actual_amplitude - *amplitude).norm() < 1e-12, "@{name}, v={value}: output amplitude");
    }
}

#[test]
fn recursive_stack_pointer_and_global_writes_are_private() {
    let native = native_table().map(|row| Ok(row[0]));
    for (mode, clang, flags) in MODES {
        let module = lower(&source(), &format!("apply-recursive-{mode}"), clang, flags);
        compare_apply(&module, "recursive_entry", &native, 0x173, options(0));
        let state = state_before_first_measurement(&module, "recursive_entry", &[]);
        let columns = registers(&module, "recursive_entry");
        let v = column(&columns, "v");
        let mut executor = Executor::new(&module, entry(&module, "recursive_entry"), &[], options(0)).expect("entry");
        for _ in 0..=state.classical.steps {
            assert_eq!(executor.step(), None);
        }
        let measured = executor.snapshot();
        let value = measured.quantum.amplitudes[0].0[v] as usize;
        let result = native[value].expect("native result") ^ 0x173 ^ 23 ^ 11;
        assert_eq!(executor.run(), Outcome::Returned(Some(result.into())), "{mode}, v={value}");
    }
}

#[test]
fn a_non_local_jump_with_live_recursive_frames_matches_native() {
    let native = native_table().map(|row| Ok(row[1]));
    // This uses libc's regular setjmp spelling, with and without the front's taint-all option.
    for (mode, clang, flags) in MODES {
        let module = lower(&source(), &format!("apply-jump-{mode}"), clang, flags);
        compare_apply(&module, "jump_entry", &native, 0, options(0));
    }
}

fn single_apply(module: &ValidModule, function: &str, limit: u64) -> FuncId {
    let applies = module.functions.iter().flat_map(|function| {
        function.blocks.iter().flat_map(move |block| {
            block.insts.iter().filter_map(move |inst| match inst {
                Inst::Apply { callee, budget, .. } if !module.function(*callee).name.starts_with("bennett.copy.") => {
                    Some((function.name.as_str(), callee, budget))
                }
                _ => None,
            })
        })
    });
    // Ignore the copies that clear measured registers. The fixture has three independent calls.
    let applies = applies.collect::<Vec<_>>();
    assert_eq!(applies.len(), 3, "one apply per entry: {applies:?}");
    let [(owner, callee, Operand::Const(budget))] =
        applies.into_iter().filter(|(owner, _, _)| *owner == function).collect::<Vec<_>>()[..]
    else {
        panic!("@{function}: expected one apply with one constant budget");
    };
    assert_eq!(budget.value(), u128::from(limit), "@{owner}: emitted budget");
    *callee
}

#[test]
fn nested_loops_share_one_budget_with_an_independent_cutoff_for_each_value() {
    let name = "nested_entry";
    let mut native = native_table().map(|row| Ok(row[2]));
    let module = lower(&source(), "apply-nested-full", CLANG_FLAGS, &[]);
    let callee = single_apply(&module, name, REGION_BUDGET);
    let worker = entry(&module, "nested_work");
    let calls: Vec<_> = module
        .function(callee)
        .blocks
        .iter()
        .flat_map(|b| &b.insts)
        .filter_map(|inst| if let Inst::Call { callee, .. } = inst { Some(*callee) } else { None })
        .collect();
    assert_eq!(calls, [worker], "one call of the loop body");
    let worker = module.function(worker);
    let back_edges = worker
        .blocks
        .iter()
        .enumerate()
        .map(|(i, block)| {
            let mut count = 0;
            block.term.visit_targets(|target| count += usize::from(target.block.index() <= i));
            count
        })
        .sum::<usize>();
    assert_eq!(back_edges, 2, "both C loops remain as jumps in the ordinary function");
    compare_apply(&module, name, &native, 0, options(0));

    // Execute the emitted callee as an ordinary entry with each definite argument to count steps.
    let steps: [u64; VALUES] = std::array::from_fn(|v| {
        let mut executor = Executor::new(&module, callee, &[v as u64], options(0)).expect("ordinary callee");
        assert_eq!(executor.run(), Outcome::Returned(Some(native[v].expect("native result").into())), "v={v}");
        executor.stats().steps
    });
    assert!(steps.windows(2).all(|pair| pair[0] < pair[1]), "{steps:?}");
    let short = steps[4];
    let text = lower_text_with_budget(&source(), "apply-nested-short", CLANG_FLAGS, &[], short).expect("lowers");
    let module = bennett_ir::load(&text).expect("valid");
    single_apply(&module, name, short);
    for (v, &steps) in steps.iter().enumerate() {
        if steps > short {
            native[v] = Err(Fault::Budget);
        }
    }
    compare_apply(&module, name, &native, 0, Options { allow_region_faults: true, ..options(0) });
    let actual = run(&module, name, &[], 0);
    assert!(
        matches!(
            actual,
            Outcome::Stopped(RunError::RegionFault { fault: Fault::Budget, faulting: 3, configurations: VALUES, .. })
        ),
        "budget {short}, costs {steps:?}: {actual:?}, expected three budget faults"
    );
    println!("ordinary callee costs {steps:?}; budget {short} faults only v=5,6,7");
}

const FAULT_SOURCE: &str = r#"#include "bennett.h"
__attribute__((noinline)) unsigned fail_inside(unsigned v) {
    if (v < 2) {
        ACTION;
    }
    return 19 + v;
}
unsigned applied_entry(void) {
    unsigned char v = 0;
    unsigned r = 0;
    bennett_h(bennett_bit(&v, 0));
    r ^= fail_inside(v);
    bennett_measure(&v);
    return (unsigned)bennett_measure(&r);
}
"#;

#[test]
fn console_and_exit_fault_for_each_value_without_escaping_the_apply() {
    for (name, action, fault) in [
        ("effect", "__bennett_effect((int)('a' + v))", Fault::EffectInRegion),
        ("exit", "__bennett_exit((int)v + 17)", Fault::ExitInRegion),
    ] {
        let source = write_c(&format!("apply-fault-{name}"), &FAULT_SOURCE.replace("ACTION", action));
        let module = lower(&source, &format!("apply-fault-{name}"), CLANG_FLAGS, &[]);
        compare_apply(
            &module,
            "applied_entry",
            &[Err(fault); 2],
            0,
            Options { allow_region_faults: true, ..options(0) },
        );
        let outcome = run(&module, "applied_entry", &[], 0);
        assert!(
            matches!(outcome, Outcome::Stopped(RunError::RegionFault {
            fault: found, faulting: 2, configurations: 2, ..
        }) if found == fault),
            "{name}: {outcome:?}"
        );
    }
}

fn lower_diagnostics(source: &Path, name: &str, clang: &[&str], flags: &[&str]) -> (String, String) {
    let include = format!("-I{}", root().join("front/include").display());
    let clang = clang.iter().copied().chain([include.as_str()]).collect::<Vec<_>>();
    let bitcode = bitcode(source, name, &clang);
    let program = out_dir().join(format!("{name}.bn"));
    let output = Command::new(front())
        .arg(bitcode)
        .arg("-o")
        .arg(&program)
        .arg(format!("--stack-bytes={STACK_BYTES}"))
        .arg(format!("--region-budget={REGION_BUDGET}"))
        .args(flags)
        .output()
        .expect("front");
    let diagnostics = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(output.status.success(), "{name}: status {}, stderr: {diagnostics:?}", output.status);
    (std::fs::read_to_string(program).expect("device text"), diagnostics)
}

#[test]
fn only_arguments_that_reach_the_qubit_byte_fault() {
    let source = write_c(
        "apply-qubit-byte",
        r#"#include "bennett.h"
volatile unsigned char bytes[] = {19, 41, 73, 107};
unsigned long byte_address(void) { return (unsigned long)&bytes[2]; }
__attribute__((noinline)) unsigned touch(unsigned n) {
    if (n & 1) {
        bytes[n - 1] = 90 + n;
        return 10 + n;
    }
    return bytes[n] + n;
}
unsigned applied_entry(void) {
    unsigned char v = 0;
    unsigned r = 0;
    bennett_h(bennett_bit(&v, 0));
    bennett_h(bennett_bit(&v, 1));
    r ^= touch(v);
    bennett_measure(&v);
    return (unsigned)bennett_measure(&r);
}
"#,
    );
    let lowered = lower(&source, "apply-qubit-byte", CLANG_FLAGS, &[]);
    let Outcome::Returned(Some(address)) = run(&lowered, "byte_address", &[], 0) else {
        panic!("no address for bytes[2]");
    };
    compare_apply(&lowered, "applied_entry", &[Ok(19), Ok(11), Ok(75), Ok(13)], 0, options(0));

    // Address gates live in the device language. Add one immediately before the front's unchanged apply.
    let function = entry(&lowered, "applied_entry");
    let mut module = lowered.into_module();
    let (block, at) = module.functions[function.index()]
        .blocks
        .iter_mut()
        .find_map(|block| {
            let at = block.insts.iter().position(|inst| matches!(inst, Inst::Apply { .. }))?;
            Some((block, at))
        })
        .expect("the applied call");
    block.insts.insert(
        at,
        Inst::GateAt {
            gate: Gate::Fixed(FixedGate::Z),
            target: Place::Memory {
                address: Operand::Const(Const::new(Width::WORD, address).expect("address")),
                bit: 7,
            },
            controls: vec![],
        },
    );
    let module = validate(module).expect("device address gate");
    let expected = [Ok(19), Ok(11), Err(Fault::QubitMemoryAccess), Err(Fault::QubitMemoryAccess)];
    compare_apply(&module, "applied_entry", &expected, 0, Options { allow_region_faults: true, ..options(0) });
    let outcome = run(&module, "applied_entry", &[], 0);
    assert!(
        matches!(
            outcome,
            Outcome::Stopped(RunError::RegionFault {
                fault: Fault::QubitMemoryAccess,
                faulting: 2,
                configurations: 4,
                ..
            })
        ),
        "{outcome:?}"
    );
}

const WARNING_SOURCE: &str = r#"#include "bennett.h"
#define NOINLINE __attribute__((noinline))
volatile unsigned global = 13, other = 29;
NOINLINE unsigned put(unsigned v, volatile unsigned *p) { *p = 19 + v; return v + 7; }
NOINLINE unsigned get(volatile unsigned *p) { return *p; }
unsigned applied_entry(volatile unsigned *arg) {
    volatile unsigned local = 23;
    unsigned char v = 0;
    unsigned r = 0;
    bennett_h(bennett_bit(&v, 0));
    r ^= put(v, WRITTEN);
    bennett_measure(&v);
    return (unsigned)bennett_measure(&r) + READ;
}
"#;

fn warning_source(name: &str, written: &str, read: &str) -> std::path::PathBuf {
    write_c(name, &WARNING_SOURCE.replace("WRITTEN", written).replace("READ", read))
}

#[test]
fn warns_for_globals_caller_parameters_and_direct_local_reads() {
    for (name, written, read, expected) in [
        ("global", "&global", "get(&global)", Some("@global")),
        ("parameter", "arg", "get(arg)", Some("argument 1")),
        ("local", "&local", "local", Some("reads a local variable")),
        ("disjoint", "&global", "get(&other)", None),
    ] {
        let source = warning_source(&format!("apply-warning-{name}"), written, read);
        for (mode, clang, flags) in MODES {
            let tag = format!("apply-warning-{name}-{mode}");
            let (text, diagnostics) = lower_diagnostics(&source, &tag, clang, flags);
            bennett_ir::load(&text).expect("warning still emits a valid program");
            if let Some(expected) = expected {
                assert!(
                    diagnostics.contains("warning: the call of @put depends on a qubit")
                        && diagnostics.contains(expected),
                    "{tag}: stderr: {diagnostics:?}"
                );
            } else {
                assert!(diagnostics.is_empty(), "{tag}: stderr: {diagnostics:?}");
            }
        }
    }
}

#[test]
fn warns_when_a_direct_callee_reads_the_callers_local_variable() {
    let name = "apply-warning-local-reader";
    let source = write_c(
        name,
        r#"#include "bennett.h"
__attribute__((noinline)) unsigned put(unsigned v, volatile unsigned *p) { *p = v; return v + 7; }
__attribute__((noinline)) unsigned get(volatile unsigned *p) { return *p; }
unsigned applied_entry(void) {
    volatile unsigned local = 23;
    unsigned char v = 0;
    unsigned r = 0;
    bennett_h(bennett_bit(&v, 0));
    r ^= put(v, &local);
    return (unsigned)bennett_measure(&r) + get(&local);
}
"#,
    );
    let (text, diagnostics) = lower_diagnostics(&source, name, CLANG_FLAGS, &[]);
    bennett_ir::load(&text).expect("warning still emits a valid program");
    assert!(
        diagnostics.contains("warning: the call of @put depends on a qubit")
            && diagnostics.contains("reads a local variable"),
        "front accepted the program; expected a warning for put(v, &local) followed by get(&local); stderr: {diagnostics:?}"
    );
}

#[test]
fn calls_with_separate_results_and_two_read_only_calls_are_accepted() {
    let source = write_c(
        "apply-separate",
        r#"#include "bennett.h"
#define NOINLINE __attribute__((noinline))
static volatile unsigned counter = 11;
NOINLINE unsigned bump(unsigned v) { counter += v + 1; return counter * 3; }
NOINLINE unsigned look(unsigned v) { return (v * 2) ^ 7; }
NOINLINE unsigned other(unsigned v) { return 13 - v; }
unsigned applied_entry(void) {
    unsigned char v = 0;
    unsigned r = 0, s = 0, t = 0, u = 0;
    bennett_h(bennett_bit(&v, 0));
    unsigned a = bump(v);
    r ^= a;
    s ^= a;
    t ^= bump(v + 2);
    u ^= look(v) + other(v);
    bennett_measure(&v);
    return (unsigned)(bennett_measure(&r) + bennett_measure(&s) + bennett_measure(&t) + bennett_measure(&u));
}
"#,
    );
    for (mode, clang, flags) in MODES {
        let module = lower(&source, &format!("apply-separate-{mode}"), clang, flags);
        let columns = registers(&module, "applied_entry");
        let columns = ["v", "r", "s", "t", "u"].map(|name| column(&columns, name));
        let snapshot = state_before_first_measurement(&module, "applied_entry", &[]);
        assert_eq!(snapshot.quantum.amplitudes.len(), 2, "{mode}");
        for (config, _) in snapshot.quantum.amplitudes {
            let [v, r, s, t, u] = columns.map(|col| config[col]);
            assert_eq!(
                [r, s, t, u],
                [(12 + v) * 3, (12 + v) * 3, (14 + v) * 3, ((v * 2) ^ 7) + 13 - v],
                "{mode}, v={v}"
            );
        }
    }
}

#[test]
fn unused_results_two_writers_and_intervening_writes_are_refused_by_name() {
    let header = r#"#include "bennett.h"
volatile unsigned counter;
__attribute__((noinline)) unsigned bump(unsigned n) { counter += n; return n + counter; }
__attribute__((noinline)) unsigned again(unsigned n) { counter ^= n; return counter + 2; }
__attribute__((noinline)) unsigned pure(unsigned n) { return n * 11; }
unsigned applied_entry(void) {
    unsigned char v = 0;
    unsigned r = 0;
    bennett_h(bennett_bit(&v, 0));
"#;
    let tail = "    bennett_measure(&v);\n    return (unsigned)bennett_measure(&r) + counter;\n}\n";
    for (name, body, expected) in [
        ("discard", "    (void)bump(v);\n", "whose result is not used, to a function that may write memory"),
        ("mixed", "    r ^= bump(v) + pure(v);\n", "two calls that depend on a qubit in one computation"),
        (
            "combined",
            "    r ^= bump(v) + again(v);\n",
            "a call that depends on a qubit and reads memory that may change before its result is used",
        ),
        (
            "conditional",
            "    if (v) r ^= bump(2);\n",
            "a call to @bump, which may write memory, controlled by a value that depends on a qubit",
        ),
        (
            "intervening",
            "    unsigned a = bump(v);\n    counter = 31;\n    r ^= a;\n",
            "a call that depends on a qubit and reads memory that may change before its result is used",
        ),
    ] {
        let name = format!("apply-refusal-{name}");
        let source = write_c(&name, &format!("{header}{body}{tail}"));
        let error = lower_text(&source, &name, CLANG_FLAGS, &[]).expect_err("expected a named refusal");
        assert!(error.contains(expected), "{name}: expected {expected:?}, got {error:?}");
    }
}

#[test]
fn a_pure_call_on_an_ordinary_argument_does_not_count_as_qubit_dependent() {
    let name = "apply-ordinary-pure";
    let source = write_c(
        name,
        r#"#include "bennett.h"
volatile unsigned cell = 13;
__attribute__((noinline)) unsigned bump(unsigned v) { cell += v; return cell; }
__attribute__((noinline)) unsigned score(unsigned n) { return n * 11; }
unsigned applied_entry(unsigned n) {
    unsigned char v = 0;
    unsigned r = 0;
    bennett_h(bennett_bit(&v, 0));
    r ^= bump(v) + score(n);
    return (unsigned)bennett_measure(&r);
}
"#,
    );
    for (mode, flags) in [("default", &[][..]), ("taint-all", &["--taint-all"][..])] {
        let text = lower_text(&source, &format!("{name}-{mode}"), CLANG_FLAGS, flags)
            .unwrap_or_else(|error| panic!("{mode}: {error}"));
        let module = bennett_ir::load(&text).expect("valid device program");
        let state = state_before_first_measurement(&module, "applied_entry", &[3]);
        let columns = registers(&module, "applied_entry");
        let (v, r) = (column(&columns, "v"), column(&columns, "r"));
        let values = state.quantum.amplitudes.iter().map(|(config, _)| (config[v], config[r])).collect::<Vec<_>>();
        assert_eq!(values, [(0, 46), (1, 47)], "{mode}");
    }
}

/// `--taint-all` moves an ordinary read-only call into the slice of an applied writing call: it must still read
/// memory as at its own site, never the applied call's writes on the private copy (`peek` reads 13 in every
/// mode, so r is 2 * (13 + v) - 13).
#[test]
fn an_ordinary_reader_in_a_slice_does_not_see_the_applied_calls_writes() {
    let name = "apply-ordinary-reader";
    let source = write_c(
        name,
        r#"#include "bennett.h"
unsigned cell = 13;
__attribute__((noinline)) unsigned bump(unsigned v) { cell += v; return cell; }
__attribute__((noinline)) unsigned peek(unsigned n) { return cell + n; }
unsigned applied_entry(unsigned n) {
    unsigned char v = 0;
    unsigned r = 0;
    bennett_h(bennett_bit(&v, 0));
    r ^= bump(v) * 2 - peek(n);
    return (unsigned)bennett_measure(&r);
}
"#,
    );
    for (mode, flags) in [("default", &[][..]), ("taint-all", &["--taint-all"][..])] {
        let text = lower_text(&source, &format!("{name}-{mode}"), CLANG_FLAGS, flags)
            .unwrap_or_else(|error| panic!("{mode}: {error}"));
        let module = bennett_ir::load(&text).expect("valid device program");
        let state = state_before_first_measurement(&module, "applied_entry", &[0]);
        let columns = registers(&module, "applied_entry");
        let (v, r) = (column(&columns, "v"), column(&columns, "r"));
        let values = state.quantum.amplitudes.iter().map(|(config, _)| (config[v], config[r])).collect::<Vec<_>>();
        assert_eq!(values, [(0, 13), (1, 15)], "{mode}");
    }
}
