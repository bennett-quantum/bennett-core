//! The translation of the handle interface's calls (`docs/HANDLES.md`, sections 5, 6 and 14; `docs/DESIGN.md`,
//! "Translating the handle interface"): every call of an interface function is one device instruction whose operands
//! are the call's, and the device makes every check. Each case below is one C program run through `bennett-cc` and
//! `bennett-front` four ways: at `-O0` and `-O2`, with its operands written as constants and with each operand of
//! the calls under test passed through a `volatile` object, so that it is a run-time value at both levels. All four
//! give the case's one expected outcome: a result, or the first failing check's fault, in the order of section 5.
use bennett_exec::{Executor, Outcome};
use bennett_front_tests::{CLANG_FLAGS, CLANG_O2_FLAGS, entry, front, options, out_dir, write_c};
use bennett_ir::Fault;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const LEVELS: [(&str, &[&str]); 2] = [("O0", CLANG_FLAGS), ("O2", CLANG_O2_FLAGS)];

/// How the operands of the calls under test are written: `V(x)` for an integer, `D(x)` for an angle, `P(x)` for
/// a pointer and `H(r)` for a handle.
const FORMS: [(&str, &str); 2] = [
    ("constant", "#define V(x) (x)\n#define D(x) (x)\n#define P(x) (x)\n#define H(r) (r)\n"),
    (
        "runtime",
        "static uint64_t rt_(uint64_t v) { volatile uint64_t s = v; return s; }\n\
         static double rtd_(double v) { volatile double s = v; return s; }\n\
         #define V(x) rt_((uint64_t)(x))\n#define D(x) rtd_(x)\n\
         #define P(x) ((__typeof__(1 ? (x) : (x)))(uintptr_t)rt_((uint64_t)(uintptr_t)(x)))\n\
         #define H(r) ((bq_reg){rt_((r).id)})\n",
    ),
];

/// Functions the cases apply, and names for constant handles and unreadable addresses.
const PRELUDE: &str = r#"
#define R(k) ((bq_reg){k})
#define FAR ((const void *)(uintptr_t)0xfffffffffffffff0ull)
#define FIELD(r, low, bits, offset, direction) {{(r), (low), (bits)}, (offset), (direction)}
static void inc(void *p) { uint8_t *b = p; b[1] = (uint8_t)(b[0] + 1); }
static void inc_int(int *p) { uint8_t *b = (uint8_t *)p; b[1] = (uint8_t)(b[0] + 1); }
static int64_t not_target(int64_t a) { return a; }
static void plain(uint64_t a) { (void)a; }
static void quantum(void *p) { (void)p; bq_free(R(1)); }
static void bad_kind(void *p) { (void)p; bq_gate(0, R(99), 0, FAR, 1); }
static void stop(void *p) { (void)p; __bennett_bad_argument(); }
static void no_memory(void *p) { (void)p; __bennett_out_of_memory(); }
static void output(void *p) { (void)p; __bennett_effect('A'); }
static void finish(void *p) { (void)p; __bennett_exit(3); }
static void divide(void *p) { volatile uint8_t *b = p; b[1] = (uint8_t)(100 / b[0]); }
/* x (3 qubits) in at byte 0, y (3 qubits) out at byte 1, the fault register e: handles 1, 2, 3. */
#define SETUP bq_reg x = bq_alloc(3), y = bq_alloc(3), e = bq_alloc(5); uint8_t image[2] = {0, 0}; \
    bq_field f[2] = {FIELD(x, 0, 3, 0, BQ_DIR_IN), FIELD(y, 0, 3, 1, BQ_DIR_OUT)}; (void)f; (void)image
/* An apply of `fn` with the setup's fields; returns the fault code. */
#define APPLY_CODE(fn, budget) (bq_apply(P(fn), V(budget), H(e), P(image), V(2), P(f), V(2), P((const bq_bit *)0), \
    V(0)), (int)bq_measure(e, 0, 5))
"#;

const PI: &str = "3.14159265358979323846";

enum Expect {
    Value(u32),
    Fault(Fault),
}
use Expect::{Fault as F, Value};

/// The cases: a name, the body of a block that returns an `int`, and the expected outcome.
fn cases() -> Vec<(&'static str, String, Expect)> {
    let pi = PI;
    let mut cases: Vec<(&'static str, String, Expect)> = vec![
        // bq_alloc: handles numbered from 1 in allocation order, never reused; the width checked first.
        (
            "alloc_numbers",
            "bq_reg a = bq_alloc(V(1)), b = bq_alloc(V(2)); bq_free(a); bq_reg c = bq_alloc(V(1));\n\
             return (int)(a.id * 100 + b.id * 10 + c.id);"
                .into(),
            Value(123),
        ),
        ("alloc_width", "bq_reg r = bq_alloc(V(5)); return (int)bq_width(r);".into(), Value(5)),
        ("alloc_zero", "bq_alloc(V(0)); return 0;".into(), F(Fault::BadWidth)),
        ("alloc_too_wide", "bq_alloc(V(1ull << 32)); return 0;".into(), F(Fault::BadWidth)),
        // bq_free: by handle, constant or not; a released, never allocated or zero handle names nothing.
        ("free_constant", "bq_alloc(1); bq_free(H(R(1))); return 5;".into(), Value(5)),
        ("free_twice", "bq_reg r = bq_alloc(1); bq_free(H(r)); bq_free(H(r)); return 0;".into(), F(Fault::DanglingRegister)),
        ("free_zero", "bq_alloc(1); bq_free(H(R(0))); return 0;".into(), F(Fault::DanglingRegister)),
        ("free_never", "bq_alloc(1); bq_free(H(R(7))); return 0;".into(), F(Fault::DanglingRegister)),
        // bq_width.
        ("width_constant", "bq_alloc(4); return (int)bq_width(H(R(1)));".into(), Value(4)),
        ("width_unallocated", "bq_alloc(4); return (int)bq_width(H(R(2)));".into(), F(Fault::DanglingRegister)),
        ("width_released", "bq_reg r = bq_alloc(4); bq_free(r); return (int)bq_width(H(r));".into(), F(Fault::DanglingRegister)),
        // bq_measure: the handle, then 1 <= bits <= 64 and low + bits <= width, without wrapping.
        (
            "measure_slice",
            "bq_reg r = bq_alloc(8); bq_gate(1, r, 2, 0, 0); bq_gate(1, r, 5, 0, 0);\n\
             return (int)bq_measure(H(r), V(2), V(4));"
                .into(),
            Value(9),
        ),
        (
            "measure_constant_handle",
            "bq_alloc(8); bq_gate(1, R(1), 7, 0, 0); return (int)bq_measure(H(R(1)), V(7), V(1));".into(),
            Value(1),
        ),
        (
            "measure_wide",
            "bq_reg r = bq_alloc(70); bq_gate(1, r, 69, 0, 0); return (int)(bq_measure(H(r), V(6), V(64)) >> 63);".into(),
            Value(1),
        ),
        ("measure_handle_first", "bq_alloc(4); return (int)bq_measure(H(R(9)), V(0), V(0));".into(), F(Fault::DanglingRegister)),
        ("measure_no_bits", "bq_reg r = bq_alloc(4); return (int)bq_measure(H(r), V(0), V(0));".into(), F(Fault::BadRange)),
        ("measure_65_bits", "bq_reg r = bq_alloc(70); return (int)bq_measure(H(r), V(0), V(65));".into(), F(Fault::BadRange)),
        ("measure_past_end", "bq_reg r = bq_alloc(4); return (int)bq_measure(H(r), V(1), V(4));".into(), F(Fault::BadRange)),
        (
            "measure_no_wrap",
            "bq_reg r = bq_alloc(4); return (int)bq_measure(H(r), V(UINT64_MAX), V(1));".into(),
            F(Fault::BadRange),
        ),
        // bq_gate: every kind (each power that makes Z, between two H, gives 1); the kind first, then the target's
        // handle and position, the list (readable, then each entry's handle and position in order), the target
        // among the controls.
        (
            "gate_kinds",
            "int bits = 0; static const uint64_t z_power[9] = {0, 0, 0, 1, 0, 2, 2, 4, 4};\n\
             for (uint64_t k = 1; k <= 8; k++) {\n\
               bq_reg r = bq_alloc(1);\n\
               if (k == BQ_GATE_X || k == BQ_GATE_Y) { bq_gate(V(k), H(r), V(0), P((const bq_bit *)0), V(0)); }\n\
               else if (k != BQ_GATE_H) { bq_gate(V(BQ_GATE_H), H(r), V(0), P((const bq_bit *)0), V(0));\n\
                 for (uint64_t i = 0; i < z_power[k]; i++) bq_gate(V(k), H(r), V(0), P((const bq_bit *)0), V(0));\n\
                 bq_gate(V(BQ_GATE_H), H(r), V(0), P((const bq_bit *)0), V(0)); }\n\
               else { bq_gate(V(k), r, 0, 0, 0); bq_gate(BQ_GATE_Z, r, 0, 0, 0); bq_gate(V(k), r, 0, 0, 0); }\n\
               bits |= (int)bq_measure(r, 0, 1) << k;\n\
             }\n\
             return bits;"
                .into(),
            Value(0x1fe),
        ),
        ("gate_kind_zero_first", "bq_gate(V(0), H(R(5)), V(99), P(FAR), V(1)); return 0;".into(), F(Fault::BadGate)),
        ("gate_rotation_code", "bq_reg r = bq_alloc(1); bq_gate(V(BQ_ROT_X), H(r), V(0), P((const bq_bit *)0), V(0)); return 0;".into(), F(Fault::BadGate)),
        ("gate_handle", "bq_alloc(1); bq_gate(V(1), H(R(5)), V(99), P(FAR), V(1)); return 0;".into(), F(Fault::DanglingRegister)),
        ("gate_position", "bq_reg r = bq_alloc(2); bq_gate(V(1), H(r), V(2), P(FAR), V(1)); return 0;".into(), F(Fault::BadRange)),
        ("gate_position_max", "bq_reg r = bq_alloc(2); bq_gate(V(1), H(r), V(UINT64_MAX), P(FAR), V(1)); return 0;".into(), F(Fault::BadRange)),
        (
            "gate_list_unreadable",
            "bq_reg r = bq_alloc(2); bq_gate(V(1), H(r), V(0), P((const bq_bit *)FAR), V(1)); return 0;".into(),
            F(Fault::BadArgument),
        ),
        (
            "gate_list_count_overflows",
            "bq_reg r = bq_alloc(2); bq_bit c[1] = {{r, 1}}; bq_gate(V(1), H(r), V(0), P(c), V(1ull << 60)); return 0;".into(),
            F(Fault::BadArgument),
        ),
        (
            "gate_list_entry_handle",
            "bq_reg r = bq_alloc(2); bq_bit c[1] = {{R(9), 0}}; bq_gate(V(1), H(r), V(0), P(c), V(1)); return 0;".into(),
            F(Fault::DanglingRegister),
        ),
        (
            "gate_list_entry_position",
            "bq_reg r = bq_alloc(2); bq_bit c[1] = {{r, 5}}; bq_gate(V(1), H(r), V(0), P(c), V(1)); return 0;".into(),
            F(Fault::BadRange),
        ),
        (
            "gate_list_order_range",
            "bq_reg r = bq_alloc(2); bq_bit c[2] = {{r, 5}, {R(9), 0}}; bq_gate(V(1), H(r), V(0), P(c), V(2)); return 0;".into(),
            F(Fault::BadRange),
        ),
        (
            "gate_list_order_handle",
            "bq_reg r = bq_alloc(2); bq_bit c[2] = {{R(9), 0}, {r, 5}}; bq_gate(V(1), H(r), V(0), P(c), V(2)); return 0;".into(),
            F(Fault::DanglingRegister),
        ),
        (
            "gate_target_among_controls",
            "bq_reg r = bq_alloc(2); bq_bit c[2] = {{r, 1}, {r, 0}}; bq_gate(V(1), H(r), V(0), P(c), V(2)); return 0;".into(),
            F(Fault::QubitAlias),
        ),
        (
            "gate_controlled",
            "bq_reg c = bq_alloc(1), r = bq_alloc(1), s = bq_alloc(1); bq_gate(1, c, 0, 0, 0);\n\
             bq_bit on[2] = {{c, 0}, {c, 0}}; bq_bit off[1] = {{s, 0}};\n\
             bq_gate(V(1), H(r), V(0), P(on), V(2)); bq_gate(V(1), H(s), V(0), P(off), V(0));\n\
             bq_bit s_on[1] = {{r, 0}}; bq_gate(V(1), H(c), V(0), P(s_on), V(1));\n\
             return (int)(bq_measure(c, 0, 1) * 100 + bq_measure(r, 0, 1) * 10 + bq_measure(s, 0, 1));"
                .into(),
            Value(11),
        ),
        (
            "gate_empty_list_anywhere",
            "bq_reg r = bq_alloc(1); bq_gate(V(1), H(r), V(0), P((const bq_bit *)FAR), V(0)); return (int)bq_measure(r, 0, 1);".into(),
            Value(1),
        ),
        // bq_rotate: each axis by pi and by -pi flips |0> (between two H for Z and the phase); the code, then the
        // angle, then as bq_gate; a controlled phase.
        (
            "rotate_axes",
            format!(
                "int bits = 0; for (uint64_t k = 9; k <= 12; k++) for (int sign = 0; sign < 2; sign++) {{\n\
                   double a = sign ? -{pi} : {pi}; bq_reg r = bq_alloc(1); int z = k >= BQ_ROT_Z;\n\
                   if (z) bq_gate(BQ_GATE_H, r, 0, 0, 0);\n\
                   bq_rotate(V(k), D(a), H(r), V(0), P((const bq_bit *)0), V(0));\n\
                   if (z) bq_gate(BQ_GATE_H, r, 0, 0, 0);\n\
                   bits += (int)bq_measure(r, 0, 1);\n\
                 }}\n\
                 return bits;"
            ),
            Value(8),
        ),
        (
            "rotate_constant_angle",
            format!(
                "bq_reg r = bq_alloc(1); bq_rotate(V(BQ_ROT_Y), D(-{pi}), H(r), V(0), P((const bq_bit *)0), V(0));\n\
                 return (int)bq_measure(r, 0, 1);"
            ),
            Value(1),
        ),
        (
            "rotate_controlled_phase",
            format!(
                "bq_reg c = bq_alloc(1), t = bq_alloc(1); bq_gate(BQ_GATE_X, t, 0, 0, 0); bq_gate(BQ_GATE_H, c, 0, 0, 0);\n\
                 bq_bit l[1] = {{{{c, 0}}}}; bq_rotate(V(BQ_ROT_PHASE), D({pi}), H(t), V(0), P(l), V(1));\n\
                 bq_gate(BQ_GATE_H, c, 0, 0, 0); return (int)bq_measure(c, 0, 1);"
            ),
            Value(1),
        ),
        ("rotate_gate_code", "bq_reg r = bq_alloc(1); bq_rotate(V(BQ_GATE_H), D(1.0), H(r), V(0), P((const bq_bit *)0), V(0)); return 0;".into(), F(Fault::BadGate)),
        ("rotate_code_13", "bq_reg r = bq_alloc(1); bq_rotate(V(13), D(1.0), H(r), V(0), P((const bq_bit *)0), V(0)); return 0;".into(), F(Fault::BadGate)),
        ("rotate_kind_before_angle", "bq_rotate(V(0), D(__builtin_inf()), H(R(9)), V(9), P(FAR), V(1)); return 0;".into(), F(Fault::BadGate)),
        ("rotate_nan", "bq_rotate(V(9), D(__builtin_nan(\"\")), H(R(9)), V(9), P(FAR), V(1)); return 0;".into(), F(Fault::BadAngle)),
        ("rotate_infinity", "bq_rotate(V(12), D(-__builtin_inf()), H(R(9)), V(9), P(FAR), V(1)); return 0;".into(), F(Fault::BadAngle)),
        ("rotate_handle", "bq_rotate(V(9), D(1.0), H(R(9)), V(9), P(FAR), V(1)); return 0;".into(), F(Fault::DanglingRegister)),
        ("rotate_position", "bq_reg r = bq_alloc(1); bq_rotate(V(9), D(1.0), H(r), V(1), P(FAR), V(1)); return 0;".into(), F(Fault::BadRange)),
        ("rotate_list_unreadable", "bq_reg r = bq_alloc(1); bq_rotate(V(9), D(1.0), H(r), V(0), P((const bq_bit *)FAR), V(1)); return 0;".into(), F(Fault::BadArgument)),
        (
            "rotate_target_among_controls",
            "bq_reg r = bq_alloc(1); bq_bit c[1] = {{r, 0}}; bq_rotate(V(9), D(1.0), H(r), V(0), P(c), V(1)); return 0;".into(),
            F(Fault::QubitAlias),
        ),
        // bq_apply: the out field receives f of the in field; controls; the checks of 6.2 in order; the applied
        // function's own faults are codes in the fault register, never a stop of the run.
        (
            "apply_result",
            "SETUP; bq_gate(1, x, 1, 0, 0); int code = APPLY_CODE(inc, 100);\n\
             return code * 100 + (int)bq_measure(y, 0, 3);"
                .into(),
            Value(3),
        ),
        (
            "apply_superposed",
            "SETUP; for (int i = 0; i < 3; i++) bq_gate(BQ_GATE_H, x, i, 0, 0); int code = APPLY_CODE(inc, 100);\n\
             uint64_t xv = bq_measure(x, 0, 3), yv = bq_measure(y, 0, 3); return code * 100 + (yv == ((xv + 1) & 7));"
                .into(),
            Value(1),
        ),
        (
            "apply_controlled",
            "SETUP; bq_reg c = bq_alloc(1); bq_gate(1, x, 1, 0, 0); bq_bit l[2] = {{c, 0}, {x, 1}};\n\
             bq_apply(P(inc), V(100), H(e), P(image), V(2), P(f), V(2), P(l), V(2)); int before = (int)bq_measure(y, 0, 3);\n\
             bq_gate(1, c, 0, 0, 0); bq_apply(P(inc), V(100), H(e), P(image), V(2), P(f), V(2), P(l), V(2));\n\
             return before * 100 + (int)bq_measure(y, 0, 3) * 10 + (int)bq_measure(e, 0, 5);"
                .into(),
            Value(30),
        ),
        (
            "apply_pointer_parameter",
            "SETUP; bq_gate(1, x, 0, 0, 0); return APPLY_CODE((bq_fn *)inc_int, 100) * 100 + (int)bq_measure(y, 0, 3);".into(),
            Value(2),
        ),
        ("apply_not_a_function", "SETUP; bq_apply(P((bq_fn *)(uintptr_t)12345), V(1), H(R(9)), P(FAR), V(0), P(FAR), V(1), P(FAR), V(1)); return 0;".into(), F(Fault::BadIndirect)),
        ("apply_other_signature", "SETUP; return APPLY_CODE((bq_fn *)not_target, 100);".into(), F(Fault::BadIndirect)),
        ("apply_integer_parameter", "SETUP; return APPLY_CODE((bq_fn *)plain, 100);".into(), F(Fault::BadIndirect)),
        ("apply_primitive", "SETUP; return APPLY_CODE((bq_fn *)bq_free, 100);".into(), F(Fault::BadIndirect)),
        ("apply_fault_handle", "SETUP; bq_apply(P(inc), V(1), H(R(9)), P(FAR), V(0), P(FAR), V(1), P(FAR), V(1)); return 0;".into(), F(Fault::DanglingRegister)),
        (
            "apply_fault_narrow",
            "SETUP; bq_reg n = bq_alloc(4); bq_apply(P(inc), V(1), H(n), P(FAR), V(0), P(FAR), V(1), P(FAR), V(1)); return 0;".into(),
            F(Fault::BadRange),
        ),
        ("apply_size_zero", "SETUP; bq_apply(P(inc), V(1), H(e), P(image), V(0), P(f), V(2), P(FAR), V(1)); return 0;".into(), F(Fault::BadArgument)),
        (
            "apply_field_count_overflows",
            "SETUP; bq_apply(P(inc), V(1), H(e), P(image), V(2), P(f), V(1ull << 60), P(FAR), V(1)); return 0;".into(),
            F(Fault::BadArgument),
        ),
        ("apply_image_unreadable", "SETUP; bq_apply(P(inc), V(1), H(e), P(FAR), V(16), P(f), V(2), P(FAR), V(1)); return 0;".into(), F(Fault::BadArgument)),
        (
            "apply_fields_unreadable",
            "SETUP; bq_apply(P(inc), V(1), H(e), P(image), V(2), P((const bq_field *)FAR), V(1), P(FAR), V(1)); return 0;".into(),
            F(Fault::BadArgument),
        ),
        (
            "apply_controls_before_fields",
            "SETUP; f[0].direction = 3; bq_bit l[1] = {{R(9), 0}};\n\
             bq_apply(P(inc), V(1), H(e), P(image), V(2), P(f), V(2), P(l), V(1)); return 0;"
                .into(),
            F(Fault::DanglingRegister),
        ),
    ];
    // The field checks of 6.2, checks 5 and 6, each bad_argument: a change to the setup's fields.
    let fields: [(&'static str, &str); 11] = [
        ("apply_field_direction", "f[1].direction = 3;"),
        ("apply_field_handle", "f[1].slice.reg = R(9);"),
        ("apply_field_no_bits", "f[0].slice.bits = 0;"),
        ("apply_field_past_register", "f[0].slice.low = 1;"),
        ("apply_field_past_block", "f[1].offset = 2;"),
        ("apply_fields_overlap", "f[1].offset = 0;"),
        ("apply_two_outs_share", "f[0] = (bq_field)FIELD(y, 0, 1, 0, BQ_DIR_OUT);"),
        ("apply_out_and_in_share", "f[0] = (bq_field)FIELD(y, 2, 1, 0, BQ_DIR_IN);"),
        ("apply_field_and_fault", "f[0] = (bq_field)FIELD(e, 4, 1, 0, BQ_DIR_IN);"),
        ("apply_wide_fault_low_bits", "bq_reg w = bq_alloc(8); e = w; f[0] = (bq_field)FIELD(w, 0, 1, 0, BQ_DIR_IN);"),
        ("apply_field_past_end_no_wrap", "f[0].slice.low = UINT64_MAX;"),
    ];
    for (name, change) in fields {
        cases.push((name, format!("SETUP; {change} return APPLY_CODE(inc, 100);"), F(Fault::BadArgument)));
    }
    cases.extend([
        (
            "apply_control_and_out",
            "SETUP; bq_bit l[1] = {{y, 0}}; bq_apply(P(inc), V(1), H(e), P(image), V(2), P(f), V(2), P(l), V(1)); return 0;".into(),
            F(Fault::BadArgument),
        ),
        (
            "apply_control_and_fault",
            "SETUP; bq_bit l[1] = {{e, 2}}; bq_apply(P(inc), V(1), H(e), P(image), V(2), P(f), V(2), P(l), V(1)); return 0;".into(),
            F(Fault::BadArgument),
        ),
        (
            "apply_wide_fault_high_bits_free",
            "SETUP; bq_reg w = bq_alloc(8); e = w; f[0] = (bq_field)FIELD(w, 5, 3, 0, BQ_DIR_IN); bq_gate(1, w, 6, 0, 0);\n\
             return APPLY_CODE(inc, 100) * 100 + (int)bq_measure(y, 0, 3);"
                .into(),
            Value(3),
        ),
        // An apply target is an ordinary function too: called directly and through a pointer.
        (
            "apply_target_called",
            "uint8_t b[2] = {4, 0}; inc(b); void (*volatile g)(void *) = inc; uint8_t c[2] = {6, 0}; g(c);\n\
             return b[1] * 10 + c[1];"
                .into(),
            Value(57),
        ),
        // The applied function's run: faults are codes in the fault register, and the run goes on.
        ("apply_budget", "SETUP; return APPLY_CODE(inc, 0);".into(), Value(1)),
        ("apply_divide_by_zero", "SETUP; return APPLY_CODE(divide, 100);".into(), Value(2)),
        ("apply_output_inside", "SETUP; return APPLY_CODE(output, 100);".into(), Value(7)),
        ("apply_quantum_inside", "SETUP; return APPLY_CODE(quantum, 100);".into(), Value(8)),
        ("apply_quantum_kind_inside", "SETUP; return APPLY_CODE(bad_kind, 100);".into(), Value(8)),
        ("apply_exit_inside", "SETUP; return APPLY_CODE(finish, 100);".into(), Value(13)),
        ("apply_out_of_memory_inside", "SETUP; return APPLY_CODE(no_memory, 100);".into(), Value(19)),
        ("apply_bad_argument_inside", "SETUP; return APPLY_CODE(stop, 100);".into(), Value(24)),
        // The named stops in the control program.
        ("bad_argument", "__bennett_effect('A'); __bennett_bad_argument();".into(), F(Fault::BadArgument)),
        ("out_of_memory", "__bennett_out_of_memory();".into(), F(Fault::OutOfMemory)),
    ]);
    cases
}

fn program(form: &str, cases: &[(&'static str, String, Expect)]) -> String {
    let mut text = format!("#include \"bq.h\"\n{form}{PRELUDE}\nint main(int which) {{\n  switch (which) {{\n");
    for (i, (_, body, _)) in cases.iter().enumerate() {
        text += &format!("  case {i}: {{\n{body}\n  }}\n");
    }
    text + "  }\n  return -1;\n}\n"
}

fn output(command: &mut Command) -> Output {
    command.output().unwrap_or_else(|e| panic!("cannot run {command:?}: {e}"))
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// `bennett-cc` then `bennett-front`: the device program's text.
fn lower(source: &Path, name: &str, flags: &[&str]) -> String {
    let bitcode = out_dir().join(format!("{name}.cc.bc"));
    let compiled =
        output(Command::new(front().with_file_name("bennett-cc")).args(flags).arg("-o").arg(&bitcode).arg(source));
    assert!(compiled.status.success(), "{name}: {}", stderr(&compiled));
    let device: PathBuf = out_dir().join(format!("{name}.bn"));
    let lowered = output(
        Command::new(front()).arg(&bitcode).arg("-o").arg(&device).args(["--region-budget=64", "--stack-bytes=65536"]),
    );
    assert!(lowered.status.success(), "{name}: {}", stderr(&lowered));
    std::fs::read_to_string(&device).unwrap_or_else(|e| panic!("cannot read {device:?}: {e}"))
}

/// The instructions of the interface in a device program's text.
fn interface_lines(text: &str) -> Vec<&str> {
    let starts =
        ["qalloc i64 ", "qfree ", "qwidth ", "measure i64 ", "gate(", "rotate(", "apply ", "fault bad_argument"];
    text.lines()
        .map(|line| match line.split_once(" = ") {
            Some((_, rest)) if line.starts_with("    %") => rest,
            _ => line.trim_start(),
        })
        .filter(|line| starts.iter().any(|start| line.starts_with(start)))
        .collect()
}

#[test]
fn each_interface_call_is_one_device_instruction_with_its_operands() {
    let cases = cases();
    let mut builds = Vec::new();
    for (form, macros) in FORMS {
        let source = write_c(&format!("handles_translate_{form}"), &program(macros, &cases));
        for (level, flags) in LEVELS {
            builds.push((format!("handles_translate_{form}.{level}"), source.clone(), flags, form, level));
        }
    }
    std::thread::scope(|scope| {
        for (name, source, flags, form, level) in &builds {
            let cases = &cases;
            scope.spawn(move || {
                let text = lower(source, name, flags);
                let module = bennett_ir::load(&text).unwrap_or_else(|e| panic!("{name}: invalid device program: {e}"));
                // The operands as the call has them: constants at -O2 in the constant form (a constant handle
                // included), slots in the run-time form.
                let lines = interface_lines(&text);
                for start in ["qfree ", "qwidth ", "measure i64 ", "gate(", "rotate(", "apply ", "qalloc i64 "] {
                    let constant = |line: &&str| {
                        line.strip_prefix(start).is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
                    };
                    let slot = |line: &&str| line.strip_prefix(start).is_some_and(|rest| rest.starts_with('%'));
                    if *form == "constant" && *level == "O2" {
                        assert!(lines.iter().any(constant), "{name}: no `{start}` with a constant operand: {lines:#?}");
                    }
                    if *form == "runtime" {
                        assert!(lines.iter().any(slot), "{name}: no `{start}` with a run-time operand: {lines:#?}");
                    }
                }
                // Exactly the functions of type one pointer parameter and no result are marked: the nine the cases
                // apply, `void *` and `int *` parameters alike; `not_target`, `plain`, `main` and the helpers of
                // the run-time form are not.
                let mut marked: Vec<&str> = text
                    .lines()
                    .filter_map(|line| line.strip_prefix("applied func @")?.split_once('(').map(|(name, _)| name))
                    .collect();
                marked.sort_unstable();
                assert_eq!(
                    marked,
                    ["bad_kind", "divide", "finish", "inc", "inc_int", "no_memory", "output", "quantum", "stop"],
                    "{name}: the apply targets"
                );
                for (i, (case, _, expected)) in cases.iter().enumerate() {
                    for seed in 0..3 {
                        let mut executor = Executor::new(&module, entry(&module, "main"), &[i as u64], options(seed))
                            .unwrap_or_else(|e| panic!("{name} {case}: {e}"));
                        let got = executor.run();
                        let want = match expected {
                            Value(v) => Outcome::Returned(Some(u128::from(*v))),
                            F(fault) => Outcome::Faulted(*fault),
                        };
                        assert_eq!(got, want, "{name}, case {case}, seed {seed}");
                    }
                }
            });
        }
    });
}

/// Each translated call is exactly one instruction: a program of one call of each kind, at both levels, has
/// one interface instruction per call written.
#[test]
fn one_instruction_per_call() {
    let text = "#include \"bq.h\"\n\
        static void f(void *p) { (void)p; }\n\
        int main(void) {\n\
          bq_reg r = bq_alloc(2), e = bq_alloc(5); uint8_t image[1] = {0};\n\
          bq_gate(BQ_GATE_H, r, 0, 0, 0); bq_rotate(BQ_ROT_Z, 0.25, r, 1, 0, 0);\n\
          bq_apply(f, 10, e, image, 1, 0, 0, 0, 0);\n\
          uint64_t v = bq_measure(r, 0, 2) + bq_width(r); bq_free(r);\n\
          if (v == 99) __bennett_bad_argument();\n\
          return (int)bq_measure(e, 0, 5);\n\
        }\n";
    let source = write_c("handles_translate_once", text);
    for (level, flags) in LEVELS {
        let name = format!("handles_translate_once.{level}");
        let device = lower(&source, &name, flags);
        let mut kinds: Vec<&str> =
            interface_lines(&device).iter().map(|line| line.split([' ', '(']).next().expect("a word")).collect();
        kinds.sort_unstable();
        assert_eq!(
            kinds,
            ["apply", "fault", "gate", "measure", "measure", "qalloc", "qalloc", "qfree", "qwidth", "rotate"],
            "{name}: {device}"
        );
    }
}
