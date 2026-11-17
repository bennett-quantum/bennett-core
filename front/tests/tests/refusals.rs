//! What this version does not handle under qubit control, or in place, is refused by name at the
//! construct's source line, never compiled to something cheaper or approximate.

use bennett_front_tests::{CLANG_FLAGS, CLANG_O2_FLAGS, lower_text, write_c};

/// The program around each case: qubit variables a, b, c (one bit), x and p (8 bits), with a, every bit
/// of x and the low bit of p in superposition. The case's body is on line `BODY_LINE`.
const PRELUDE: &str = "#include \"bennett.h\"
long sink;
__attribute__((noinline)) void touch(unsigned char *v) { *v += 1; }
int main(int n, char **argv) {
  (void)argv;
  _Bool a = 0, b = 0, c = 0;
  unsigned char x = 0, p = 0;
  bennett_h(&a); for (int i = 0; i < 8; i++) bennett_h(bennett_bit(&x, i)); bennett_h(bennett_bit(&p, 0));
";
const BODY_LINE: usize = 9;
const EPILOGUE: &str = "
  unsigned long long r = bennett_measure(&a) + bennett_measure(&b) + bennett_measure(&c);
  return (int)(r + bennett_measure(&x) + bennett_measure(&p));
}
";

#[test]
fn what_cannot_run_under_qubit_control_or_in_place_is_refused_by_name_at_its_line() {
    let cases = [
        // A gate that changes its own condition is not a controlled gate.
        (
            "gate_on_its_condition",
            "if (a) bennett_x(&a);",
            "a gate on qubit variable 'a' under a condition that depends on 'a'",
        ),
        (
            "gate_on_its_condition_bit",
            "if (x & 2) bennett_x(bennett_bit(&x, 0));",
            "a gate on qubit variable 'x' under a condition that depends on 'x'",
        ),
        // Updates with no inverse the front knows keep the old value since 2026-11-17 (tests/retained.rs).
        (
            "branchy_update",
            "if (a && p != 0 && 10 / p == 5) x += 1;",
            "an update of qubit variable 'x' under the control of a value that depends on a qubit, which neither",
        ),
        // A loop with gates whose number of passes depends on a qubit: the circuit model needs it ordinary.
        (
            "loop",
            "for (unsigned char i = 0; i < p; i++) bennett_x(&c);",
            "a loop with gates whose number of passes depends on a qubit",
        ),
        ("measurement", "if (a) bennett_measure(&b);", "a measurement under the control of a value"),
        (
            "stale",
            "bennett_h(&b); if (a) { _Bool t = b; bennett_x(&b); if (t) bennett_x(&c); }",
            "the value of qubit variable 'b' read here decides a quantum operation after the variable changed",
        ),
        (
            "read_after_change",
            "bennett_h(&b); _Bool t = 0; if (a) { bennett_x(&b); t = b; } c ^= t;",
            "the value of qubit variable 'b' read here, after a quantum operation",
        ),
        // The marker for one bit of a wider variable.
        (
            "bit_index",
            "bennett_h(bennett_bit(&x, (unsigned)n & 7));",
            "bennett_bit on qubit variable 'x' with a bit index",
        ),
        ("bit_escapes", "sink = (long)bennett_bit(&x, 1);", "the result of bennett_bit used other than as the qubit"),
        // What was refused before stays refused.
        ("passed", "touch(&x);", "qubit variable 'x' is passed to a function"),
        ("ordinary_use", "sink = x;", "used by ordinary code"),
    ];
    for (name, body, expected) in cases {
        let text = format!("{PRELUDE}  {body}{EPILOGUE}");
        assert_eq!(text.lines().nth(BODY_LINE - 1).map(str::trim), Some(body));
        let file = format!("refused_{name}");
        let source = write_c(&file, &text);
        match lower_text(&source, &file, CLANG_FLAGS, &[]) {
            Ok(program) => panic!("{name}: accepted\n{program}"),
            Err(message) => assert!(
                message.starts_with("not handled yet: ")
                    && message.contains(expected)
                    && message.contains(&format!("{file}.c:{BODY_LINE}:")),
                "{name}: {message}"
            ),
        }
    }
}

/// Programs the front once translated into a program with another result, accepting them silently; each
/// is now refused by name at its line, from unoptimised and optimised C and with `--taint-all`. The
/// comment of each case says what the old translation computed.
#[test]
fn what_was_once_translated_wrongly_is_refused_by_name_in_every_mode() {
    let cases: [(&str, &str, &str, usize); 7] = [
        // A longjmp resumes the function at its setjmp with q changed, an edge the qubit dataflow did not
        // see: the read of q after the setjmp became the constant 0 where C gives 1.
        (
            "resumed_read",
            "#include \"bennett.h\"\n#include <setjmp.h>\njmp_buf env;\nint main(void) {\n  \
             volatile _Bool q = 0;\n  if (setjmp(env) == 0) { bennett_x((_Bool *)&q); longjmp(env, 1); }\n  \
             int v = q;\n  bennett_measure((_Bool *)&q);\n  return v;\n}\n",
            "a setjmp in a function with qubit variables",
            6,
        ),
        // The same resumed q as a gate's condition: the gate was dropped, so y ended 0 where C gives 1.
        (
            "resumed_condition",
            "#include \"bennett.h\"\n#include <setjmp.h>\njmp_buf env;\nint main(void) {\n  \
             volatile _Bool q = 0; _Bool y = 0;\n  if (setjmp(env) == 0) { bennett_x((_Bool *)&q); longjmp(env, 1); }\n  \
             if (q) bennett_x(&y);\n  bennett_measure((_Bool *)&q);\n  return bennett_measure(&y);\n}\n",
            "a setjmp in a function with qubit variables",
            6,
        ),
        // The jump comes from a callee: the resumption is no edge of this function's control flow at all.
        (
            "resumed_from_callee",
            "#include \"bennett.h\"\n#include <setjmp.h>\njmp_buf env;\n\
             __attribute__((noinline)) void bail(void) { longjmp(env, 1); }\nint main(void) {\n  \
             volatile _Bool q = 0;\n  if (setjmp(env) == 0) { bennett_x((_Bool *)&q); bail(); }\n  \
             int v = q;\n  bennett_measure((_Bool *)&q);\n  return v;\n}\n",
            "a setjmp in a function with qubit variables",
            7,
        ),
        // An atomic update whose operand depends on a qubit, its result unused: it was dropped, so `cell`
        // stayed 0 where C gives 1.
        (
            "effect_atomic_update",
            "#include \"bennett.h\"\nint cell;\nint main(void) {\n  _Bool q = 0;\n  bennett_x(&q);\n  \
             __atomic_fetch_add(&cell, (int)q, __ATOMIC_SEQ_CST);\n  bennett_measure(&q);\n  return cell;\n}\n",
            "an effect on memory (the instruction `atomicrmw`) that depends on a qubit",
            6,
        ),
        // A volatile read at an address that depends on a qubit, its value unused: it was dropped, so the
        // device read nothing where C reads one of the two words.
        (
            "effect_volatile_read",
            "#include \"bennett.h\"\nvolatile int port[2];\nint main(void) {\n  _Bool q = 0;\n  bennett_h(&q);\n  \
             (void)port[q];\n  return (int)bennett_measure(&q);\n}\n",
            "an effect on memory (the instruction `load`) that depends on a qubit",
            6,
        ),
        // An atomic read at an address that depends on a qubit, its value unused: dropped likewise.
        (
            "effect_atomic_read",
            "#include \"bennett.h\"\nint cells[2];\nint main(void) {\n  _Bool q = 0;\n  bennett_h(&q);\n  \
             (void)__atomic_load_n(&cells[q], __ATOMIC_ACQUIRE);\n  return (int)bennett_measure(&q);\n}\n",
            "an effect on memory (the instruction `load`) that depends on a qubit",
            6,
        ),
        // A gate controlled by a value read from q before q changed: the gate was controlled by q's current
        // qubit instead, so y ended 0 where it must end 1.
        (
            "stale_literal_control",
            "#include \"bennett.h\"\nint main(void) {\n  _Bool q = 0, y = 0;\n  bennett_x(&q);\n  _Bool old = q;\n  \
             bennett_x(&q);\n  if (old) bennett_x(&y);\n  bennett_measure(&q);\n  return bennett_measure(&y);\n}\n",
            "the value of qubit variable 'q' read here is used after the variable changed",
            5,
        ),
    ];
    let modes: [(&[&str], &[&str], &str); 3] =
        [(CLANG_FLAGS, &[], ""), (CLANG_O2_FLAGS, &[], ".o2"), (CLANG_FLAGS, &["--taint-all"], ".taint-all")];
    for (name, text, expected, line) in cases {
        let source = write_c(name, text);
        for (clang_flags, front_flags, tag) in modes {
            match lower_text(&source, &format!("{name}{tag}"), clang_flags, front_flags) {
                Ok(program) => panic!("{name}{tag}: accepted\n{program}"),
                Err(message) => assert!(
                    message.starts_with("not handled yet: ")
                        && message.contains(expected)
                        && message.contains(&format!("{name}.c:{line}:")),
                    "{name}{tag}: {message}"
                ),
            }
        }
    }
}
