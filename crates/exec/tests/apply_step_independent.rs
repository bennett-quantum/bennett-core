//! Hand-counted instruction attempts and exact cutoffs on dirty apply targets.

mod common;

use bennett_exec::{BranchJournal, Options, Outcome, SitePos, Undo};
use bennett_ir::{Fault, ValidModule};
use common::{bounded, entry, executor, load};
use num_complex::Complex64;

const DRIVER: &str = r#"
func @main(i64 %budget) {
entry:
    %out = qalloc 8
    x %out[0]
    x %out[7]
    %fault = qalloc 5
    x %fault[0]
    x %fault[4]
    %untouched = qalloc 1
    h %untouched
    s %untouched
    x [255:0]
    apply @walk() -> %out, %fault budget %budget
    ret
}
"#;

fn check_apply(module: &ValidModule, budget: u64, outcome: Outcome, completed: u64, check: impl Fn(&BranchJournal)) {
    let callee = entry(module, "walk");
    let mut run = executor(module, "main", &[budget], Options { machine_meaning: true, ..bounded() });
    run.measure_journal_content(true);
    for _ in 0..10 {
        assert_eq!(run.step(), None);
    }
    let mut expected = run.snapshot();
    let half = std::f64::consts::FRAC_1_SQRT_2;
    assert_eq!(
        expected.quantum.amplitudes,
        [(vec![0x81, 0x11, 0, 1], Complex64::new(half, 0.0)), (vec![0x81, 0x11, 1, 1], Complex64::new(0.0, half))],
    );
    expected.classical.steps += 1;
    expected.classical.frames[0].inst += 1;
    for (config, _) in &mut expected.quantum.amplitudes {
        match outcome {
            Outcome::Returned(Some(value)) => config[0] ^= value as u64,
            Outcome::Faulted(fault) => config[1] ^= fault.code(),
            _ => panic!("an apply returns an integer or a fault code"),
        }
    }
    assert_eq!(run.step(), None);
    assert_eq!(run.snapshot(), expected, "B={budget}, {outcome:?}");
    assert_eq!(run.stats().region_steps, 2 * completed, "B={budget}, {outcome:?}");
    let [apply] = run.journal_measurement().expect("observer enabled").applies.as_slice() else {
        panic!("one apply");
    };
    assert_eq!(apply.callee, callee);
    assert_eq!(apply.branches.len(), 2);
    for recording in &apply.branches {
        assert!(recording.args.is_empty());
        assert_eq!(recording.outcome, outcome, "B={budget}");
        assert_eq!(recording.steps, completed, "B={budget}");
        assert_eq!(recording.records, completed as usize + 1, "B={budget}");
        assert_eq!(recording.entries[0].site.pos, SitePos::Entry);
        assert_eq!(recording.entries[0].kind, Undo::Call);
        check(recording);
    }
}

#[test]
fn budget_cuts_off_at_each_instruction_of_a_call_and_longjmp() {
    let module = load(&format!(
        r#"
memory 256
{DRIVER}
func @walk() -> i8 {{
entry:
    %n = add i8 3, 4
    %cond = icmp eq i8 %n, 7
    %picked = select i8 %cond, %n, 80
    %word = zext i8 %picked to i16
    %fp = faddr @leaf
    %saved = stacksave
    %p = alloca 8
    store i16 %word, %p
    %loaded = load i8 %p
    stackrestore %saved
    %r = setjmp 0
    %initial = icmp eq i32 %r, 0
    br %initial, call_path, resumed
call_path:
    %x = call_indirect fn(i8) -> i8 %fp(%loaded)
    call @bounce(%x)
    fault unreachable
resumed:
    %v = trunc i32 %r to i8
    br swap(%v, i8 0x51, i1 0)
swap(i8 %a, i8 %b, i1 %swapped):
    switch i1 %swapped, 0: swap(%b, %a, i1 1), default: done(%a, %b)
done(i8 %left, i8 %right):
    %addr = retaddr 0
    %pos = trunc i64 %addr to i8
    %delta = sub i8 %left, %right
    %result = add i8 %delta, %pos
    ret %result
}}
func @leaf(i8 %x) -> i8 {{
entry:
    %v = xor i8 %x, 0x20
    ret %v
}}
func @bounce(i8 %x) {{
entry:
    %v = zext i8 %x to i32
    longjmp 0, %v
}}
"#
    ));
    use SitePos::{Inst as I, Term};

    let walk = entry(&module, "walk");
    let leaf = entry(&module, "leaf");
    let bounce = entry(&module, "bounce");
    // The position of each attempted step in program order, with the return from longjmp
    // going to the comparison after setjmp. In particular, switch must swap both old operands.
    let trace = [
        (walk, 0, I(0), Undo::Slot),
        (walk, 0, I(1), Undo::Slot),
        (walk, 0, I(2), Undo::Slot),
        (walk, 0, I(3), Undo::Slot),
        (walk, 0, I(4), Undo::Slot),
        (walk, 0, I(5), Undo::Slot),
        (walk, 0, I(6), Undo::Alloca),
        (walk, 0, I(7), Undo::Store),
        (walk, 0, I(8), Undo::Slot),
        (walk, 0, I(9), Undo::StackRestore),
        (walk, 0, I(10), Undo::Setjmp),
        (walk, 0, I(11), Undo::Slot),
        (walk, 0, Term, Undo::Jump),
        (walk, 1, I(0), Undo::Call),
        (leaf, 0, I(0), Undo::Slot),
        (leaf, 0, Term, Undo::Return),
        (walk, 1, I(1), Undo::Call),
        (bounce, 0, I(0), Undo::Slot),
        (bounce, 0, Term, Undo::Longjmp),
        (walk, 0, I(11), Undo::Slot),
        (walk, 0, Term, Undo::Jump),
        (walk, 2, I(0), Undo::Slot),
        (walk, 2, Term, Undo::Jump),
        (walk, 3, Term, Undo::Jump),
        (walk, 3, Term, Undo::Jump),
        (walk, 4, I(0), Undo::Slot),
        (walk, 4, I(1), Undo::Slot),
        (walk, 4, I(2), Undo::Slot),
        (walk, 4, I(3), Undo::Slot),
        (walk, 4, Term, Undo::Finish),
    ];
    let finish = trace.len() as u64;
    for budget in 0..=finish + 1 {
        let completed = budget.min(finish);
        // (0x51 - (7 XOR 0x20)) + 11, the position in @main after the apply.
        let outcome = if budget < finish { Outcome::Faulted(Fault::Budget) } else { Outcome::Returned(Some(53)) };
        check_apply(&module, budget, outcome, completed, |recording| {
            let actual: Vec<_> =
                recording.entries[1..].iter().map(|r| (r.site.func, r.site.block.0, r.site.pos, r.kind)).collect();
            assert_eq!(actual, trace[..completed as usize], "B={budget}");
        });
    }
}

#[test]
fn a_faulting_attempt_costs_nothing_even_when_it_is_the_last_allowed_attempt() {
    for (stop, fault) in [
        ("%x = srem i128 -7, 0\n    ret 7", Fault::DivideByZero),
        ("%x = alloca 18446744073709551615\n    ret 7", Fault::StackOverflow),
        ("stackrestore 225\n    ret 7", Fault::BadStackRestore),
        ("%x = call_indirect fn() -> i8 1()\n    ret %x", Fault::BadIndirect),
        ("call @too_large()\n    ret 7", Fault::StackOverflow),
        ("%x = load i9 255\n    ret 7", Fault::OutOfBounds),
        ("store i9 9, 254\n    ret 7", Fault::QubitMemoryAccess),
        ("%x = setjmp 225\n    ret 7", Fault::OutOfBounds),
        ("longjmp 224, 0", Fault::QubitMemoryAccess),
        ("longjmp 0, 0", Fault::BadLongjmp),
        ("effect 65\n    ret 7", Fault::EffectInRegion),
        ("exit i16 7", Fault::ExitInRegion),
        ("%x = qalloc 1\n    ret 7", Fault::QuantumInRegion),
        ("fault unreachable", Fault::Unreachable),
    ] {
        let module = load(&format!(
            "memory 256\ndata 210 = zero 1\n{DRIVER}\nfunc @walk() -> i8 {{\nentry:\n    store i8 9, 16\n    {stop}\n}}\n\
             func @too_large() {{\nentry:\n    %a = add i8 0, 1\n    %b = add i8 0, 2\n    ret\n}}"
        ));
        for budget in 0..=3 {
            let code = if budget < 2 { Fault::Budget } else { fault };
            check_apply(&module, budget, Outcome::Faulted(code), budget.min(1), |recording| {
                if budget >= 1 {
                    assert_eq!(recording.entries[1].kind, Undo::Store, "{stop}");
                }
            });
        }
    }
}

fn check_value(body: &str, expected: u128, steps: u64) {
    let module = load(&format!("memory 256\n{DRIVER}\nfunc @walk() -> i8 {{\nentry:\n{body}\n}}"));
    for budget in steps - 1..=steps + 1 {
        let outcome = if budget < steps { Outcome::Faulted(Fault::Budget) } else { Outcome::Returned(Some(expected)) };
        check_apply(&module, budget, outcome, steps.min(budget), |_| {});
    }
}

#[test]
fn wide_signed_arithmetic_and_shift_counts_use_all_the_operand_bits() {
    for (instruction, expected) in [
        ("ashr i65 -2, 64", 0xff),
        ("ashr i65 -2, 65", 0),
        ("shl i128 5, 18446744073709551616", 0),
        ("srem i128 0x80000000000000000000000000000000, -1", 0),
        ("srem i128 -7, 3", 0xff),
    ] {
        let width = instruction.split_ascii_whitespace().nth(1).expect("integer width");
        check_value(&format!("    %x = {instruction}\n    %v = trunc {width} %x to i8\n    ret %v"), expected, 3);
    }
    check_value(
        "    %x = sdiv i128 0x80000000000000000000000000000000, -1\n    %hi = lshr i128 %x, 120\n    %v = trunc i128 %hi to i8\n    ret %v",
        0x80,
        4,
    );
}

#[test]
fn stackrestore_changes_the_pointer_at_both_endpoints_and_between_them() {
    // The caller has 4 slots, the callee 3, so alloca lowers 200 to 192.
    for pointer in [191, 192, 193, 199, 200, 201] {
        let module = load(&format!(
            "memory 256\n{DRIVER}\nfunc @walk() -> i8 {{\nentry:\n    %p = alloca 1\n    stackrestore {pointer}\n    %sp = stacksave\n    %v = trunc i64 %sp to i8\n    ret %v\n}}"
        ));
        for budget in 0..=6 {
            let (outcome, completed) = if budget >= 2 && !(192..=200).contains(&pointer) {
                (Outcome::Faulted(Fault::BadStackRestore), 1)
            } else if budget < 5 {
                (Outcome::Faulted(Fault::Budget), budget)
            } else {
                (Outcome::Returned(Some(pointer)), 5)
            };
            check_apply(&module, budget, outcome, completed, |_| {});
        }
    }
}

#[test]
fn alloca_rounds_down_after_subtraction_even_for_zero_bytes() {
    // The current pointer is 199 after stackrestore, and the floor is 192.
    // Subtracting 7 reaches the floor. Subtracting 8 rounds one word below it.
    for size in [0, 6, 7, 8] {
        let module = load(&format!(
            "memory 256\ndata 191 = zero 1\n{DRIVER}\nfunc @walk() -> i8 {{\nentry:\n    %reserve = alloca 1\n    stackrestore 199\n    %p = alloca {size}\n    %v = trunc i64 %p to i8\n    ret %v\n}}"
        ));
        for budget in 2..=6 {
            let (outcome, completed) = if size == 8 && budget >= 3 {
                (Outcome::Faulted(Fault::StackOverflow), 2)
            } else if budget < 5 {
                (Outcome::Faulted(Fault::Budget), budget)
            } else {
                (Outcome::Returned(Some(192)), 5)
            };
            check_apply(&module, budget, outcome, completed, |_| {});
        }
    }
}

#[test]
fn function_addresses_are_one_based_even_without_an_indirect_call() {
    // @main is first in the module. @walk is second.
    for (name, address) in [("main", 1), ("walk", 2)] {
        check_value(&format!("    %ptr = faddr @{name}\n    %v = trunc i64 %ptr to i8\n    ret %v"), address, 3);
    }
}

#[test]
fn nested_retaddr_can_read_every_suspended_frame_and_no_farther() {
    let driver = DRIVER.replace("@main", "@driver").replace("qalloc 8", "qalloc 64");
    for (level, expected) in [
        (0_u32, 0x0000_0003_0000_0001),
        (1, 0x0000_0002_0000_000b),
        (2, 0x0000_0001_0000_0001),
        (3, 0xffff_ffff_0000_0000),
        (4, 0),
        (u32::MAX, 0),
    ] {
        let module = load(&format!(
            r#"
memory 256
func @main(i64 %budget) {{
entry:
    call @driver(%budget)
    ret
}}
{driver}
func @walk() -> i64 {{
entry:
    %v = call @leaf()
    ret %v
}}
func @leaf() -> i64 {{
entry:
    %v = retaddr {level}
    ret %v
}}
"#
        ));
        let mut run = executor(&module, "main", &[4], bounded());
        assert_eq!(run.run(), Outcome::Returned(None));
        let state = run.snapshot();
        let results: Vec<_> = state.quantum.amplitudes.iter().map(|(config, _)| config.as_slice()).collect();
        assert_eq!(results, [&[0x81 ^ expected, 0x11, 0, 1], &[0x81 ^ expected, 0x11, 1, 1]], "depth {level}");
        assert_eq!(run.stats().region_steps, 8);
    }
}
