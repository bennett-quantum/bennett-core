//! The front end's own part of the handle interface (`docs/HANDLES.md`, sections 4 and 14): the declaration
//! rule, checked on each unit's parsed form by the driver `bennett-cc` before it writes bitcode and on the module
//! the front reads (`bennett-front`); calls of a primitive through a pointer; and the refusal of a program that
//! uses both the marker interface and the handle interface. Every refusal is triggered by a C program at `-O0`
//! and at `-O2`, and the shapes only LLVM text can write are also given as LLVM text.
use bennett_exec::{Executor, Outcome};
use bennett_front_tests::{CLANG_FLAGS, CLANG_O2_FLAGS, clang, entry, front, options, out_dir, root, write_c};
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

fn driver() -> PathBuf {
    front().with_file_name("bennett-cc")
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

/// `bennett-cc` on `source` at a level, with `extra` arguments for clang: the documented compilation of a unit,
/// which checks its parsed form first. The bitcode, if any, is `<name>.cc.bc`, removed before the run.
fn driver_on(source: &Path, name: &str, flags: &[&str], extra: &[&str]) -> (Output, PathBuf) {
    let bitcode = out_dir().join(format!("{name}.cc.bc"));
    let _ = std::fs::remove_file(&bitcode);
    let result = output(Command::new(driver()).args(flags).args(extra).arg("-o").arg(&bitcode).arg(source));
    (result, bitcode)
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

/// `bennett-front` on LLVM text written to `<name>.ll`.
fn front_on_text(text: &str, name: &str) -> Output {
    let path = out_dir().join(format!("{name}.ll"));
    std::fs::write(&path, text).unwrap_or_else(|e| panic!("cannot write {path:?}: {e}"));
    front_of(&path, name)
}

/// The documented path, `bennett-cc` then `bennett-front`, then a run of `main`: the outcome and the console.
fn run_through_driver(source: &Path, name: &str, flags: &[&str]) -> (Outcome, Vec<u8>) {
    let (compiled, bitcode) = driver_on(source, name, flags, &[]);
    assert!(compiled.status.success(), "{name}: {}", stderr(&compiled));
    let lowered = front_of(&bitcode, name);
    assert!(lowered.status.success(), "{name}: {}", stderr(&lowered));
    let path = out_dir().join(format!("{name}.bn"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {path:?}: {e}"));
    let module = bennett_ir::load(&text).unwrap_or_else(|e| panic!("{name}: invalid device program: {e}"));
    let mut executor = Executor::new(&module, entry(&module, "main"), &[], options(0)).expect("starts");
    let outcome = executor.run();
    (outcome, executor.tape().to_vec())
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

/// A conforming program passes the driver's check; `bennett-front` then stops, by name, at the translation of
/// its calls, which waits on the device forms.
#[test]
fn a_conforming_program_passes_the_declaration_rule() {
    let source = write_c("handles_conforming", CONFORMING);
    for (level, flags) in LEVELS {
        let name = format!("handles_conforming.{level}");
        let (unit, bitcode) = driver_on(&source, &name, flags, &[]);
        assert!(unit.status.success(), "{name}: {}", stderr(&unit));
        let result = front_of(&bitcode, &name);
        assert_refused(&result, NOT_HANDLED, "translating its calls into device instructions waits", &name);
    }
}

/// Each way the module shows a break of the declaration rule, refused by `bennett-front` by name. These units are
/// compiled by clang alone, with the header included by force, so that the module check is what refuses them.
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
        "a call of the device function @bq_free through the type i32 (i32)",
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
        "the primitive @bq_free is defined by an alias",
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
    // Review finding R8: the calling convention is part of the header's declaration.
    (
        "convention",
        "void mine(char) __attribute__((ms_abi)) __asm__(\"__bennett_effect\");\n\
         int main(void) { mine('a'); return 0; }",
        "the primitive @__bennett_effect is declared with the calling convention win64cc",
    ),
    (
        "visibility",
        "void mine(char) __attribute__((visibility(\"hidden\"))) __asm__(\"__bennett_effect\");\n\
         int main(void) { mine('a'); return 0; }",
        "the primitive @__bennett_effect is declared with a visibility or DLL storage other than the default",
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

/// Review finding R8 as the reviewer wrote it: an unused declaration with another calling convention, which
/// clang does not write for an unused C declaration.
#[test]
fn an_unused_declaration_with_another_convention_is_refused() {
    let result = front_on_text(
        "declare fastcc void @__bennett_effect(i8)\ndefine i32 @main() {\n  ret i32 7\n}\n",
        "handles_unused_convention",
    );
    assert_refused(&result, REFUSED, "is declared with the calling convention fastcc", "unused fastcc");
}

/// Each break of the rule that the parsed form shows, refused by `bennett-cc` by name, before it writes bitcode.
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
    // Review finding R1: an unused redeclaration leaves nothing in the module; the driver refuses it.
    (
        "unused_redeclaration",
        "void __bennett_effect(char);\nint main(void) { return 7; }",
        "a declaration of the primitive __bennett_effect outside the handle header",
    ),
    // Review finding R2: a declaration selected by its linker name alone, whatever its identifier.
    (
        "unused_label",
        "extern void release(bq_reg) __asm__(\"bq_free\");\nint main(void) { return 7; }",
        "the declaration of release has the linker name of the primitive bq_free",
    ),
    (
        "unused_variable_label",
        "extern int status __asm__(\"__bennett_exit\");\nint main(void) { return 7; }",
        "the declaration of status has the linker name of the primitive __bennett_exit",
    ),
    (
        "block_scope_label",
        "int main(void) { extern void quit(int) __asm__(\"__bennett_exit\"); return 7; }",
        "the declaration of quit has the linker name of the primitive __bennett_exit",
    ),
    // Review finding R5: a pragma that makes the header's own declaration weak.
    (
        "unused_weak",
        "#pragma weak bq_free\nint main(void) { return 7; }",
        "the header's declaration of the primitive bq_free has the attribute WeakAttr",
    ),
    (
        "used_weak",
        "#pragma weak __bennett_effect\nint main(void) { __bennett_effect('a'); return 7; }",
        "the header's declaration of the primitive __bennett_effect has the attribute WeakAttr",
    ),
];

#[test]
fn the_unit_check_refuses_each_break_by_name() {
    for (case, text, expected) in UNIT_CASES {
        let source = write_c(&format!("handles_unit_{case}"), text);
        for (level, flags) in LEVELS {
            let name = format!("handles_unit_{case}.{level}");
            let (result, bitcode) = driver_on(&source, &name, flags, &[]);
            assert_refused(&result, REFUSED, expected, &name);
            assert!(!bitcode.exists(), "{name}: a refused unit has no bitcode");
        }
    }
}

/// The rename of review finding R1 of the document is invisible in the module: `bennett-front` sees an ordinary
/// function, so only the driver's unit check refuses it (above). This pins that the module check is not what
/// refuses it.
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

/// A unit in which the header declares nothing (its guard macro defined beforehand) is refused: by its own
/// declaration of a primitive, or, with none, by the header's missing declarations.
#[test]
fn a_unit_without_the_header_is_refused() {
    let guard = ["-DBENNETT_BQ_H_INCLUDED"];
    let source = write_c(
        "handles_no_header",
        "typedef struct { unsigned long long id; } bq_reg;\nbq_reg bq_alloc(unsigned long long);\n\
         int main(void) { return (int)bq_alloc(1).id; }",
    );
    for (level, flags) in LEVELS {
        let name = format!("handles_no_header.{level}");
        assert_refused(
            &driver_on(&source, &name, flags, &guard).0,
            REFUSED,
            "a declaration of the primitive bq_alloc outside the handle header",
            &name,
        );
    }
    let empty = write_c("handles_no_primitive", "int main(void) { return 0; }");
    for (level, flags) in LEVELS {
        let name = format!("handles_no_primitive.{level}");
        assert_refused(
            &driver_on(&empty, &name, flags, &guard).0,
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

/// Review finding R4: a declaration nothing uses is checked, and is no use of an interface: an ordinary module
/// with an unused interface declaration and an unused marker declaration is translated and returns 7.
#[test]
fn an_unused_declaration_is_no_use_of_an_interface() {
    let result = front_on_text(
        "declare void @bq_free(i64)\ndeclare void @bennett_h(ptr)\ndefine i32 @main() {\n  ret i32 7\n}\n",
        "handles_unused_declarations",
    );
    assert!(result.status.success(), "{}", stderr(&result));
}

/// Review finding R3: in a program that takes a primitive's address, a call through a pointer of another type is
/// the program's own (here a callback that takes a structure by value, `byval` in LLVM), translated as before.
/// Review finding R7: a call of a primitive through an ordinary function pointer reaches the primitive.
#[test]
fn calls_through_pointers_reach_the_primitive_and_only_it() {
    let programs: &[(&str, &str, Outcome, &[u8])] = &[
        (
            "byval_callback",
            "struct packet { long x[3]; };\nint first(struct packet p) { return p.x[0]; }\n\
             int (*volatile reader)(struct packet) = first;\nint main(void) {\n\
             void (*volatile unused)(char) = __bennett_effect; (void)unused;\n\
             return reader((struct packet){{7}});\n}\n",
            Outcome::Returned(Some(7)),
            b"",
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

/// The four reserved effect primitives and one interface function, each with its LLVM type, the operands of a call
/// and whether the header declares it `noreturn`; and the same as C: the parameter list and the arguments.
const RESERVED: &[(&str, &str, &str, bool, &str, &str)] = &[
    ("__bennett_effect", "void (i8)", "i8 1", false, "char", "'A'"),
    ("__bennett_exit", "void (i32)", "i32 1", true, "int", "3"),
    ("__bennett_out_of_memory", "void ()", "", true, "void", ""),
    ("__bennett_bad_argument", "void ()", "", true, "void", ""),
    ("bq_free", "void (i64)", "i64 1", false, "bq_reg", "bq_alloc(1)"),
];

/// The shapes in which a program can name a reserved primitive so that LLVM may drop or redirect its call, as
/// LLVM text that clang can write or a program can hand to the front: attributes on a direct call, an alias or an
/// ifunc in its place, attributes on a call through a pointer that may name it, and `noreturn` on such a call to
/// a primitive that returns.
fn reserved_shapes(name: &str, ty: &str, args: &str, noreturn: bool) -> Vec<(&'static str, String, String)> {
    let params = ty.trim_start_matches("void ").trim_matches(|c| c == '(' || c == ')');
    let declare = format!("declare void @{name}({params}){}\n", if noreturn { " noreturn" } else { "" });
    let body = format!("define internal void @replacement({params}) {{\n  ret void\n}}\n");
    let main_direct = format!("define i32 @main() {{\n  call void @{name}({args})\n  ret i32 7\n}}\n");
    let indirect = |attributes: &str| {
        format!(
            "{declare}@target = internal constant ptr @{name}\ndefine i32 @main() {{\n  %callee = load ptr, ptr @target\n  \
             call void %callee({args}) {attributes}\n  ret i32 7\n}}\n"
        )
    };
    let mut shapes = vec![
        (
            "call_attributes",
            format!(
                "{declare}define i32 @main() {{\n  call void @{name}({args}) nounwind willreturn memory(none)\n  ret i32 7\n}}\n"
            ),
            format!("a call of the device function @{name} has attributes that let LLVM remove or move its calls"),
        ),
        (
            "alias",
            format!("@{name} = alias {ty}, ptr @replacement\n{body}{main_direct}"),
            format!("the primitive @{name} is defined by an alias"),
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
            indirect("nounwind willreturn memory(none)"),
            format!("takes the address of the primitive @{name}, has attributes that let LLVM remove or move"),
        ),
        (
            "indirect_convention",
            indirect("").replace("call void %callee", "call fastcc void %callee"),
            format!("takes the address of the primitive @{name}, uses the calling convention fastcc"),
        ),
    ];
    if !noreturn {
        shapes.push((
            "indirect_noreturn",
            indirect("noreturn"),
            format!("takes the address of the primitive @{name}, is marked `noreturn`"),
        ));
    }
    shapes
}

/// Each shape is refused by name, before any code is generated, for every reserved effect primitive and an
/// interface function, whether or not the program includes the handle header (these modules do not).
#[test]
fn a_reserved_primitive_cannot_be_dropped_or_redirected() {
    for &(name, ty, args, noreturn, _, _) in RESERVED {
        for (shape, text, expected) in reserved_shapes(name, ty, args, noreturn) {
            let tag = format!("handles_reserved_{name}_{shape}");
            assert_refused(&front_on_text(&text, &tag), REFUSED, &expected, &format!("{tag}:\n{text}"));
        }
    }
}

/// Review finding R9: the same shapes as C, where C can write them, through clang's optimiser at both levels: an
/// alias and an ifunc in a primitive's place, refused by the driver and, compiled by clang alone, by the module
/// check; and a call through a `noreturn` function pointer, refused where the primitive returns. (Attributes on a
/// call of a `void` primitive cannot be written in C: clang ignores `const` and `pure` on a `void` function and on
/// a function-pointer type; the `const` case of a primitive with a result is in the module cases above.)
#[test]
fn the_reserved_shapes_written_in_c_are_refused() {
    for &(name, _, _, noreturn, params, args) in RESERVED {
        let other = format!("void other({params}) {{}}\n");
        let mut shapes = vec![
            (
                "alias",
                format!(
                    "{other}void {name}({params}) __attribute__((alias(\"other\")));\nint main(void) {{ {name}({args}); return 7; }}\n"
                ),
                format!("a definition of the primitive {name} outside the handle header"),
                format!("the primitive @{name} is defined by an alias"),
            ),
            (
                "ifunc",
                format!(
                    "{other}static void (*resolve(void))({params}) {{ return other; }}\n\
                     void {name}({params}) __attribute__((ifunc(\"resolve\")));\nint main(void) {{ {name}({args}); return 7; }}\n"
                ),
                format!("a definition of the primitive {name} outside the handle header"),
                format!("an ifunc) named after the primitive @{name}"),
            ),
        ];
        if !noreturn {
            shapes.push((
                "noreturn_pointer",
                format!(
                    "typedef void (*__attribute__((noreturn)) never)({params});\n\
                     static never volatile target = (never){name};\nint main(void) {{ target({args}); }}\n"
                ),
                String::new(),
                format!("takes the address of the primitive @{name}, is marked `noreturn`"),
            ));
        }
        for (shape, text, unit, module) in shapes {
            let source = write_c(&format!("handles_c_{name}_{shape}"), &text);
            for (level, flags) in LEVELS {
                let tag = format!("handles_c_{name}_{shape}.{level}");
                if !unit.is_empty() {
                    assert_refused(&driver_on(&source, &tag, flags, &[]).0, REFUSED, &unit, &tag);
                }
                assert_refused(&front_on(&source, &tag, flags), REFUSED, &module, &tag);
            }
        }
    }
}

/// A call through a pointer with such an attribute is the program's own business where no primitive's address is
/// taken, or where its type is no taken primitive's: the check does not refuse it.
#[test]
fn an_indirect_call_that_cannot_reach_a_primitive_is_not_refused_by_the_check() {
    let cases = [
        ("handles_indirect_helper", "declare void @helper()\n@target = internal constant ptr @helper\n"),
        (
            "handles_indirect_other_type",
            "declare void @helper()\ndeclare void @__bennett_effect(i8)\n\
             @target = internal constant ptr @helper\n@taken = constant ptr @__bennett_effect\n",
        ),
    ];
    for (name, globals) in cases {
        let text = format!(
            "{globals}define i32 @main() {{\n  %callee = load ptr, ptr @target\n  \
             call void %callee() nounwind willreturn memory(none)\n  ret i32 7\n}}\n"
        );
        let result = front_on_text(&text, name);
        assert!(!stderr(&result).contains("takes the address of the primitive"), "{name}: {}", stderr(&result));
    }
}
