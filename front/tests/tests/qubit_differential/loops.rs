//! Loops that read and overwrite variables under qubit-dependent control.

use std::fmt::Write as _;

use rand::Rng;
use rand_chacha::ChaCha8Rng;

use super::super::{Case, Check, Expectation, Field, measure_objects, prepare};

#[derive(Clone, Copy)]
enum Start {
    Known,
    Store,
    Call,
    Region,
}

const CLEAN_SHAPES: [(Start, bool); 7] = [
    (Start::Known, false),
    (Start::Store, false),
    (Start::Store, true),
    (Start::Call, false),
    (Start::Call, true),
    (Start::Region, false),
    (Start::Region, true),
];
pub(super) const SHAPES: usize = CLEAN_SHAPES.len() + 1;

pub(super) fn generate(shape: usize, random: &mut ChaCha8Rng) -> Case {
    let qubits = random.random_range(2..=3);
    let Some(&(start, two)) = CLEAN_SHAPES.get(shape) else {
        return retained(random, qubits);
    };
    let phase_bit = random.random_range(0..qubits);
    let initial = random.random_range(9..=31);
    let scale = random.random_range(3..=15);
    let trips = random.random_range(1..=7);
    let local = matches!(start, Start::Known) && random.random_bool(0.5);
    let mut body = String::new();
    if !local {
        writeln!(body, "uint8_t y = {initial}U;").expect("write global");
    }
    if two {
        body.push_str("uint8_t u = 5U;\n");
    }
    if matches!(start, Start::Call) {
        writeln!(
            body,
            "__attribute__((noinline)) static unsigned spread(unsigned x, unsigned n) {{\n    \
                 return ({scale}U * x + n) | ((9U * x + 7U) << 8);\n}}"
        )
        .expect("write helper");
    }
    if matches!(start, Start::Known) && !local {
        body.push_str("static const uint8_t table[4] = { 91, 14, 47, 20 };\n");
    }
    body.push_str("packet entry(unsigned prep, unsigned n, unsigned column) {\n");
    if local {
        writeln!(body, "    uint8_t y = {initial}U;").expect("write local");
    }
    body.push_str("    uint8_t fence = 0;\n");
    body.push_str(&prepare(qubits));
    let phased = random.random_bool(0.5);
    if phased {
        writeln!(body, "    ZBIT(q, {phase_bit});").expect("write phase");
    }
    match start {
        Start::Known => {} // y still holds its constant initial value.
        Start::Store => {
            writeln!(body, "    y = {scale}U * q + n;").expect("write initial store");
            if two {
                body.push_str("    u = 11U * q + 7U;\n");
            }
        }
        Start::Call => {
            body.push_str("    unsigned v = spread(q, n);\n    y = (uint8_t)v;\n");
            if two {
                body.push_str("    u = (uint8_t)(v >> 8);\n");
            }
        }
        Start::Region => {
            writeln!(body, "    if (q & 1U) {{ y = {scale}U * q + n;").expect("write earlier region");
            if two {
                body.push_str("        u = q + 7U;\n");
            }
            body.push_str("    }\n");
        }
    }
    // The gate leaves a zero register unchanged and keeps preceding writes outside the loop region.
    body.push_str(
        "    ZBIT(fence, 0);\n    #pragma clang loop unroll(disable)\n    \
             for (unsigned t = q; t != 0; --t) {\n",
    );
    match start {
        Start::Known if local => body.push_str("        y = (uint8_t)((y << 1) | (y >> 7));\n"),
        Start::Known => body.push_str("        y = table[(y + t) & 3U];\n"),
        Start::Region if two => body.push_str(
            "        #pragma clang loop unroll(disable)\n        \
                     for (unsigned j = 0; j < (t & 3U) + 1U; ++j) y = (uint8_t)(y * 3U + j + t);\n",
        ),
        _ => body.push_str("        y = (uint8_t)(y * 3U + t);\n"),
    }
    if two {
        body.push_str("        u = (uint8_t)((u << 1) | (u >> 7));\n");
    }
    body.push_str("    }\n");
    let (packets, fields) = if local {
        body.push_str(
            "    packet key = bennett_measure(&q);\n    return key | ((packet)bennett_measure(&y) << 8);\n}\n",
        );
        (1, vec![Field { packet: 0, shift: 0 }, Field { packet: 0, shift: 8 }])
    } else {
        let mut objects = vec![("y".to_owned(), 1)];
        if two {
            objects.push(("u".to_owned(), 1));
        }
        measure_objects(&mut body, &objects)
    };
    let mut check = Check::new(fields);
    if phased {
        check.preparation.z = 1 << phase_bit;
    }
    Case { body, qubits, trips, packets, expectation: Expectation::LoopClean, state: Some(check) }
}

fn retained(random: &mut ChaCha8Rng, qubits: u32) -> Case {
    let phase_bit = random.random_range(1..qubits);
    let initial = random.random_range(5..=15);
    let bit = random.random_range(0..=1);
    let divisor = random.random_range(2..=5);
    let body = format!(
        "packet entry(unsigned prep, unsigned n, unsigned column) {{\n    \
             uint8_t x = 0;\n{}    SBIT(q, {phase_bit});\n    \
             x = {initial}U + (q & 1U);\n    XBIT(x, {bit});\n    \
             #pragma clang loop unroll(disable)\n    \
             for (unsigned t = q; t != 0; --t) x = (uint8_t)(x / {divisor}U + (t & 1U));\n    \
             packet key = bennett_measure(&q);\n    return key | ((packet)bennett_measure(&x) << 8);\n}}\n",
        prepare(qubits),
    );
    let mut check = Check::new(vec![Field { packet: 0, shift: 0 }, Field { packet: 0, shift: 8 }]);
    check.preparation.s = 1 << phase_bit;
    // Values of q with the same low bit leave the same old value, so some off-diagonal
    // terms survive the partial trace while those between distinct old values vanish.
    check.retained = Some((0..1_u64 << qubits).map(|q| (initial + (q & 1)) ^ (1 << bit)).collect());
    Case {
        body,
        qubits,
        trips: 0,
        packets: 1,
        expectation: Expectation::LoopRetained { object: "x", bits: 8 },
        state: Some(check),
    }
}
