//! Generated translation units for the declaration rule in docs/HANDLES.md, section 4.
//! Compare actual primitive calls from pass-free and driver-produced bitcode. Counting only direct
//! calls would miss pointer calls, and inlining or unrolling can change the number of sites in the IR.

use bennett_front_tests::{c_dir, clang, front, out_dir, root};
use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const LEVELS: [&str; 6] = ["-O0", "-O1", "-O2", "-O3", "-Os", "-Oz"];
const PRIMITIVES: [&str; 11] = [
    "bq_alloc",
    "bq_free",
    "bq_width",
    "bq_measure",
    "bq_gate",
    "bq_rotate",
    "bq_apply",
    "__bennett_effect",
    "__bennett_exit",
    "__bennett_out_of_memory",
    "__bennett_bad_argument",
];

const STOPS: [&str; 3] = ["__bennett_exit", "__bennett_out_of_memory", "__bennett_bad_argument"];

fn output(command: &mut Command) -> Output {
    command.output().unwrap_or_else(|error| panic!("{command:?}: {error}"))
}

fn checked(command: &mut Command) -> String {
    let result = output(command);
    assert!(result.status.success(), "{command:?}: {}\n{}", result.status, String::from_utf8_lossy(&result.stderr));
    String::from_utf8(result.stdout).expect("tool output is UTF-8")
}

fn driver() -> Command {
    Command::new(front().with_file_name("bennett-cc"))
}

fn write(path: &Path, text: &str) {
    fs::write(path, text).unwrap_or_else(|error| panic!("write {path:?}: {error}"));
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    Direct,
    Local,
    Global,
    ConstPointer,
    VolatilePointer,
    StaticHelper,
    InlineHelper,
    Loop,
    Restrict,
    PointerClaim,
}

const SHAPES: [Shape; 10] = [
    Shape::Direct,
    Shape::Local,
    Shape::Global,
    Shape::ConstPointer,
    Shape::VolatilePointer,
    Shape::StaticHelper,
    Shape::InlineHelper,
    Shape::Loop,
    Shape::Restrict,
    Shape::PointerClaim,
];

fn conforming(shape: Shape, entry: &str, seed: usize) -> String {
    let mut text = String::from(
        r#"#include "bq.h"
typedef unsigned char byte;
_Static_assert(sizeof(byte) == 1, "image fits in one byte");
#define WIDTH_ARG(x) width_arg(x)
__attribute__((const)) static unsigned width_arg(unsigned x) { return 3 + x; }
static void flip(void *block) { *(byte *)block ^= 1; }
"#,
    );
    if shape == Shape::Direct && seed & 1 == 1 {
        text.push_str("extern void ordinary_release(bq_reg) __asm__(\"bq_free\");\n#define bq_free ordinary_release\n");
    }
    let pointers = match shape {
        Shape::Local => Some(""),
        Shape::Global => Some("static "),
        Shape::ConstPointer => Some("static const "),
        Shape::VolatilePointer => Some("static volatile "),
        _ => None,
    };
    let mut declarations = String::new();
    if let Some(qualifiers) = pointers {
        writeln!(text, "#define FN(name) ptr_ ## name").expect("write macro");
        for name in PRIMITIVES {
            if shape == Shape::ConstPointer && STOPS.contains(&name) {
                // The ignored const-pointer witness covers the refusal of these three calls.
                writeln!(declarations, "#define ptr_{name} {name}").expect("write direct stop");
            } else {
                writeln!(declarations, "{qualifiers}__typeof__(&{name}) ptr_{name} = &{name};").expect("write pointer");
            }
        }
    } else {
        text.push_str("#define FN(name) name\n");
    }
    if shape != Shape::Local {
        text.push_str(&declarations);
    }
    if shape == Shape::PointerClaim {
        // clang emits these claims on the indirect calls even with the LLVM passes disabled.
        text.push_str(
            "__attribute__((const)) static uint64_t (*measure_const)(bq_reg, uint64_t, uint64_t) = bq_measure;\n\
             __attribute__((pure)) static uint64_t (*volatile measure_pure)(bq_reg, uint64_t, uint64_t) = bq_measure;\n",
        );
    }
    let helper = match shape {
        Shape::StaticHelper => {
            Some("static __attribute__((noinline)) void body(unsigned path, unsigned times, byte *image)")
        }
        Shape::InlineHelper => {
            Some("static inline __attribute__((always_inline)) void body(unsigned path, unsigned times, byte *image)")
        }
        Shape::Restrict => Some("static inline void body(unsigned path, unsigned times, byte *restrict image)"),
        _ => None,
    };
    if let Some(header) = helper {
        writeln!(text, "{header} {{").expect("write helper header");
    } else {
        writeln!(text, "void {entry}(unsigned path, unsigned times) {{\nbyte image[] = {{{}}};", seed % 256)
            .expect("write entry header");
    }
    if shape == Shape::Local {
        text.push_str(&declarations);
    }
    let bit = seed % 2;
    writeln!(
        text,
        r#"bq_reg r = FN(bq_alloc)(WIDTH_ARG(times));
bq_reg fault = FN(bq_alloc)(BQ_FAULT_BITS);
uint64_t bits = FN(bq_width)(r);
bq_bit control = {{fault, {bit}}};
bq_field field = {{{{r, 0, 1}}, 0, BQ_DIR_OUT}};
FN(bq_gate)(BQ_GATE_X, r, {bit}, &control, 1);
FN(bq_rotate)(BQ_ROT_PHASE, (double)times / 8, r, {bit}, 0, 0);
FN(bq_apply)(flip, bits + 20, fault, image, 1, &field, 1, 0, 0);"#,
    )
    .expect("write operations");
    let measure = "(void)FN(bq_measure)(r, 0, 1);\n(void)FN(bq_measure)(r, 1, 1);\n";
    match shape {
        Shape::Loop => {
            text.push_str("#pragma clang loop unroll_count(2)\nfor (unsigned i = 0; i < times; ++i) {\n");
            text.push_str(measure);
            text.push_str("}\n");
        }
        Shape::PointerClaim => {
            text.push_str("(void)measure_const(r, 0, 1);\n(void)measure_pure(r, 1, 1);\n");
        }
        _ => text.push_str(measure),
    }
    text.push_str(
        r#"FN(bq_free)(r);
FN(bq_free)(fault);
FN(__bennett_effect)((char)('A' + path));
switch (path) {
case 0: FN(__bennett_exit)((int)times); return;
case 1: FN(__bennett_out_of_memory)(); return;
case 2: FN(__bennett_bad_argument)(); return;
}
}
"#,
    );
    if helper.is_some() {
        writeln!(
            text,
            "void {entry}(unsigned path, unsigned times) {{ byte image[] = {{{}}}; body(path, times, image); }}",
            seed % 256,
        )
        .expect("write entry");
    }
    text
}

fn pass_free(source: &Path, flags: &[&str], destination: &Path) {
    let include = root().join("front/include");
    checked(
        Command::new(clang())
            .args(flags)
            .args(["-Xclang", "-disable-llvm-passes", "-emit-llvm", "-c"])
            .arg("-I")
            .arg(include)
            .arg(source)
            .arg("-o")
            .arg(destination),
    );
}

/// Count direct sites for the forms that run their body once and use no function pointers.
fn direct_calls(unit: &Path) -> [usize; 11] {
    let ir = checked(Command::new("llvm-dis-18").arg(unit).args(["-o", "-"]));
    let mut counts = [0; 11];
    for line in ir.lines().map(str::trim_start) {
        let opcode = line.split_once(" = ").map_or(line, |(_, instruction)| instruction);
        let opcode = match opcode.split_once(' ') {
            Some(("tail" | "musttail" | "notail", instruction)) => instruction,
            _ => opcode,
        };
        let Some(call) = opcode.strip_prefix("call ") else { continue };
        let Some((callee, _)) = call.split_once('(') else { continue };
        if let Some(name) = callee.split_whitespace().last().and_then(|name| name.strip_prefix('@'))
            && let Some(index) = PRIMITIVES.iter().position(|primitive| *primitive == name)
        {
            counts[index] += 1;
        }
    }
    counts
}

fn execute_units(dir: &Path, name: &str, runner: &Path, units: &[PathBuf]) -> String {
    let linked = dir.join(format!("{name}.bc"));
    let executable = dir.join(name);
    checked(Command::new("llvm-link-18").args(units).arg("-o").arg(&linked));
    checked(
        Command::new(clang())
            .args(["-O0", "-Xclang", "-disable-llvm-passes", "-I"])
            .arg(root().join("front/include"))
            .args([&linked, &c_dir().join("cc_pass_trace.c"), runner])
            .arg("-o")
            .arg(&executable),
    );
    checked(&mut Command::new(executable))
}

fn compare_traces(level: &str, expected: &str, actual: &str) {
    let mut actual = actual.lines();
    let mut case = "before first case";
    for want in expected.lines() {
        if want.starts_with("case ") {
            case = want;
        }
        assert_eq!(actual.next(), Some(want), "{level} {case}: first differing trace line");
    }
    assert_eq!(actual.next(), None, "{level} {case}: extra trace event");
}

/// Check the source's expected events against the pass-free backend before using it as a reference.
fn check_reference_trace(cases: &[(String, Shape, usize)], trace: &str) {
    let mut lines = trace.lines();
    for (name, shape, times) in cases {
        for path in 0..4 {
            let start = format!("case {name} path {path}");
            assert_eq!(lines.next(), Some(start.as_str()));
            let iterations = if *shape == Shape::Loop { times + path } else { 1 };
            let before = ["bq_alloc", "bq_alloc", "bq_width", "bq_gate", "bq_rotate", "bq_apply"];
            let measures = std::iter::repeat_n("bq_measure", 2 * iterations);
            let end = ["bq_free", "bq_free", "__bennett_effect"];
            let stop = STOPS.get(path).copied();
            for name in before.into_iter().chain(measures).chain(end).chain(stop) {
                let event = lines.next().and_then(|line| line.split_whitespace().next());
                assert_eq!(event, Some(name), "{start}: pass-free call sequence");
            }
        }
    }
    assert_eq!(lines.next(), None, "every pass-free event counted");
}

fn check_conforming(dir: &Path, level: &str, seeds: usize) {
    let mut declarations = String::from("void run_case(const char *, void (*)(unsigned, unsigned), unsigned);\n");
    let mut main = String::from("int main(void) {\n");
    let mut cases = Vec::new();
    let mut reference = Vec::new();
    let mut compiled = Vec::new();
    for (index, shape) in SHAPES.into_iter().enumerate() {
        for seed in 0..seeds {
            let entry = format!("probe_{index}_{seed}");
            let source = dir.join(format!("{entry}.c"));
            let baseline = dir.join(format!("{entry}.reference.bc"));
            let result = dir.join(format!("{entry}.driver.bc"));
            write(&source, &conforming(shape, &entry, seed));
            let options = [
                &[][..],
                &["-gline-tables-only"][..],
                &["-ffreestanding"][..],
                &["-fvisibility=hidden", "-fno-builtin"][..],
                &["-funsigned-char", "-g"][..],
                &["-fno-inline", "-fno-discard-value-names"][..],
            ];
            let flags: Vec<_> = std::iter::once(level).chain(options[seed % options.len()].iter().copied()).collect();
            pass_free(&source, &flags, &baseline);
            checked(driver().args(&flags).arg("-o").arg(&result).arg(&source));
            if matches!(shape, Shape::Direct | Shape::StaticHelper | Shape::InlineHelper | Shape::Restrict) {
                let counts = direct_calls(&baseline);
                assert_eq!(counts, [2, 2, 1, 2, 1, 1, 1, 1, 1, 1, 1], "{level} {entry}: pass-free direct calls");
                assert_eq!(direct_calls(&result), counts, "{level} {entry}: optimised direct calls");
            }
            let name = format!("{shape:?}-{seed}");
            let times = seed % 4;
            writeln!(declarations, "void {entry}(unsigned, unsigned);").expect("declare probe");
            writeln!(main, "run_case(\"{name}\", {entry}, {times});").expect("call probe");
            cases.push((name, shape, times));
            reference.push(baseline);
            compiled.push(result);
        }
    }
    let runner = dir.join("runner.c");
    write(&runner, &format!("{declarations}{main}return 0;\n}}\n"));
    let expected = execute_units(dir, "reference", &runner, &reference);
    check_reference_trace(&cases, &expected);
    let actual = execute_units(dir, "driver", &runner, &compiled);
    compare_traces(level, &expected, &actual);
}

fn check_refused(dir: &Path, level: &str, seeds: usize) {
    let source = dir.join("refusal.c");
    let bitcode = dir.join("refusal.bc");
    for seed in 0..seeds {
        let (name, prototype, arguments) =
            [("bq_width", "bq_reg r", "r"), ("bq_measure", "bq_reg r, uint64_t low, uint64_t bits", "r, 0, 1")]
                [seed % 2];
        let cases = [
            (
                "extern void bq_free(unsigned);\nvoid probe(void) { bq_free(0); }\n".into(),
                "conflicting types for 'bq_free'".into(),
                None,
                1,
            ),
            (
                format!(
                    "__attribute__((const)) uint64_t {name}({prototype});\n\
                         void probe(bq_reg r) {{ (void){name}({arguments}); }}\n"
                ),
                format!("a call of the device function @{name} has attributes"),
                None,
                2,
            ),
            (
                format!(
                    "void ordinary(unsigned) __asm__(\"bq_free\");\n\
                         void probe(void) {{ ordinary({seed}); }}\n"
                ),
                "a call of the device function @bq_free through the type".into(),
                None,
                2,
            ),
            (
                format!(
                    "static unsigned ordinary __asm__(\"{name}\") = {seed};\n\
                         unsigned probe(void) {{ return ordinary; }}\n"
                ),
                format!("a variable with the linker name of the primitive @{name}"),
                None,
                2,
            ),
            (
                format!(
                    "void ordinary(bq_reg) __asm__(\"\\x01\" \"bq_free\");\n\
                         void probe(void) {{ ordinary(bq_alloc({})); }}\n",
                    seed + 1
                ),
                "the linker name of the primitive @bq_free with an assembler label".into(),
                None,
                2,
            ),
            (
                format!("void probe(bq_reg r) {{ (void){name}({arguments}); }}\n"),
                format!("the header's declaration of the primitive {name} stands for the symbol"),
                Some(format!("-D{name}=renamed_probe")),
                2,
            ),
        ];
        for (text, expected, flag, status) in cases {
            let text = format!("#include \"bq.h\"\n{text}");
            write(&source, &text);
            write(&bitcode, "earlier output");
            let result = output(driver().arg(level).args(flag).arg("-o").arg(&bitcode).arg(&source));
            let diagnostic = String::from_utf8_lossy(&result.stderr);
            assert_eq!(result.status.code(), Some(status), "{level}: expected {expected}\n{text}\n{diagnostic}");
            assert!(diagnostic.contains(&expected), "{level}: expected {expected}\n{text}\n{diagnostic}");
            assert!(!bitcode.exists(), "{level}: refused unit left the earlier output\n{text}");
        }
    }
}

fn check(level: &str, sweep: &str, seeds: usize) {
    let dir = out_dir().join(format!("cc-pass-{sweep}-{level}"));
    fs::create_dir_all(&dir).expect("create test directory");
    check_refused(&dir, level, seeds);
    check_conforming(&dir, level, seeds);
}

macro_rules! check_level {
    ($name:ident, $level:literal) => {
        #[test]
        fn $name() {
            check($level, "quick", 2);
        }
    };
}
check_level!(driver_o0, "-O0");
check_level!(driver_o1, "-O1");
check_level!(driver_o2, "-O2");
check_level!(driver_o3, "-O3");
check_level!(driver_os, "-Os");
check_level!(driver_oz, "-Oz");

#[test]
#[ignore = "larger sweep: front/test.sh --test cc_pass -- larger_sweep --ignored --exact"]
fn larger_sweep() {
    for level in LEVELS {
        check(level, "sweep", 12);
    }
}

/// Clang 18 folds a const pointer to one of the stops to a direct call before optimisation, omitting
/// the call-site noreturn attribute even though the callee remains noreturn. The driver currently
/// refuses it with `no noreturn differs from the header's call`. The source pointer has the header's
/// C function type and adds no promises. The driver should accept it and keep the call.
#[test]
#[ignore = "wrong refusal: front/test.sh --test cc_pass -- const_pointers_to_the_stops_are_accepted --ignored --exact"]
fn const_pointers_to_the_stops_are_accepted() {
    let dir = out_dir().join("cc-pass-const-stop");
    fs::create_dir_all(&dir).expect("create witness directory");
    let mut failures = Vec::new();
    for (index, (name, argument)) in STOPS.into_iter().zip(["7", "", ""]).enumerate() {
        let source = dir.join(format!("{name}.c"));
        write(
            &source,
            &format!(
                "#include \"bq.h\"\nstatic __typeof__(&{name}) const stop = &{name};\n\
                 void probe(void) {{ stop({argument}); }}\n"
            ),
        );
        for level in LEVELS {
            let baseline = dir.join(format!("{name}.{level}.reference.bc"));
            let bitcode = dir.join(format!("{name}.{level}.driver.bc"));
            pass_free(&source, &[level], &baseline);
            let mut expected = [0; PRIMITIVES.len()];
            expected[PRIMITIVES.len() - STOPS.len() + index] = 1;
            assert_eq!(direct_calls(&baseline), expected, "{level} {name}: Clang folded the pointer");
            let result = output(driver().arg(level).arg("-o").arg(&bitcode).arg(&source));
            if result.status.success() {
                assert_eq!(direct_calls(&bitcode), expected, "{level} {name}: kept the call");
            } else {
                failures.push(format!(
                    "{level} {name}: {}\n{}",
                    result.status,
                    String::from_utf8_lossy(&result.stderr)
                ));
            }
        }
    }
    assert!(failures.is_empty(), "expected bitcode for all const-pointer calls:\n{}", failures.join("\n"));
}
