//! Small generated programs exercise the reference as a whole, from parsed text to completion.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use bennett_ir::Fault;
use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

use super::{BUDGET, Finish, module, observe};

const DEFAULT_PROGRAMS: u64 = 96;
const HEAVY_PROGRAMS: u64 = 2048;
const SEED_START: u64 = 0x2fd6_c8a0_547e_0620;

fn layer(rng: &mut ChaCha8Rng, registers: &[&str]) -> String {
    let mut wires = registers.to_vec();
    let mut text = String::new();
    for _ in 0..rng.random_range(3..=7) {
        wires.shuffle(rng);
        let gate = match rng.random_range(0..10) {
            0 => "x".into(),
            1 => "h".into(),
            2 => "z".into(),
            3 => "s".into(),
            4 => "sdg".into(),
            5 => "t".into(),
            6 => "tdg".into(),
            axis => {
                let op = ["rx", "ry", "rz"][axis - 7];
                let angle = rng.random_range(-2.8..=2.8);
                format!("{op}({angle:.8})")
            }
        };
        let controls = rng.random_range(0..wires.len().min(4));
        write!(text, " {gate} {}", wires[0]).expect("write gate");
        if controls > 0 {
            write!(text, " ctrl {}", wires[1..=controls].join(", ")).expect("write controls");
        }
        text.push('\n');
    }
    text
}

fn generate(seed: u64) -> (String, u64, u64) {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let initial = layer(&mut rng, &["%q[0]", "%q[1]", "%env", "%out[0]", "%out[1]"]);
    let low = layer(&mut rng, &["%q[0]", "%q[1]", "%out[0]", "%out[1]"]);
    let high = layer(&mut rng, &["%q[0]", "%q[1]", "%out[0]", "%out[1]"]);
    let operand = rng.random_range(0..=255);
    let stack = rng.random_range(1..=19);
    let apply_budget = rng.random_range(3..=12);
    let budget = if rng.random_ratio(1, 8) { rng.random_range(14..=42) } else { BUDGET };
    let mode = rng.random_range(0..4);
    let combine = ["add", "sub", "mul", "and", "or", "xor"][rng.random_range(0..6)];
    let predicate = ["slt", "sge", "ugt", "ult"][rng.random_range(0..4)];
    let arithmetic = ["add", "sub", "mul", "xor", "or", "shl", "lshr", "ashr", "sdiv", "urem"];
    let applied = arithmetic[rng.random_range(0..arithmetic.len())];
    let call = if rng.random() { "call @process(%written)" } else { "call_indirect fn(i8) -> i8 %fp(%written)" };

    let scrub = if seed.is_multiple_of(2) { "store i8 0, %local" } else { "" };
    let text = format!(
        "memory 512
data 0 = i64 @process, zero 24
func @main(i8 %input) -> i8 {{
entry:
 %q = qalloc 2
 %env = qalloc 1
 %out = qalloc 2
 %flag = qalloc 5
 %saved = stacksave
 %local = alloca {stack}
 store i8 %input, %local
 %written = load i8 %local
 {scrub}
 stackrestore %saved
 %fp = load i64 0
 %value = {call}
 store i8 %value, 16
 h %q[0]
 ry(0.71) %q[1]
 x %env ctrl %q[0]
 {initial}
 %bound = zext i8 {apply_budget} to i64
 apply @transform(%q, %value) -> %out, %flag budget %bound
 qfree %env
 %flagged = measure %flag
 %pre = measure %out
 %check = icmp eq i2 %pre, 0
 br %check, low, high
low:
 {low}
 br later(i8 11)
high:
 {high}
 br later(i8 29)
later(i8 %path):
 %last = call @sample(%q)
 %last8 = zext i2 %last to i8
 %code = zext i5 %flagged to i8
 %mixed = xor i8 %last8, %code
 %answer = {combine} i8 %mixed, %value
 store i8 %answer, 17
 effect %path
 effect %answer
 switch i8 {mode}, 0: done, 1: leave, 2: faulted, default: more
done:
 ret %answer
leave:
 call @finish(%answer)
 fault unreachable
faulted:
 fault unreachable
more:
 %adjusted = add i8 %answer, 1
 ret %adjusted
}}
func @process(i8 %input) -> i8 {{
entry:
 %cond = icmp {predicate} i8 %input, {operand}
 %next = select i8 %cond, %input, {operand}
 %wide = sext i8 %next to i16
 %shifted = ashr i16 %wide, 2
 %small = trunc i16 %shifted to i8
 br loop(i8 2, %small, %input)
loop(i8 %left, i8 %a, i8 %b):
 %n = sub i8 %left, 1
 %mix = {combine} i8 %a, %b
 %again = icmp ne i8 %n, 0
 br %again, loop(%n, %b, %a), done(%mix)
done(i8 %result):
 ret %result
}}
func @transform(i2 %input, i8 %control) -> i2 {{
entry:
 %read = load i8 16
 %sum = add i8 %read, %control
 store i8 %sum, 16
 %small = trunc i8 %sum to i2
 %result = {applied} i2 %small, %input
 ret %result
}}
func @sample(q2 %r) -> i2 {{
entry:
 %m = measure %r
 ret %m
}}
func @finish(i8 %answer) {{
entry:
 effect 33
 exit %answer
}}"
    );

    (text, rng.random_range(0..=255), budget)
}

fn sweep(programs: u64) {
    let mut totals = observe::Counts::default();
    let mut terminations = BTreeSet::new();
    let mut flags = BTreeSet::new();
    let mut paths = BTreeSet::new();
    for seed in SEED_START..SEED_START + programs {
        let (text, input, budget) = generate(seed);
        // Retain the seed and complete program for replay if the comparison fails.
        let result = std::panic::catch_unwind(|| {
            let module = module(&text);
            let result = observe::run(&module, "main", &[input], seed, budget);
            let finish = result.reference.finish.expect("completed");
            let tape = result.reference.tape;
            let flag = result.measurements.first().map(|(value, _)| *value);
            (finish, tape, flag, result.counts)
        })
        .unwrap_or_else(|failure| {
            eprintln!("seed {seed:#018x}, input {input}, budget {budget}\n{text}");
            std::panic::resume_unwind(failure)
        });
        let (finish, tape, flag, counts) = result;
        if let Some(flag) = flag {
            flags.insert(flag);
        }
        if let Some(&path) = tape.first() {
            paths.insert(path);
        }
        terminations.insert(match finish {
            Finish::Returned(_) if tape.len() == 3 => "exit",
            Finish::Returned(_) => "return",
            Finish::Faulted(Fault::Unreachable) => "fault",
            Finish::Faulted(Fault::Budget) => "budget",
            _ => panic!("unexpected generated outcome: {finish:?}"),
        });
        totals.add(&counts);
    }
    assert_eq!(paths, BTreeSet::from([11, 29]));
    assert_eq!(flags, BTreeSet::from([0, Fault::Budget.code(), Fault::DivideByZero.code()]));
    assert_eq!(terminations, BTreeSet::from(["return", "exit", "fault", "budget"]));
    assert_eq!(totals.programs as u64, programs);
    assert_eq!(totals.states, 2 * totals.measurements + totals.programs);
    println!(
        "{} generated programs, {} operations, {} checkpoint state comparisons, {} density entries, {} measurements",
        totals.programs, totals.operations, totals.states, totals.density_entries, totals.measurements,
    );
}

#[test]
fn seeded_device_programs() {
    sweep(DEFAULT_PROGRAMS);
}

#[test]
#[ignore = "heavy sweep"]
fn seeded_device_programs_heavy() {
    sweep(HEAVY_PROGRAMS);
}
