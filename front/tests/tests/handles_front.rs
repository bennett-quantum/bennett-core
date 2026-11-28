//! The declaration check of the handle interface (`docs/HANDLES.md`, section 4; `docs/DESIGN.md`, "The declaration
//! check"): the driver `bennett-cc` checks each unit's LLVM IR before any optimiser pass against the baseline,
//! removes the claims of calls through a pointer, optimises, and checks redirection and call shape again;
//! `bennett-front` removes the same claims, checks redirection and shape on every module, and refuses the interface
//! in bitcode that did not come from `bennett-cc`. Every refusal of a C program is checked at `-O0` and at `-O2`,
//! and the facts about clang 18 the check rests on are pinned here, so that a clang that behaves otherwise fails.
use bennett_exec::{Executor, Outcome};
use bennett_front_tests::{CLANG_FLAGS, CLANG_O2_FLAGS, clang, entry, front, options, out_dir, root, write_c};
use bennett_ir::Fault;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The two optimisation levels, as the other front-end tests compile C.
const LEVELS: [(&str, &[&str]); 2] = [("O0", CLANG_FLAGS), ("O2", CLANG_O2_FLAGS)];
/// The status of a refusal of input that breaks a rule (`misuse`), and of one that waits on work not done
/// yet (`notHandled`), as `front/src/Diagnostic.h` gives them.
const REFUSED: i32 = 2;
const NOT_HANDLED: i32 = 3;

fn header() -> PathBuf {
    root().join("front/include/bq.h")
}

fn references() -> PathBuf {
    root().join("front/include/bq-references.h")
}

fn driver() -> PathBuf {
    front().with_file_name("bennett-cc")
}

fn output(command: &mut Command) -> Output {
    command.output().unwrap_or_else(|e| panic!("cannot run {command:?}: {e}"))
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {path:?}: {e}"))
}

/// `bennett-cc` on `source` with `flags`: the documented compilation of a unit. The output, if any, is
/// `<name>.cc.<extension>` (`ll` writes LLVM text), removed before the run.
fn driver_to(source: &Path, name: &str, flags: &[&str], extension: &str) -> (Output, PathBuf) {
    let compiled = out_dir().join(format!("{name}.cc.{extension}"));
    let _ = std::fs::remove_file(&compiled);
    let result = output(Command::new(driver()).args(flags).arg("-o").arg(&compiled).arg(source));
    (result, compiled)
}

fn driver_on(source: &Path, name: &str, flags: &[&str]) -> (Output, PathBuf) {
    driver_to(source, name, flags, "bc")
}

/// `bennett-cc`'s output as LLVM text; the unit must pass.
fn driver_text(source: &Path, name: &str, flags: &[&str]) -> String {
    let (result, compiled) = driver_to(source, name, flags, "ll");
    assert!(result.status.success(), "{name}: {}", stderr(&result));
    read(&compiled)
}

/// `bennett-front` on a file of bitcode or LLVM text.
fn front_of(input: &Path, name: &str) -> Output {
    output(
        Command::new(front())
            .arg(input)
            .arg("-o")
            .arg(out_dir().join(format!("{name}.bn")))
            .args(["--region-budget=64", "--stack-bytes=4096"]),
    )
}

fn write_text(text: &str, name: &str) -> PathBuf {
    let path = out_dir().join(format!("{name}.ll"));
    std::fs::write(&path, text).unwrap_or_else(|e| panic!("cannot write {path:?}: {e}"));
    path
}

/// `bennett-front` on LLVM text written to `<name>.ll`.
fn front_on_text(text: &str, name: &str) -> Output {
    front_of(&write_text(text, name), name)
}

/// Plain clang on `source` with `flags`, the header included by force: `<name>.plain.ll`, LLVM text. With
/// `pass_free`, every LLVM pass is disabled and the array of references is included too, as `bennett-cc` compiles.
fn plain_clang(source: &Path, name: &str, flags: &[&str], pass_free: bool) -> Output {
    let text = out_dir().join(format!("{name}.plain.ll"));
    let _ = std::fs::remove_file(&text);
    let mut command = Command::new(clang());
    command.arg("-include").arg(header());
    if pass_free {
        command.arg("-include").arg(references()).args(["-Xclang", "-disable-llvm-passes"]);
    }
    output(command.args(flags).args(["-S", "-emit-llvm"]).arg(source).arg("-o").arg(&text))
}

fn plain_text(source: &Path, name: &str, flags: &[&str], pass_free: bool) -> String {
    let result = plain_clang(source, name, flags, pass_free);
    assert!(result.status.success(), "{name}: {}", stderr(&result));
    read(&out_dir().join(format!("{name}.plain.ll")))
}

/// `bennett-front` on a file of bitcode or LLVM text, then a run of `main`: the outcome and the console.
fn run_lowered(input: &Path, name: &str) -> (Outcome, Vec<u8>) {
    let lowered = front_of(input, name);
    assert!(lowered.status.success(), "{name}: {}", stderr(&lowered));
    let path = out_dir().join(format!("{name}.bn"));
    let module = bennett_ir::load(&read(&path)).unwrap_or_else(|e| panic!("{name}: invalid device program: {e}"));
    let mut executor = Executor::new(&module, entry(&module, "main"), &[], options(0)).expect("starts");
    let outcome = executor.run();
    (outcome, executor.tape().to_vec())
}

/// The documented path, `bennett-cc` then `bennett-front`, then a run of `main`: the outcome and the console.
fn run_through_driver(source: &Path, name: &str, flags: &[&str]) -> (Outcome, Vec<u8>) {
    let (compiled, bitcode) = driver_on(source, name, flags);
    assert!(compiled.status.success(), "{name}: {}", stderr(&compiled));
    run_lowered(&bitcode, name)
}

fn assert_refused(result: &Output, status: i32, expected: &str, context: &str) {
    let text = stderr(result);
    assert_eq!(result.status.code(), Some(status), "{context}: {text}");
    assert!(text.contains(expected), "{context}: expected `{expected}` in: {text}");
}

/// A refusal by `bennett-cc` at both levels, with no output written.
fn assert_driver_refuses(case: &str, text: &str, extra: &[&str], expected: &str) {
    let source = write_c(&format!("handles_cc_{case}"), text);
    for (level, flags) in LEVELS {
        let name = format!("handles_cc_{case}.{level}");
        let flags: Vec<&str> = flags.iter().chain(extra).copied().collect();
        let (result, compiled) = driver_on(&source, &name, &flags);
        assert_refused(&result, REFUSED, expected, &format!("{name}:\n{text}"));
        assert!(!compiled.exists(), "{name}: a refused unit has no output");
    }
}

/// A program in the handle interface that keeps the declaration rule.
const CONFORMING: &str = r#"
static void flip(void *block) { unsigned char *b = block; b[1] = b[0] ^ 1; }
int main(void) {
    bq_reg r = bq_alloc(2);
    bq_bit control = bq_bit_of(r, 1);
    bq_gate(BQ_GATE_H, r, 1, 0, 0);
    bq_gate(BQ_GATE_X, r, 0, &control, 1);
    bq_rotate(BQ_ROT_PHASE, 0.5, r, 0, 0, 0);
    bq_reg out = bq_alloc(1);
    unsigned char image[2] = {0, 0};
    bq_field fields[2] = { bq_field_of(bq_slice_of(r, 0, 1), 0, 1, BQ_DIR_IN),
                           bq_field_of(bq_slice_of(out, 0, 1), 1, 1, BQ_DIR_OUT) };
    bq_reg fault = bq_fault_alloc();
    bq_apply(flip, 100, fault, image, sizeof image, fields, 2, 0, 0);
    uint64_t code = bq_fault_take(fault);
    uint64_t m = bq_measure(r, 0, bq_width(r)) + bq_measure(out, 0, 1);
    bq_free(out);
    bq_free(r);
    if (code) __bennett_bad_argument();
    __bennett_effect('0' + (char)m);
    __bennett_exit(0);
}
"#;

const NOT_YET: &str = "translating its calls into device instructions waits";

/// A conforming program passes `bennett-cc`; `bennett-front` then stops, by name, at the translation of its calls,
/// which waits on the device forms.
#[test]
fn a_conforming_program_passes_the_check() {
    let source = write_c("handles_conforming", CONFORMING);
    for (level, flags) in LEVELS {
        let name = format!("handles_conforming.{level}");
        let (unit, bitcode) = driver_on(&source, &name, flags);
        assert!(unit.status.success(), "{name}: {}", stderr(&unit));
        assert_refused(&front_of(&bitcode, &name), NOT_HANDLED, NOT_YET, &name);
    }
}

/// The facts about clang 18 that make the check see every rename: a declaration's symbol is fixed at its first
/// use, which the forced array of references makes come before the unit's own text. After it, `#pragma
/// redefine_extname`, `#pragma weak` and a `weak` redeclaration have no effect on the IR (the calls stay on the
/// external `@bq_free`), and an assembler label is clang's own error. With them, the programs mean what they say:
/// `bennett-cc` accepts the first three, and its output calls the primitive.
#[test]
fn clang_fixes_a_declarations_symbol_at_its_first_use() {
    let programs = [
        ("pragma_rename", "#pragma redefine_extname bq_free ordinary_release\n"),
        ("pragma_weak", "#pragma weak bq_free\n"),
        ("weak_redeclaration", "__attribute__((weak)) void bq_free(bq_reg);\n"),
    ];
    for (case, line) in programs {
        let text = format!("{line}int main(void) {{ bq_reg r = bq_alloc(1); bq_free(r); return 7; }}\n");
        let source = write_c(&format!("handles_first_use_{case}"), &text);
        for (level, flags) in LEVELS {
            let name = format!("handles_first_use_{case}.{level}");
            let ir = plain_text(&source, &name, flags, true);
            let declaration = ir.lines().find(|l| l.starts_with("declare") && l.contains("@bq_free("));
            let external = declaration.is_some_and(|l| l.contains(" void @bq_free(i64) #") && !l.contains("weak"));
            assert!(external, "{name}: the declaration is external:\n{ir}");
            assert!(ir.contains("call void @bq_free(i64"), "{name}: the call is the primitive's:\n{ir}");
            assert!(!ir.contains("ordinary_release"), "{name}: the rename took effect:\n{ir}");
            let compiled = driver_text(&source, &name, flags);
            assert!(compiled.contains("call void @bq_free(i64"), "{name}:\n{compiled}");
        }
    }
    let late_label = write_c(
        "handles_first_use_late_label",
        "void bq_free(bq_reg) __asm__(\"ordinary_release\");\n\
         int main(void) { bq_reg r = bq_alloc(1); bq_free(r); return 7; }\n",
    );
    for (level, flags) in LEVELS {
        let name = format!("handles_first_use_late_label.{level}");
        let plain = plain_clang(&late_label, &name, flags, true);
        assert!(stderr(&plain).contains("cannot apply asm label to function after its first use"), "{name}");
        let (result, compiled) = driver_on(&late_label, &name, flags);
        assert_eq!(result.status.code(), plain.status.code(), "{name}: {}", stderr(&result));
        assert!(stderr(&result).contains("cannot apply asm label"), "{name}: {}", stderr(&result));
        assert!(!compiled.exists(), "{name}");
    }
}

/// Rule 2 by kind: a C program that gives another global a primitive's linker name, or renames the header's
/// declaration, is refused by `bennett-cc` by name at both levels.
#[test]
fn another_global_with_a_primitives_name_is_refused_by_kind() {
    let cases: &[(&str, &str, &[&str], &str)] = &[
        (
            "definition",
            "void __bennett_effect(char c) { (void)c; }\nint main(void) { __bennett_effect('A'); return 7; }",
            &[],
            "the program defines the primitive @__bennett_effect",
        ),
        (
            "label_definition",
            "void my_effect(char c) __asm__(\"__bennett_effect\");\nvoid my_effect(char c) { (void)c; }\n\
             int main(void) { __bennett_effect('A'); return 7; }",
            &[],
            "the program defines the primitive @__bennett_effect",
        ),
        (
            "alias",
            "static void repl(void) { __bennett_exit(7); }\n\
             void __bennett_out_of_memory(void) __attribute__((alias(\"repl\")));\n\
             int main(void) { __bennett_out_of_memory(); }",
            &[],
            "the primitive @__bennett_out_of_memory is defined by an alias",
        ),
        (
            "ifunc",
            "static void f(void) {}\nstatic void *resolve(void) { return (void *)f; }\n\
             void __bennett_out_of_memory(void) __attribute__((ifunc(\"resolve\")));\n\
             int main(void) { __bennett_out_of_memory(); }",
            &[],
            "(an ifunc) named after the primitive @__bennett_out_of_memory",
        ),
        (
            "static_variable",
            "static int bq_alloc_copy __asm__(\"bq_alloc\") = 3;\nint main(void) { return bq_alloc_copy; }",
            &[],
            "a variable with the linker name of the primitive @bq_alloc",
        ),
        (
            "undecorated_label",
            "void rel(bq_reg) __asm__(\"\\x01\" \"bq_free\");\n\
             int main(void) { bq_reg r = bq_alloc(1); rel(r); return 7; }",
            &[],
            "gives a symbol the linker name of the primitive @bq_free with an assembler label",
        ),
        (
            "macro_rename",
            "int main(void) { bq_reg r = bq_alloc(1); bq_free(r); return 7; }",
            &["-Dbq_free=other_release"],
            "the header's declaration of the primitive bq_free stands for the symbol `other_release`",
        ),
    ];
    for (case, text, extra, expected) in cases {
        assert_driver_refuses(case, text, extra, expected);
    }
}

/// Rules 3 and 4: a redeclaration that tells LLVM something about a primitive, or calls it with another type or
/// calling convention, is refused by name at both levels. clang 18 keeps the header's attributes on the
/// declaration (it was emitted at the array) and puts the later declaration's on the calls.
#[test]
fn a_call_that_is_not_the_headers_call_is_refused() {
    let cases: &[(&str, &str, &str)] = &[
        (
            "const_redeclaration",
            "__attribute__((const)) uint64_t bq_width(bq_reg);\n\
             int main(void) { bq_reg r = bq_alloc(1); bq_width(r); return 7; }",
            "a call of the device function @bq_width has attributes that let LLVM remove or move its calls",
        ),
        (
            "const_label",
            "__attribute__((const)) uint64_t width2(bq_reg) __asm__(\"bq_width\");\n\
             int main(void) { bq_reg r = bq_alloc(1); width2(r); return 7; }",
            "a call of the device function @bq_width has attributes that let LLVM remove or move its calls",
        ),
        (
            "noreturn_redeclaration",
            "__attribute__((noreturn)) void bq_free(bq_reg);\nint main(void) { bq_free(bq_alloc(1)); }",
            "a call of the device function @bq_free has attributes that let LLVM remove or move its calls",
        ),
        (
            "other_type",
            "void rel(int) __asm__(\"bq_free\");\nint main(void) { rel(1); return 7; }",
            "a call of the device function @bq_free through the type void (i32)",
        ),
        (
            "ms_abi",
            "__attribute__((ms_abi)) void rel(bq_reg) __asm__(\"bq_free\");\n\
             int main(void) { bq_reg r = bq_alloc(1); rel(r); return 7; }",
            "a call of the device function @bq_free uses the calling convention win64cc",
        ),
        // Review finding R12 of the earlier step: a declaration in an evaluated array bound, `const`, whose call
        // clang -O2 would drop.
        (
            "declaration_in_an_evaluated_bound",
            "int main(void) {\n    volatile int n = 1;\n    int a[({\n        \
             extern bq_reg make(uint64_t) __asm__(\"bq_alloc\") __attribute__((const));\n        \
             (void)make(1);\n        n;\n    })];\n    return sizeof a == sizeof(int) ? 7 : 1;\n}\n",
            "a call of the device function @bq_alloc has attributes that let LLVM remove or move its calls",
        ),
    ];
    for (case, text, expected) in cases {
        assert_driver_refuses(case, text, &[], expected);
    }
}

/// A declaration that emits nothing cannot change any call, and is accepted: an unused redeclaration or label, one
/// at block scope, one only inside the text of a type (review finding R12 of the earlier step, an unevaluated
/// array bound and an initializer a later one overrides), and a label that names the same primitive with the same
/// type, which makes the header's call. Each runs to its end.
#[test]
fn declarations_that_emit_nothing_are_accepted() {
    let programs = [
        ("unused_redeclaration", "void __bennett_effect(char);\nint main(void) { return 7; }"),
        ("unused_label", "extern void release(bq_reg) __asm__(\"bq_free\");\nint main(void) { return 7; }"),
        ("unused_variable_label", "extern int status __asm__(\"__bennett_exit\");\nint main(void) { return 7; }"),
        ("block_scope_label", "int main(void) { extern void quit(int) __asm__(\"__bennett_exit\"); return 7; }"),
        (
            "type_only_bound",
            "int main(void) {\n  int a[({ extern void release(bq_reg) __asm__(\"bq_free\"); 1; })];\n  \
             return sizeof a == sizeof(int) ? 7 : 1;\n}\n",
        ),
        (
            "type_only_initializer",
            "int main(void) {\n  int a[2] = { [0] = ({ extern void release(bq_reg) __asm__(\"bq_free\"); 1; }), \
             [0] = 7 };\n  return a[0];\n}\n",
        ),
        (
            "same_label_same_type",
            "void say(char) __asm__(\"__bennett_effect\");\nint main(void) { say('A'); return 7; }",
        ),
    ];
    for (case, text) in programs {
        let source = write_c(&format!("handles_nothing_{case}"), text);
        for (level, flags) in LEVELS {
            let name = format!("handles_nothing_{case}.{level}");
            let (outcome, tape) = run_through_driver(&source, &name, flags);
            assert_eq!(outcome, Outcome::Returned(Some(7)), "{name}");
            let expected: &[u8] = if case == "same_label_same_type" { b"A" } else { b"" };
            assert_eq!(tape, expected, "{name}");
        }
    }
}

/// A unit that defines the guard of the header or of the array: without the header, clang's own error; without the
/// array, refused by name.
#[test]
fn a_unit_without_the_forced_includes_is_refused() {
    let source = write_c("handles_no_header", "int main(void) { return 7; }\n");
    for (level, flags) in LEVELS {
        let name = format!("handles_no_header.{level}");
        let guarded: Vec<&str> = flags.iter().copied().chain(["-DBENNETT_BQ_H_INCLUDED"]).collect();
        let (result, compiled) = driver_on(&source, &name, &guarded);
        assert!(!result.status.success() && !compiled.exists(), "{name}: {}", stderr(&result));
        assert!(stderr(&result).contains("bq_alloc"), "{name}: {}", stderr(&result));
        let guarded: Vec<&str> = flags.iter().copied().chain(["-DBENNETT_BQ_REFERENCES_H_INCLUDED"]).collect();
        let (result, compiled) = driver_on(&source, &name, &guarded);
        assert_refused(&result, REFUSED, "the unit lacks bennett-cc's array of the primitives' addresses", &name);
        assert!(!compiled.exists(), "{name}");
    }
}

/// Rule 5 and the facts it rests on (`q6` of the check's record): clang 18 puts a pointer's `const` on the call
/// through it, and plain `clang -O2` then deletes both measurements, the one through a static pointer it resolves
/// and the one through a `volatile` pointer it loads but does not call. After
/// `bennett-cc` removes the claims before optimising, both measurements stay at both levels.
#[test]
fn a_pointers_claim_cannot_remove_a_measurement() {
    let text = "__attribute__((const)) static uint64_t (*m)(bq_reg, uint64_t, uint64_t) = bq_measure;\n\
                __attribute__((const)) uint64_t (*volatile mv)(bq_reg, uint64_t, uint64_t) = bq_measure;\n\
                int main(void) {\n  bq_reg r = bq_alloc(1);\n  (void)m(r, 0, 1);\n  (void)mv(r, 0, 1);\n  \
                return 7;\n}\n";
    let source = write_c("handles_pointer_claims", text);
    let measurements =
        |ir: &str| ir.lines().filter(|l| l.contains("call i64 @bq_measure(") || l.contains("call i64 %")).count();
    for (level, flags) in LEVELS {
        let name = format!("handles_pointer_claims.{level}");
        let pass_free = plain_text(&source, &format!("{name}.pass_free"), flags, true);
        let claimed = pass_free.lines().filter(|l| l.contains("call i64 %")).count();
        assert_eq!(claimed, 2, "{name}: two calls through a pointer before optimisation:\n{pass_free}");
        assert!(pass_free.contains("memory(none)"), "{name}: the pointer's claim is on its calls:\n{pass_free}");
        let compiled = driver_text(&source, &name, flags);
        assert_eq!(measurements(&compiled), 2, "{name}:\n{compiled}");
        assert!(!compiled.contains("memory(none) }"), "{name}: a claim survived:\n{compiled}");
    }
    let plain = plain_text(&source, "handles_pointer_claims.plain", CLANG_O2_FLAGS, false);
    assert_eq!(measurements(&plain), 0, "plain clang -O2 deletes both:\n{plain}");
}

/// `q2` of the check's record: a `const` or `pure` pointer declared `const` is folded by clang 18 into a direct
/// call that carries the claim, before any pass, at both levels; plain `clang -O2` then deletes one measurement
/// and merges two. Through `bennett-cc` that call is a direct call with a claim, refused by name (rule 4).
#[test]
fn a_claim_folded_into_a_direct_call_is_refused() {
    let text = "__attribute__((const)) uint64_t (*const measure)(bq_reg, uint64_t, uint64_t) = bq_measure;\n\
                __attribute__((pure)) uint64_t (*const measure2)(bq_reg, uint64_t, uint64_t) = bq_measure;\n\
                int main(void) {\n  bq_reg r = bq_alloc(1);\n  (void)measure(r, 0, 1);\n  \
                uint64_t a = measure2(r, 0, 1), b = measure2(r, 0, 1);\n  return (int)(a + b);\n}\n";
    let source = write_c("handles_folded_claims", text);
    let measurements = |ir: &str| ir.lines().filter(|l| l.contains("call i64 @bq_measure(")).count();
    for (level, flags) in LEVELS {
        let name = format!("handles_folded_claims.{level}");
        let pass_free = plain_text(&source, &format!("{name}.pass_free"), flags, true);
        assert_eq!(measurements(&pass_free), 3, "{name}: folded into direct calls:\n{pass_free}");
        assert!(pass_free.contains("memory(none)") && pass_free.contains("memory(read)"), "{name}:\n{pass_free}");
        let (result, compiled) = driver_on(&source, &name, flags);
        assert_refused(
            &result,
            REFUSED,
            "a call of the device function @bq_measure has attributes that let LLVM remove or move its calls",
            &name,
        );
        assert!(!compiled.exists(), "{name}");
    }
    let plain = plain_text(&source, "handles_folded_claims.plain", CLANG_O2_FLAGS, false);
    assert_eq!(measurements(&plain), 1, "plain clang -O2 keeps one of the three:\n{plain}");
}

/// Rule 5 on the device: calls through pointers reach the primitive and only it. A `const` pointer to the
/// out-of-memory stop faults instead of being deleted (lane 17's witness R3 written in C), an ordinary pointer to
/// the output or exit primitive prints or exits, a callback of the output primitive's type that is the program's
/// own, `noreturn`, runs; a `noreturn` pointer that reaches the output primitive faults `unreachable` after it, and
/// an unrelated callback with a structure passed by value (`byval`) is untouched.
#[test]
fn calls_through_pointers_reach_the_primitive() {
    let programs: &[(&str, &str, Outcome, &[u8])] = &[
        (
            "const_out_of_memory",
            "__attribute__((const)) static void (*volatile stop)(void) = __bennett_out_of_memory;\n\
             int main(void) { __bennett_effect('A'); stop(); return 7; }\n",
            Outcome::Faulted(Fault::OutOfMemory),
            b"A",
        ),
        (
            "output_pointer",
            "static void (*volatile output)(char) = __bennett_effect;\n\
             int main(void) { output('A'); output('B'); __bennett_effect('C'); return 0; }\n",
            Outcome::Returned(Some(0)),
            b"ABC",
        ),
        (
            "exit_pointer",
            "static void (*volatile quit)(int) = __bennett_exit;\n\
             int main(void) { __bennett_effect('A'); quit(3); __bennett_effect('B'); return 0; }\n",
            Outcome::Returned(Some(3)),
            b"A",
        ),
        (
            "same_type_callback",
            "typedef void (* __attribute__((noreturn)) stop_fn)(char);\n\
             static void stop(char c) __attribute__((noreturn));\nstatic void stop(char c) { __bennett_exit(c); }\n\
             static stop_fn volatile finish = stop;\nstatic void (*volatile output)(char) = __bennett_effect;\n\
             int main(void) { output('A'); finish(7); }\n",
            Outcome::Returned(Some(7)),
            b"A",
        ),
        (
            "noreturn_pointer",
            "typedef void (*__attribute__((noreturn)) never)(char);\n\
             static never volatile target = (never)__bennett_effect;\nint main(void) { target('A'); }\n",
            Outcome::Faulted(Fault::Unreachable),
            b"A",
        ),
        (
            "byval_callback",
            "struct packet { long x[3]; };\nint first(struct packet p) { return p.x[0]; }\n\
             int (*volatile reader)(struct packet) = first;\nint main(void) {\n\
             void (*volatile unused)(char) = __bennett_effect; (void)unused;\n\
             return reader((struct packet){{7}});\n}\n",
            Outcome::Returned(Some(7)),
            b"",
        ),
    ];
    for (case, text, outcome, console) in programs {
        let source = write_c(&format!("handles_pointer_{case}"), text);
        for (level, flags) in LEVELS {
            let name = format!("handles_pointer_{case}.{level}");
            let (got, tape) = run_through_driver(&source, &name, flags);
            assert_eq!(&got, outcome, "{name}");
            assert_eq!(&tape, console, "{name}");
        }
    }
}

/// Rule 7 on LLVM text: `bennett-front` removes the claims of every call through a pointer, in every module, so a
/// `noreturn` call through a pointer reaches the primitive and faults `unreachable`, a `fastcc` one runs, and one
/// that claims `memory(none)` is not deleted.
#[test]
fn the_front_removes_pointer_claims_from_every_module() {
    let indirect = |call: &str| {
        format!(
            "declare void @__bennett_effect(i8)\n@target = internal constant ptr @__bennett_effect\n\
             define i32 @main() {{\n  %callee = load ptr, ptr @target\n  {call}\n  ret i32 7\n}}\n"
        )
    };
    let texts = [
        ("noreturn", indirect("call void %callee(i8 65) noreturn"), Outcome::Faulted(Fault::Unreachable)),
        ("fastcc", indirect("call fastcc void %callee(i8 65)"), Outcome::Returned(Some(7))),
        (
            "memory_none",
            indirect("call void %callee(i8 65) nounwind willreturn memory(none)"),
            Outcome::Returned(Some(7)),
        ),
    ];
    for (case, text, outcome) in texts {
        let name = format!("handles_front_pointer_{case}");
        let (got, tape) = run_lowered(&write_text(&text, &name), &name);
        assert_eq!(got, outcome, "{name}");
        assert_eq!(tape, b"A", "{name}");
    }
}

/// Rule 6 in `bennett-front` on every module, whether or not it uses the interface: an alias, an ifunc, a variable,
/// a definition or an assembler label under a primitive's name, a declaration of other than external linkage, and a
/// call of another type, each refused by name; and the interface in bitcode that did not come from `bennett-cc`.
#[test]
fn the_front_checks_redirection_and_shape_on_every_module() {
    let main = "define i32 @main() {\n  ret i32 7\n}\n";
    let cases = [
        (
            "alias",
            format!("@bq_free = alias void (i64), ptr @x\ndefine internal void @x(i64 %r) {{\n  ret void\n}}\n{main}"),
            "the primitive @bq_free is defined by an alias",
        ),
        (
            "ifunc",
            format!(
                "@__bennett_effect = ifunc void (i8), ptr @resolve\ndefine internal ptr @resolve() {{\n  ret ptr null\n}}\n{main}"
            ),
            "(an ifunc) named after the primitive @__bennett_effect",
        ),
        ("variable", format!("@bq_width = global i32 0\n{main}"), "a variable with the linker name of the primitive"),
        (
            "definition",
            format!("define void @__bennett_bad_argument() {{\n  ret void\n}}\n{main}"),
            "the program defines the primitive @__bennett_bad_argument",
        ),
        (
            "label",
            format!("declare void @\"\\01bq_free\"(i64)\n{main}"),
            "gives a symbol the linker name of the primitive @bq_free with an assembler label",
        ),
        (
            "weak",
            format!("declare extern_weak void @__bennett_effect(i8)\n{main}"),
            "the primitive @__bennett_effect is declared weak",
        ),
        (
            "other_type",
            "declare void @__bennett_effect(i8)\ndefine i32 @main() {\n  call void @__bennett_effect(i32 1)\n  \
             ret i32 7\n}\n"
                .to_string(),
            "a call of the device function @__bennett_effect through the type void (i32)",
        ),
        (
            "unmarked",
            "declare i64 @bq_alloc(i64)\ndefine i32 @main() {\n  %r = call i64 @bq_alloc(i64 1)\n  ret i32 7\n}\n"
                .to_string(),
            "the program uses the handle interface (@bq_alloc) in bitcode that did not come from bennett-cc",
        ),
    ];
    for (case, text, expected) in cases {
        let name = format!("handles_front_{case}");
        assert_refused(&front_on_text(&text, &name), REFUSED, expected, &format!("{name}:\n{text}"));
    }
    // Compiled by clang alone, a program using the interface is refused the same way.
    let source = write_c("handles_front_clang_alone", "int main(void) { bq_free(bq_alloc(1)); return 7; }\n");
    for (level, flags) in LEVELS {
        let name = format!("handles_front_clang_alone.{level}");
        let bitcode = out_dir().join(format!("{name}.bc"));
        let compiled = output(
            Command::new(clang())
                .arg("-include")
                .arg(header())
                .args(flags)
                .args(["-emit-llvm", "-c"])
                .arg(&source)
                .arg("-o")
                .arg(&bitcode),
        );
        assert!(compiled.status.success(), "{name}: {}", stderr(&compiled));
        assert_refused(&front_of(&bitcode, &name), REFUSED, "did not come from bennett-cc", &name);
    }
}

/// A declaration nothing uses is no use of either interface: an ordinary module with an unused interface
/// declaration and an unused marker declaration is translated and returns 7.
#[test]
fn an_unused_declaration_is_no_use_of_an_interface() {
    let path = write_text(
        "declare void @bq_free(i64)\ndeclare void @bennett_h(ptr)\ndefine i32 @main() {\n  ret i32 7\n}\n",
        "handles_unused_declarations",
    );
    let (outcome, _) = run_lowered(&path, "handles_unused_declarations");
    assert_eq!(outcome, Outcome::Returned(Some(7)));
}

/// A program that uses a marker and an interface function is refused by name: the two interfaces are not mixed
/// (a choice that is cheap to reverse; `docs/DESIGN.md`, "Not yet").
#[test]
fn a_program_with_markers_and_handles_is_refused() {
    let source = write_c(
        "handles_mixed",
        "void bennett_h(_Bool *qubit);\nint main(void) { _Bool q = 0; bennett_h(&q); bq_free(bq_alloc(1)); \
         return q; }",
    );
    for (level, flags) in LEVELS {
        let name = format!("handles_mixed.{level}");
        let (unit, bitcode) = driver_on(&source, &name, flags);
        assert!(unit.status.success(), "{name}: {}", stderr(&unit));
        assert_refused(
            &front_of(&bitcode, &name),
            REFUSED,
            "the program uses both the marker interface (@bennett_h",
            &name,
        );
    }
}

/// Rule 7: `bennett-cc` takes LLVM IR as a unit and applies rules 2 to 6 without the array. The pass-free IR clang
/// writes for a conforming unit passes; the same IR with a claim on a call of a primitive is refused.
#[test]
fn llvm_ir_units_get_the_same_check() {
    let source = write_c("handles_ir_unit", "int main(void) { __bennett_effect('A'); return 7; }\n");
    for (level, flags) in LEVELS {
        let name = format!("handles_ir_unit.{level}");
        let mut command = Command::new(clang());
        let ir = out_dir().join(format!("{name}.ll"));
        let written = output(
            command
                .arg("-include")
                .arg(header())
                .args(flags)
                .args(["-S", "-emit-llvm", "-Xclang", "-disable-llvm-passes"])
                .arg(&source)
                .arg("-o")
                .arg(&ir),
        );
        assert!(written.status.success(), "{name}: {}", stderr(&written));
        let (outcome, tape) = run_through_driver(&ir, &name, flags);
        assert_eq!((outcome, tape), (Outcome::Returned(Some(7)), b"A".to_vec()), "{name}");
        let text = read(&ir);
        let call = "call void @__bennett_effect(i8 noundef signext 65)";
        assert!(text.contains(call), "{name}:\n{text}");
        let claimed = text.replace(call, &format!("{call} memory(none)"));
        let claimed_path = write_text(&claimed, &format!("{name}.claimed"));
        let (result, compiled) = driver_on(&claimed_path, &format!("{name}.claimed"), flags);
        assert_refused(&result, REFUSED, "a call of the device function @__bennett_effect has attributes", &name);
        assert!(!compiled.exists(), "{name}");
    }
    let reserved = [
        (
            "callback_metadata",
            "declare !callback !0 void @bq_apply(ptr, i64, i64, ptr, i64, ptr, i64, ptr, i64)\n\
             define i32 @main() {\n  ret i32 7\n}\n!0 = !{!1}\n!1 = !{i64 0, i64 3, i1 false}\n",
            "the declaration of the primitive @bq_apply is `",
        ),
        (
            "fastcc_declaration",
            "declare fastcc void @__bennett_effect(i8)\ndefine i32 @main() {\n  ret i32 7\n}\n",
            "the declaration of the primitive @__bennett_effect is `declare fastcc",
        ),
    ];
    for (case, text, expected) in reserved {
        let name = format!("handles_ir_{case}");
        let (result, _) = driver_on(&write_text(text, &name), &name, CLANG_FLAGS);
        assert_refused(&result, REFUSED, expected, &name);
    }
}

/// Options for which clang 18 puts code-generation attributes, metadata or value facts on the header's
/// declarations and their calls (`optsize`, `minsize`, `nobuiltin`, `"no-builtins"`, `!kcfi_type`, `!dbg`, the
/// target strings, ...): the baseline is compiled with the same options, so they agree.
fn codegen_options() -> Vec<Vec<&'static str>> {
    let mut options = vec![
        vec!["-Os"],
        vec!["-Oz"],
        vec!["-O1"],
        vec!["-O3"],
        vec!["-O2", "-ffast-math"],
        vec!["-O2", "-ffp-model=strict"],
        vec!["-O2", "-fno-builtin"],
        vec!["-O2", "-ffreestanding"],
        vec!["-O2", "-fno-plt"],
        vec!["-O2", "-fPIC"],
        vec!["-O2", "-fno-pic"],
        vec!["-O2", "-funsigned-char"],
        vec!["-O2", "-g"],
        vec!["-O2", "-fstack-protector-all"],
        vec!["-O2", "-fzero-call-used-regs=all"],
        vec!["-O2", "-fsplit-stack"],
        vec!["-O2", "-fsanitize=kcfi"],
    ];
    if cfg!(target_arch = "x86_64") {
        options.extend([
            vec!["-O2", "-mno-red-zone"],
            vec!["-O2", "-mspeculative-load-hardening"],
            vec!["-O2", "-mstackrealign"],
        ]);
    }
    options
}

/// Review finding R13 of the earlier step: the conforming program passes under each option (the front stops at the
/// translation of its calls); with `#pragma STDC FENV_ACCESS ON` (`strictfp` on its calls) and the statement
/// attributes `nomerge`, `noinline` and `always_inline` on calls, at both levels; and a program of the output and
/// exit primitives runs to its end.
#[test]
fn the_attributes_clang_adds_for_an_option_are_accepted() {
    let conforming = write_c("handles_options_conforming", CONFORMING);
    let strict = write_c("handles_options_strict", &format!("#pragma STDC FENV_ACCESS ON\n{CONFORMING}"));
    let effects = write_c("handles_options_effects", "int main(void) { __bennett_effect('A'); return 7; }\n");
    let contexts = write_c(
        "handles_options_contexts",
        "#pragma STDC FENV_ACCESS ON\nint main(void) {\n  __bennett_effect('A');\n  \
         [[clang::nomerge]] __bennett_effect('B');\n  [[clang::noinline]] __bennett_effect('C');\n  \
         [[clang::always_inline]] __bennett_effect('D');\n#pragma clang loop vectorize(assume_safety)\n  \
         for (int i = 0; i < 1; ++i) __bennett_effect('E');\n  __bennett_exit(7);\n}\n",
    );
    let mut runs: Vec<(PathBuf, String, Vec<&str>, &[u8])> = Vec::new();
    for (i, option) in codegen_options().into_iter().enumerate() {
        runs.push((conforming.clone(), format!("handles_options_{i}"), option.clone(), b""));
        runs.push((effects.clone(), format!("handles_options_effects_{i}"), option, b"A"));
    }
    for (level, flags) in LEVELS {
        runs.push((strict.clone(), format!("handles_options_strict.{level}"), flags.to_vec(), b""));
        runs.push((contexts.clone(), format!("handles_options_contexts.{level}"), flags.to_vec(), b"ABCDE"));
    }
    std::thread::scope(|scope| {
        for (source, name, option, console) in &runs {
            scope.spawn(move || {
                let context = format!("{name} ({})", option.join(" "));
                if console.is_empty() {
                    let (unit, bitcode) = driver_on(source, name, option);
                    assert!(unit.status.success(), "{context}: {}", stderr(&unit));
                    assert_refused(&front_of(&bitcode, name), NOT_HANDLED, NOT_YET, &context);
                } else {
                    let (got, tape) = run_through_driver(source, name, option);
                    assert_eq!(got, Outcome::Returned(Some(7)), "{context}");
                    assert_eq!(&tape, console, "{context}");
                }
            });
        }
    });
}

/// Review finding R14 of the earlier step: `CLANG` may name clang by its file name alone, found on `PATH`.
#[test]
fn the_driver_finds_clang_by_name_on_the_path() {
    let clang = clang();
    let bare = clang.file_name().expect("clang has a file name").to_owned();
    let mut path = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect::<Vec<_>>();
    if let Some(parent) = clang.parent().filter(|p| !p.as_os_str().is_empty()) {
        path.insert(0, parent.to_path_buf());
    }
    let source = write_c("handles_clang_by_name", "int main(void) { __bennett_effect('A'); return 7; }\n");
    let compiled = out_dir().join("handles_clang_by_name.cc.bc");
    let result = output(
        Command::new(driver())
            .env("CLANG", &bare)
            .env("PATH", std::env::join_paths(path).expect("a PATH"))
            .args(CLANG_FLAGS)
            .arg("-o")
            .arg(&compiled)
            .arg(&source),
    );
    assert!(result.status.success(), "CLANG={bare:?}: {}", stderr(&result));
}

/// Review finding R16 of the earlier step: a unit clang refuses, a unit the check refuses and a unit that passes
/// all leave no temporary file behind; a refused unit also removes an output left from an earlier run.
#[test]
fn the_driver_leaves_no_temporary_file() {
    let units = [
        ("bad_syntax", "int main(void) { return ; }\n?!?\n", false),
        ("refused", "void __bennett_effect(char c) { (void)c; }\nint main(void) { return 7; }\n", false),
        ("passes", "int main(void) { __bennett_effect('A'); return 7; }\n", true),
    ];
    for (case, text, passes) in units {
        let temporary = out_dir().join(format!("handles_driver_tmp_{case}"));
        let _ = std::fs::remove_dir_all(&temporary);
        std::fs::create_dir_all(&temporary).unwrap_or_else(|e| panic!("cannot create {temporary:?}: {e}"));
        let source = write_c(&format!("handles_driver_{case}"), text);
        let compiled = out_dir().join(format!("handles_driver_{case}.cc.bc"));
        std::fs::write(&compiled, "left from an earlier run").expect("write");
        let result = output(
            Command::new(driver()).env("TMPDIR", &temporary).args(CLANG_FLAGS).arg("-o").arg(&compiled).arg(&source),
        );
        assert_eq!(result.status.success(), passes, "{case}: {}", stderr(&result));
        assert_eq!(compiled.exists(), passes, "{case}");
        let left: Vec<_> = std::fs::read_dir(&temporary).expect("read the folder").collect();
        assert!(left.is_empty(), "{case}: left behind: {left:?}");
    }
}

/// The pass-free LLVM IR clang writes for a unit that prints `A` and returns 7, with the header included by force
/// (no array of references): what `bennett-cc` accepts as an LLVM IR unit, to be changed one property at a time.
fn effect_ir(name: &str) -> String {
    let source = write_c(&format!("{name}_source"), "int main(void) { __bennett_effect('A'); return 7; }\n");
    let ir = out_dir().join(format!("{name}_source.ll"));
    let written = output(
        Command::new(clang())
            .arg("-include")
            .arg(header())
            .args(CLANG_FLAGS)
            .args(["-S", "-emit-llvm", "-Xclang", "-disable-llvm-passes"])
            .arg(&source)
            .arg("-o")
            .arg(&ir),
    );
    assert!(written.status.success(), "{name}: {}", stderr(&written));
    read(&ir)
}

/// Rule 3, each branch alone (review finding F5 of this step): the declaration's printed text, here only an
/// `alloc-family` string attribute added, and its metadata attachments, here only an `!annotation` added to a
/// declaration whose text is the header's. The unchanged IR passes (`llvm_ir_units_get_the_same_check`).
#[test]
fn each_part_of_a_declaration_is_compared_alone() {
    let text = effect_ir("handles_rule3");
    let declaration = text.lines().find(|l| l.starts_with("declare void @__bennett_effect(")).expect("declared");
    let (head, group) = declaration.rsplit_once(" #").expect("an attribute group");
    let cases = [
        (
            "alloc_family",
            text.replace(declaration, &format!("{head} \"alloc-family\"=\"malloc\" #{group}")),
            "\"alloc-family\"=\"malloc\"",
        ),
        (
            "annotation",
            text.replace(declaration, &declaration.replacen("declare ", "declare !annotation !900 ", 1))
                + "!900 = !{!\"claim\"}\n",
            "has the metadata !annotation where the header's has none",
        ),
    ];
    for (case, changed, expected) in cases {
        let name = format!("handles_rule3_{case}");
        let (result, compiled) = driver_on(&write_text(&changed, &name), &name, CLANG_FLAGS);
        assert_refused(&result, REFUSED, expected, &format!("{name}:\n{changed}"));
        assert!(!compiled.exists(), "{name}");
    }
}

/// Review finding F4 of this step: LLVM IR that LLVM's reader stops on (here a `!tbaa` attachment that is not an
/// access tag, which the verifier of upgraded debug information reports as a fatal error) is refused by name with
/// status 2 and no output, as `bennett-front` refuses it, instead of stopping with a stack dump.
#[test]
fn invalid_llvm_ir_is_refused_by_name() {
    let text = effect_ir("handles_invalid_ir");
    let call = text.lines().find(|l| l.contains("call void @__bennett_effect(")).expect("the call");
    let broken = text.replace(call, &format!("{call}, !tbaa !900")) + "!900 = !{!901, !901, i64 0}\n!901 = !{!\"x\"}\n";
    let path = write_text(&broken, "handles_invalid_ir");
    let (result, compiled) = driver_on(&path, "handles_invalid_ir", CLANG_FLAGS);
    assert_refused(&result, REFUSED, "is not valid LLVM IR", "bennett-cc");
    assert!(!compiled.exists(), "no output");
    assert_refused(&front_of(&path, "handles_invalid_ir"), REFUSED, "the input is not valid LLVM IR", "bennett-front");
}

/// Review finding F2 of this step: the baseline is compiled with the user's options, so its identifiers are
/// reserved ones. A unit compiled with `-include` of a header that defines, as macros, every ordinary name the
/// baseline once used passes and runs.
#[test]
fn the_users_macros_cannot_change_the_baseline() {
    let names = ["which", "r", "n", "angle", "controls", "f", "image", "fields", "byte", "status"];
    let macros: String = names.iter().map(|name| format!("#define {name} @\n")).collect();
    let header = out_dir().join("handles_user_macros.h");
    std::fs::write(&header, macros).unwrap_or_else(|e| panic!("cannot write {header:?}: {e}"));
    let source = write_c("handles_user_macros", "int main(void) { __bennett_effect('A'); return 7; }\n");
    let include = header.to_str().expect("a UTF-8 path");
    for (level, flags) in LEVELS {
        let name = format!("handles_user_macros.{level}");
        let flags: Vec<&str> = flags.iter().copied().chain(["-include", include]).collect();
        let (outcome, tape) = run_through_driver(&source, &name, &flags);
        assert_eq!((outcome, tape), (Outcome::Returned(Some(7)), b"A".to_vec()), "{name}");
    }
}

/// Review finding F1 of this step, the documented limit of the check: a claim the program makes about its own
/// function is the program's statement and is trusted, as native clang trusts it. A GNU `const` function that
/// applies a gate is a false claim (undefined in C); at `-O2` clang deletes its call, and the gate with it, as it
/// does natively. This pins that the check does not look at a program's own functions: if a change starts or stops
/// doing so, this test fails and the documents must change with it.
#[test]
fn a_claim_on_the_programs_own_function_is_trusted() {
    let source = write_c(
        "handles_own_claim",
        "__attribute__((const)) static int flip(bq_reg r) { bq_gate(BQ_GATE_X, r, 0, 0, 0); return 0; }\n\
         int main(void) {\n  bq_reg r = bq_alloc(1);\n  flip(r);\n  return (int)bq_measure(r, 0, 1);\n}\n",
    );
    for (level, flags, gates) in [("O0", CLANG_FLAGS, 1), ("O2", CLANG_O2_FLAGS, 0)] {
        let name = format!("handles_own_claim.{level}");
        let compiled = driver_text(&source, &name, flags);
        let found = compiled.lines().filter(|l| l.contains("call void @bq_gate(")).count();
        assert_eq!(found, gates, "{name}:\n{compiled}");
    }
}

/// Review finding R17 of the earlier step, its `restrict` half (finding F6 of this step): after clang inlines a
/// function with a `restrict` parameter, the output call carries the optimiser's `!noalias`; it is the optimiser's
/// and trusted (rule 6). The program prints `A` and returns 7 at both levels, and the `-O2` output has the
/// attachment, so the test fails if clang stops writing it.
#[test]
fn metadata_the_optimiser_adds_is_trusted() {
    let source = write_c(
        "handles_restrict",
        "static inline void send(char *restrict p) {\n    __bennett_effect(*p);\n    ++*p;\n}\n\
         int main(void) {\n    char c = 'A';\n    send(&c);\n    return 7;\n}\n",
    );
    for (level, flags) in LEVELS {
        let name = format!("handles_restrict.{level}");
        let (outcome, tape) = run_through_driver(&source, &name, flags);
        assert_eq!((outcome, tape), (Outcome::Returned(Some(7)), b"A".to_vec()), "{name}");
    }
    let compiled = driver_text(&source, "handles_restrict.text", CLANG_O2_FLAGS);
    let call = compiled.lines().find(|l| l.contains("call void @__bennett_effect(")).expect("the call");
    assert!(call.contains("!noalias"), "the optimiser's attachment:\n{compiled}");
}
