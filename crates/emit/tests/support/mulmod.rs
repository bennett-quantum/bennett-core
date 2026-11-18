//! An applied product at twice the operand width, reduced by an ordinary modulus.

use super::{
    ApplyInput, ApplyMode, Argument, CountingDevice, Device, Executor, Options, Outcome, bits, compile_apply_with,
    gates, load,
};

const COMPLETE: u64 = 7;

fn source(width: usize) -> String {
    let wide = 2 * width;
    format!(
        r#"
memory 4096
func @f(i{width} %a, i{width} %b, i{width} %modulus) -> i{width} {{
entry:
    %x = zext i{width} %a to i{wide}
    %y = zext i{width} %b to i{wide}
    %n = zext i{width} %modulus to i{wide}
    %product = mul i{wide} %x, %y
    %residue = urem i{wide} %product, %n
    %answer = trunc i{wide} %residue to i{width}
    ret %answer
}}
"#
    )
}

fn modulus(width: usize) -> u128 {
    (1u128 << width) - 3
}

#[test]
fn mulmod_preserves_amplitudes_at_the_return_boundary() {
    for width in [4, 6, 8] {
        let n = modulus(width);
        let high = width - 1;
        let middle = width - 2;
        let source = format!(
            r#"
{}
func @main(i64 %budget) {{
entry:
    %a = qalloc {width}
    %b = qalloc {width}
    h %a[0]
    h %a[1]
    h %a[{high}]
    x %a[{middle}]
    h %b[0]
    h %b[1]
    h %b[{high}]
    z %b[1]
    %witness = qalloc 1
    x %witness ctrl %a[0]
    s %witness
    %out = qalloc {width}
    x %out[{high}]
    %fault = qalloc 5
    x %fault[4]
    apply @f(%a, %b, i{width} {n}) -> %out, %fault budget %budget
    ret
}}
"#,
            source(width)
        );
        let module = load(&source).expect("valid mulmod");
        let entry = module.function_named("main").expect("main");
        for budget in [COMPLETE - 1, COMPLETE, COMPLETE + 1] {
            for mode in [ApplyMode::Automatic, ApplyMode::StepMachine] {
                let options = Options { machine_meaning: true, ..Options::default() };
                let mut direct = Executor::new(&module, entry, &[budget], options).expect("direct mulmod");
                let mut emitted = Executor::new(&module, entry, &[budget], options).expect("emitted mulmod");
                let mut compiler = |input: ApplyInput<'_>| Ok(compile_apply_with(input, mode)?.block);
                loop {
                    let outcome = direct.step();
                    assert_eq!(emitted.step_emitted(&mut compiler), outcome, "i{width}, B={budget}, {mode:?}");
                    assert_eq!(emitted.snapshot(), direct.snapshot(), "i{width}, B={budget}, {mode:?}");
                    if let Some(outcome) = outcome {
                        assert_eq!(outcome, Outcome::Returned(None));
                        break;
                    }
                }
                let amplitudes = direct.snapshot().quantum.amplitudes;
                assert_eq!(amplitudes.len(), 64);
                for (basis, _) in amplitudes {
                    let (out, fault) = if budget < COMPLETE {
                        (0, 1)
                    } else {
                        (((u128::from(basis[0]) * u128::from(basis[1])) % n) as u64, 0)
                    };
                    assert_eq!(basis[2..], [basis[0] & 1, (1 << high) ^ out, 16 ^ fault]);
                }
            }
        }
    }
}

#[test]
fn mulmod_permutation_and_count_from_four_to_sixty_four_bits() {
    for width in [4, 6, 8, 64] {
        let module = load(&source(width)).expect("valid applied mulmod");
        let n = modulus(width);
        let w = width as u64;
        let args = [Argument::Register(0..w), Argument::Register(w..2 * w), Argument::Value(n)];
        let input = ApplyInput {
            module: &module,
            callee: module.function_named("f").expect("f"),
            args: &args,
            out: 2 * w..3 * w,
            fault: 3 * w..3 * w + 5,
            budget: COMPLETE,
            entry_fault: None,
            stack_available: 0,
            scratch_start: 3 * w + 5,
        };
        let auto = compile_apply_with(input.clone(), ApplyMode::Automatic).expect("automatic mulmod");
        let forced = compile_apply_with(input.clone(), ApplyMode::StepMachine).expect("forced mulmod");
        assert_eq!(auto.block.ops(), forced.block.ops());
        let mut counter = CountingDevice::default();
        counter.allocate(0..input.scratch_start).expect("interface wires");
        auto.send(&mut counter).expect("count native gates");
        let counts = counter.counts();
        let w_terms = u128::from(w);
        assert_eq!(counts.toffoli, 36 * w_terms * w_terms - 12 * w_terms);
        assert_eq!(counts.discarded_qubits, 0);
        assert_eq!(counts.zero_released_qubits, counts.peak_qubits as u128 - u128::from(input.scratch_start));
        println!("mulmod i{width} modulo {n}: {counts:?}");

        let (gates, layout) = gates(&auto, 2 * width, width + 5);
        let mask = (1u128 << width) - 1;
        let expected = |x: u128| ((x & mask) * (x >> width)) % n;
        if width == 64 {
            bits::sampled(&gates, layout, 128, 314, expected).expect("sample full-width operand pairs");
            let corners = [0, mask, mask << width, u128::MAX, (1 << 63) | (1 << 127)];
            bits::check_states(&gates, layout, corners.into_iter().map(|x| (x, mask | (16 << width))), expected)
                .expect("full-width corners");
        } else {
            let inputs = (0..1u128 << (2 * width)).map(|x| (x, (x * 151 + 0x12a5) & ((1 << (width + 5)) - 1)));
            bits::check_states(&gates, layout, inputs, expected).expect("all operands and zero scratch");
        }
    }
}
