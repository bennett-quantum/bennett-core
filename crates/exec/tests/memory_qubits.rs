//! Gates that take addresses (`docs/DESIGN.md`, "Qubits in memory"): a qubit can live at a bit of
//! memory, and every register program rewritten to memory qubits gives the same state.
//!
//! The main experiment is `worlds_oracle.rs`'s program (prepare q, r and o; mark; read q as an integer;
//! gates G; rewind keeping f(q) into o) with each of q, r and o placed in a register or in memory, every
//! gate rewritten to name the qubits where they live, and q read by a load of its byte where it is in
//! memory. Its meaning is the dense calculation of the register program (`oracle_support::dense`): the
//! part where q = v becomes G† X_o^{f(v)} G of itself, unless G carries o into q, where the un-run of the
//! read is unfaithful and the rewind faults `bad_rewind`. The whole-world reference
//! (`oracle_support::worlds`) does not model memory qubits; it is run on the register program, and each
//! layout must give its state. Both oracles are compared through the documents' meaning of a memory
//! qubit: the byte's value is a register's value in each configuration.

#[allow(dead_code, reason = "this test uses the dense calculation and the whole-world reference only")]
mod oracle_support;

use std::collections::BTreeMap;

use bennett_exec::{Executor, Options, Outcome, RunError, Snapshot};
use bennett_ir::{Fault, Inst, ValidModule};
use nalgebra::DVector;
use num_complex::Complex64 as C;
use oracle_support::dense::{Dense, EPS, gate_matrix};

/// The three registers of the program, their widths, and the byte each lives at when in memory.
const NAMES: [(&str, u32, u64); 3] = [("q", 2, 64), ("r", 1, 65), ("o", 2, 66)];

/// Gates G between the read and the rewind, written for registers; the marked one carries o into q.
const MIDDLE: [&str; 9] = [
    "h %r",
    "ry(0.4) %r",
    "x %r ctrl %q[0]",
    "x %o[0] ctrl %r",
    "h %o[1]",
    "x %q[1] ctrl %o[0]", // carries o into q
    "s %q[0]",
    "h %q[1]",
    "x %q[0] ctrl %r",
];

/// Where each of q, r, o lives: `true` in memory.
type Layout = [bool; 3];

/// Every layout. The keep target o in memory is kept into with `keep (... -> [66])`.
fn layouts() -> Vec<Layout> {
    (0..8).map(|k| [k & 1 != 0, k & 2 != 0, k & 4 != 0]).collect()
}

/// `gate` with each register qubit of a name placed in memory rewritten as `[ADDRESS:BIT]`.
fn place(gate: &str, layout: Layout) -> String {
    let mut out = String::new();
    let mut rest = gate;
    while let Some(at) = rest.find('%') {
        out.push_str(&rest[..at]);
        let name = &rest[at + 1..at + 2];
        let (index, &(_, _, address)) = NAMES.iter().enumerate().find(|(_, n)| n.0 == name).expect("a register");
        let after = &rest[at + 2..];
        let (bit, tail) = match after.strip_prefix('[') {
            Some(inner) => {
                let close = inner.find(']').expect("`]`");
                (inner[..close].parse::<u8>().expect("a bit"), &inner[close + 1..])
            }
            None => (0, after),
        };
        if layout[index] {
            out.push_str(&format!("[{address}:{bit}]"));
        } else if bit == 0 && !after.starts_with('[') {
            out.push_str(&format!("%{name}"));
        } else {
            out.push_str(&format!("%{name}[{bit}]"));
        }
        rest = tail;
    }
    out.push_str(rest);
    out
}

/// The program with gates `middle` in `layout`; with `writes`, each world also writes memory between the
/// read and the rewind, as in `worlds_oracle.rs`.
fn program(middle: &[&str], writes: bool, layout: Layout) -> String {
    let (callee_store, caller_store) = if writes {
        (
            "    %w = zext i2 %z to i8\n    %a = zext i2 %x to i64\n    %p = add i64 %a, 8\n    store i8 %w, %p\n",
            "    %yb = zext i2 %y to i8\n    store i8 %yb, 32\n",
        )
    } else {
        ("", "")
    };
    let allocations: String = NAMES
        .iter()
        .zip(layout)
        .filter(|(_, in_memory)| !in_memory)
        .map(|(&(name, width, _), _)| format!("    %{name} = qalloc {width}\n"))
        .collect();
    let gates =
        |gates: &[&str]| -> String { gates.iter().map(|gate| format!("    {}\n", place(gate, layout))).collect() };
    let read = if layout[0] {
        "    %qb = load i8 64\n    %qv = trunc i8 %qb to i2\n    %y = call @f(%qv)\n"
    } else {
        "    %y = call @f(%q)\n"
    };
    let target = if layout[2] { "[66]" } else { "%o" };
    format!(
        "memory 256
data 8 = i8 7, i8 7, i8 7, i8 7
func @f(i2 %x) -> i2 {{
entry:
    %y = mul i2 %x, 3
    %z = add i2 %y, 1
{callee_store}    ret %z
}}
func @main() {{
entry:
{allocations}{prepare}    %m = mark
{read}{caller_store}{middle}    rewind %m keep (%y -> {target})
{after}    ret
}}
",
        prepare = gates(&["h %q[0]", "ry(0.7) %q[1]", "h %r", "rx(0.3) %o[0]"]),
        middle = gates(middle),
        after = gates(&["h %q[0]"]),
    )
}

fn f(v: u64) -> u64 {
    (3 * v + 1) % 4
}

type Gates = Vec<(bennett_ir::Gate, (usize, u8), Vec<(usize, u8)>)>;

/// The register program's gates, split at the call and the rewind.
fn gates(module: &ValidModule) -> [Gates; 3] {
    let main = module.function(module.function_named("main").expect("@main"));
    let registers: Vec<bennett_ir::Slot> = main.blocks[0]
        .insts
        .iter()
        .filter(|inst| matches!(inst, Inst::Qalloc { .. }))
        .filter_map(Inst::result)
        .collect();
    let column = |slot| registers.iter().position(|&r| r == slot).expect("a register");
    let mut phases: [Gates; 3] = Default::default();
    let mut phase = 0;
    for inst in &main.blocks[0].insts {
        match inst {
            Inst::Call { .. } | Inst::Rewind { .. } => phase += 1,
            Inst::Gate { gate, target, controls } => phases[phase.min(2)].push((
                *gate,
                (column(target.register), target.bit),
                controls.iter().map(|c| (column(c.register), c.bit)).collect(),
            )),
            _ => {}
        }
    }
    phases
}

fn apply(dense: &mut Dense, gates: &Gates, adjoint: bool) {
    let mut order: Vec<_> = gates.iter().collect();
    if adjoint {
        order.reverse();
    }
    for (gate, target, controls) in order {
        let matrix = gate_matrix(*gate);
        dense.gate(if adjoint { matrix.adjoint() } else { matrix }, *target, controls);
    }
}

/// The dense meaning of the register program, or `None` where some world's un-run is unfaithful.
fn expected(register_program: &ValidModule) -> Option<Dense> {
    let [before, middle, after] = gates(register_program);
    let mut state = DVector::from_element(32, C::new(0.0, 0.0));
    state[0] = C::new(1.0, 0.0);
    let mut prepared = Dense { widths: vec![2, 1, 2], state };
    apply(&mut prepared, &before, false);
    let mut total = Dense { widths: prepared.widths.clone(), state: DVector::zeros(32) };
    for v in 0..4u64 {
        let mut part = prepared.clone();
        for basis in 0..32 {
            if part.config(basis)[0] != v {
                part.state[basis] = C::new(0.0, 0.0);
            }
        }
        if part.state.norm() <= EPS {
            continue;
        }
        apply(&mut part, &middle, false);
        let widths = part.widths.clone();
        part.permute(|basis| basis ^ ((f(v) as usize) << (widths[0] + widths[1])));
        apply(&mut part, &middle, true);
        if !(0..32).all(|basis| part.config(basis)[0] == v || part.state[basis].norm() <= EPS) {
            return None;
        }
        total.state += &part.state;
    }
    apply(&mut total, &after, false);
    Some(total)
}

/// A state as the values of (q, r, o) to amplitudes, read from a snapshot: a register's column is its
/// allocation's (the program allocates before any gate, so registers take the first columns in order),
/// a memory qubit's the column of the cell holding its byte, whose other bits must be 0.
fn logical(snapshot: &Snapshot, layout: Layout) -> BTreeMap<[u64; 3], C> {
    let cells: BTreeMap<u64, usize> = snapshot.quantum.cells.iter().copied().collect();
    let mut next_register = 0;
    let columns: Vec<usize> = NAMES
        .iter()
        .zip(layout)
        .map(|(&(_, _, address), in_memory)| {
            if in_memory {
                cells[&address]
            } else {
                next_register += 1;
                next_register - 1
            }
        })
        .collect();
    let mut state = BTreeMap::new();
    for (config, amplitude) in &snapshot.quantum.amplitudes {
        let values = [config[columns[0]], config[columns[1]], config[columns[2]]];
        for (value, &(_, width, _)) in values.iter().zip(&NAMES) {
            assert!(value >> width == 0, "a memory qubit's byte has nothing above its qubits");
        }
        *state.entry(values).or_insert(C::new(0.0, 0.0)) += amplitude;
    }
    state
}

fn assert_same(found: &BTreeMap<[u64; 3], C>, expected: &BTreeMap<[u64; 3], C>, what: &str) {
    for key in found.keys().chain(expected.keys()) {
        let (a, b) = (found.get(key).copied().unwrap_or_default(), expected.get(key).copied().unwrap_or_default());
        assert!((a - b).norm() <= 1e-10, "{what}: {key:?}: {a} vs {b}");
    }
}

fn dense_state(dense: &Dense) -> BTreeMap<[u64; 3], C> {
    (0..32)
        .filter(|&basis| dense.state[basis].norm() > EPS)
        .map(|basis| {
            let c = dense.config(basis);
            ([c[0], c[1], c[2]], dense.state[basis])
        })
        .collect()
}

fn run(module: &ValidModule, options: Options) -> (Outcome, Executor<'_>) {
    let mut executor =
        Executor::new(module, module.function_named("main").expect("@main"), &[], options).expect("starts");
    (executor.run(), executor)
}

/// No gates, each gate and each pair, without and with memory writes in the worlds.
fn cases() -> Vec<(Vec<&'static str>, bool)> {
    let mut middles: Vec<Vec<&str>> = vec![Vec::new()];
    middles.extend(MIDDLE.iter().map(|&g| vec![g]));
    middles.extend(MIDDLE.iter().flat_map(|&a| MIDDLE.iter().map(move |&b| vec![a, b])));
    [false, true].into_iter().flat_map(|writes| middles.iter().map(move |m| (m.clone(), writes))).collect()
}

#[test]
fn memory_qubits_give_the_register_programs_state_and_un_run_to_the_conjugated_xor() {
    let options = Options { budget: Some(10_000), max_configurations: Some(1 << 10), ..Options::default() };
    let (mut matched, mut refused) = (0, 0);
    for (middle, writes) in cases() {
        let registers = bennett_ir::load(&program(&middle, writes, [false; 3])).expect("valid");
        let meaning = expected(&registers);
        let (register_outcome, register_run) = run(&registers, options);
        for layout in layouts() {
            let text = program(&middle, writes, layout);
            let module = bennett_ir::load(&text).unwrap_or_else(|e| panic!("{e}\n{text}"));
            let (outcome, executor) = run(&module, options);
            let what = format!("{middle:?} writes={writes} layout={layout:?}");
            let Some(dense) = &meaning else {
                assert_eq!(
                    outcome,
                    Outcome::Stopped(RunError::WorldFault { fault: Fault::BadRewind, worlds: 4 }),
                    "{what}"
                );
                assert_eq!(register_outcome, outcome, "{what}");
                refused += 1;
                continue;
            };
            assert_eq!(outcome, Outcome::Returned(None), "{what}");
            let snapshot = executor.snapshot();
            assert!(snapshot.worlds.is_empty(), "{what}: the worlds merged");
            assert_eq!(&snapshot.classical.memory[8..12], &[7; 4], "{what}: memory restored");
            assert_eq!(snapshot.classical.memory[32], 0, "{what}: memory restored");
            // The classical memory under a byte that holds qubits is 0.
            assert_eq!(&snapshot.classical.memory[64..67], &[0; 3], "{what}");
            let in_memory = layout.iter().filter(|&&m| m).count();
            assert_eq!(executor.stats().memory_qubit_bytes, in_memory, "{what}");
            let state = logical(&snapshot, layout);
            assert_same(&state, &dense_state(dense), &format!("{what}, against the dense calculation"));
            assert_same(&state, &logical(&register_run.snapshot(), [false; 3]), &format!("{what}, against registers"));
            // Forward then back returns to the start: no cell is left, memory and state as at entry.
            let reversible = Options { reversible: true, ..options };
            let mut back =
                Executor::new(&module, module.function_named("main").expect("@main"), &[], reversible).expect("starts");
            let at_entry = back.snapshot();
            assert_eq!(back.run(), Outcome::Returned(None), "{what}");
            back.reverse().expect("reverses");
            assert_eq!(back.snapshot().classical, at_entry.classical, "{what}");
            assert!(back.snapshot().quantum.distance(&at_entry.quantum).is_some_and(|d| d <= 1e-10), "{what}");
            matched += 1;
        }
    }
    // The 182 programs of `worlds_oracle.rs` in eight layouts: 150 and 32 each.
    assert_eq!((matched, refused), (150 * 8, 32 * 8));
}

/// The whole-world reference, run on the register program, gives the state of every layout.
#[test]
fn every_layout_gives_the_whole_world_references_state() {
    use oracle_support::worlds::{Finish, Model, basis};
    let options = Options { budget: Some(10_000), max_configurations: Some(1 << 10), ..Options::default() };
    let (mut agreed, mut both_refused) = (0, 0);
    for (middle, writes) in cases() {
        let registers = bennett_ir::load(&program(&middle, writes, [false; 3])).expect("valid");
        let model = Model::new(&registers, "main");
        let reference = model.run(basis(model.initial("main", &[])), 1_000);
        let refused = reference.keys().any(|w| w.classical.finish == Some(Finish::Faulted(Fault::BadRewind)));
        let mut state = BTreeMap::<[u64; 3], C>::new();
        for (world, &amplitude) in &reference {
            *state.entry([world.quantum[0], world.quantum[1], world.quantum[2]]).or_default() += amplitude;
        }
        for layout in layouts() {
            let module = bennett_ir::load(&program(&middle, writes, layout)).expect("valid");
            let (outcome, executor) = run(&module, options);
            let what = format!("{middle:?} writes={writes} layout={layout:?}");
            if refused {
                assert!(
                    matches!(outcome, Outcome::Stopped(RunError::WorldFault { fault: Fault::BadRewind, .. })),
                    "{what}"
                );
                both_refused += 1;
            } else {
                assert_eq!(outcome, Outcome::Returned(None), "{what}");
                assert_same(&logical(&executor.snapshot(), layout), &state, &what);
                agreed += 1;
            }
        }
    }
    assert_eq!((agreed, both_refused), (150 * 8, 32 * 8));
}

/// Grover over `examples/grover.bn`'s predicate in the history idiom (mark, call, rewind keep), with x
/// and the kick qubit in registers or in memory (x the low four bits of byte 64, the kick bit 0 of 65).
fn grover(memory: bool, iterations: u8) -> String {
    let (x, kick) = if memory {
        (["[64:0]", "[64:1]", "[64:2]", "[64:3]"], "[65]")
    } else {
        (["%x[0]", "%x[1]", "%x[2]", "%x[3]"], "%kick")
    };
    let diffuse: String = ["h", "x"]
        .iter()
        .flat_map(|g| x.iter().map(move |q| format!("    {g} {q}\n")))
        .chain([format!("    z {} ctrl {}, {}, {}\n", x[3], x[0], x[1], x[2])])
        .chain(["x", "h"].iter().flat_map(|g| x.iter().map(move |q| format!("    {g} {q}\n"))))
        .collect();
    let prepare: String = x.iter().map(|q| format!("    h {q}\n")).collect();
    let (allocate, read) = if memory {
        ("", "    %b = load i8 64\n    %v = trunc i8 %b to i4\n    %y = call @is_solution(%v)\n".to_owned())
    } else {
        ("    %x = qalloc 4\n    %kick = qalloc 1\n", "    %y = call @is_solution(%x)\n".to_owned())
    };
    let body = format!(
        "    br iterate(i8 0)
iterate(i8 %k):
    %h = mark
{read}    rewind %h keep (%y -> {kick})
{diffuse}    %k.next = add i8 %k, 1
    %more = icmp ult i8 %k.next, {iterations}
    br %more, iterate(%k.next), finish
finish:
"
    );
    format!(
        "memory 256
func @is_solution(i4 %x) -> i1 {{
entry:
    %square = mul i4 %x, %x
    %sum = add i4 %square, %x
    %hit = icmp eq i4 %sum, 6
    ret %hit
}}
func @main() {{
entry:
{allocate}{prepare}    x {kick}
    h {kick}
{body}    ret
}}
"
    )
}

/// The values of x and the kick qubit to amplitudes.
fn grover_state(snapshot: &Snapshot, memory: bool) -> BTreeMap<(u64, u64), C> {
    let cells: BTreeMap<u64, usize> = snapshot.quantum.cells.iter().copied().collect();
    let (x, kick) = if memory { (cells[&64], cells[&65]) } else { (0, 1) };
    let mut state = BTreeMap::new();
    for (config, amplitude) in &snapshot.quantum.amplitudes {
        *state.entry((config[x], config[kick])).or_insert(C::new(0.0, 0.0)) += amplitude;
    }
    state
}

/// What the register and the memory forms cost, measured exactly (deterministic counts). Both give the
/// same state; the memory form takes two more steps per oracle (the load and the truncation that read x),
/// journals the promotions and the read cell, and holds 8 qubits per byte that holds qubits.
#[test]
fn memory_qubits_cost_the_register_forms_worlds_and_a_byte_per_cell() {
    let options = Options { budget: Some(100_000), reversible: true, ..Options::default() };
    let mut rows = Vec::new();
    for iterations in [1u8, 2, 8] {
        let mut states = Vec::new();
        for memory in [false, true] {
            let module = bennett_ir::load(&grover(memory, iterations)).expect("valid");
            let (outcome, executor) = run(&module, options);
            assert_eq!(outcome, Outcome::Returned(None));
            states.push(grover_state(&executor.snapshot(), memory));
            rows.push((
                format!("grover k={iterations} {}", if memory { "memory" } else { "registers" }),
                executor.stats(),
            ));
        }
        for key in states[0].keys().chain(states[1].keys()) {
            let (a, b) =
                (states[0].get(key).copied().unwrap_or_default(), states[1].get(key).copied().unwrap_or_default());
            assert!((a - b).norm() <= 1e-10, "k={iterations} {key:?}: {a} vs {b}");
        }
    }
    for layout in layouts() {
        let module = bennett_ir::load(&program(&["x %o[0] ctrl %r", "h %q[1]"], true, layout)).expect("valid");
        let (outcome, executor) = run(&module, options);
        assert_eq!(outcome, Outcome::Returned(None));
        rows.push((format!("layout {layout:?}"), executor.stats()));
    }
    println!(
        "form | steps | world steps | journal bytes | peak overlay bytes | peak qubits | memory qubit bytes | peak worlds | splits | merges | peak configurations"
    );
    for (name, s) in &rows {
        println!(
            "{name} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {}",
            s.steps,
            s.world_steps,
            s.journal_bytes.unwrap_or(0),
            s.peak_overlay_bytes,
            s.peak_live_qubits,
            s.memory_qubit_bytes,
            s.peak_worlds,
            s.splits,
            s.merges,
            s.peak_configurations
        );
    }
    for (pair, k) in rows[..6].chunks(2).zip([1u64, 2, 8]) {
        let (registers, memory) = (&pair[0].1, &pair[1].1);
        // No qallocs (2 steps), and per oracle a load and a truncation (2 steps, in each of the 16 worlds
        // the load splits into); the journal keeps two promotions (address and old byte, and a count:
        // 32 bytes each) instead of two qalloc records (17 bytes each), whatever the iterations.
        assert_eq!(memory.steps, registers.steps + 2 * k - 2);
        assert_eq!(memory.world_steps, registers.world_steps + 32 * k - 2);
        assert_eq!(memory.journal_bytes.zip(registers.journal_bytes).map(|(m, r)| m - r), Some(30));
        assert_eq!(
            (memory.splits, memory.merges, memory.peak_worlds),
            (registers.splits, registers.merges, registers.peak_worlds)
        );
        assert_eq!(memory.peak_configurations, registers.peak_configurations);
        assert_eq!(memory.peak_live_qubits, 16, "two bytes hold qubits; the registers hold 5");
        assert_eq!(registers.peak_live_qubits, 5);
    }
}

/// Grover measured where x lives: `measure %x` in registers, `measure i4 [64]` in memory (x is the low four
/// bits of byte 64; its upper four stay 0). Both sample the same distribution, ordered the same way, with one
/// draw, so every seed gives the same answer. The predicate's solutions are 2 and 13 (x*x + x = 6 mod 16);
/// with two of 16, one iteration finds one with probability sin²(3θ) = 0.78 and two with sin²(5θ) = 0.95
/// (sin²θ = 1/8).
#[test]
fn grover_measured_at_its_address_agrees_with_the_register_form_for_every_seed() {
    let measured = |memory: bool, iterations: u8| {
        let measure = if memory { "    %m = measure i4 [64]\n" } else { "    %m = measure %x\n" };
        let text = grover(memory, iterations)
            .replace("func @main() {", "func @main() -> i4 {")
            .replace("finish:\n    ret\n", &format!("finish:\n{measure}    ret %m\n"));
        bennett_ir::load(&text).expect("valid")
    };
    for iterations in [1u8, 2] {
        let (registers, memory) = (measured(false, iterations), measured(true, iterations));
        let (mut seen, mut hits) = (std::collections::BTreeSet::new(), Vec::new());
        for seed in 0..64 {
            let options = Options { seed, budget: Some(100_000), ..Options::default() };
            let (a, _) = run(&registers, options);
            let (b, _) = run(&memory, options);
            assert_eq!(a, b, "k={iterations} seed {seed}");
            let Outcome::Returned(Some(x)) = a else { panic!("{a:?}") };
            seen.insert(x);
            hits.push(x);
        }
        let hits = hits.iter().filter(|&&x| x == 2 || x == 13).count();
        assert!(seen.contains(&2) && seen.contains(&13), "k={iterations}: {seen:?}");
        // Expected 50 and 61 of 64; the bounds sit several binomial standard deviations below.
        assert!(hits >= [40, 50][usize::from(iterations - 1)], "k={iterations}: {hits} of 64");
    }
}

fn start<'m>(module: &'m ValidModule, entry: &str, reversible: bool) -> Executor<'m> {
    let options = Options { reversible, budget: Some(10_000), ..Options::default() };
    Executor::new(module, module.function_named(entry).expect("entry"), &[], options).expect("starts")
}

/// The thesis program's interference in memory: rotate a bit of a byte, let ordinary code read it (the
/// world splits), un-run the read (the worlds merge), rotate back: the byte reads 0 every time.
#[test]
fn reading_a_rotated_bit_and_un_running_the_read_interferes() {
    let module = bennett_ir::load(
        "memory 64
func @main() -> i8 {
entry:
    h [16:3]
    %h = mark
    %v = load i8 16
    %c = icmp eq i8 %v, 8
    br %c, one, done
one:
    store i8 9, 20
    br done
done:
    rewind %h
    h [16:3]
    %w = load i8 16
    ret %w
}
",
    )
    .expect("valid");
    for seed in 0..16 {
        let options = Options { seed, ..Options::default() };
        let mut executor =
            Executor::new(&module, module.function_named("main").expect("@main"), &[], options).expect("starts");
        assert_eq!(executor.run(), Outcome::Returned(Some(0)));
        let stats = executor.stats();
        assert_eq!((stats.splits, stats.merges, stats.observations, stats.worlds), (1, 1, 0, 1));
        assert_eq!(executor.snapshot().classical.memory[20], 0, "the store in one world is un-run");
    }
}

/// A coherent region reads the byte's value on each configuration from its private memory.
#[test]
fn a_coherent_region_reads_memory_qubits_per_configuration() {
    let module = bennett_ir::load(
        "memory 64
data 17 = i8 5
func @peek() -> i8 {
entry:
    %v = load i8 16
    %w = load i8 17
    %s = mul i8 %v, %w
    ret %s
}
func @main() {
entry:
    %o = qalloc 8
    %e = qalloc 5
    h [16:0]
    h [16:1]
    apply @peek() -> %o, %e budget 10
    ret
}
",
    )
    .expect("valid");
    let mut executor = start(&module, "main", true);
    assert_eq!(executor.run(), Outcome::Returned(None));
    let snapshot = executor.snapshot();
    assert_eq!(snapshot.quantum.cells, vec![(16, 2)]);
    assert_eq!(snapshot.classical.memory[16..18], [0, 5], "classical memory under the qubits is 0");
    let configs: Vec<Vec<u64>> = snapshot.quantum.amplitudes.iter().map(|(c, _)| c.clone()).collect();
    assert_eq!(configs, vec![vec![0, 0, 0], vec![5, 0, 1], vec![10, 0, 2], vec![15, 0, 3]]);
    executor.reverse().expect("reverses");
    assert!(executor.snapshot().quantum.cells.is_empty());
}

/// A store over a byte whose qubits disagree reads them first: one world per old value, each keeping its
/// old value as history; the byte then holds the stored value in each.
#[test]
fn a_store_over_superposed_qubits_splits_and_keeps_the_old_value() {
    let module = bennett_ir::load(
        "memory 64
func @main() -> i8 {
entry:
    h [16:2]
    store i8 5, 16
    %v = load i8 16
    ret %v
}
",
    )
    .expect("valid");
    let mut executor = start(&module, "main", true);
    assert_eq!(executor.run(), Outcome::Returned(Some(5)));
    let stats = executor.stats();
    // Two worlds that differ only in history: the host's reading of the result observes one.
    assert_eq!((stats.splits, stats.peak_worlds, stats.observations), (1, 2, 1));
    let snapshot = executor.snapshot();
    assert_eq!(snapshot.quantum.amplitudes.len(), 1);
    assert_eq!(snapshot.quantum.amplitudes[0].0[snapshot.quantum.cells[0].1], 5);
}

/// A keep into memory XORs into bytes that already held qubits at the mark, as a keep into a register
/// needs it allocated before the mark; un-running through a gate at an address conjugates.
#[test]
fn keeps_into_memory_conjugate_and_need_qubits_held_since_the_mark() {
    let module = bennett_ir::load(
        "memory 64
func @conjugated() -> i8 {
entry:
    h [16]
    %h = mark
    h [16]
    rewind %h keep (i1 1 -> [16])
    h [16]
    %v = load i8 16
    ret %v
}
func @promoted_since() -> i8 {
entry:
    %h = mark
    x [16]
    %z = add i8 0, 0
    rewind %h keep (i1 1 -> [16])
    ret 0
}
func @classical() -> i8 {
entry:
    %h = mark
    rewind %h keep (i8 1 -> [16])
    ret 0
}
func @read_since() -> i8 {
entry:
    x [16]
    %h = mark
    %v = load i8 16
    rewind %h keep (i8 1 -> [16])
    ret 0
}
func @carried() -> i8 {
entry:
    x [17]
    x [17]
    %h = mark
    x [16] ctrl [17]
    rewind %h keep (i8 1 -> [17])
    ret 0
}
",
    )
    .expect("valid");
    // H, then X, then un-running the H: H X H = Z on |+>, which the last H turns into |1>.
    assert_eq!(start(&module, "conjugated", false).run(), Outcome::Returned(Some(1)));
    for entry in ["promoted_since", "classical", "read_since"] {
        let mut executor = start(&module, entry, false);
        assert_eq!(executor.run(), Outcome::Faulted(Fault::BadRewind), "{entry}");
        // Refused before un-running anything.
        assert_eq!(executor.stats().rewound_steps, 0, "{entry}");
    }
    // The keep into 17 is carried into 16 by the gate's un-run, so 16, promoted since the mark, no longer
    // holds its old byte when its promotion is un-run: the rewind faults and changes nothing.
    let mut executor = start(&module, "carried", true);
    for _ in 0..4 {
        assert_eq!(executor.step(), None);
    }
    let before = executor.snapshot();
    assert_eq!(executor.step(), Some(Outcome::Faulted(Fault::BadRewind)));
    let after = executor.snapshot();
    assert_eq!(after.classical.memory, before.classical.memory);
    assert_eq!(after.quantum.cells, before.quantum.cells);
    assert!(after.quantum.distance(&before.quantum).is_some_and(|d| d <= 1e-12));
}

/// A jump record held in qubits jumps where they agree, and is `bad_longjmp` where they do not: worlds
/// that jumped to different places, some faulting, are a debt.
#[test]
fn a_jump_record_in_qubits_jumps_only_where_they_agree() {
    let text = |gates: &str| {
        format!(
            "memory 256
func @main() -> i32 {{
entry:
    %r = setjmp 64
    %again = icmp eq i32 %r, 0
    br %again, jump, done
jump:
{gates}    longjmp 64, 7
done:
    ret %r
}}
"
        )
    };
    let agreeing = bennett_ir::load(&text("    x [64:3]\n    x [64:3]\n")).expect("valid");
    assert_eq!(start(&agreeing, "main", true).run(), Outcome::Returned(Some(7)));
    let superposed = bennett_ir::load(&text("    h [64:3]\n")).expect("valid");
    assert_eq!(start(&superposed, "main", true).run(), Outcome::Faulted(Fault::BadLongjmp));
}

/// A rewind un-runs a promotion the worlds shared before a read split them: below the split each world
/// un-runs its part of the shared state, which need not hold the promoted byte alone; the parts sum to a
/// state that does, so the un-promotion keeps each part where it holds (its adjoint) and the worlds merge.
#[test]
fn a_rewind_un_runs_a_promotion_shared_by_the_worlds_a_read_split() {
    let module = bennett_ir::load(
        "memory 64
data 16 = i8 6
func @main() -> i8 {
entry:
    %h = mark
    h [16:0]
    ry(0.9) [16:3]
    %v = load i8 16
    %w = add i8 %v, 1
    store i8 %w, 30
    rewind %h
    %u = load i8 16
    ret %u
}
",
    )
    .expect("valid");
    let mut executor = start(&module, "main", true);
    assert_eq!(executor.run(), Outcome::Returned(Some(6)));
    let stats = executor.stats();
    assert_eq!((stats.splits, stats.merges, stats.worlds, stats.memory_qubit_bytes), (3, 3, 1, 0));
    let snapshot = executor.snapshot();
    assert_eq!((snapshot.classical.memory[16], snapshot.classical.memory[30]), (6, 0));
    assert!((snapshot.quantum.amplitudes[0].1.norm() - 1.0).abs() < 1e-12);
}

/// The memory form of the worlds testing pass's F1 (`worlds_independent.rs`): a later split must not
/// hide an earlier read the keep changed. Every placement of q and out, registers or memory.
#[test]
fn a_later_split_does_not_hide_a_changed_earlier_read_of_memory_qubits() {
    for layout in [[false, false], [true, false], [false, true], [true, true]] {
        let (q, out) = (if layout[0] { "[64]" } else { "%q" }, if layout[1] { "[65]" } else { "%out" });
        let read =
            if layout[0] { "%b = load i8 64\n    %t = trunc i8 %b to i1\n    call @id(%t)" } else { "call @id(%q)" };
        let allocate = |name: &str, in_memory: bool| {
            if in_memory {
                format!("z [{}]\n", if name == "q" { 64 } else { 65 })
            } else {
                format!("%{name} = qalloc 1\n")
            }
        };
        let text = format!(
            "memory 512
func @id(i1 %x) -> i1 {{
entry:
    ret %x
}}
func @main() {{
entry:
    {}    {}    %m = mark
    {read}
    x {q} ctrl {out}
    h {q}
    {}
    rewind %m keep (i1 1 -> {out})
    ret
}}
",
            allocate("q", layout[0]),
            allocate("out", layout[1]),
            read.replace("%b", "%b2").replace("%t", "%t2"),
        );
        let module = bennett_ir::load(&text).unwrap_or_else(|e| panic!("{e}\n{text}"));
        let got = start(&module, "main", true).run();
        assert!(
            matches!(
                got,
                Outcome::Faulted(Fault::BadRewind)
                    | Outcome::Stopped(RunError::WorldFault { fault: Fault::BadRewind, .. })
            ),
            "{layout:?}: {got:?}"
        );
    }
}

/// A byte promoted after the mark and before a read that splits the worlds: the keep, carried into it by
/// the promoting gate's un-run, leaves it not holding its old byte in the sum of the worlds' parts, so its
/// un-promotion (checked on that sum) refuses and the rewind faults `bad_rewind`, as a register allocated
/// there and left nonzero would. Without the keep the same program merges back.
#[test]
fn an_un_promotion_below_a_split_is_checked_on_the_sum_of_the_parts() {
    for (keep, out) in [("i1 1", "%out"), ("i1 0", "%out"), ("i1 1", "[65]"), ("i1 0", "[65]")] {
        let allocate = if out == "%out" { "%out = qalloc 1" } else { "z [65]" };
        let text = format!(
            "memory 512
func @id(i1 %x) -> i1 {{
entry:
    ret %x
}}
func @main() {{
entry:
    {allocate}
    %q = qalloc 1
    h %q
    %m = mark
    x [64] ctrl {out}
    call @id(%q)
    rewind %m keep ({keep} -> {out})
    ret
}}
"
        );
        let module = bennett_ir::load(&text).expect("valid");
        let mut executor = start(&module, "main", true);
        let got = executor.run();
        if keep == "i1 1" {
            assert!(
                matches!(
                    got,
                    Outcome::Faulted(Fault::BadRewind)
                        | Outcome::Stopped(RunError::WorldFault { fault: Fault::BadRewind, .. })
                ),
                "{out}: {got:?}"
            );
        } else {
            assert_eq!(got, Outcome::Returned(None), "{out}");
            assert_eq!(executor.stats().worlds, 1);
            assert_eq!(executor.snapshot().quantum.cells.len(), usize::from(out != "%out"), "64 un-promoted");
        }
    }
}

/// CARGO_BUILD_JOBS=3 cargo test -p bennett-exec --test memory_qubits direct_address_reference_extended -- --ignored --nocapture
#[test]
#[ignore = "182 dense witnesses in eight layouts interpreted directly as memory qubits"]
fn direct_address_reference_extended() {
    use oracle_support::worlds::{Finish, Model, basis};
    use oracle_support::worlds_executor::assert_worlds;
    let options =
        Options { reversible: true, budget: Some(10_000), max_configurations: Some(1024), ..Options::default() };
    let (mut agreed, mut refused) = (0, 0);
    for (middle, writes) in cases() {
        let registers = bennett_ir::load(&program(&middle, writes, [false; 3])).expect("valid");
        let dense = expected(&registers);
        for layout in layouts() {
            let text = program(&middle, writes, layout);
            let module = bennett_ir::load(&text).expect("valid");
            let model = Model::new(&module, "main");
            let reference = model.run_joined(basis(model.initial("main", &[])), 2000);
            let refuses = reference.keys().any(|w| w.classical.finish == Some(Finish::Faulted(Fault::BadRewind)));
            assert_eq!(refuses, dense.is_none(), "{text}");
            let (outcome, e) = run(&module, options);
            if refuses {
                assert!(matches!(outcome, Outcome::Stopped(RunError::WorldFault { fault: Fault::BadRewind, .. })));
                refused += 1;
            } else {
                assert_eq!(outcome, Outcome::Returned(None));
                assert_worlds(&model, &reference, &e.snapshot());
                assert_same(&logical(&e.snapshot(), layout), &dense_state(dense.as_ref().expect("faithful")), &text);
                agreed += 1;
            }
        }
    }
    assert_eq!((agreed, refused), (1200, 256));
    println!("{agreed} direct address/dense/full-world agreements; {refused} exact refusals");
}

#[test]
fn replaying_a_reversed_cell_split_still_checks_dirty_registers() {
    let module = bennett_ir::load(
        "memory 64
func @main() {
entry:
    %q = qalloc 1
    x %q
    h [16]
    %v = load i8 16
    qfree %q
    ret
}
",
    )
    .expect("valid");
    let mut executor = start(&module, "main", true);
    for _ in 0..3 {
        assert_eq!(executor.step(), None);
    }
    let before_read = executor.snapshot();
    assert_eq!(executor.step(), None);
    assert_eq!(executor.stats().worlds, 2);
    assert_eq!(executor.step_back(), Ok(true));
    // Reverse restores the complete state, including the phases, before replaying the splitting load.
    assert_eq!(executor.snapshot(), before_read);
    assert_eq!(
        executor.run(),
        Outcome::Stopped(RunError::WorldFault { fault: Fault::DirtyFree, worlds: 2 }),
        "an ordinary replay must still refuse to discard the register holding 1"
    );
}

#[test]
fn a_gate_at_the_first_byte_past_memory_faults_without_promoting_it() {
    for (address, bit) in [(64, 0), (49, 127)] {
        let module = bennett_ir::load(&format!("memory 64\nfunc @main() {{\nentry:\n z [{address}:{bit}]\n ret\n}}\n"))
            .expect("valid");
        let mut executor = start(&module, "main", true);
        let before = executor.snapshot();
        let journal = executor.stats().journal_bytes;
        assert_eq!(executor.step(), Some(Outcome::Faulted(Fault::OutOfBounds)));
        let mut after = executor.snapshot();
        after.classical.outcome = None;
        assert_eq!(after, before, "bit {bit} at {address} reaches byte 64, outside memory");
        assert_eq!(executor.stats().journal_bytes, journal);
    }
}

#[test]
fn a_memory_keep_does_not_recheck_cell_reads_before_its_mark() {
    let module = bennett_ir::load(
        "memory 64
func @main() -> i8 {
entry:
    z [16]
    %old = load i8 16
    %m = mark
    rewind %m keep (i8 1 -> [16])
    %result = load i8 16
    ret %result
}
",
    )
    .expect("valid");
    let mut executor = start(&module, "main", true);
    let before = executor.snapshot();
    assert_eq!(executor.run(), Outcome::Returned(Some(1)));
    assert_eq!(executor.snapshot().quantum.amplitudes, vec![(vec![1], C::new(1.0, 0.0))]);
    executor.reverse().expect("a read outside the rewound region is restored too");
    assert_eq!(executor.snapshot(), before);
}

#[test]
fn canceled_memory_keeps_preserve_a_read_and_its_interference() {
    let module = bennett_ir::load(
        "memory 64
func @main() -> i8 {
entry:
    h [16]
    %m = mark
    %value = load i8 16
    rewind %m keep (i8 1 -> [16], i8 1 -> [16])
    h [16]
    %result = load i8 16
    ret %result
}
",
    )
    .expect("valid");
    let mut executor = start(&module, "main", true);
    // The two copies XOR to zero before anything is un-run: the read still holds its value.
    assert_eq!(executor.run(), Outcome::Returned(Some(0)));
    let quantum = executor.snapshot().quantum;
    assert_eq!(quantum.amplitudes.len(), 1);
    assert!((quantum.amplitude(&[0]) - C::new(1.0, 0.0)).norm() < EPS);
}

#[test]
fn memory_keeps_xor_every_little_endian_byte_at_every_integer_width() {
    for bits in 1..=128 {
        let width = bennett_ir::Width::new(bits).expect("integer width");
        let value = 0x8102_0304_0506_0708_090a_0b0c_0d0e_0f13_u128 & width.mask();
        let promotions: String = (0..width.bytes()).map(|offset| format!(" z [{}]\n", 32 + offset)).collect();
        let module = bennett_ir::load(&format!(
            "memory 256\nfunc @main() -> i{bits} {{\nentry:\n{promotions} %m = mark\n \
             rewind %m keep (i{bits} {value} -> [32])\n %result = load i{bits} 32\n ret %result\n}}\n"
        ))
        .expect("valid");
        let mut executor = start(&module, "main", true);
        let before = executor.snapshot();
        assert_eq!(executor.run(), Outcome::Returned(Some(value)), "i{bits}");
        let quantum = executor.snapshot().quantum;
        assert_eq!(quantum.amplitudes.len(), 1);
        assert_eq!(quantum.amplitudes[0].1, C::new(1.0, 0.0), "i{bits}: phase is preserved");
        assert_eq!(quantum.cells.len() as u64, width.bytes());
        let bytes = value.to_le_bytes();
        for (offset, &(address, column)) in quantum.cells.iter().enumerate() {
            assert_eq!(address, 32 + offset as u64);
            assert_eq!(quantum.amplitudes[0].0[column], u64::from(bytes[offset]), "i{bits}, byte {offset}");
        }
        executor.reverse().expect("the byte-wise keep reverses");
        assert_eq!(executor.snapshot(), before, "i{bits}: full reversal");
    }
}
