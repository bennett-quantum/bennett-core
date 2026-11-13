//! What this version does not handle under qubit control, or in place, is refused by name at the
//! construct's source line, never compiled to something cheaper or approximate.

use bennett_front_tests::{CLANG_FLAGS, lower_text, write_c};

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
        // Updates with no inverse the front knows: they would need history kept as quantum workspace.
        ("square", "x = x * x;", "an update of qubit variable 'x' that is not a bijection"),
        ("double", "x += x;", "an update of qubit variable 'x' that is not a bijection"),
        ("divide", "x = x / 3;", "an update of qubit variable 'x' that is not a bijection"),
        ("controlled_square", "if (a) x = x * x;", "an update of qubit variable 'x' that is not a bijection"),
        ("overwrite", "x = 5;", "an overwrite of qubit variable 'x'"),
        (
            "branchy_update",
            "if (a && p != 0 && 10 / p == 5) x += 1;",
            "an update of qubit variable 'x' under the control of a value that depends on a qubit, which neither",
        ),
        // Loops under qubit control with quantum operations inside need a budgeted construction.
        ("loop", "for (unsigned char i = 0; i < p; i++) bennett_x(&c);", "a loop under the control of a value"),
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
