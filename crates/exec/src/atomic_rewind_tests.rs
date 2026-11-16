//! A failed rewind is atomic (`HANDOFF.md`, decisions of 2026-11-16): the fault is `bad_rewind` and
//! nothing changed. These tests compare the whole machine before and after each way a rewind fails: every
//! world's memory, every slot (dead ones too), frames, stack pointer, tape, journal with its side tables,
//! register table, cells, allocation history, amplitudes, deferred checks, kept copies and status, and the
//! executor's own state (the generator `measure` samples with, observations, splits, the departed states,
//! the counters of nested un-runs). Only the work counters may differ: undoing and running again is work.

use bennett_ir::{BlockId, Fault, Inst, ValidModule};
use rand_chacha::ChaCha8Rng;

use crate::executor::{Executor, Frame, Options, Outcome};
use crate::journal::Journal;
use crate::quantum::QuantumState;
use crate::worlds::{Defect, Status, World};

/// Amplitudes after an un-run and its run again round; a few gates give errors of order 1e-16.
const TOLERANCE: f64 = 1e-12;

#[derive(Debug)]
struct WorldState {
    memory: Vec<u8>,
    slots: Vec<u128>,
    frames: Vec<Frame>,
    stack_pointer: u64,
    tape: Vec<u8>,
    journal: Option<Journal>,
    outcome: Option<Outcome>,
    status: Status,
    partial: bool,
    deferred: Vec<Defect>,
    backups: Vec<WorldState>,
    pending: bool,
    quantum: QuantumState,
}

impl WorldState {
    fn of(world: &World) -> Self {
        WorldState {
            memory: world.memory.to_vec(),
            slots: world.slots.clone(),
            frames: world.frames.clone(),
            stack_pointer: world.stack_pointer,
            tape: world.tape.clone(),
            journal: world.journal.clone(),
            outcome: world.outcome,
            status: world.status,
            partial: world.partial,
            deferred: world.deferred.clone(),
            backups: world.backups.iter().map(WorldState::of).collect(),
            pending: world.pending,
            quantum: world.quantum.clone(),
        }
    }

    fn same(&self, other: &WorldState) -> bool {
        self.memory == other.memory
            && self.slots == other.slots
            && self.frames == other.frames
            && self.stack_pointer == other.stack_pointer
            && self.tape == other.tape
            && self.journal == other.journal
            && self.outcome == other.outcome
            && self.status == other.status
            && self.partial == other.partial
            && self.deferred == other.deferred
            && self.backups.len() == other.backups.len()
            && self.backups.iter().zip(&other.backups).all(|(a, b)| a.same(b))
            && self.pending == other.pending
            && self.quantum.same_state(&other.quantum, TOLERANCE)
    }
}

#[derive(Debug)]
struct Machine {
    worlds: Vec<WorldState>,
    current: usize,
    forked: usize,
    steps: u64,
    rng: ChaCha8Rng,
    run_outcome: Option<Outcome>,
    departed: Vec<u64>,
    observed: bool,
    observations: u64,
    splits: u64,
    merges: u64,
    mismatches: u64,
    deferring: bool,
    rewinding: u32,
    rerunning: u32,
    controls: (Option<u64>, Option<usize>, bool, bool, bool, usize, usize, bool),
}

impl Machine {
    fn of(e: &Executor<'_>) -> Self {
        let current = WorldState::of(&e.clone_world(None));
        let worlds = if e.has_worlds() {
            let mut current = Some(current);
            e.worlds
                .iter()
                .map(|world| world.as_ref().map_or_else(|| current.take().expect("one current world"), WorldState::of))
                .collect()
        } else {
            vec![current]
        };
        Machine {
            worlds,
            current: e.current,
            forked: e.forked.len(),
            steps: e.steps,
            rng: e.rng.clone(),
            run_outcome: e.run_outcome,
            departed: e.departed.clone(),
            observed: e.observed,
            observations: e.observations,
            splits: e.splits,
            merges: e.merges,
            mismatches: e.mismatches,
            deferring: e.deferring,
            rewinding: e.rewinding,
            rerunning: e.rerunning,
            controls: (
                e.budget,
                e.max_configurations,
                e.allow_region_faults,
                e.in_region,
                e.materialised,
                e.base_depth,
                e.region_records,
                e.splitting,
            ),
        }
    }

    fn assert_same(&self, other: &Machine, context: &str) {
        assert_eq!(self.worlds.len(), other.worlds.len(), "{context}: number of worlds");
        for (index, (a, b)) in self.worlds.iter().zip(&other.worlds).enumerate() {
            assert!(a.same(b), "{context}: world {index} differs\nafter: {a:#?}\nbefore: {b:#?}");
        }
        let scalars =
            |m: &Machine| (m.current, m.forked, m.steps, m.run_outcome, m.departed.clone(), m.observed, m.observations);
        assert_eq!(scalars(self), scalars(other), "{context}: run state");
        let nested = |m: &Machine| (m.splits, m.merges, m.mismatches, m.deferring, m.rewinding, m.rerunning);
        assert_eq!(nested(self), nested(other), "{context}: un-run state");
        assert_eq!(self.controls, other.controls, "{context}: limits and execution controls");
        assert_eq!(self.rng, other.rng, "{context}: the generator measurements sample with");
    }
}

fn load(text: &str) -> ValidModule {
    bennett_ir::load(text).unwrap_or_else(|error| panic!("{error}"))
}

fn start<'m>(module: &'m ValidModule, options: Options) -> Executor<'m> {
    let main = module.function_named("main").expect("main");
    let options = Options { reversible: true, budget: Some(10_000), max_configurations: Some(1 << 12), ..options };
    Executor::new(module, main, &[], options).expect("start")
}

/// Steps until every world's top frame is at the last `rewind` of `@main`, in block order.
fn at_last_rewind(e: &mut Executor<'_>, module: &ValidModule) {
    let main = module.function_named("main").expect("main");
    let (block, index) = (module.function(main).blocks.iter().enumerate().rev())
        .find_map(|(block, b)| {
            let index = b.insts.iter().rposition(|inst| matches!(inst, Inst::Rewind { .. }))?;
            Some((BlockId(block as u32), index))
        })
        .expect("a rewind in @main");
    for _ in 0..1000 {
        let snapshot = e.snapshot();
        let all = std::iter::once(&snapshot).chain(&snapshot.worlds).all(|world| {
            let frames = &world.classical.frames;
            frames.len() == 1 && frames[0].func == main && frames[0].block == block && frames[0].inst == index
        });
        if all {
            return;
        }
        assert_eq!(e.step(), None, "the run reaches the last rewind");
    }
    panic!("the last rewind is not reached");
}

/// Runs `text` to its last rewind, takes that step, and checks that it faults `bad_rewind` and changes
/// nothing but the outcome.
fn refuses_unchanged(text: &str, context: &str) {
    let module = load(text);
    let mut e = start(&module, Options::default());
    at_last_rewind(&mut e, &module);
    let before = Machine::of(&e);
    assert_eq!(e.step(), Some(Outcome::Faulted(Fault::BadRewind)), "{context}");
    let mut after = Machine::of(&e);
    after.worlds[0].outcome = None;
    after.assert_same(&before, context);
    e.outcome = None;
    // Still the state its history describes: reversal reaches the start.
    e.reverse().unwrap_or_else(|error| panic!("{context}: reverses after the refusal: {error}"));
}

/// An outer rewind whose keep sets `%out`, so that `ry %q ctrl %out` puts `%q` in superposition when it
/// is un-run; the inner rewind's region, run again, then reads `%q` and splits. `callee` is the inner
/// region's callee, called with `%q` as `@f(%q)` or, if `two`, `@f(%q, %q)`. If `one`, `%q` is 1 going
/// forward, else 0.
fn replay_split(callee: &str, two: bool, one: bool) -> String {
    let args = if two { "%q, %q" } else { "%q" };
    let prepare = if one { "x %q" } else { "" };
    format!(
        "memory 512
{callee}
func @main() {{
entry:
    %q = qalloc 1
    %out = qalloc 1
    {prepare}
    %outer = mark
    ry(0.7) %q ctrl %out
    %inner = mark
    call @f({args})
    rewind %inner
    ry(-0.7) %q ctrl %out
    rewind %outer keep (i1 1 -> %out)
    ret
}}
"
    )
}

#[test]
fn a_fault_in_a_world_split_off_by_a_replay_changes_nothing() {
    let callee = "func @f(i1 %x) {\nentry:\n    br %x, bad, done\nbad:\n    fault unreachable\ndone:\n    ret\n}";
    refuses_unchanged(&replay_split(callee, false, false), "the world of value 1 faults");
}

#[test]
fn a_fault_in_the_part_a_replay_keeps_changes_nothing() {
    let callee = "func @f(i1 %x) {\nentry:\n    br %x, done, bad\nbad:\n    fault unreachable\ndone:\n    ret\n}";
    refuses_unchanged(&replay_split(callee, false, true), "the world of value 0 faults");
}

#[test]
fn a_measurement_newly_reached_by_a_replay_samples_and_observes_nothing() {
    for (first, second, one, context) in [
        ("observe", "done", false, "the world of value 1 reaches the measurement"),
        ("done", "observe", true, "the world of value 0 reaches the measurement"),
    ] {
        let callee = format!(
            "func @f(i1 %x, q1 %q) {{\nentry:\n    br %x, {first}, {second}\nobserve:\n    \
             %measured = measure %q\n    ret\ndone:\n    ret\n}}"
        );
        refuses_unchanged(&replay_split(&callee, true, one), context);
    }
}

#[test]
fn a_failure_in_a_split_off_world_while_it_un_runs_further_changes_nothing() {
    // The later inner rewind's replay splits; the world that split off un-runs on and replays the earlier
    // inner rewind, whose region faults on value 1, so a world fails while the outer rewind's un-run
    // continues in the other.
    refuses_unchanged(
        "memory 512
func @id(i1 %x) -> i1 {
entry:
    ret %x
}
func @f(i1 %x) {
entry:
    br %x, bad, done
bad:
    fault unreachable
done:
    ret
}
func @main() {
entry:
    %q = qalloc 1
    %out = qalloc 1
    %outer = mark
    ry(0.7) %q ctrl %out
    %early = mark
    call @f(%q)
    rewind %early
    ry(0.4) %q ctrl %out
    %late = mark
    %v = call @id(%q)
    rewind %late
    ry(-1.1) %q ctrl %out
    rewind %outer keep (i1 1 -> %out)
    ret
}
",
        "a split-off world fails further down",
    );
}

#[test]
fn an_unfaithful_allocation_run_forward_again_changes_nothing() {
    refuses_unchanged(
        "memory 512
func @main() {
entry:
    %out = qalloc 1
    %m = mark
    %temporary = qalloc 1
    x %temporary ctrl %out
    z %out
    rewind %m keep (i1 1 -> %out)
    ret
}
",
        "a temporary left nonzero",
    );
}

#[test]
fn keeps_whose_net_change_to_a_read_byte_is_not_zero_change_nothing() {
    for keeps in ["i8 1 -> [16], i8 3 -> [16]", "i8 1 -> [16], i8 1 -> [16], i8 1 -> [16]"] {
        refuses_unchanged(
            &format!(
                "memory 64\nfunc @main() {{\nentry:\n    z [16]\n    %m = mark\n    %value = load i8 16\n    \
                 rewind %m keep ({keeps})\n    ret\n}}\n"
            ),
            keeps,
        );
    }
}

#[test]
fn keeps_whose_net_change_to_a_read_register_is_not_zero_change_nothing() {
    refuses_unchanged(
        "memory 512
func @id(i2 %x) -> i2 {
entry:
    ret %x
}
func @main() {
entry:
    %q = qalloc 2
    x %q[0]
    %m = mark
    %v = call @id(%q)
    rewind %m keep (i2 1 -> %q, i2 2 -> %q)
    ret
}
",
        "1 then 2 into a read register",
    );
}

#[test]
fn keeps_that_cancel_on_a_read_register_interfere_as_no_keep() {
    let module = load(
        "memory 512
func @id(i1 %x) -> i1 {
entry:
    ret %x
}
func @main() {
entry:
    %q = qalloc 1
    h %q
    %m = mark
    %v = call @id(%q)
    rewind %m keep (i1 1 -> %q, i1 1 -> %q)
    h %q
    %r = measure %q
    ret
}
",
    );
    for seed in 0..8 {
        let mut e = start(&module, Options { seed, ..Options::default() });
        assert_eq!(e.run(), Outcome::Returned(None), "seed {seed}");
        assert_eq!(e.stats().merges, 1, "seed {seed}: the worlds merge again");
        let quantum = e.snapshot().quantum;
        assert_eq!(quantum.amplitudes.len(), 1, "seed {seed}: H H is the identity");
        assert!((quantum.amplitude(&[0]) - num_complex::Complex64::new(1.0, 0.0)).norm() < TOLERANCE);
    }
}

/// With several worlds already, a rewind fails in one of them: that world is exactly as before, ended
/// with the fault (region faults allowed), and the others are not observed or dropped. The prelude
/// splits on `%s`; only the world of value 1 keeps a value, so only it replays under a changed state.
#[test]
fn a_failed_rewind_among_several_worlds_changes_nothing_in_its_world() {
    let callees = [
        (
            "fault",
            "func @f(i1 %x, q1 %q) {\nentry:\n    br %x, bad, done\nbad:\n    fault unreachable\ndone:\n    ret\n}",
        ),
        (
            "measure",
            "func @f(i1 %x, q1 %q) {\nentry:\n    br %x, observe, done\nobserve:\n    %measured = measure %q\n    \
             ret\ndone:\n    ret\n}",
        ),
    ];
    for (name, callee) in callees {
        let module = load(&format!(
            "memory 512
func @id(i1 %x) -> i1 {{
entry:
    ret %x
}}
{callee}
func @main() {{
entry:
    %s = qalloc 1
    %q = qalloc 1
    %out = qalloc 1
    h %s
    %k = call @id(%s)
    %outer = mark
    ry(0.7) %q ctrl %out
    %inner = mark
    call @f(%q, %q)
    rewind %inner
    ry(-0.7) %q ctrl %out
    rewind %outer keep (%k -> %out)
    ret
}}
"
        ));
        let mut e = start(&module, Options { allow_region_faults: true, ..Options::default() });
        at_last_rewind(&mut e, &module);
        let before = Machine::of(&e);
        assert_eq!(before.worlds.len(), 2, "{name}");
        assert_eq!(e.step(), None, "{name}: the world of value 0 rewinds and goes on");
        let after = Machine::of(&e);
        assert_eq!(after.worlds.len(), 2, "{name}: no world is dropped or split");
        let faulted: Vec<&WorldState> =
            after.worlds.iter().filter(|w| w.outcome == Some(Outcome::Faulted(Fault::BadRewind))).collect();
        assert_eq!(faulted.len(), 1, "{name}: one world faults");
        let mut restored = clone_state(faulted[0]);
        restored.outcome = None;
        restored.status = Status::Running;
        let matches = before.worlds.iter().filter(|world| world.same(&restored)).count();
        assert_eq!(matches, 1, "{name}: the faulted world is as it was before its rewind\n{restored:#?}");
        assert_eq!((after.observed, after.observations), (false, 0), "{name}: nothing observed");
        assert_eq!(after.rng, before.rng, "{name}: nothing sampled");
    }
}

fn clone_state(world: &WorldState) -> WorldState {
    WorldState {
        memory: world.memory.clone(),
        slots: world.slots.clone(),
        frames: world.frames.clone(),
        stack_pointer: world.stack_pointer,
        tape: world.tape.clone(),
        journal: world.journal.clone(),
        outcome: world.outcome,
        status: world.status,
        partial: world.partial,
        deferred: world.deferred.clone(),
        backups: world.backups.iter().map(clone_state).collect(),
        pending: world.pending,
        quantum: world.quantum.clone(),
    }
}

/// The inner region branches on a value read from `%q`; a world whose replay takes `leave` never reaches
/// the inner rewind. If `one`, `%q` is 1 going forward, so the part a replay keeps (value 0) leaves.
fn replay_leaves(leave: &str, one: bool) -> String {
    let (prepare, first, second) = if one { ("x %q", "stay", "leave") } else { ("", "leave", "stay") };
    format!(
        "memory 512
func @id(i1 %x) -> i1 {{
entry:
    ret %x
}}
func @main() -> i32 {{
entry:
    %q = qalloc 1
    %out = qalloc 1
    {prepare}
    %outer = mark
    ry(0.7) %q ctrl %out
    %inner = mark
    %v = call @id(%q)
    br %v, {first}, {second}
leave:
    {leave}
stay:
    rewind %inner
    ry(-0.7) %q ctrl %out
    rewind %outer keep (i1 1 -> %out)
    ret 0
}}
"
    )
}

#[test]
fn a_replay_that_leaves_the_rewinds_frame_or_ends_the_run_changes_nothing() {
    // Each of these looped for ever before: running again only stopped at the rewind.
    for leave in ["ret 1", "exit i32 2"] {
        for one in [false, true] {
            refuses_unchanged(&replay_leaves(leave, one), &format!("{leave}, value {}", u8::from(!one)));
        }
    }
}

#[test]
fn a_replay_that_returns_from_a_callee_before_its_rewind_changes_nothing() {
    refuses_unchanged(
        "memory 512
func @id(i1 %x) -> i1 {
entry:
    ret %x
}
func @g(q1 %q) {
entry:
    %inner = mark
    %v = call @id(%q)
    br %v, leave, stay
leave:
    ret
stay:
    rewind %inner
    ret
}
func @main() {
entry:
    %q = qalloc 1
    %out = qalloc 1
    %outer = mark
    ry(0.7) %q ctrl %out
    call @g(%q)
    ry(-0.7) %q ctrl %out
    rewind %outer keep (i1 1 -> %out)
    ret
}
",
        "the world of value 1 returns from @g",
    );
    // A world that returned from the first activation of @g reaches the same rewind in the second: that
    // is another activation, not the end of the region run again.
    refuses_unchanged(
        "memory 512
func @id(i1 %x) -> i1 {
entry:
    ret %x
}
func @g(q1 %q, i1 %read) {
entry:
    %inner = mark
    br %read, test, stay
test:
    %v = call @id(%q)
    br %v, leave, stay
leave:
    ret
stay:
    rewind %inner
    ret
}
func @main() {
entry:
    %q = qalloc 1
    %out = qalloc 1
    %outer = mark
    ry(0.7) %q ctrl %out
    call @g(%q, i1 1)
    call @g(%q, i1 0)
    ry(-0.7) %q ctrl %out
    rewind %outer keep (i1 1 -> %out)
    ret
}
",
        "the world of value 1 returns from @g and calls it again",
    );
}

/// Independent replay-test family. HANDOFF's 2026-11-16 decision requires every new path here to
/// refuse atomically: a fault, measurement, leaving the marked activation, exit, or dirty free.
/// Four input values make the replay split twice; each forbidden value can be the retained part or
/// a fork. The input and the keep target independently live in registers or memory bytes.
fn generated_replay_failure(layout: u8, depth: usize, initial: u8, bad: u8, action: &str, rotation: bool) -> String {
    let input_memory = layout & 1 != 0;
    let output_memory = layout & 2 != 0;
    let q = |bit| if input_memory { format!("[16:{bit}]") } else { format!("%q[{bit}]") };
    let control = if output_memory { "[20:0]" } else { "%out[0]" };
    let target = if output_memory { "[20]" } else { "%out" };
    let prepare: String = (0..2).filter(|bit| initial & (1 << bit) != 0).map(|bit| format!("x {}\n", q(bit))).collect();
    let (forward, inverse) = if rotation { ("ry(0.7)", "ry(-0.7)") } else { ("x", "x") };
    let gates = |gate| -> String { (0..2).map(|bit| format!("{gate} {} ctrl {control}\n", q(bit))).collect() };
    let read = if input_memory { "%byte = load i8 16\n%v = trunc i8 %byte to i2" } else { "%v = call @read(%q)" };
    let mut callees = format!(
        "func @read(i2 %x) -> i2 {{\nentry:\nret %x\n}}\n\
         func @g0(q2 %q) {{\nentry:\n%inner = mark\n{read}\n\
         %bad = icmp eq i2 %v, {bad}\nbr %bad, bad, stay\n\
         bad:\neffect 88\n%p = alloca 8\nstore i8 73, %p\n{action}\n\
         stay:\nrewind %inner\nret\n}}\n"
    );
    for d in 1..=depth {
        callees
            .push_str(&format!("func @g{d}(q2 %q) {{\nentry:\n%m = mark\ncall @g{}(%q)\nrewind %m\nret\n}}\n", d - 1));
    }
    format!(
        "memory 512\n{callees}\nfunc @main() -> i8 {{\nentry:\n\
         %q = qalloc 2\n%out = qalloc 2\nz [16]\nz [20]\n{prepare}\n\
         %outer = mark\n{forward}call @g{depth}(%q)\ncall @g{depth}(%q)\n{inverse}\
         rewind %outer keep (i2 1 -> {target}, i2 2 -> {target}, i2 2 -> {target})\nret 0\n}}\n",
        forward = gates(forward),
        inverse = gates(inverse)
    )
}

#[test]
fn generated_replay_failures_restore_every_private_machine_field() {
    let mut count = 0;
    for layout in 0..4 {
        for depth in 0..3 {
            for initial in 0..4 {
                for bad in (0..4).filter(|&v| v != initial) {
                    for rotation in [false, true] {
                        // X reaches only initial^3; rotations reach every input value.
                        if !rotation && bad != initial ^ 3 {
                            continue;
                        }
                        for action in [
                            "fault unreachable",
                            "%m = measure %q\nret",
                            "ret",
                            "exit i8 7",
                            "%t = qalloc 1\nx %t\nqfree %t\nret",
                        ] {
                            // A return from a callee during a wrapper replay does not leave the
                            // wrapper's marked activation (DESIGN, History); that path may succeed.
                            if action == "ret" && depth > 0 {
                                continue;
                            }
                            let context = format!(
                                "layout {layout}, depth {depth}, initial {initial}, bad {bad}, rotation {rotation}, {action}"
                            );
                            refuses_unchanged(
                                &generated_replay_failure(layout, depth, initial, bad, action, rotation),
                                &context,
                            );
                            count += 1;
                        }
                    }
                }
            }
        }
    }
    println!("{count} generated replay refusals: full private machine unchanged, then reversed");
}

#[test]
fn generated_nonzero_net_keeps_refuse_before_mutating_any_world() {
    let mut count = 0;
    for targets in 0..8 {
        for values in 0..64 {
            let mut net = [0; 2];
            let keep: Vec<_> = (0..3)
                .map(|i| {
                    let value = (values >> (2 * i)) & 3;
                    let target = (targets >> i) & 1;
                    net[target] ^= value;
                    format!("i2 {value} -> {}", ["%q", "%out"][target])
                })
                .collect();
            if net[0] == 0 {
                continue;
            }
            let p = load(&format!(
                "memory 128\nfunc @id(i2 %v) -> i2 {{\nentry:\nret %v\n}}\n\
                 func @main() {{\nentry:\n%q = qalloc 2\n%out = qalloc 2\nh %q[0]\nh %q[1]\n\
                 %mark = mark\n%v = call @id(%q)\nrewind %mark keep ({})\nret\n}}\n",
                keep.join(", ")
            ));
            let mut e = start(&p, Options::default());
            at_last_rewind(&mut e, &p);
            let before = Machine::of(&e);
            assert_eq!(before.worlds.len(), 4);
            // execute rather than step: compare the instruction itself before the scheduler marks a
            // faulted world Ended or stops the run. All other worlds must remain byte-for-byte intact.
            assert!(matches!(e.execute(), Err(crate::executor::Halt::Fault(Fault::BadRewind))));
            Machine::of(&e).assert_same(&before, &format!("targets {targets}, values {values}"));
            count += 1;
        }
    }
    println!("{count} nonzero-net three-keep refusals: all four worlds and executor fields unchanged");
}

#[test]
fn generated_replay_failures_among_four_worlds_preserve_the_failed_worlds() {
    let mut count = 0;
    for layout in 0..4 {
        for depth in 0..3 {
            for action in ["fault unreachable", "%m = measure %q\nret", "ret", "exit i8 7"] {
                if action == "ret" && depth > 0 {
                    continue;
                }
                let text = generated_replay_failure(layout, depth, 0, 3, action, true)
                    .replace("%outer = mark", "%s = qalloc 2\nh %s[0]\nh %s[1]\n%k = call @read(%s)\n%outer = mark")
                    .replace("i2 1 ->", "%k ->");
                let p = load(&text);
                let mut e = start(&p, Options { allow_region_faults: true, ..Options::default() });
                at_last_rewind(&mut e, &p);
                let before = Machine::of(&e);
                assert_eq!(before.worlds.len(), 4);
                assert_eq!(e.step(), None);
                let after = Machine::of(&e);
                assert_eq!(after.worlds.len(), 4, "{text}");
                let mut failures = 0;
                for failed in after.worlds.iter().filter(|w| w.outcome == Some(Outcome::Faulted(Fault::BadRewind))) {
                    let mut restored = clone_state(failed);
                    restored.outcome = None;
                    restored.status = Status::Running;
                    assert_eq!(before.worlds.iter().filter(|w| w.same(&restored)).count(), 1, "{text}");
                    failures += 1;
                }
                assert_eq!(failures, 2, "{text}");
                assert_eq!(before.rng, after.rng);
                assert_eq!((after.observed, after.observations), (false, 0));
                assert_eq!(
                    (before.splits, before.merges, before.mismatches),
                    (after.splits, after.merges, after.mismatches)
                );
                count += 1;
            }
        }
    }
    println!("{count} four-world programs: two replay failures restored and two successful rewinds retained");
}

#[test]
fn a_deferred_failure_after_replaying_a_split_restores_the_whole_machine() {
    let p = load(
        "memory 512
func @id(i1 %x) -> i1 {
entry:
    ret %x
}
func @main() {
entry:
    %q = qalloc 1
    %out = qalloc 1
    h %q
    %outer = mark
    %temporary = qalloc 1
    x %temporary ctrl %out
    %inner = mark
    %v = call @id(%q)
    rewind %inner
    rewind %outer keep (i1 1 -> %out)
    ret
}
",
    );
    let mut e = start(&p, Options::default());
    let initial = e.snapshot();
    at_last_rewind(&mut e, &p);
    let before = Machine::of(&e);
    assert_eq!(e.step(), Some(Outcome::Faulted(Fault::BadRewind)));
    let mut after = Machine::of(&e);
    after.worlds[0].outcome = None;
    // DESIGN, History/Cost and Worlds/Scheduling: a completed rewind tick counts even when a
    // subsequent deferred check refuses it. The replay split and joined again; these are work counts.
    assert_eq!(after.steps, before.steps + 1);
    assert_eq!(after.splits, before.splits + 1);
    assert_eq!(after.merges, before.merges + 1);
    after.merges = before.merges;
    after.steps = before.steps;
    after.splits = before.splits;
    after.assert_same(&before, "deferred refusal restores the machine, charging the attempted work");
    e.reverse().expect("deferred failure reverses");
    let restored = e.snapshot();
    assert_eq!(restored.classical, initial.classical);
    assert!(restored.quantum.distance(&initial.quantum).is_some_and(|d| d < TOLERANCE));
}
