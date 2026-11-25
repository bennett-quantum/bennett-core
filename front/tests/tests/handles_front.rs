//! The front end's own part of the handle interface (`docs/HANDLES.md`, sections 4 and 14): the declaration
//! rule, checked on each unit's parsed form (`bennett-handles check-unit`) and on the module the front reads
//! (`bennett-front`); and the refusal of a program that uses both the marker interface and the handle interface.
//! Every refusal is triggered by a C program at `-O0` and at `-O2`.
use bennett_front_tests::{CLANG_FLAGS, CLANG_O2_FLAGS, clang, front, out_dir, root, write_c};
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

fn handles_tool() -> PathBuf {
    front().with_file_name("bennett-handles")
}

fn output(command: &mut Command) -> Output {
    command.output().unwrap_or_else(|e| panic!("cannot run {command:?}: {e}"))
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn checked(command: &mut Command) -> Output {
    let result = output(command);
    assert!(result.status.success(), "{command:?}: {}", stderr(&result));
    result
}

/// Bitcode of `source` at a level, with the header included by force, as the driver compiles a unit.
fn bitcode(source: &Path, name: &str, flags: &[&str]) -> PathBuf {
    let bitcode = out_dir().join(format!("{name}.bc"));
    checked(
        Command::new(clang())
            .args(flags)
            .arg("-include")
            .arg(header())
            .args(["-emit-llvm", "-c"])
            .arg(source)
            .arg("-o")
            .arg(&bitcode),
    );
    bitcode
}

/// `bennett-front` on `source` at a level.
fn front_on(source: &Path, name: &str, flags: &[&str]) -> Output {
    let bitcode = bitcode(source, name, flags);
    output(
        Command::new(front())
            .arg(&bitcode)
            .arg("-o")
            .arg(out_dir().join(format!("{name}.bn")))
            .args(["--region-budget=64", "--stack-bytes=4096"]),
    )
}

/// The unit's two filtered declaration dumps (`front/src/UnitDeclarations.h`), with `include` deciding whether
/// the header is included by force, checked by `bennett-handles check-unit`.
fn unit_check_on(source: &Path, name: &str, flags: &[&str], include: bool) -> Output {
    let mut dump = Vec::new();
    for filter in ["bq_", "__bennett_"] {
        let mut command = Command::new(clang());
        command.args(flags);
        if include {
            command.arg("-include").arg(header());
        }
        let result = checked(
            command
                .args(["-fsyntax-only", "-Xclang", "-ast-dump=json", "-Xclang"])
                .arg(format!("-ast-dump-filter={filter}"))
                .arg(source),
        );
        dump.extend_from_slice(&result.stdout);
    }
    let path = out_dir().join(format!("{name}.declarations.json"));
    std::fs::write(&path, dump).unwrap_or_else(|e| panic!("cannot write {path:?}: {e}"));
    output(Command::new(handles_tool()).arg("check-unit").arg("--header").arg(header()).arg(&path))
}

fn assert_refused(result: &Output, status: i32, expected: &str, context: &str) {
    let text = stderr(result);
    assert_eq!(result.status.code(), Some(status), "{context}: {text}");
    assert!(text.contains(expected), "{context}: expected `{expected}` in: {text}");
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

/// A conforming program passes both checks; `bennett-front` then stops, by name, at the translation of its
/// calls, which waits on the device forms.
#[test]
fn a_conforming_program_passes_the_declaration_rule() {
    let source = write_c("handles_conforming", CONFORMING);
    for (level, flags) in LEVELS {
        let name = format!("handles_conforming.{level}");
        let unit = unit_check_on(&source, &name, flags, true);
        assert!(unit.status.success(), "{name}: {}", stderr(&unit));
        let result = front_on(&source, &name, flags);
        assert_refused(&result, NOT_HANDLED, "translating its calls into device instructions waits", &name);
    }
}

/// Each way the module shows a break of the declaration rule, refused by `bennett-front` by name.
const MODULE_CASES: &[(&str, &str, &str)] = &[
    (
        "definition",
        "void bq_free(bq_reg r) { (void)r; }\nint main(void) { bq_free(bq_alloc(1)); return 0; }",
        "the program defines the primitive @bq_free",
    ),
    (
        "attribute",
        "uint64_t bq_width(bq_reg) __attribute__((const));\nint main(void) { return (int)bq_width(bq_alloc(1)); }",
        "the primitive @bq_width is declared with `",
    ),
    (
        "noreturn",
        "__attribute__((noreturn)) void bq_free(bq_reg);\nint main(void) { bq_free(bq_alloc(1)); }",
        "the primitive @bq_free is declared with `noreturn`",
    ),
    (
        "weak",
        "__attribute__((weak)) void bq_free(bq_reg);\nint main(void) { bq_free(bq_alloc(1)); return 0; }",
        "the primitive @bq_free is declared weak",
    ),
    (
        "other_type_label",
        "int mine(int) __asm__(\"bq_free\");\nint main(void) { bq_free(bq_alloc(1)); return mine(3); }",
        "a call of the primitive @bq_free through the type i32 (i32)",
    ),
    (
        "renamed_onto_a_primitive",
        "#pragma redefine_extname ordinary bq_free\nvoid ordinary(void);\nint main(void) { ordinary(); return 0; }",
        "the primitive @bq_free is declared with the type void ()",
    ),
    (
        "alias",
        "void x(bq_reg r) { (void)r; }\nvoid bq_free(bq_reg) __attribute__((alias(\"x\")));\n\
         int main(void) { bq_free(bq_alloc(1)); return 0; }",
        "the program defines an alias named after the primitive @bq_free",
    ),
    (
        "variable",
        "extern int var __asm__(\"bq_alloc\");\nint main(void) { return var; }",
        "the program has a variable with the linker name of the primitive @bq_alloc",
    ),
    (
        "undecorated_label",
        "void mine(bq_reg) __asm__(\"\\x01\" \"bq_free\");\nint main(void) { mine(bq_alloc(1)); return 0; }",
        "gives a symbol the linker name of the primitive @bq_free with an assembler label",
    ),
];

#[test]
fn the_module_check_refuses_each_break_by_name() {
    for (case, text, expected) in MODULE_CASES {
        let source = write_c(&format!("handles_module_{case}"), text);
        for (level, flags) in LEVELS {
            let name = format!("handles_module_{case}.{level}");
            assert_refused(&front_on(&source, &name, flags), REFUSED, expected, &name);
        }
    }
}

/// Each break of the rule that the parsed form shows, refused by `bennett-handles check-unit` by name.
const UNIT_CASES: &[(&str, &str, &str)] = &[
    (
        "rename",
        "#pragma redefine_extname bq_free ordinary_release\n\
         void ordinary_release(bq_reg r) { (void)r; }\nint main(void) { bq_free(bq_alloc(1)); return 0; }",
        "the header's declaration of the primitive bq_free has the linker name `ordinary_release`",
    ),
    (
        "redeclaration",
        "int main(void) { extern void bq_free(bq_reg); bq_free(bq_alloc(1)); return 0; }",
        "a declaration of the primitive bq_free outside the handle header",
    ),
    (
        "definition",
        "void bq_free(bq_reg r) { (void)r; }\nint main(void) { return 0; }",
        "a definition of the primitive bq_free outside the handle header",
    ),
    (
        "alias",
        "void x(bq_reg r) { (void)r; }\nvoid bq_free(bq_reg) __attribute__((alias(\"x\")));\nint main(void) { return 0; }",
        "a definition of the primitive bq_free outside the handle header",
    ),
    (
        "label",
        "void bq_mine(bq_reg) __asm__(\"bq_free\");\nint main(void) { bq_mine(bq_alloc(1)); return 0; }",
        "the declaration of bq_mine has the linker name of the primitive bq_free",
    ),
    (
        "effect_redeclaration",
        "void __bennett_effect(char byte);\nint main(void) { __bennett_effect('a'); return 0; }",
        "a declaration of the primitive __bennett_effect outside the handle header",
    ),
];

#[test]
fn the_unit_check_refuses_each_break_by_name() {
    for (case, text, expected) in UNIT_CASES {
        let source = write_c(&format!("handles_unit_{case}"), text);
        for (level, flags) in LEVELS {
            let name = format!("handles_unit_{case}.{level}");
            assert_refused(&unit_check_on(&source, &name, flags, true), REFUSED, expected, &name);
        }
    }
}

/// The rename of review finding R1 is invisible in the module: `bennett-front` sees an ordinary function, so
/// only the unit check refuses it (above). This pins that the module check is not what refuses it.
#[test]
fn the_rename_needs_the_unit_check() {
    let source = write_c(
        "handles_rename_module",
        "#pragma redefine_extname bq_free ordinary_release\nint main(void) { bq_free(bq_alloc(1)); return 0; }",
    );
    for (level, flags) in LEVELS {
        let name = format!("handles_rename_module.{level}");
        assert_refused(&front_on(&source, &name, flags), NOT_HANDLED, "@bq_alloc", &name);
    }
}

/// A unit compiled without the forced header has no header declaration of the primitives.
#[test]
fn a_unit_without_the_header_is_refused() {
    let source = write_c(
        "handles_no_header",
        "typedef struct { unsigned long long id; } bq_reg;\nbq_reg bq_alloc(unsigned long long);\n\
         int main(void) { return (int)bq_alloc(1).id; }",
    );
    for (level, flags) in LEVELS {
        let name = format!("handles_no_header.{level}");
        assert_refused(
            &unit_check_on(&source, &name, flags, false),
            REFUSED,
            "a declaration of the primitive bq_alloc outside the handle header",
            &name,
        );
    }
    let empty = write_c("handles_no_primitive", "int main(void) { return 0; }");
    for (level, flags) in LEVELS {
        let name = format!("handles_no_primitive.{level}");
        assert_refused(
            &unit_check_on(&empty, &name, flags, false),
            REFUSED,
            "the unit has no declaration of the primitive bq_alloc from the handle header",
            &name,
        );
    }
}

/// A program that uses a marker and an interface function is refused by name: the two interfaces are not mixed
/// (a choice that is cheap to reverse; `docs/DESIGN.md`, debts).
#[test]
fn a_program_with_markers_and_handles_is_refused() {
    let source = write_c(
        "handles_mixed",
        "void bennett_h(_Bool *qubit);\nint main(void) { _Bool q = 0; bennett_h(&q); bq_free(bq_alloc(1)); \
         return q; }",
    );
    for (level, flags) in LEVELS {
        let name = format!("handles_mixed.{level}");
        assert_refused(
            &front_on(&source, &name, flags),
            REFUSED,
            "the program uses both the marker interface (@bennett_h",
            &name,
        );
    }
}

/// The four reserved effect primitives and one interface function, each with its LLVM type, the operands of a call
/// and whether the header declares it `noreturn`.
const RESERVED: &[(&str, &str, &str, bool)] = &[
    ("__bennett_effect", "void (i8)", "i8 1", false),
    ("__bennett_exit", "void (i32)", "i32 1", true),
    ("__bennett_out_of_memory", "void ()", "", true),
    ("__bennett_bad_argument", "void ()", "", true),
    ("bq_free", "void (i64)", "i64 1", false),
];

/// The shapes in which a program can name a reserved primitive so that LLVM may drop or redirect its call, as
/// LLVM text that clang can write or a program can hand to the front: attributes on a direct call, an alias or an
/// ifunc in its place, and attributes on a call through a pointer that may name it.
fn reserved_shapes(name: &str, ty: &str, args: &str, noreturn: bool) -> Vec<(&'static str, String, String)> {
    let params = ty.trim_start_matches("void ").trim_matches(|c| c == '(' || c == ')');
    let declare = format!("declare void @{name}({params}){}\n", if noreturn { " noreturn" } else { "" });
    let body = format!("define internal void @replacement({params}) {{\n  ret void\n}}\n");
    let main_direct = format!("define i32 @main() {{\n  call void @{name}({args})\n  ret i32 7\n}}\n");
    vec![
        (
            "call_attributes",
            format!(
                "{declare}define i32 @main() {{\n  call void @{name}({args}) nounwind willreturn memory(none)\n  ret i32 7\n}}\n"
            ),
            format!("a call of the primitive @{name} carries `"),
        ),
        (
            "alias",
            format!("@{name} = alias {ty}, ptr @replacement\n{body}{main_direct}"),
            format!("an alias named after the primitive @{name}"),
        ),
        (
            "ifunc",
            format!(
                "@{name} = ifunc {ty}, ptr @resolve\ndefine internal ptr @resolve() {{\n  ret ptr @replacement\n}}\n{body}{main_direct}"
            ),
            format!("an ifunc) named after the primitive @{name}"),
        ),
        (
            "indirect",
            format!(
                "{declare}@target = internal constant ptr @{name}\ndefine i32 @main() {{\n  %callee = load ptr, ptr @target\n  \
                 call void %callee({args}) nounwind willreturn memory(none)\n  ret i32 7\n}}\n"
            ),
            format!("takes the address of the primitive @{name}"),
        ),
    ]
}

/// Each shape is refused by name, before any code is generated, for every reserved effect primitive and an
/// interface function, whether or not the program includes the handle header (these modules do not).
#[test]
fn a_reserved_primitive_cannot_be_dropped_or_redirected() {
    for &(name, ty, args, noreturn) in RESERVED {
        for (shape, text, expected) in reserved_shapes(name, ty, args, noreturn) {
            let tag = format!("handles_reserved_{name}_{shape}");
            let path = out_dir().join(format!("{tag}.ll"));
            std::fs::write(&path, &text).unwrap_or_else(|e| panic!("cannot write {path:?}: {e}"));
            let result = output(
                Command::new(front())
                    .arg(&path)
                    .arg("-o")
                    .arg(out_dir().join(format!("{tag}.bn")))
                    .args(["--region-budget=64", "--stack-bytes=4096"]),
            );
            assert_refused(&result, REFUSED, &expected, &format!("{tag}:\n{text}"));
        }
    }
}

/// A call through a pointer with such an attribute is the program's own business where no primitive's address is
/// taken: the check does not refuse it.
#[test]
fn an_indirect_call_without_a_primitive_address_is_not_refused_by_the_check() {
    let text = "declare void @helper()\n@target = internal constant ptr @helper\ndefine i32 @main() {\n  \
                %callee = load ptr, ptr @target\n  call void %callee() nounwind willreturn memory(none)\n  ret i32 7\n}\n";
    let path = out_dir().join("handles_indirect_helper.ll");
    std::fs::write(&path, text).unwrap_or_else(|e| panic!("cannot write {path:?}: {e}"));
    let result = output(
        Command::new(front())
            .arg(&path)
            .arg("-o")
            .arg(out_dir().join("handles_indirect_helper.bn"))
            .args(["--region-budget=64", "--stack-bytes=4096"]),
    );
    assert!(!stderr(&result).contains("takes the address of the primitive"), "{}", stderr(&result));
}
