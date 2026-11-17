//! The entry frame at the stack's edge: every memory size from nine bytes short of the entry frame to nine
//! bytes past it, for entries of several signatures, with and without static data below the stack. A frame
//! is eight bytes per named value (`docs/DESIGN.md`; parameters, block parameters and results, counted by
//! hand below), reserved down from the top of memory and never below the end of the static data. Where it
//! does not fit, the run is `stack_overflow` before its first step; where it fits, the run ends as with
//! ample memory, except that a callee whose own frame does not fit faults at its call. Each case is also
//! stepped against the independent whole-world reference (`oracle_support::worlds`).

#[allow(dead_code, reason = "this test uses the whole-world reference only")]
mod oracle_support;

use bennett_exec::{Executor, Options, Outcome};
use bennett_ir::Fault;
use oracle_support::worlds::{Model, basis};
use oracle_support::worlds_executor::{compare, observed_one_of, observed_reference};

const FUNCTIONS: &str = r#"
func @none() {
entry:
 ret
}
func @narrow(i8 %a) -> i8 {
entry:
 ret %a
}
func @mixed(i128 %a, i1 %b, i64 %c) -> i64 {
entry:
 %d = add i64 %c, 1
 ret %d
}
func @blocks(i8 %a) -> i8 {
entry:
 br done(%a)
done(i8 %x):
 %y = add i8 %x, 1
 ret %y
}
func @quantum() -> i8 {
entry:
 %q = qalloc 1
 x %q[0]
 x %q[0]
 qfree %q
 ret 4
}
func @history() -> i8 {
entry:
 %h = mark
 %v = add i8 1, 2
 rewind %h
 ret 7
}
func @calls() -> i8 {
entry:
 %v = call @narrow(i8 5)
 ret %v
}
"#;

/// (entry, arguments, slots counted by hand, the callee's slots or 0, the result with room enough).
const ENTRIES: &[(&str, &[u64], u64, u64, Outcome)] = &[
    ("none", &[], 0, 0, Outcome::Returned(None)),
    ("narrow", &[200], 1, 0, Outcome::Returned(Some(200))),
    ("mixed", &[u64::MAX, 1, 41], 4, 0, Outcome::Returned(Some(42))),
    ("blocks", &[254], 3, 0, Outcome::Returned(Some(255))),
    ("quantum", &[], 1, 0, Outcome::Returned(Some(4))),
    ("history", &[], 2, 0, Outcome::Returned(Some(7))),
    ("calls", &[], 1, 1, Outcome::Returned(Some(5))),
];

/// Static data ending at 0 (none), at 3 and at 17 bytes, so that the stack's floor is unaligned too.
const DATA: &[(&str, u64)] = &[("", 0), ("data 0 = i8 1, i8 2, i8 3\n", 3), ("data 9 = i64 -1\n", 17)];

fn expected(stack: u64, slots: u64, callee: u64, ample: Outcome) -> Outcome {
    if stack < 8 * slots || stack < 8 * (slots + callee) { Outcome::Faulted(Fault::StackOverflow) } else { ample }
}

#[test]
fn every_memory_size_around_each_entry_frame_agrees_with_the_count_and_the_reference() {
    let mut cases = 0;
    for &(data, floor) in DATA {
        for &(name, args, slots, callee, ample) in ENTRIES {
            let frame = 8 * (slots + callee);
            for memory in floor + frame.saturating_sub(9)..=floor + frame + 9 {
                let text = format!("memory {memory}\n{data}{FUNCTIONS}");
                let module = bennett_ir::load(&text).expect("valid fixture");
                let f = module.function_named(name).expect("entry");
                assert_eq!(module.static_end(), floor);
                assert_eq!(module.function(f).slot_count() as u64, slots, "@{name}");
                let want = expected(memory - floor, slots, callee, ample);
                for (reversible, machine_meaning) in [(false, false), (true, false), (false, true), (true, true)] {
                    let options = Options { reversible, machine_meaning, ..Options::default() };
                    let mut e = Executor::new(&module, f, args, options).expect("start");
                    let initial = e.snapshot();
                    let at_start = (memory - floor < 8 * slots).then_some(Outcome::Faulted(Fault::StackOverflow));
                    assert_eq!(e.outcome(), at_start, "@{name} memory {memory} floor {floor}");
                    assert_eq!(e.run(), want, "@{name} memory {memory} floor {floor}");
                    assert_eq!(e.tape(), b"");
                    if reversible {
                        e.reverse().expect("reverse");
                        let mut back = e.snapshot();
                        back.classical.outcome = initial.classical.outcome;
                        assert_eq!(back, initial, "@{name} memory {memory}: reversal restores the start");
                    }
                }
                compare(&module, name, args, 64, true);
                let model = Model::new(&module, name);
                let args128: Vec<u128> = args.iter().map(|&n| n.into()).collect();
                let reference = observed_reference(&model, basis(model.initial(name, &args128)), 64);
                let mut e = Executor::new(&module, f, args, Options::default()).expect("start");
                e.run();
                assert!(observed_one_of(&model, &reference, &e.snapshot()), "@{name} memory {memory}");
                cases += 1;
            }
        }
    }
    // Per data layout: 19 sizes for frames of 16 bytes or more (four entries), 18 for the two 8-byte frames
    // and 10 for the empty one, whose sizes below the floor do not exist.
    assert_eq!(cases, 3 * (4 * 19 + 2 * 18 + 10));
}

/// A memory that is only static data: the zero-slot entry fits and runs; one with a slot does not.
#[test]
fn a_memory_with_no_stack_runs_only_an_entry_without_slots() {
    for &(data, floor) in DATA {
        let module = bennett_ir::load(&format!("memory {floor}\n{data}{FUNCTIONS}")).expect("valid");
        for &(name, args, slots, _, ample) in ENTRIES {
            let f = module.function_named(name).expect("entry");
            let mut e = Executor::new(&module, f, args, Options::default()).expect("start");
            let want = if slots == 0 { ample } else { Outcome::Faulted(Fault::StackOverflow) };
            assert_eq!(e.run(), want, "@{name} memory {floor}");
        }
    }
}
