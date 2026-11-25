//! Worked wide values and return addresses in ordinary code and definite apply evaluations.

use std::f64::consts::FRAC_1_SQRT_2;
use std::fmt::Write as _;

use bennett_ir::Fault;
use num_complex::Complex64 as C;

use super::{BUDGET, Finish, assert_density, module, pure, run};

fn assert_apply_pair(program: &bennett_ir::ValidModule, name: &str, zero: (u64, u64), one: (u64, u64)) {
    let result = run(program, name, &[], 0, BUDGET);
    assert_eq!(result.reference.finish, Some(Finish::Returned(None)));
    let amplitude = C::new(FRAC_1_SQRT_2, 0.0);
    let expected = pure(&[(vec![0, zero.0, zero.1], amplitude), (vec![1, one.0, one.1], amplitude)]);
    assert_density(&result.reference.quantum.density, &expected, name);
}

fn assert_word(width: u32, body: &str, expected: u128) {
    let program = module(&format!(
        "memory 512
func @main() -> i{width} {{
entry:
 %answer = call @word()
 ret %answer
}}
func @lifted() {{
entry:
 %q = qalloc 1
 %out = qalloc 1
 %flag = qalloc 5
 h %q
 apply @check(%q) -> %out, %flag budget 40
 ret
}}
func @check(i1 %bit) -> i1 {{
entry:
 %answer = call @word()
 %equal = icmp eq i{width} %answer, {expected:#x}
 %result = and i1 %equal, %bit
 ret %result
}}
func @word() -> i{width} {{
entry:
 {body}
}}"
    ));
    assert_eq!(
        run(&program, "main", &[], 0, BUDGET).reference.finish,
        Some(Finish::Returned(Some(expected))),
        "{body}"
    );
    assert_apply_pair(&program, "lifted", (0, 0), (1, 0));
}

#[test]
fn every_integer_width_wraps_and_sign_extends() {
    for width in 1..=128 {
        let all = u128::MAX >> (128 - width);
        assert_word(width, &format!("%v = add i{width} {all:#x}, 1\n ret %v"), 0);
        let sign = 1u128 << (width - 1);
        assert_word(width, &format!("%v = ashr i{width} {sign:#x}, {}\n ret %v", width - 1), all);
    }
}

#[test]
fn wide_quotients_and_remainders_have_worked_values() {
    for width in [65, 96, 127, 128] {
        let all = u128::MAX >> (128 - width);
        let min = 1u128 << (width - 1);
        for (op, lhs, rhs, result) in [
            ("sdiv", min, all, min),
            ("srem", min, all, 0),
            ("sdiv", min, 2, min | (min >> 1)),
            ("srem", min + 3, all - 1, all),
            ("sdiv", all - 12, 5, all - 1),       // -13 / 5 = -2
            ("sdiv", 13, all - 4, all - 1),       // 13 / -5 = -2
            ("sdiv", all - 12, all - 4, 2),       // -13 / -5 = 2
            ("srem", all - 12, 5, all - 2),       // -13 = -2 * 5 - 3
            ("srem", 13, all - 4, 3),             // 13 = -2 * -5 + 3
            ("srem", all - 12, all - 4, all - 2), // -13 = 2 * -5 - 3
            ("udiv", min + 5, 2, (min >> 1) + 2),
            ("urem", min + 5, min, 5),
        ] {
            assert_word(width, &format!("%v = {op} i{width} {lhs:#x}, {rhs:#x}\n ret %v"), result);
        }
    }
}

#[test]
fn wide_zero_divisors_fault_in_the_run_and_on_one_apply_input() {
    for op in ["sdiv", "srem", "udiv", "urem"] {
        let program = module(&format!(
            "memory 512
func @main() -> i1 {{
entry:
 %answer = call @divide(i1 0)
 ret %answer
}}
func @lifted() {{
entry:
 %q = qalloc 1
 %out = qalloc 1
 %flag = qalloc 5
 h %q
 apply @divide(%q) -> %out, %flag budget 8
 ret
}}
func @divide(i1 %choose) -> i1 {{
entry:
 %by = zext i1 %choose to i128
 %word = {op} i128 0x80000000000000000000000000000001, %by
 %bit = trunc i128 %word to i1
 ret %bit
}}"
        ));
        let fault = Fault::DivideByZero;
        assert_eq!(run(&program, "main", &[], 0, BUDGET).reference.finish, Some(Finish::Faulted(fault)));
        let quotient_bit = u64::from(op.ends_with("div"));
        assert_apply_pair(&program, "lifted", (0, fault.code()), (quotient_bit, 0));
    }
}

#[test]
fn wide_shifts_use_the_entire_shift_amount() {
    for width in [65, 96, 127, 128] {
        let all = u128::MAX >> (128 - width);
        let min = 1u128 << (width - 1);
        for (op, lhs, at_last_bit, at_64) in [
            ("shl", 3, min, 3u128 << 64 & all),
            ("lshr", min | 3, 1, min >> 64),
            ("ashr", min | 3, all, all ^ ((min >> 64) - 1)),
        ] {
            for (shift, result) in [(width as u128 - 1, at_last_bit), (64, at_64), (width as u128, 0), (1 << 64, 0)] {
                assert_word(width, &format!("%v = {op} i{width} {lhs:#x}, {shift:#x}\n ret %v"), result);
            }
        }
    }
}

#[test]
fn wide_casts_comparisons_and_select_keep_the_high_bits() {
    assert_word(128, "%v = sext i64 0x8000000000000001 to i128\n ret %v", 0xffff_ffff_ffff_ffff_8000_0000_0000_0001);
    assert_word(128, "%v = sext i65 0x1fedcba9876543210 to i128\n ret %v", 0xffff_ffff_ffff_ffff_fedc_ba98_7654_3210);
    assert_word(128, "%v = zext i65 0x1fedcba9876543210 to i128\n ret %v", 0x1_fedc_ba98_7654_3210);
    assert_word(65, "%v = trunc i128 0x88776655443322110011223344556677 to i65\n ret %v", 0x1_0011_2233_4455_6677);
    assert_word(64, "%v = trunc i128 0x88776655443322110011223344556677 to i64\n ret %v", 0x0011_2233_4455_6677);

    let negative = "0x80000000000000000000000000000001";
    for (pred, against_positive, against_self) in [
        ("eq", false, true),
        ("ne", true, false),
        ("ult", false, false),
        ("ule", false, true),
        ("ugt", true, false),
        ("uge", true, true),
        ("slt", true, false),
        ("sle", true, true),
        ("sgt", false, false),
        ("sge", false, true),
    ] {
        for (rhs, yes) in [("1", against_positive), (negative, against_self)] {
            let body = format!("%c = icmp {pred} i128 {negative}, {rhs}\n %v = select i128 %c, {negative}, 1\n ret %v");
            assert_word(128, &body, if yes { (1 << 127) | 1 } else { 1 });
        }
    }
}

#[test]
fn wide_memory_load_masks_and_store_writes_all_bytes() {
    let original = 0xaffe_dcba_9876_5433_fedc_ba98_7654_3210_u128;
    for (width, low) in
        [(65u32, 0x1_fedc_ba98_7654_3210_u128), (100, 0xa_9876_5433_fedc_ba98_7654_3210), (128, original)]
    {
        // The short stores leave the bytes after their extent untouched, including at i65.
        let bytes = width.div_ceil(8);
        let full = low | u128::MAX.checked_shl(bytes * 8).unwrap_or(0);
        let widen =
            if width == 128 { "or i128 %value, 0".to_string() } else { format!("zext i{width} %value to i128") };
        let program = module(&format!(
            "memory 512
data 16 = i128 {original:#x}
data 48 = i128 0xffffffffffffffffffffffffffffffff
func @main() -> i128 {{
entry:
 %result = call @copy()
 ret %result
}}
func @lifted() {{
entry:
 %q = qalloc 1
 %out = qalloc 1
 %flag = qalloc 5
 h %q
 apply @check(%q) -> %out, %flag budget 20
 ret
}}
func @copy() -> i128 {{
entry:
 %value = load i{width} 16
 %wide = {widen}
 store i128 %wide, 32
 store i{width} %value, 48
 %readback = load i128 48
 ret %readback
}}
func @check(i1 %bit) -> i1 {{
entry:
 %copied = call @copy()
 %loaded = load i128 32
 %copy_ok = icmp eq i128 %copied, {full:#x}
 %load_ok = icmp eq i128 %loaded, {low:#x}
 %ok = and i1 %copy_ok, %load_ok
 %result = and i1 %bit, %ok
 ret %result
}}"
        ));
        let outer = run(&program, "main", &[], 0, BUDGET);
        assert_eq!(outer.reference.finish, Some(Finish::Returned(Some(full))));
        assert_eq!(outer.reference.memory[32..48], low.to_le_bytes());
        assert_eq!(outer.reference.memory[48..64], full.to_le_bytes());
        assert_apply_pair(&program, "lifted", (0, 0), (1, 0));
    }
}

#[test]
fn wide_memory_checks_bounds_before_held_bytes() {
    for width in [65u64, 100, 128] {
        let last = 512 - width.div_ceil(8);
        for (address, fault) in
            [(last, Fault::QubitMemoryAccess), (last + 1, Fault::OutOfBounds), (u64::MAX, Fault::OutOfBounds)]
        {
            for access in [format!("%v = load i{width} %addr"), format!("store i{width} 0x1122334455667788, %addr")] {
                let program = module(&format!(
                    "memory 512
func @main() -> i1 {{
entry:
 store i128 0x80112233445566778899aabbccddeeff, 496
 x [511:7]
 %result = call @access(i64 {address})
 ret %result
}}
func @lifted() {{
entry:
 store i128 0x80112233445566778899aabbccddeeff, 496
 x [511:7]
 %q = qalloc 1
 %out = qalloc 1
 %flag = qalloc 5
 h %q
 apply @choose(%q) -> %out, %flag budget 12
 ret
}}
func @choose(i1 %bit) -> i1 {{
entry:
 %addr = select i64 %bit, {address}, 32
 %result = call @access(%addr)
 ret %result
}}
func @access(i64 %addr) -> i1 {{
entry:
 {access}
 ret 1
}}"
                ));
                let outer = run(&program, "main", &[], 0, BUDGET);
                assert_eq!(outer.reference.finish, Some(Finish::Faulted(fault)), "{access} at {address}");
                assert_eq!(
                    outer.reference.memory[496..],
                    [255, 238, 221, 204, 187, 170, 153, 136, 119, 102, 85, 68, 51, 34, 17, 0]
                );
                let lifted = run(&program, "lifted", &[], 0, BUDGET);
                assert_eq!(lifted.reference.finish, Some(Finish::Returned(None)));
                assert_eq!(&lifted.reference.memory[32..48], &[0; 16], "wide stores are private");
                let a = C::new(FRAC_1_SQRT_2, 0.0);
                let expected = pure(&[(vec![0, 1, 0, 0], a), (vec![1, 0, fault.code(), 0], a)]);
                assert_density(&lifted.reference.quantum.density, &expected, &access);
            }
        }
    }
}

#[test]
fn wide_switch_call_parameters_and_nested_exit_keep_the_high_bits() {
    let program = module(
        "memory 512
func @main(i1 %high) -> i128 {
entry:
 %value = select i128 %high, 0x80000000000000000000000000001234, 0x1234
 call @dispatch(%value)
 fault unreachable
}
func @dispatch(i128 %value) {
entry:
 switch i128 %value, 0x1234: low, 0x80000000000000000000000000001234: high(%value), default: bad
low:
 exit i128 0xfeed000000000000000000000000beef
high(i128 %selected):
 %r = sub i128 %selected, 1
 exit %r
bad:
 fault unreachable
}",
    );
    for (arg, expected) in [(0, 0xfeed_0000_0000_0000_0000_0000_0000_beef), (1, (1 << 127) | 0x1233)] {
        assert_eq!(run(&program, "main", &[arg], 0, BUDGET).reference.finish, Some(Finish::Returned(Some(expected))));
    }
}

#[test]
fn wide_switch_inside_apply_distinguishes_cases_with_equal_low_64_bits() {
    assert_word(
        128,
        "br dispatch(i128 0x100000000000000000005)
dispatch(i128 %value):
 switch i128 %value, 5: low, 0x100000000000000000005: high(%value), default: bad
low:
 ret 5
high(i128 %matched):
 %answer = add i128 %matched, 8
 ret %answer
bad:
 fault unreachable",
        0x1_0000_0000_0000_0000_0005 + 8,
    );
}

#[test]
fn return_addresses_count_blocks_and_terminators_across_direct_and_indirect_calls() {
    let program = module(
        "memory 512
func @main() {
entry:
 %fp = faddr @middle
 br live
skipped:
 effect 111
 fault unreachable
live:
 call_indirect fn() %fp()
 ret
}
func @middle() {
entry:
 br live
live:
 call @relay()
 ret
}
func @relay() {
entry:
 %unused = add i65 0x10000000000000000, 7
 %fp = faddr @leaf
 call_indirect fn() %fp()
 ret
}
func @leaf() {
entry:
 %a = retaddr 0
 %b = retaddr 1
 %c = retaddr 2
 %d = retaddr 3
 %e = retaddr 4
 %f = retaddr 4294967295
 store i64 %a, 0
 store i64 %b, 8
 store i64 %c, 16
 store i64 %d, 24
 store i64 %e, 32
 store i64 %f, 40
 ret
}",
    );
    // Relay has two instructions before the call. Middle has an earlier terminator.
    // Main has two earlier blocks, including an unexecuted instruction and terminator.
    let expected = [0x3_0000_0003_u64, 0x2_0000_0002, 0x1_0000_0005, 0xffff_ffff_0000_0000, 0, 0];
    let result = run(&program, "main", &[], 0, BUDGET);
    assert_eq!(result.reference.finish, Some(Finish::Returned(None)));
    assert_eq!(&result.reference.memory[..48], &expected.map(u64::to_le_bytes).concat());
}

fn check_addresses(expected: &[(u32, &str)]) -> String {
    let mut text = String::new();
    for (index, (depth, value)) in expected.iter().enumerate() {
        writeln!(text, " %ra{index} = retaddr {depth}\n %eq{index} = icmp eq i64 %ra{index}, {value}")
            .expect("write address check");
        let previous = if index == 0 { "1".to_string() } else { format!("%ok{}", index - 1) };
        writeln!(text, " %ok{index} = and i1 {previous}, %eq{index}").expect("write cumulative check");
    }
    text
}

#[test]
fn apply_return_addresses_include_the_apply_site_and_suspended_outer_frames() {
    let applied_checks = check_addresses(&[
        (0, "0x200000006"),
        (1, "0x100000003"),
        (2, "0xffffffff00000000"),
        (3, "0"),
        (u32::MAX, "0"),
    ]);
    let leaf_checks = check_addresses(&[
        (0, "%site"),
        (1, "0x200000006"),
        (2, "0x100000003"),
        (3, "0xffffffff00000000"),
        (4, "0"),
        (u32::MAX, "0"),
    ]);
    let program = module(&format!(
        "memory 1024
func @main() {{
entry:
 %q = qalloc 1
 h %q
 call @bridge(%q)
 ret
}}
func @bridge(q1 %q) {{
entry:
 %out = qalloc 64
 %flag = qalloc 5
 br live
skipped:
 fault unreachable
live:
 s %q
 apply @applied(%q) -> %out, %flag budget 48
 ret
}}
func @applied(i1 %bit) -> i64 {{
entry:
 br %bit, one, zero
one:
 %a = call @leaf(i64 0x300000002)
 br verify(%a)
zero:
 %b = call @leaf(i64 0x300000004)
 br verify(%b)
verify(i64 %address):
 {applied_checks}
 %answer = select i64 %ok4, %address, 0
 ret %answer
}}
func @leaf(i64 %site) -> i64 {{
entry:
 {leaf_checks}
 %answer = select i64 %ok5, %ra0, 0
 ret %answer
}}"
    ));
    // The two definite inputs call the leaf at different points, then both return to the apply site.
    let result = run(&program, "main", &[], 0, BUDGET);
    assert_eq!(result.reference.finish, Some(Finish::Returned(None)));
    let amplitude = FRAC_1_SQRT_2;
    let expected = pure(&[
        (vec![0, 0x3_0000_0004, 0], C::new(amplitude, 0.0)),
        (vec![1, 0x3_0000_0002, 0], C::new(0.0, amplitude)),
    ]);
    assert_density(&result.reference.quantum.density, &expected, "suspended apply call chain");
}
