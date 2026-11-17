//! Gates that take addresses (`docs/DESIGN.md`, "Qubits in memory"): a qubit can live at a bit of memory.
//! Grover with x in memory gives the register form's answers seed by seed, its oracle applied to a register
//! that x is copied into by controlled X gates; an applied function that reads a byte holding qubits faults
//! `qubit_memory_access` for every argument value, while the byte beside it is memory; a gate past the end of
//! memory faults without promoting.

use bennett_exec::{Executor, Options, Outcome};
use bennett_ir::{Fault, ValidModule};

fn run(module: &ValidModule, options: Options) -> (Outcome, Executor<'_>) {
    let mut executor =
        Executor::new(module, module.function_named("main").expect("@main"), &[], options).expect("starts");
    (executor.run(), executor)
}

/// Grover over `examples/grover.bn`'s predicate, with x in a register or in memory (the low four bits of
/// byte 64). The oracle is an `apply` into the kick qubit. Ordinary code, an applied function included, may
/// not read a byte holding qubits, so where x is in memory the oracle copies it into a fresh register with
/// one controlled X per bit (`x %c[i] ctrl [64:i]`), applies the predicate to that register, copies again to
/// return it to zero and releases it: an expansion into gates, `apply` and `qfree`.
fn grover(memory: bool, iterations: u8) -> String {
    let x = if memory { ["[64:0]", "[64:1]", "[64:2]", "[64:3]"] } else { ["%x[0]", "%x[1]", "%x[2]", "%x[3]"] };
    let diffuse: String = ["h", "x"]
        .iter()
        .flat_map(|g| x.iter().map(move |q| format!("    {g} {q}\n")))
        .chain([format!("    z {} ctrl {}, {}, {}\n", x[3], x[0], x[1], x[2])])
        .chain(["x", "h"].iter().flat_map(|g| x.iter().map(move |q| format!("    {g} {q}\n"))))
        .collect();
    let prepare: String = x.iter().map(|q| format!("    h {q}\n")).collect();
    let (allocate, oracle) = if memory {
        let copy: String = (0..4).map(|i| format!("    x %c[{i}] ctrl [64:{i}]\n")).collect();
        let oracle = format!(
            "    %c = qalloc 4\n{copy}    apply @is_solution(%c) -> %kick, %fault budget 100\n{copy}    qfree %c\n"
        );
        ("", oracle)
    } else {
        ("    %x = qalloc 4\n", "    apply @is_solution(%x) -> %kick, %fault budget 100\n".to_string())
    };
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
{allocate}    %kick = qalloc 1
    %fault = qalloc 5
{prepare}    x %kick
    h %kick
    br iterate(i8 0)
iterate(i8 %k):
{oracle}{diffuse}    %k.next = add i8 %k, 1
    %more = icmp ult i8 %k.next, {iterations}
    br %more, iterate(%k.next), finish
finish:
    ret
}}
"
    )
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

/// An applied function is ordinary code: its load of byte 16, which holds qubits, faults
/// `qubit_memory_access` on each of the four configurations, so strict mode stops (a region fault, the
/// simulator's) and, with region faults allowed, the code 17 is XORed into `%e` everywhere and `%o` stays 0.
/// Its load of byte 17, beside it, is a load of memory: 5 on every configuration.
#[test]
fn an_applied_function_reading_qubits_in_memory_faults_for_every_argument_value() {
    let text = |byte: u8| {
        format!(
            "memory 64
data 17 = i8 5
func @peek() -> i8 {{
entry:
    %v = load i8 {byte}
    ret %v
}}
func @main() {{
entry:
    %o = qalloc 8
    %e = qalloc 5
    h [16:0]
    h [16:1]
    apply @peek() -> %o, %e budget 10
    ret
}}
"
        )
    };
    let module = bennett_ir::load(&text(16)).expect("valid");
    let mut executor = start(&module, "main", true);
    let expected = bennett_exec::RunError::RegionFault {
        callee: module.function_named("peek").expect("peek"),
        fault: Fault::QubitMemoryAccess,
        faulting: 4,
        configurations: 4,
    };
    assert_eq!(executor.run(), Outcome::Stopped(expected));
    let options = Options { reversible: true, allow_region_faults: true, budget: Some(10_000), ..Options::default() };
    for (byte, out, fault) in [(16, 0, Fault::QubitMemoryAccess.code()), (17, 5, 0)] {
        let module = bennett_ir::load(&text(byte)).expect("valid");
        let (outcome, mut executor) = run(&module, options);
        assert_eq!(outcome, Outcome::Returned(None));
        let snapshot = executor.snapshot();
        assert_eq!(snapshot.quantum.cells, vec![(16, 2)]);
        assert_eq!(snapshot.classical.memory[16..18], [0, 5], "classical memory under the qubits is 0");
        let configs: Vec<Vec<u64>> = snapshot.quantum.amplitudes.iter().map(|(c, _)| c.clone()).collect();
        assert_eq!(configs, (0..4).map(|v| vec![out, fault, v]).collect::<Vec<_>>(), "byte {byte}");
        executor.reverse().expect("reverses");
        assert!(executor.snapshot().quantum.cells.is_empty());
    }
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
