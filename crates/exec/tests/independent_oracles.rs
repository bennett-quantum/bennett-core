//! Independent dense and bijection checks of gates, controls, `apply` and reversal (`docs/DESIGN.md`, "Tests").
//! Run: cargo test -p bennett-exec --test independent_oracles -- --nocapture

mod oracle_support;

use bennett_exec::{Executor, Options, Outcome, QuantumSnapshot};
use bennett_ir::ValidModule;
use nalgebra::{DMatrix, DVector};
use num_complex::Complex64 as C;
use oracle_support::dense::{Dense, EPS, Oracle};
use oracle_support::permutation::{check_bijection, words};

fn load(text: &str) -> ValidModule {
    bennett_ir::load(text).expect("valid test program")
}
fn executor<'a>(module: &'a ValidModule, entry: &str, args: &[u64]) -> Executor<'a> {
    Executor::new(
        module,
        module.function_named(entry).expect("entry"),
        args,
        Options {
            budget: Some(100_000),
            max_configurations: Some(1 << 13),
            reversible: true,
            allow_region_faults: true,
            ..Options::default()
        },
    )
    .expect("executor starts")
}

fn assert_dense(actual: &QuantumSnapshot, expected: &Dense, context: &str) {
    assert!((expected.state.norm_squared() - 1.0).abs() <= EPS, "{context}: oracle norm");
    for basis in 0..expected.state.len() {
        let mut config = expected.config(basis);
        let freed_tail =
            config.split_off(actual.registers.iter().rposition(|r| r.width.is_some()).map_or(0, |r| r + 1));
        let actual = if freed_tail.iter().any(|&n| n != 0) { C::new(0.0, 0.0) } else { actual.amplitude(&config) };
        assert!(
            (actual - expected.state[basis]).norm() <= EPS,
            "{context}: basis {basis}, actual {actual}, expected {}",
            expected.state[basis]
        );
    }
    // Also check support, so an unexpected high register value cannot escape the dense loop.
    for (config, amplitude) in &actual.amplitudes {
        let mut basis = 0;
        let mut offset = 0;
        for (r, &n) in config.iter().enumerate() {
            assert!(n < 1 << expected.widths[r], "{context}: unexpected register value");
            basis |= (n as usize) << offset;
            offset += expected.widths[r];
        }
        assert!((*amplitude - expected.state[basis]).norm() <= EPS, "{context}: extra support {config:?}");
    }
}

fn compare_prefix(text: &str, entry: &str, args: &[u64]) {
    let module = load(text);
    let mut oracle = Oracle::new(&module);
    let expected_outcome = oracle.prefix(entry, args);
    let mut actual = executor(&module, entry, args);
    for step in 0..oracle.steps {
        let outcome = actual.step();
        assert!(outcome.is_none() || step + 1 == oracle.steps, "early stop at {step}: {outcome:?}");
        assert!(!matches!(outcome, Some(Outcome::Faulted(_) | Outcome::Stopped(_))), "{outcome:?}");
    }
    if let Some(value) = expected_outcome {
        assert_eq!(actual.outcome(), Some(Outcome::Returned(value)));
    } else {
        assert_eq!(actual.outcome(), None, "prefix stops BEFORE measuring");
    }
    assert_dense(&actual.snapshot().quantum, &oracle.dense, &format!("@{entry}{args:?}"));
}

#[test]
fn examples_match_independent_dense_vectors_before_measurement() {
    compare_prefix(include_str!("../../../examples/calls.bn"), "main", &[]);
    compare_prefix(include_str!("../../../examples/exit.bn"), "main", &[]);
    for n in [0, 1, 2, 10, 100] {
        compare_prefix(include_str!("../../../examples/sum.bn"), "main", &[n]);
    }
    let thesis = include_str!("../../../examples/thesis.bn");
    compare_prefix(thesis, "entangled", &[]);
    compare_prefix(thesis, "main", &[]);
    for iterations in 0..=4 {
        compare_prefix(include_str!("../../../examples/grover.bn"), "searched", &[iterations]);
    }
    for n in 0..16 {
        compare_prefix(include_str!("../../../examples/fault.bn"), "main", &[n]);
    }
}

const GATES: &[&str] = &["x", "h", "z", "s", "sdg", "t", "tdg", "rx(0.73)", "ry(-1.13)", "rz(2.17)"];

#[test]
fn every_gate_and_control_list_matches_all_dense_matrix_columns() {
    // 10 gates x 3 control counts x 8 basis columns, plus coherent inputs. Controls include
    // cross-register and same-register bits and are 1-controls (SYNTAX.md / MARKERS.md).
    for gate in GATES {
        for controls in ["", " ctrl %q[1]", " ctrl %q[1], %c"] {
            for basis in 0..8 {
                let prepare = (0..3)
                    .filter(|b| basis & (1 << b) != 0)
                    .map(|b| match b {
                        0 => "x %q[0]\n",
                        1 => "x %q[1]\n",
                        _ => "x %c\n",
                    })
                    .collect::<String>();
                let text = format!(
                    "memory 256\nfunc @main() {{\nentry:\n%q = qalloc 2\n%c = qalloc 1\n{prepare}{gate} %q[0]{controls}\nret\n}}\n"
                );
                compare_prefix(&text, "main", &[]);
                let coherent =
                    text.replace(&format!("{gate} %q[0]"), &format!("h %q[0]\nh %q[1]\nh %c\ns %q[1]\n{gate} %q[0]"));
                compare_prefix(&coherent, "main", &[]);
            }
        }
    }
}

const COPY: &str = "func @copy(i1 %x) -> i1 {\nentry:\nret %x\n}\n";

#[test]
fn unrunning_a_region_through_phase_gates_is_matrix_conjugation() {
    // U = A† G A with A: (x,y,e)->(x,y XOR x,e). All 128 columns, INCLUDING dirty
    // out/fault registers (a 5-qubit fault register). A is derived from the documented XOR meaning, not by
    // running @copy.
    let a = DMatrix::from_fn(128, 128, |row, col| C::new(f64::from(row == (col ^ ((col & 1) << 1))), 0.0));
    let g = DMatrix::from_diagonal(&DVector::from_iterator(
        128,
        (0..128).map(|b| if b & 2 == 0 { C::new(1.0, 0.0) } else { C::new(0.0, 1.0) }),
    ));
    let conjugated = a.adjoint() * g * a;
    for basis in 0..128 {
        let prepare: String = (0..7)
            .filter(|bit| basis & (1 << bit) != 0)
            .map(|bit| {
                if bit == 0 {
                    "x %x\n".into()
                } else if bit == 1 {
                    "x %y\n".into()
                } else {
                    format!("x %e[{}]\n", bit - 2)
                }
            })
            .collect();
        let text = format!(
            "memory 256\n{COPY}func @main() {{\nentry:\n%x = qalloc 1\n%y = qalloc 1\n%e = qalloc 5\n{prepare}apply @copy(%x) -> %y, %e budget 2\ns %y\napply @copy(%x) -> %y, %e budget 2\nret\n}}\n"
        );
        compare_prefix(&text, "main", &[]);
        let module = load(&text);
        let mut actual = executor(&module, "main", &[]);
        assert_eq!(actual.run(), Outcome::Returned(None));
        let expected = Dense { widths: vec![1, 1, 5], state: conjugated.column(basis).into_owned() };
        assert_dense(&actual.snapshot().quantum, &expected, &format!("A† G A column {basis}"));
    }
    // A coherent input probes the relative phases as well as every matrix column.
    compare_prefix(
        &format!(
            "memory 256\n{COPY}func @main() {{\nentry:\n%x = qalloc 1\n%y = qalloc 1\n%e = qalloc 5\nh %x\nh %y\napply @copy(%x) -> %y, %e budget 2\ns %y\napply @copy(%x) -> %y, %e budget 2\nret\n}}"
        ),
        "main",
        &[],
    );
}

#[test]
fn unrunning_gates_matches_independent_adjoint_matrix() {
    for gate in GATES {
        let prefix = "memory 256\nfunc @main() {\nentry:\n%q = qalloc 1\nh %q\n";
        let module = load(&format!("{prefix}{gate} %q\nret\n}}"));
        let mut actual = executor(&module, "main", &[]);
        assert_eq!(actual.run(), Outcome::Returned(None));
        assert_eq!(actual.step_back(), Ok(true)); // ret
        assert_eq!(actual.step_back(), Ok(true)); // gate†, not gate
        let mut expected = Oracle::new(&module);
        // Independent adjoint acts on the expected FORWARD state, so a wrong forward/inverse
        // pair that happens to cancel does not satisfy the check.
        expected.prefix("main", &[]);
        let f = module.function(module.function_named("main").expect("main"));
        let bennett_ir::Inst::Gate { gate, .. } = f.blocks[0].insts[2] else { panic!("gate") };
        expected.dense.gate(oracle_support::dense::gate_matrix(gate).adjoint(), (0, 0), &[]);
        assert_dense(&actual.snapshot().quantum, &expected.dense, "undo gate against adjoint");
    }
}

#[test]
fn finite_checker_rejects_noninjective_and_out_of_domain_steps() {
    let domain = words(&[1, 1]);
    assert!(check_bijection(&domain, &domain, Clone::clone).is_ok());
    let error = check_bijection(&domain, &domain, |s| vec![s[0], 0]).expect_err("erasing history collides");
    assert!(error.contains("collision"), "{error}");
    assert!(check_bijection(&domain, &domain, |_| vec![2, 0]).expect_err("not closed").contains("not closed"));
    assert!(check_bijection(&domain, &domain[..2], Clone::clone).is_err());
    assert!(check_bijection(&[vec![0], vec![0]], &[vec![0], vec![1]], Clone::clone).is_err());
}

fn region_program(width: u32, body: &str, budget: u64) -> String {
    let copies = (0..width).map(|bit| format!("%bx{bit} = lshr i{width} %a, {bit}\n%bit{bit} = trunc i{width} %bx{bit} to i1\nbr %bit{bit}, x{bit}, n{bit}\nx{bit}:\nx %x[{bit}]\nbr n{bit}\nn{bit}:\n")).collect::<String>();
    let prepare = |name: &str, param: &str, bits: u32| {
        (0..bits).map(|bit| format!("%{name}b{bit} = lshr i{bits} %{param}, {bit}\n%{name}c{bit} = trunc i{bits} %{name}b{bit} to i1\nbr %{name}c{bit}, {name}t{bit}, {name}n{bit}\n{name}t{bit}:\nx %{name}[{bit}]\nbr {name}n{bit}\n{name}n{bit}:\n")).collect::<String>()
    };
    // widths start at 2 so the explicit truncations above are always narrowing.
    format!(
        "memory 512\nfunc @f(i{width} %a) -> i{width} {{\nentry:\n{body}\n}}\nfunc @main(i{width} %a, i{width} %o, i5 %e0) {{\nentry:\n%x = qalloc {width}\n%y = qalloc {width}\n%e = qalloc 5\n{copies}{}{}apply @f(%x) -> %y, %e budget {budget}\nret\n}}",
        prepare("y", "o", width),
        prepare("e", "e0", 5)
    )
}

#[test]
fn apply_is_exhaustively_the_claimed_permutation_with_history_spectators() {
    let mut checked = 0;
    for width in 2..=3 {
        for (body, budget) in [
            (format!("%r = mul i{width} %a, %a\nret %r"), 8),
            (format!("%r = udiv i{width} 1, %a\nret %r"), 8),
            ("br loop\nloop:\nbr loop".into(), 3),
            (format!("store i{width} %a, 0\n%r = load i{width} 0\nret %r"), 8),
        ] {
            let module = load(&region_program(width, &body, budget));
            // Complete argument/out/fault space, not only zero out/fault. Include one opaque
            // history word, both clean and every dirty encoding; apply must preserve that prefix.
            let domain = words(&[width, width, 5, 2]);
            let mut images = std::collections::BTreeMap::new();
            let mut expectations = std::collections::BTreeMap::new();
            for input in words(&[width, width, 5]) {
                let mut direct = Oracle::new(&module);
                let expected = direct.classical("f", &[input[0]], budget);
                let mut actual = executor(&module, "main", &input);
                assert_eq!(actual.run(), Outcome::Returned(None));
                let snapshot = actual.snapshot();
                assert_eq!(snapshot.quantum.amplitudes.len(), 1);
                let (image, amplitude) = &snapshot.quantum.amplitudes[0];
                assert_eq!(*amplitude, C::new(1.0, 0.0));
                let expected = match expected {
                    Ok(Some(r)) => vec![input[0], input[1] ^ r as u64, input[2]],
                    Err(f) => vec![input[0], input[1], input[2] ^ f.code()],
                    other => panic!("{other:?}"),
                };
                expectations.insert(input.clone(), expected);
                assert!(snapshot.classical.memory.iter().all(|&b| b == 0), "region memory is private");
                images.insert(input, image.clone());
            }
            checked += check_bijection(&domain, &domain, |s| {
                let mut image = images[&s[..3]].clone();
                image.push(s[3]);
                image
            })
            .expect("actual apply is a bijection");
            for input in &domain {
                let image = &images[&input[..3]];
                assert_eq!(image, &expectations[&input[..3]], "{body}, input {input:?}");
                assert_eq!(&images[image], &input[..3], "apply is its own inverse");
            }
        }
    }
    assert_eq!(checked, 40_960);
    println!("exhausted {checked} apply states (10,240 executor runs; four histories per run)");
}

#[test]
fn generated_region_circuits_match_dense_vectors() {
    use rand::{Rng, SeedableRng};
    let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(0x6f72_6163_6c65);
    for case in 0..32 {
        let mut circuit = String::from("h %x[0]\nh %x[1]\nh %y[0]\ns %y[0]\n");
        for _ in 0..12 {
            let gate = GATES[rng.random_range(0..GATES.len())];
            let bit = rng.random_range(0..2);
            circuit.push_str(&format!("apply @f(%x) -> %y, %e budget 20\n{gate} %y[{bit}] ctrl %x[0]\napply @f(%x) -> %y, %e budget 20\n{gate} %x[{bit}]\n"));
        }
        // Includes classical calls, branching, a private store/load, success and division faults.
        let body = match case % 4 {
            0 => "%r = mul i2 %a, %a\nret %r",
            1 => "%r = udiv i2 3, %a\nret %r",
            2 => "%b = icmp ult i2 %a, 2\nbr %b, yes, no\nyes:\nret 3\nno:\nret 1",
            _ => "store i2 %a, 0\n%r = load i2 0\n%v = call @g(%r)\nret %v",
        };
        let text = format!(
            "memory 512\nfunc @g(i2 %a) -> i2 {{\nentry:\n%r = add i2 %a, 1\nret %r\n}}\nfunc @f(i2 %a) -> i2 {{\nentry:\n{body}\n}}\nfunc @main() {{\nentry:\n%x = qalloc 2\n%y = qalloc 2\n%e = qalloc 5\n{circuit}ret\n}}"
        );
        compare_prefix(&text, "main", &[]);
    }
    println!("32 seeded circuits, 768 apply operations and 768 gates beyond preparation");
}

#[test]
fn apply_step_preserves_and_restores_an_existing_journal_prefix() {
    // Public stepping cannot inject an arbitrary journal encoding. Exercise actual reachable
    // prefixes instead: untouched memory and a dirty prefix which destroys 0 then 165 at address 64.
    for dirty in [false, true] {
        let prefix = if dirty { "store i8 165, 64\nstore i8 90, 64\n" } else { "" };
        let text = region_program(2, "%r = mul i2 %a, %a\nret %r", 8)
            .replace("%x = qalloc 2", &format!("{prefix}%x = qalloc 2"));
        let module = load(&text);
        for input in words(&[2, 2, 5]) {
            let mut actual = executor(&module, "main", &input);
            loop {
                let snapshot = actual.snapshot();
                let top = snapshot.classical.frames.last().expect("running");
                if matches!(
                    module.function(top.func).block(top.block).insts.get(top.inst),
                    Some(bennett_ir::Inst::Apply { .. })
                ) {
                    let bytes = actual.stats().journal_bytes;
                    assert_eq!(actual.step(), None);
                    assert_eq!(actual.step_back(), Ok(true));
                    let restored = actual.snapshot();
                    assert_eq!(restored.classical, snapshot.classical);
                    assert!(restored.quantum.distance(&snapshot.quantum).is_some_and(|d| d <= EPS));
                    assert_eq!(actual.stats().journal_bytes, bytes, "journal prefix size restored");
                    // Pop the entire prefix too, comparing with an independently captured start.
                    let initial = executor(&module, "main", &input).snapshot();
                    actual.reverse().expect("reversible prefix");
                    assert_eq!(actual.snapshot().classical, initial.classical);
                    assert_eq!(actual.snapshot().quantum, initial.quantum);
                    break;
                }
                assert_eq!(actual.step(), None);
            }
        }
    }
    println!("512 apply-step round trips with clean/dirty reachable journal prefixes");
}

#[test]
fn finite_checker_accepts_distinct_domains_and_detects_erased_history() {
    // An adapter example only. A finite push moves one old value from a machine cell into a history cell.
    // Include PC tags in both domains.
    let before: Vec<_> = words(&[2, 2]).into_iter().map(|s| vec![0, s[0], s[1], 0]).collect();
    let after: Vec<_> = words(&[2, 2]).into_iter().map(|s| vec![1, s[0], 0, s[1]]).collect();
    assert_eq!(check_bijection(&before, &after, |s| vec![1, s[1], 0, s[2]]), Ok(16));
    assert_eq!(check_bijection(&after, &before, |s| vec![0, s[1], s[3], 0]), Ok(16));
    let bad = check_bijection(&before, &after, |s| vec![1, s[1], 0, 0]);
    assert!(bad.expect_err("destroying old machine data collides").contains("collision"));
}
