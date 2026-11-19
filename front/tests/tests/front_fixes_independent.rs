//! Independent variations on resumption, unused effects, and delayed gate controls.
use bennett_front_tests::{CLANG_FLAGS, CLANG_O2_FLAGS, lower_text, write_c};

fn refused_in_every_mode(name: &str, text: &str, expected: &str) {
    let source = write_c(name, text);
    let modes: [(&[&str], &[&str], &str); 3] =
        [(CLANG_FLAGS, &[], "o0"), (CLANG_O2_FLAGS, &[], "o2"), (CLANG_FLAGS, &["--taint-all"], "taint")];
    for (clang, front, tag) in modes {
        match lower_text(&source, &format!("{name}_{tag}"), clang, front) {
            Ok(program) => panic!("{name} {tag}: accepted\n{program}"),
            Err(error) => {
                assert!(error.starts_with("not handled yet: ") && error.contains(expected), "{name} {tag}: {error}")
            }
        }
    }
}

#[test]
fn nonlocal_continuations_are_refused_for_each_setjmp_spelling() {
    for (name, env, save, jump) in [
        ("plain", "jmp_buf", "setjmp(env)", "longjmp(env, 2)"),
        ("underscore", "jmp_buf", "_setjmp(env)", "_longjmp(env, 2)"),
        ("signal", "sigjmp_buf", "sigsetjmp(env, 0)", "siglongjmp(env, 2)"),
        ("builtin", "void *", "__builtin_setjmp(env)", "__builtin_longjmp(env, 1)"),
    ] {
        let declaration = if name == "builtin" { "void *env[5]".to_string() } else { format!("{env} env") };
        let text = format!(
            "#include \"bennett.h\"\n#include <setjmp.h>\n{declaration};\nint main(void) {{\nvolatile _Bool q=0; _Bool y=0;\nif ({save}==0) {{ bennett_x((_Bool*)&q); {jump}; }}\nif(q) bennett_x(&y);\nbennett_measure((const void*)&q);\nreturn bennett_measure(&y);\n}}\n"
        );
        refused_in_every_mode(&format!("ind_resumption_{name}"), &text, "a setjmp in a function with qubit variables");
    }
}

#[test]
fn unused_atomic_and_volatile_effect_variants_do_not_disappear() {
    for (name, declarations, statement, instruction) in [
        ("exchange_value", "int cells[2];", "(void)__atomic_exchange_n(&cells[0], q, __ATOMIC_SEQ_CST);", "atomicrmw"),
        (
            "exchange_address",
            "int cells[2];",
            "(void)__atomic_exchange_n(&cells[q], 7, __ATOMIC_RELAXED);",
            "atomicrmw",
        ),
        (
            "cmpxchg",
            "int cells[2]; int expected;",
            "(void)__atomic_compare_exchange_n(&cells[q], &expected, 7, 0, __ATOMIC_SEQ_CST, __ATOMIC_SEQ_CST);",
            "cmpxchg",
        ),
        ("volatile_pointer", "volatile int cells[2];", "volatile int *p=q ? &cells[0] : &cells[1]; (void)*p;", "load"),
        ("relaxed_read", "int cells[2];", "(void)__atomic_load_n(&cells[q], __ATOMIC_RELAXED);", "load"),
    ] {
        let text = format!(
            "#include \"bennett.h\"\n{declarations}\nint main(void) {{\n_Bool q=0; bennett_h(&q);\n{statement}\nreturn bennett_measure(&q);\n}}\n"
        );
        // clang stores the old value after a `cmpxchg` only where the exchange failed, a condition read from
        // memory: that store's bytes would hold qubits from before the code also where it does not run, and when the
        // function returns, which is refused first (docs/DESIGN.md, "Memory written under a qubit"). The others
        // refuse the effect itself.
        let expected = if instruction == "cmpxchg" {
            String::from("from whose exchange some ordinary path does not write every byte")
        } else {
            format!("an effect on memory (the instruction `{instruction}`) that depends on a qubit")
        };
        refused_in_every_mode(&format!("ind_effect_{name}"), &text, &expected);
    }
}

#[test]
fn literal_controls_reject_stale_reads_through_negation_conjunction_and_updates() {
    for (name, preparation, mutation, condition, gate) in [
        ("negative", "_Bool old=q;", "bennett_x(&q);", "!old", "bennett_x(&y)"),
        ("conjunction", "_Bool old=q;", "bennett_x(&q);", "old && r", "bennett_x(&y)"),
        ("store", "_Bool old=q;", "q ^= 1;", "old", "bennett_x(&y)"),
        ("loop", "_Bool old=q;", "for(int i=0;i<n;i++) bennett_x(&q);", "old", "bennett_x(&y)"),
        ("rotation", "_Bool old=q;", "bennett_h(&q);", "old", "bennett_rz(&y, 0.375)"),
    ] {
        let text = format!(
            "#include \"bennett.h\"\nint test(int n) {{\n_Bool q=0,y=0,r=0; bennett_h(&q); bennett_h(&r);\n{preparation}\n{mutation}\nif({condition}) {gate};\nbennett_measure(&q); bennett_measure(&r);\nreturn bennett_measure(&y);\n}}\n"
        );
        refused_in_every_mode(&format!("ind_stale_{name}"), &text, "after the variable changed");
    }
}

#[test]
fn current_volatile_qubit_reads_still_control_gates() {
    let text = "#include \"bennett.h\"\nint main(void) {\nvolatile _Bool q=0; _Bool y=0;\nbennett_h((_Bool*)&q);\nif(q) bennett_x(&y);\nbennett_x((_Bool*)&q);\nif(!q) bennett_x(&y);\nbennett_measure((const void*)&q);\nreturn bennett_measure(&y);\n}\n";
    let source = write_c("ind_current_volatile", text);
    for (index, (clang, front)) in
        [(CLANG_FLAGS, &[][..]), (CLANG_O2_FLAGS, &[][..]), (CLANG_FLAGS, &["--taint-all"][..])].into_iter().enumerate()
    {
        let program = lower_text(&source, &format!("ind_current_volatile_{index}"), clang, front)
            .expect("current controls are valid");
        let module = bennett_ir::load(&program).expect("valid emitted program");
        for seed in 0..8 {
            assert_eq!(bennett_front_tests::run(&module, "main", &[], seed), bennett_exec::Outcome::Returned(Some(0)));
        }
    }
}
