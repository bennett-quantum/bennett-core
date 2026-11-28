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

/// The declarations file `bennett-cc` includes by force first in every unit.
fn prefix() -> PathBuf {
    root().join("front/include/bq-prefix.h")
}

/// The folder of `bq.h`, on the include path of every compilation here.
fn include() -> PathBuf {
    root().join("front/include")
}

/// A C unit that includes `bq.h`, as a program in the handle interface does.
fn write_unit(name: &str, text: &str) -> PathBuf {
    write_c(name, &format!("#include \"bq.h\"\n{text}"))
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

/// Plain clang on `source` with `flags` and the header's folder on the include path: `<name>.plain.ll`, LLVM text.
/// With `pass_free`, every LLVM pass is disabled and the declarations file is included by force first, as
/// `bennett-cc` compiles.
fn plain_clang(source: &Path, name: &str, flags: &[&str], pass_free: bool) -> Output {
    let text = out_dir().join(format!("{name}.plain.ll"));
    let _ = std::fs::remove_file(&text);
    let mut command = Command::new(clang());
    command.arg("-I").arg(include());
    if pass_free {
        command.arg("-include").arg(prefix()).args(["-Xclang", "-disable-llvm-passes"]);
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
    let source = write_unit(&format!("handles_cc_{case}"), text);
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
    let source = write_unit("handles_conforming", CONFORMING);
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
        let source = write_unit(&format!("handles_first_use_{case}"), &text);
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
    let late_label = write_unit(
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
        let source = write_unit(&format!("handles_nothing_{case}"), text);
        for (level, flags) in LEVELS {
            let name = format!("handles_nothing_{case}.{level}");
            let (outcome, tape) = run_through_driver(&source, &name, flags);
            assert_eq!(outcome, Outcome::Returned(Some(7)), "{name}");
            let expected: &[u8] = if case == "same_label_same_type" { b"A" } else { b"" };
            assert_eq!(tape, expected, "{name}");
        }
    }
}

/// A unit that defines the guard of the declarations file has no array of references and is refused by name,
/// naming the guard (review finding H4 of the earlier step), also when it uses no primitive; one that defines
/// `bq.h`'s guard and uses the header's library gets clang's own error.
#[test]
fn a_unit_without_the_declarations_file_is_refused() {
    let plain = write_c("handles_no_prefix", "int main(void) { return 7; }\n");
    let library = write_unit("handles_no_header", "int main(void) { return (int)bq_fault_alloc().id; }\n");
    for (level, flags) in LEVELS {
        let name = format!("handles_no_prefix.{level}");
        let guarded: Vec<&str> = flags.iter().copied().chain(["-DBENNETT_BQ_PREFIX_H_INCLUDED"]).collect();
        let (result, compiled) = driver_on(&plain, &name, &guarded);
        assert_refused(&result, REFUSED, "must not define that file's guard macro BENNETT_BQ_PREFIX_H_INCLUDED", &name);
        assert!(!compiled.exists(), "{name}");
        let name = format!("handles_no_header.{level}");
        let guarded: Vec<&str> = flags.iter().copied().chain(["-DBENNETT_BQ_H_INCLUDED"]).collect();
        let (result, compiled) = driver_on(&library, &name, &guarded);
        assert!(!result.status.success() && !compiled.exists(), "{name}: {}", stderr(&result));
        assert!(stderr(&result).contains("bq_fault_alloc"), "{name}: {}", stderr(&result));
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
    let source = write_unit("handles_pointer_claims", text);
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
    let source = write_unit("handles_folded_claims", text);
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
        let source = write_unit(&format!("handles_pointer_{case}"), text);
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
    let source = write_unit("handles_front_clang_alone", "int main(void) { bq_free(bq_alloc(1)); return 7; }\n");
    for (level, flags) in LEVELS {
        let name = format!("handles_front_clang_alone.{level}");
        let bitcode = out_dir().join(format!("{name}.bc"));
        let compiled = output(
            Command::new(clang())
                .arg("-I")
                .arg(include())
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
    let source = write_unit(
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
                .arg(prefix())
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
/// target strings, ...): the table of `front/src/BaselineOptions.cpp` gives each to the baseline too, so they agree.
fn codegen_options() -> Vec<Vec<&'static str>> {
    let mut options = vec![
        vec!["-Os"],
        vec!["-Oz"],
        vec!["-O1"],
        vec!["-O3"],
        vec!["-O4"],
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
        vec!["-O2", "-fexceptions"],
    ];
    if cfg!(target_arch = "x86_64") {
        options.extend([
            vec!["-O2", "-mno-red-zone"],
            vec!["-O2", "-mspeculative-load-hardening"],
            vec!["-O2", "-mstackrealign"],
            vec!["-O2", "-march=x86-64-v3"],
            vec!["-O2", "-mavx2"],
            vec!["-O2", "-msse4.2"],
            vec!["-O2", "-mfma"],
            vec!["-O2", "-mbmi2"],
            vec!["-O2", "-maes"],
            vec!["-O2", "-mpopcnt"],
            vec!["-O2", "-mno-avx"],
        ]);
    }
    options
}

/// Review finding R13 of the earlier step (and P3 of review r1 of the prefix step: `-O4`, `-pg` and seven x86
/// target features were refused before they were in the table): the conforming program passes under each option (the front stops at the
/// translation of its calls); with `#pragma STDC FENV_ACCESS ON` (`strictfp` on its calls) and the statement
/// attributes `nomerge`, `noinline` and `always_inline` on calls, at both levels; and a program of the output and
/// exit primitives runs to its end. Under `-pg` the driver accepts a unit with both calls of `bq_width` at both
/// levels; its output calls `mcount`, which no device library defines yet, so it is not run.
#[test]
fn the_attributes_clang_adds_for_an_option_are_accepted() {
    let conforming = write_unit("handles_options_conforming", CONFORMING);
    let strict = write_unit("handles_options_strict", &format!("#pragma STDC FENV_ACCESS ON\n{CONFORMING}"));
    let effects = write_unit("handles_options_effects", "int main(void) { __bennett_effect('A'); return 7; }\n");
    let contexts = write_unit(
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
    let widths_unit = write_unit("handles_options_pg", TWO_WIDTHS);
    for (level, flags) in LEVELS {
        let name = format!("handles_options_pg.{level}");
        let flags: Vec<&str> = flags.iter().copied().chain(["-pg"]).collect();
        let (result, compiled) = driver_with(&widths_unit, &name, &flags, &[]);
        assert!(result.status.success(), "{name}: {}", stderr(&result));
        assert_eq!(widths(&read(&compiled)), 2, "{name}");
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
    let source = write_unit("handles_clang_by_name", "int main(void) { __bennett_effect('A'); return 7; }\n");
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
        let source = write_unit(&format!("handles_driver_{case}"), text);
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

/// The pass-free LLVM IR clang writes for a unit that prints `A` and returns 7, with the declarations file included
/// by force: what `bennett-cc` accepts as an LLVM IR unit, to be changed one property at a time.
fn effect_ir(name: &str) -> String {
    let source = write_c(&format!("{name}_source"), "int main(void) { __bennett_effect('A'); return 7; }\n");
    let ir = out_dir().join(format!("{name}_source.ll"));
    let written = output(
        Command::new(clang())
            .arg("-include")
            .arg(prefix())
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

/// Review findings F2 and G1 of the earlier step: no macro of the user's reaches the baseline, and the declarations
/// file is written only with keywords, reserved names and the primitives' names in parentheses. A unit that uses
/// only the declarations file passes and runs with macros of every ordinary name the baseline or the declarations
/// file ever used, given by `-include` (after the declarations file) and by `-D` (before it): object-like ones,
/// the member `id` of a handle, and function-like ones with a primitive's name.
#[test]
fn the_users_macros_cannot_change_the_baseline() {
    let names = [
        "which", "r", "n", "angle", "controls", "f", "image", "fields", "byte", "status", "id", "uint64_t", "bq_reg",
        "bq_bit", "bq_fn", "bq_field", "bq_slice",
    ];
    let macros: String = names.iter().map(|name| format!("#define {name} @\n")).collect();
    let header = out_dir().join("handles_user_macros.h");
    std::fs::write(&header, format!("{macros}#define bq_gate(a, b, c, d, e) ((void)0)\n"))
        .unwrap_or_else(|e| panic!("cannot write {header:?}: {e}"));
    let source = write_c("handles_user_macros", "int main(void) { __bennett_effect('A'); return 7; }\n");
    let include = header.to_str().expect("a UTF-8 path");
    let mut defined: Vec<String> = names.iter().map(|name| format!("-D{name}=@")).collect();
    defined.push("-Dbq_gate(a,b,c,d,e)=((void)0)".to_string());
    defined.push("-Dbq_alloc(n)=((void)0)".to_string());
    for (level, flags) in LEVELS {
        let name = format!("handles_user_macros.{level}");
        let included: Vec<&str> = flags.iter().copied().chain(["-include", include]).collect();
        let (outcome, tape) = run_through_driver(&source, &name, &included);
        assert_eq!((outcome, tape), (Outcome::Returned(Some(7)), b"A".to_vec()), "{name}");
        let name = format!("handles_user_macros_defined.{level}");
        let defined: Vec<&str> = flags.iter().copied().chain(defined.iter().map(String::as_str)).collect();
        let (outcome, tape) = run_through_driver(&source, &name, &defined);
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
    let source = write_unit(
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
    let source = write_unit(
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

/// `bennett-cc` on `source` with `flags` and the variables `env` added to its environment: LLVM text.
fn driver_with(source: &Path, name: &str, flags: &[&str], env: &[(&str, &str)]) -> (Output, PathBuf) {
    let compiled = out_dir().join(format!("{name}.cc.ll"));
    let _ = std::fs::remove_file(&compiled);
    let mut command = Command::new(driver());
    command.args(flags).arg("-o").arg(&compiled).arg(source);
    for (key, value) in env {
        command.env(key, value);
    }
    (output(&mut command), compiled)
}

fn path_text(path: &Path) -> &str {
    path.to_str().expect("a UTF-8 path")
}

fn write_file(name: &str, text: &str) -> PathBuf {
    let path = out_dir().join(name);
    std::fs::write(&path, text).unwrap_or_else(|e| panic!("cannot write {path:?}: {e}"));
    path
}

/// A unit that discards two calls of `bq_width`, which a claim of `const` lets clang `-O2` delete.
const TWO_WIDTHS: &str = "int main(void) { bq_reg r = bq_alloc(1); bq_width(r); bq_width(r); return 7; }\n";

fn widths(ir: &str) -> usize {
    ir.lines().filter(|l| l.contains("call i64 @bq_width(")).count()
}

/// The claim `const` on `bq_width`, unprototyped so that it also merges with a later prototype.
fn claim_header() -> PathBuf {
    write_file("handles_class_claim.h", "__attribute__((const)) unsigned long bq_width();\n")
}

/// A copy of `original` whose declaration of `bq_width` carries the claim, and an overlay file that puts it in
/// the original's place: `-ivfsoverlay` remaps any path clang opens.
fn overlay(name: &str, original: &Path, copy_text: &str) -> PathBuf {
    let copy = write_file(&format!("handles_class_{name}.copy.h"), copy_text);
    let yaml = format!(
        "{{\"version\": 0, \"roots\": [{{\"name\": \"{}\", \"type\": \"file\", \"external-contents\": \"{}\"}}]}}\n",
        path_text(original),
        path_text(&copy)
    );
    write_file(&format!("handles_class_{name}.yaml"), &yaml)
}

/// The advisor's principle for the declaration check, nothing the user or the host controls reaches the baseline,
/// tested on every member of the class measured on the earlier step (each accepted there with both calls of
/// `bq_width` deleted at `-O2`): a precompiled header, a `--config` file, a default configuration file, an overlay
/// over the declarations file and over `bq.h`, a header that shadows `<stdint.h>` through `-I` and through
/// `CPATH`, and a `-D` that gives a keyword the claim. Each carries `const` on `bq_width` into the unit; the
/// baseline never sees it, so the unit is refused by name at `-O0` and `-O2` with no output: by rule 3 where the
/// claim comes before the declarations file, by rule 4 where it comes after it. A refusal of rule 3 names what the
/// baseline did not get.
#[test]
fn nothing_the_user_or_the_host_controls_reaches_the_baseline() {
    let source = write_unit("handles_class", TWO_WIDTHS);
    let claim = claim_header();
    let config = write_file("handles_class.cfg", &format!("-include {}\n", path_text(&claim)));
    let config_dir = out_dir().join("handles_class_config");
    std::fs::create_dir_all(&config_dir).expect("a folder");
    std::fs::write(config_dir.join("clang.cfg"), format!("-include {}\n", path_text(&claim))).expect("write");
    let shadow = out_dir().join("handles_class_shadow");
    std::fs::create_dir_all(&shadow).expect("a folder");
    std::fs::write(shadow.join("stdint.h"), "#include_next <stdint.h>\n__attribute__((const)) uint64_t bq_width();\n")
        .expect("write");
    let prefix_text = read(&prefix());
    let claimed_prefix =
        prefix_text.replace("__UINT64_TYPE__ (bq_width)(", "__attribute__((const)) __UINT64_TYPE__ (bq_width)(");
    assert_ne!(claimed_prefix, prefix_text, "the copy carries the claim");
    let header_text = read(&include().join("bq.h"));
    let at = header_text.rfind("#endif").expect("the guard's end");
    let claimed_header =
        format!("{}__attribute__((const)) uint64_t bq_width(bq_reg);\n{}", &header_text[..at], &header_text[at..]);
    let over_prefix = overlay("prefix", &prefix(), &claimed_prefix);
    let over_header = overlay("header", &include().join("bq.h"), &claimed_header);
    let declaration = "the declaration of the primitive @bq_width is `declare i64 @bq_width(i64) { nounwind willreturn \
                       memory(none)";
    let call = "a call of the device function @bq_width has attributes that let LLVM remove or move its calls";
    for (level, flags) in LEVELS {
        let header = write_file(
            &format!("handles_class_pch.{level}.h"),
            "#include \"bq.h\"\n__attribute__((const)) uint64_t bq_width(bq_reg);\n",
        );
        let pch = out_dir().join(format!("handles_class.{level}.pch"));
        let made = output(
            Command::new(clang())
                .arg("-I")
                .arg(include())
                .args(flags)
                .args(["-x", "c-header"])
                .arg(&header)
                .arg("-o")
                .arg(&pch),
        );
        assert!(made.status.success(), "{level}: {}", stderr(&made));
        let config_option = format!("--config={}", path_text(&config));
        let config_dir_option = format!("--config-user-dir={}", path_text(&config_dir));
        let shadow_text = path_text(&shadow);
        let cases: Vec<(&str, Vec<&str>, Vec<(&str, &str)>, &str, &str)> = vec![
            (
                "pch",
                vec!["-include-pch", path_text(&pch)],
                vec![],
                declaration,
                "withheld from the baseline here: -include-pch",
            ),
            ("config", vec![&config_option], vec![], declaration, "withheld from the baseline here: --config="),
            ("default_config", vec![&config_dir_option], vec![], declaration, "--config-user-dir="),
            ("overlay_prefix", vec!["-ivfsoverlay", path_text(&over_prefix)], vec![], declaration, "-ivfsoverlay"),
            ("overlay_header", vec!["-ivfsoverlay", path_text(&over_header)], vec![], call, "-ivfsoverlay"),
            ("search_path", vec!["-I", shadow_text], vec![], call, "withheld from the baseline here: -I"),
            ("cpath", vec![], vec![("CPATH", shadow_text)], call, "an environment variable may be the cause"),
            ("keyword", vec!["-Dlong=__attribute__((const)) long"], vec![], declaration, "-Dlong="),
        ];
        for (case, extra, env, expected, named) in cases {
            let name = format!("handles_class_{case}.{level}");
            let flags: Vec<&str> = flags.iter().copied().chain(extra).collect();
            let (result, compiled) = driver_with(&source, &name, &flags, &env);
            assert_refused(&result, REFUSED, expected, &name);
            assert!(stderr(&result).contains(named), "{name}: expected `{named}` in: {}", stderr(&result));
            assert!(!compiled.exists(), "{name}: a refused unit has no output");
        }
    }
    // The control: without any of them both calls stay.
    let compiled = driver_text(&source, "handles_class_control", CLANG_O2_FLAGS);
    assert_eq!(widths(&compiled), 2, "{compiled}");
}

/// The unit keeps the user's options, but no variable rewrites the driver's own command line: `bennett-cc` clears
/// `CCC_OVERRIDE_OPTIONS` (and clang-cl's `CL` and `_CL_`, which clang reads only in clang-cl mode). With it set to
/// append the claim's `-include`, plain clang `-O2` deletes both calls (the fact this rests on), and `bennett-cc`
/// keeps both at both levels.
#[test]
fn variables_that_rewrite_the_command_line_are_cleared() {
    let source = write_unit("handles_override", TWO_WIDTHS);
    let claim = claim_header();
    let rewrite = format!("#+-include +{}", path_text(&claim));
    let plain = out_dir().join("handles_override.plain.ll");
    let compiled = output(
        Command::new(clang())
            .env("CCC_OVERRIDE_OPTIONS", &rewrite)
            .arg("-I")
            .arg(include())
            .args(CLANG_O2_FLAGS)
            .args(["-S", "-emit-llvm"])
            .arg(&source)
            .arg("-o")
            .arg(&plain),
    );
    assert!(compiled.status.success(), "{}", stderr(&compiled));
    assert_eq!(widths(&read(&plain)), 0, "plain clang follows the variable");
    for (level, flags) in LEVELS {
        let name = format!("handles_override.{level}");
        let (result, compiled) = driver_with(&source, &name, flags, &[("CCC_OVERRIDE_OPTIONS", &rewrite)]);
        assert!(result.status.success(), "{name}: {}", stderr(&result));
        assert_eq!(widths(&read(&compiled)), 2, "{name}");
    }
}

/// A configuration file reaches the unit (it is the user's or the host's), but none can remove the driver's own
/// options: clang places a file's options before the command line, where the forced declarations file, the
/// pass-free compile and the driver's output come after them. Each file, given by `--config` and found as a
/// default file, either is refused by name with no output or is accepted with both calls of `bq_width` kept: a
/// file that stops the compile at syntax (`-fsyntax-only`, then no array of references) or makes the unit a header
/// (refused: what clang wrote is not LLVM IR), one that asks for preprocessing or assembly text (refused as not
/// LLVM IR, or accepted where clang's own `-c` comes later), and one that sets optimisation options of its own.
#[test]
fn no_configuration_file_removes_the_drivers_options() {
    let source = write_unit("handles_config", TWO_WIDTHS);
    let files: [(&str, &str, Option<&str>); 6] = [
        ("syntax_only", "-fsyntax-only\n", Some("the unit lacks bennett-cc's array of the primitives' addresses")),
        ("preprocess", "-E\n", None),
        ("assembly", "-S\n", None),
        ("level", "-O2 -mllvm -inline-threshold=10000\n", None),
        ("pass_level", "-Xclang -disable-O0-optnone -O3\n", None),
        ("header", "-x c-header\n", Some("is not valid LLVM IR")),
    ];
    for (case, text, refusal) in files {
        let config = write_file(&format!("handles_config_{case}.cfg"), text);
        let folder = out_dir().join(format!("handles_config_{case}"));
        std::fs::create_dir_all(&folder).expect("a folder");
        std::fs::write(folder.join("clang.cfg"), text).expect("write");
        let ways = [format!("--config={}", path_text(&config)), format!("--config-user-dir={}", path_text(&folder))];
        for (way, option) in ways.iter().enumerate() {
            for (level, flags) in LEVELS {
                let name = format!("handles_config_{case}_{way}.{level}");
                let flags: Vec<&str> = flags.iter().copied().chain([option.as_str()]).collect();
                let (result, compiled) = driver_with(&source, &name, &flags, &[]);
                if result.status.success() {
                    assert!(refusal.is_none(), "{name}: accepted");
                    assert_eq!(widths(&read(&compiled)), 2, "{name}");
                } else {
                    let expected = refusal.unwrap_or("is not valid LLVM IR");
                    assert_refused(&result, REFUSED, expected, &name);
                    assert!(!compiled.exists(), "{name}");
                }
            }
        }
    }
}

/// The baseline gets only the code-generation options of `bennett-cc`'s table (`front/src/BaselineOptions.cpp`). The
/// user's other options stay with the unit: options that change no declaration or call of a primitive (a language
/// standard, warnings, macros, include paths, an included file) are accepted with both calls kept; one that does
/// change them, here a target feature given to clang's front end through `-Xclang`, is refused by name, and the
/// refusal says that an option outside the table may be the cause and lists the options withheld.
#[test]
fn options_outside_the_table_stay_with_the_unit() {
    let source = write_unit("handles_withheld", TWO_WIDTHS);
    let benign = write_file("handles_withheld_benign.h", "static inline int helper(int x) { return x + 1; }\n");
    let folder = out_dir().join("handles_withheld_folder");
    std::fs::create_dir_all(&folder).expect("a folder");
    // A configuration file named `-O2` that sets no optimisation level.
    std::fs::write(folder.join("-O2"), "-Wall\n").expect("write");
    let config_folder = format!("--config-user-dir={}", path_text(&folder));
    let options: Vec<Vec<&str>> = vec![
        vec!["-std=c11"],
        vec!["-std=gnu2x"],
        vec!["-Wall", "-Wextra"],
        vec!["-DVALUE=7", "-UOTHER"],
        vec!["-I", path_text(&folder)],
        vec!["-include", path_text(&benign)],
        // Review r1 of the prefix step, P4: `-O2` here is the value of `-Xclang`, not an option, and was forwarded
        // alone, so the unit was refused at `-O0`.
        vec!["-Xclang", "-O2"],
        // Re-check r2 of the prefix step, Q1: `-O2` is the value of options that clang's option table defines
        // without a name (`--config`, `--system-header-prefix`, `-Xmicrosoft-windows-sdk-root`), which the list of
        // such options missed, so it was forwarded and the unit refused at `-O0`.
        vec![config_folder.as_str(), "--config", "-O2"],
        vec!["--system-header-prefix", "-O2"],
        vec!["-Xmicrosoft-windows-sdk-root", "-O2"],
        // The other way round: options of clang's front end (`-cc1`), whose spellings the driver reads as other
        // options of its own, so `-O2` after them is an option; the list had them and withheld it, refusing the
        // unit at `-O0`.
        vec!["-load", "-O2"],
        vec!["-defsym", "-O2"],
    ];
    for (level, flags) in LEVELS {
        for (i, option) in options.iter().enumerate() {
            let name = format!("handles_withheld_{i}.{level}");
            let flags: Vec<&str> = flags.iter().chain(option).copied().collect();
            let (result, compiled) = driver_with(&source, &name, &flags, &[]);
            assert!(result.status.success(), "{name} {option:?}: {}", stderr(&result));
            assert_eq!(widths(&read(&compiled)), 2, "{name}");
        }
    }
    if cfg!(target_arch = "x86_64") {
        let option = ["-Xclang", "-target-feature", "-Xclang", "+avx2"];
        for (level, flags) in LEVELS {
            let name = format!("handles_withheld_feature.{level}");
            let flags: Vec<&str> = flags.iter().chain(&option).copied().collect();
            let (result, compiled) = driver_with(&source, &name, &flags, &[]);
            assert_refused(&result, REFUSED, "an option outside the table", &name);
            assert!(
                stderr(&result).contains("withheld from the baseline here: -Xclang -target-feature -Xclang +avx2"),
                "{name}: {}",
                stderr(&result)
            );
            assert!(!compiled.exists(), "{name}");
        }
    }
}

/// Re-check r2 of the prefix step, Q2: the driver's folder is the first `-I` folder of the unit, before the
/// user's. A `bq.h` that stops the compile (`#error`) in one of the user's `-I` or `-isystem` folders is not found,
/// by `#include "bq.h"` or `#include <bq.h>`, and the unit is accepted with both calls; one beside the unit or in a
/// `-iquote` folder is found by the quoted search, as natively, and clang's error stops the unit.
#[test]
fn the_drivers_header_comes_before_the_users_include_folders() {
    let base = out_dir().join("handles_search");
    let folder = |name: &str| {
        let path = base.join(name);
        std::fs::create_dir_all(&path).expect("a folder");
        path
    };
    for name in ["beside", "angled", "system", "quote"] {
        std::fs::write(folder(name).join("bq.h"), format!("#error the {name} bq.h\n")).expect("write");
    }
    let unit = |place: &str, include: &str| {
        let path = folder(place).join(if include.starts_with('<') { "angled.c" } else { "quoted.c" });
        std::fs::write(&path, format!("#include {include}\n{TWO_WIDTHS}")).expect("write");
        path
    };
    let (angled, system, quote) = (folder("angled"), folder("system"), folder("quote"));
    let cases: [(&str, &str, &str, &Path, Option<&str>); 6] = [
        ("unit", "\"bq.h\"", "-I", &angled, None),
        ("unit", "<bq.h>", "-I", &angled, None),
        ("unit", "\"bq.h\"", "-isystem", &system, None),
        ("unit", "<bq.h>", "-iquote", &quote, None),
        ("unit", "\"bq.h\"", "-iquote", &quote, Some("the quote bq.h")),
        ("beside", "\"bq.h\"", "-I", &angled, Some("the beside bq.h")),
    ];
    for (level, flags) in LEVELS {
        for (i, (place, include, option, path, error)) in cases.iter().enumerate() {
            let name = format!("handles_search_{i}.{level}");
            let flags: Vec<&str> = flags.iter().copied().chain([*option, path_text(path)]).collect();
            let (result, compiled) = driver_with(&unit(place, include), &name, &flags, &[]);
            match error {
                None => {
                    assert!(result.status.success(), "{name}: {}", stderr(&result));
                    assert_eq!(widths(&read(&compiled)), 2, "{name}");
                }
                Some(error) => {
                    assert!(!result.status.success() && stderr(&result).contains(error), "{name}: {}", stderr(&result));
                    assert!(!compiled.exists(), "{name}");
                }
            }
        }
    }
}

/// Runs `work` on each item on a few threads at a time.
fn on_threads<T: Sync>(items: &[T], work: impl Fn(&T) + Sync) {
    let next = std::sync::atomic::AtomicUsize::new(0);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(8));
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                while let Some(item) = items.get(next.fetch_add(1, std::sync::atomic::Ordering::Relaxed)) {
                    work(item);
                }
            });
        }
    });
}

/// The entries of the table of `front/src/BaselineOptions.cpp`, spelling and form, read from its source.
fn table_entries() -> Vec<(String, String)> {
    let source = read(&root().join("front/src/BaselineOptions.cpp"));
    let entries: Vec<(String, String)> = source
        .lines()
        .filter_map(|line| {
            let (spelling, rest) = line.trim().strip_prefix("{\"")?.split_once('"')?;
            let form = rest.strip_prefix(", Form::")?.split(|c: char| !c.is_alphanumeric()).next()?;
            Some((spelling.to_string(), form.to_string()))
        })
        .collect();
    assert!(entries.len() > 90 && entries.iter().any(|(s, f)| s == "-target" && f == "Triple"), "{entries:?}");
    entries
}

/// The entries of `front/src/SeparateOptions.def`: spelling, match (`Exact` or `Prefix`) and number of values.
fn separate_entries() -> Vec<(String, String, usize)> {
    read(&root().join("front/src/SeparateOptions.def"))
        .lines()
        .filter_map(|line| {
            let (spelling, rest) = line.strip_prefix("BENNETT_SEPARATE(\"")?.split_once("\", ")?;
            let (matching, values) = rest.strip_suffix(')')?.split_once(", ")?;
            Some((spelling.to_string(), matching.to_string(), values.parse().expect("a count")))
        })
        .collect()
}

/// Re-check r2 of the prefix step, Q1: the list of options whose next arguments clang's driver reads as their
/// values (`front/src/SeparateOptions.def`) says how clang 18 reads a command. For each entry, clang's driver given
/// the spelling (a `Prefix` one with a value joined), one value fewer than the entry says, and an argument that is
/// no option reads that argument as a value; given all the values, it reads the argument as an option (an unknown
/// one). The options clang's table defines without a name or through a class, which the list once missed, are
/// in it; the options of clang's front end that the driver reads as others, which it once had, are not.
#[test]
fn the_separate_options_are_those_clang_reads() {
    let entries = separate_entries();
    assert!(entries.len() > 100, "{entries:?}");
    let listed = |spelling: &str| entries.iter().any(|(s, _, _)| s == spelling);
    for spelling in ["--config", "--system-header-prefix", "-fnew-alignment", "-Xmicrosoft-windows-sdk-root", "-o"] {
        assert!(listed(spelling), "{spelling} is not listed");
    }
    for spelling in ["-load", "-defsym", "-objc-isystem", "-Fo", "-E"] {
        assert!(!listed(spelling), "{spelling} is listed");
    }
    let source = write_c("handles_separate", "int main(void) { return 0; }\n");
    // The values are relative names of files in a folder of the test's own: the driver writes some even under
    // `-###` (`--serialize-diagnostics v`).
    let folder = out_dir().join("handles_separate");
    std::fs::create_dir_all(&folder).expect("a folder");
    let probe = "--bennett-not-an-option";
    let unknown = format!("unknown argument: '{probe}'");
    on_threads(&entries, |(spelling, matching, values)| {
        let first = if matching == "Prefix" { format!("{spelling}x86_64") } else { spelling.clone() };
        for given in [values - 1, *values] {
            let args: Vec<&str> =
                std::iter::once(first.as_str()).chain(std::iter::repeat_n("v", given)).chain([probe]).collect();
            let result = output(
                Command::new(clang())
                    .current_dir(&folder)
                    .args(["--no-default-config", "-###", "-c"])
                    .arg(&source)
                    .args(&args),
            );
            assert_eq!(stderr(&result).contains(&unknown), given == *values, "{args:?}: {}", stderr(&result));
        }
    });
}

/// The host's target triple, as clang names it.
fn host_triple() -> String {
    let printed = output(Command::new(clang()).arg("-dumpmachine"));
    assert!(printed.status.success(), "{}", stderr(&printed));
    String::from_utf8_lossy(&printed.stdout).trim().to_string()
}

/// Review r1 of the prefix step, P1: clang expands an argument `@file` into options wherever it stands, and its
/// driver hands an option's value to its front end as an argument of its own, where an `@` value is expanded too.
/// The table forwarded `-target` with whatever followed it, so `-target @file`, the file naming the host's triple
/// and `-include` of a claim (or `-include-pch` of one), gave the claim to the baseline too, and the unit was
/// accepted with both calls of `bq_width` deleted at `-O2`. A value is now forwarded only in a closed form, so both
/// are refused by name at `-O0` and `-O2`, the refusal listing the response file as withheld. Then `@file` after
/// and inside every entry of the table (a file that includes the claim): each is refused (by name or by clang)
/// with no output, or accepted with both calls; a refusal by name never lists a response file as forwarded.
#[test]
fn a_response_file_never_reaches_the_baseline() {
    let source = write_unit("handles_rsp", TWO_WIDTHS);
    let claim = claim_header();
    let triple = host_triple();
    let anywhere = write_file("handles_rsp_anywhere.rsp", &format!("-include {}\n", path_text(&claim)));
    let at_anywhere = format!("@{}", path_text(&anywhere));
    let declaration = "the declaration of the primitive @bq_width is";
    let call = "a call of the device function @bq_width has attributes that let LLVM remove or move its calls";
    for (level, flags) in LEVELS {
        let header = write_file(
            &format!("handles_rsp_pch.{level}.h"),
            "#include \"bq.h\"\n__attribute__((const)) uint64_t bq_width(bq_reg);\n",
        );
        let pch = out_dir().join(format!("handles_rsp.{level}.pch"));
        let made = output(
            Command::new(clang())
                .arg("-I")
                .arg(include())
                .args(flags)
                .args(["-x", "c-header"])
                .arg(&header)
                .arg("-o")
                .arg(&pch),
        );
        assert!(made.status.success(), "{level}: {}", stderr(&made));
        let included = write_file(
            &format!("handles_rsp_include.{level}.rsp"),
            &format!("{triple} -include {}\n", path_text(&claim)),
        );
        let precompiled = write_file(
            &format!("handles_rsp_pch.{level}.rsp"),
            &format!("{triple} -include-pch {}\n", path_text(&pch)),
        );
        for (case, file, expected) in [("include", &included, call), ("pch", &precompiled, declaration)] {
            let name = format!("handles_rsp_target_{case}.{level}");
            let at = format!("@{}", path_text(file));
            let flags: Vec<&str> = flags.iter().copied().chain(["-target", at.as_str()]).collect();
            let (result, compiled) = driver_with(&source, &name, &flags, &[]);
            assert_refused(&result, REFUSED, expected, &name);
            let withheld = format!("withheld from the baseline here: -target {at}");
            assert!(stderr(&result).contains(&withheld), "{name}: expected `{withheld}` in: {}", stderr(&result));
            assert!(!compiled.exists(), "{name}: a refused unit has no output");
        }
    }
    let mut runs: Vec<(String, Vec<String>, &[&str])> = Vec::new();
    for (i, (spelling, form)) in table_entries().into_iter().enumerate() {
        let joined = form == "Joined" || (form == "Triple" && spelling.ends_with('='));
        let placed =
            if joined { vec![format!("{spelling}{at_anywhere}")] } else { vec![spelling.clone(), at_anywhere.clone()] };
        for (level, flags) in LEVELS {
            runs.push((format!("handles_rsp_entry_{i}.{level}"), placed.clone(), flags));
        }
    }
    on_threads(&runs, |(name, placed, flags)| {
        let flags: Vec<&str> = flags.iter().copied().chain(placed.iter().map(String::as_str)).collect();
        let (result, compiled) = driver_with(&source, name, &flags, &[]);
        let text = stderr(&result);
        if result.status.success() {
            assert_eq!(widths(&read(&compiled)), 2, "{name} {placed:?}");
        } else {
            assert!(!compiled.exists(), "{name} {placed:?}: a refused unit has no output");
        }
        if let Some((_, withheld)) = text.split_once("withheld from the baseline here: ") {
            assert!(withheld.contains(&at_anywhere), "{name} {placed:?}: the response file is withheld: {text}");
        }
    });
}

/// Review r1 of the prefix step, P2: the unit and the baseline were read into one LLVM context, which renames the
/// second module's copy of a named structure type (`%struct.__bennett_reg.0`). On targets that pass or return a
/// handle in memory the declarations name that type (i386 `sret(%struct.__bennett_reg)`, sparc64 a returned
/// `%struct.__bennett_reg`), so every unit for them was refused. Each is now read into a context of its own and
/// types are compared by name and body: a unit of the declarations file alone (`bq.h` asserts a handle aligned to
/// 8, which i386 does not give) passes at both levels with both calls, given the target joined, as the next
/// argument, or as `-m32`; a claim on `bq_width` is still refused; and LLVM IR whose handle type has another body
/// is refused, naming the body.
#[test]
fn targets_that_name_the_handle_type_are_accepted() {
    let body = "struct __bennett_reg r = bq_alloc(1);\n  bq_width(r);\n  bq_width(r);\n  __bennett_effect('A');\n  \
                return 7;\n}\n";
    let plain = write_c("handles_targets", &format!("int main(void) {{\n  {body}"));
    let claimed = write_c(
        "handles_targets_claim",
        &format!("__attribute__((const)) __UINT64_TYPE__ bq_width(struct __bennett_reg);\nint main(void) {{\n  {body}"),
    );
    let mut targets = vec![
        vec!["--target=i386-pc-linux-gnu"],
        vec!["-target", "i386-pc-linux-gnu"],
        vec!["--target=sparc64-unknown-linux-gnu"],
    ];
    if cfg!(target_arch = "x86_64") {
        targets.push(vec!["-m32"]);
    }
    for target in &targets {
        for (level, flags) in LEVELS {
            let name = format!("handles_targets_{}.{level}", target.join("_").replace(['-', '='], ""));
            let flags: Vec<&str> = flags.iter().chain(target).copied().collect();
            let (result, compiled) = driver_with(&plain, &name, &flags, &[]);
            assert!(result.status.success(), "{name}: {}", stderr(&result));
            let text = read(&compiled);
            assert_eq!(widths(&text), 2, "{name}:\n{text}");
            assert!(text.contains("%struct.__bennett_reg"), "{name}: the declarations name the type:\n{text}");
            let (result, compiled) = driver_with(&claimed, &format!("{name}.claim"), &flags, &[]);
            let call = "a call of the device function @bq_width has attributes that let LLVM remove or move its calls";
            assert_refused(&result, REFUSED, call, &name);
            assert!(!compiled.exists(), "{name}");
        }
    }
    let i386 = ["--target=i386-pc-linux-gnu"];
    let flags: Vec<&str> = CLANG_FLAGS.iter().chain(&i386).copied().collect();
    let text = plain_text(&plain, "handles_targets_body", &flags, true);
    let original = "%struct.__bennett_reg = type { i64 }";
    assert!(text.contains(original), "{text}");
    let changed = text.replace(original, "%struct.__bennett_reg = type { i32, i32 }");
    let (result, compiled) = driver_on(&write_text(&changed, "handles_targets_body"), "handles_targets_body", &flags);
    assert_refused(&result, REFUSED, "where %struct.__bennett_reg = type { i32, i32 }", "a changed body");
    assert!(!compiled.exists(), "a changed body");
}

/// Review r1 of the prefix step, P5: a refusal by rule 2 (here a definition of a primitive that the unit's
/// `-include` brings) lists the options withheld from the baseline, as the refusals of rules 3 and 4 do.
#[test]
fn a_redirection_refusal_names_the_withheld_options() {
    let definition = write_file("handles_redirect.h", "void (__bennett_effect)(char c) { (void)c; }\n");
    let source = write_unit("handles_redirect", "int main(void) { __bennett_effect('A'); return 7; }\n");
    for (level, flags) in LEVELS {
        let name = format!("handles_redirect.{level}");
        let flags: Vec<&str> = flags.iter().copied().chain(["-include", path_text(&definition)]).collect();
        let (result, compiled) = driver_with(&source, &name, &flags, &[]);
        assert_refused(&result, REFUSED, "the program defines the primitive @__bennett_effect", &name);
        let withheld = format!("withheld from the baseline here: -include {}", path_text(&definition));
        assert!(stderr(&result).contains(&withheld), "{name}: expected `{withheld}` in: {}", stderr(&result));
        assert!(!compiled.exists(), "{name}");
    }
}
