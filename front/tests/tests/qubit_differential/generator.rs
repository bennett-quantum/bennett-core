//! Seeded C programs for the qubit differential test.

use std::fmt::Write as _;

use rand::Rng;
use rand_chacha::ChaCha8Rng;

use super::{Case, Expectation, MODES, measure_objects, prepare};

pub(super) const EXTRA_SHAPES: usize = 7;

/// Alternate full rounds of the three compiler modes. The original stores keep their own random
/// stream and mode, so adding a shape here does not change their source or how they are lowered.
pub(super) fn generate(index: usize, original: &mut ChaCha8Rng, extra: &mut ChaCha8Rng) -> Case {
    let round = index / MODES.len();
    if round.is_multiple_of(2) {
        return stores(original);
    }
    let extra_index = round / 2 * MODES.len() + index % MODES.len();
    let recipe =
        [gate_loop, applied_write, conditional_write, wide_results, repeated_store, retained_store, measured_bytes]
            [extra_index % EXTRA_SHAPES];
    recipe(extra)
}

fn condition(random: &mut ChaCha8Rng, qubits: u32) -> String {
    let mask = random.random_range(1..1_u32 << qubits);
    format!("(q & {mask}U) {} 0", if random.random_bool(0.25) { "==" } else { "!=" })
}

fn stores(random: &mut ChaCha8Rng) -> Case {
    let qubits = random.random_range(2..=3);
    let mut body = String::new();
    let mut locals = String::new();
    let mut objects = Vec::new();
    for i in 0..random.random_range(1..=3) {
        let (bits, initial) = match random.random_range(0..4) {
            0 => (8, u64::from(random.random::<u8>())),
            1 => (16, u64::from(random.random::<u16>())),
            2 => (32, u64::from(random.random::<u32>())),
            _ => (64, random.random()),
        };
        let name = format!("cell{i}");
        let storage = random.random_range(0..3);
        let object = if storage == 2 {
            let index = random.random_range(0..2);
            let values = if index == 0 { format!("{initial}ULL, 11") } else { format!("11, {initial}ULL") };
            writeln!(body, "static uint{bits}_t {name}[2] = {{{values}}};").expect("write array");
            format!("{name}[{index}]")
        } else {
            let decl = if storage == 0 { &mut locals } else { &mut body };
            writeln!(decl, "    uint{bits}_t {name} = {initial}ULL;").expect("write object");
            name
        };
        objects.push((object, bits / 8));
    }

    let call = random.random_bool(0.3);
    if call {
        let expression = format!("((uint64_t)x * {}ULL) ^ {}ULL", random.random::<u32>(), random.random::<u64>());
        writeln!(body, "__attribute__((noinline)) static uint64_t helper(unsigned x) {{").expect("write helper");
        if random.random_bool(0.5) {
            writeln!(body, "    volatile uint64_t scratch = {expression}; scratch ^= 9; return scratch;")
                .expect("write helper");
        } else {
            writeln!(body, "    return {expression};").expect("write helper");
        }
        body.push_str("}\n");
    }
    body.push_str("packet entry(unsigned prep, unsigned n, unsigned column) {\n");
    body.push_str(&locals);
    body.push_str(&prepare(qubits));

    // A straight write before the first region brings that object into the region already held.
    let mut written = vec![false; objects.len()];
    let mut clean = true;
    if random.random_bool(0.45) {
        let i = random.random_range(0..objects.len());
        written[i] = true;
        let expression = if call { "helper(q)" } else { "17ULL * q + 3" };
        writeln!(body, "    {} = {expression};", objects[i].0).expect("write straight store");
    } else if call {
        let i = random.random_range(0..objects.len());
        written[i] = true;
        writeln!(body, "    {} = helper(q);", objects[i].0).expect("write applied call");
    }

    let regions = 1 + usize::from(random.random_bool(0.45));
    for _ in 0..regions {
        writeln!(body, "    if ({}) {{", condition(random, qubits)).expect("write condition");
        let mut region_writes = Vec::new();
        for _ in 0..random.random_range(1..=objects.len()) {
            let i = random.random_range(0..objects.len());
            // The clean check covers known starting values, including multiple writes in one
            // region. Repeated writes across separate regions can require unbuilt cleanup.
            clean &= !written[i];
            region_writes.push(i);
            let op = ["=", "=", "+=", "^=", "-="][random.random_range(0..5)];
            let value = random.random::<u64>();
            let expression =
                if random.random_bool(0.6) { format!("{value}ULL + 3ULL * q") } else { format!("{value}ULL") };
            writeln!(body, "        {} {op} {expression};", objects[i].0).expect("write update");
            if random.random_bool(0.28) {
                let bound = ["3U", "n", "(q & 3U)"][random.random_range(0..3)];
                body.push_str("        #pragma clang loop unroll(disable)\n");
                writeln!(
                    body,
                    "        for (unsigned j = 0; j < {bound}; ++j) {} ^= (j + 1U) * (q + 3U);",
                    objects[i].0
                )
                .expect("write loop");
            }
        }
        for i in region_writes {
            written[i] = true;
        }
        body.push_str("    }\n");
    }
    let packets = measure_objects(&mut body, &objects);
    Case::values(body, qubits, random.random_range(0..=3), packets, clean)
}

fn gate_loop(random: &mut ChaCha8Rng) -> Case {
    let qubits = random.random_range(2..=3);
    let trips = random.random_range(0..=5);
    let inner_trips = random.random_range(0..=3);
    let site = random.random_range(0..=4);
    let gate = if random.random_bool(0.5) {
        format!("if (i == {site}U) XBIT(y, 0);")
    } else {
        format!(
            "#pragma clang loop unroll(disable)\n            \
             for (unsigned j = 0; j < ((n + {inner_trips}U) & 3U); ++j) {{\n                \
                 XBIT(y, 0);\n                if ((i + j) & 1U) XBIT(y, 1);\n            }}"
        )
    };
    let body = format!(
        "packet entry(unsigned prep, unsigned n, unsigned column) {{\n    \
             unsigned char y = {initial};\n{prepare}    \
             if ({condition}) {{\n        \
                 #pragma clang loop unroll(disable)\n        \
                 for (unsigned i = 0; i < n; ++i) {{\n            {gate}\n        }}\n    }}\n    \
             packet key = bennett_measure(&q);\n    \
             return key | ((packet)bennett_measure(&y) << 8);\n}}\n",
        initial = random.random_range(0..4),
        prepare = prepare(qubits),
        condition = condition(random, qubits),
    );
    Case { body, qubits, trips, packets: 1, expectation: Expectation::Clean }
}

fn applied_write(random: &mut ChaCha8Rng) -> Case {
    let qubits = random.random_range(2..=3);
    let initial = random.random::<u32>();
    let (global, parameter, local, argument, object) = if random.random_bool(0.5) {
        (format!("volatile uint32_t cell = {initial}U;\n"), "", String::new(), "", "reads @cell")
    } else {
        (
            String::new(),
            ", volatile uint32_t *p",
            format!("    volatile uint32_t cell = {initial}U;\n"),
            ", &cell",
            "reads a local variable it writes",
        )
    };
    let destination = if parameter.is_empty() { "cell" } else { "*p" };
    let body = format!(
        "{global}__attribute__((noinline)) uint32_t write_cell(unsigned x{parameter}) {{\n    \
             {destination} += {scale}U * x + 1U;\n    return {destination} ^ {mask}U;\n}}\n\n\
         packet entry(unsigned prep, unsigned n, unsigned column) {{\n{local}{prepare}    \
             uint32_t result = 0;\n\
         #ifdef NATIVE\n    uint32_t before = cell;\n#endif\n    \
             result ^= write_cell(q{argument});\n\
         #ifdef NATIVE\n    cell = before;\n#endif\n    \
             packet key = bennett_measure(&q);\n    \
             return key | ((packet)bennett_measure(&result) << 8) | ((packet)cell << 40);\n}}\n",
        scale = random.random::<u32>() | 1,
        mask = random.random::<u32>(),
        prepare = prepare(qubits),
    );
    // Native C evaluates the same call and then restores its writes, as apply does.
    Case { body, qubits, trips: 0, packets: 1, expectation: Expectation::AppliedWrite(object) }
}

fn conditional_write(random: &mut ChaCha8Rng) -> Case {
    let qubits = random.random_range(2..=3);
    let body = format!(
        "volatile uint32_t cell = {initial}U;\n\
         __attribute__((noinline)) void write_cell(unsigned x) {{ cell ^= x; }}\n\n\
         packet entry(unsigned prep, unsigned n, unsigned column) {{\n{prepare}    \
             if ({condition}) write_cell(n + 1U);\n    \
             packet key = bennett_measure(&q);\n    return key | ((packet)cell << 8);\n}}\n",
        initial = random.random::<u32>(),
        prepare = prepare(qubits),
        condition = condition(random, qubits),
    );
    Case { body, qubits, trips: random.random_range(0..=3), packets: 1, expectation: Expectation::ConditionalWrite }
}

fn wide_results(random: &mut ChaCha8Rng) -> Case {
    let qubits = random.random_range(2..=3);
    let b_bits = if random.random_bool(0.5) { 64 } else { 32 };
    let mut body = String::new();
    let mut locals = String::new();
    let mut updates = String::new();
    let mut objects = Vec::new();
    for (name, bits) in [("a", 64), ("b", b_bits), ("c", 16), ("d", 8)] {
        let declaration = if random.random_bool(0.5) { &mut body } else { &mut locals };
        let initial = random.random::<u64>() >> (64 - bits);
        writeln!(declaration, "uint{bits}_t {name} = {initial}ULL;").expect("write declaration");
        let scale = random.random::<u64>() | (1 << 63) | 1;
        writeln!(updates, "            {name} ^= {scale}ULL * (q + i + 1U);").expect("write update");
        objects.push((name.to_owned(), bits / 8));
    }
    let condition = condition(random, qubits);
    writeln!(
        body,
        "packet entry(unsigned prep, unsigned n, unsigned column) {{\n{locals}{prepare}    \
             if ({condition}) {{\n        \
                 #pragma clang loop unroll(disable)\n        \
                 for (unsigned i = 0; i < n; ++i) {{\n{updates}        }}\n    }}",
        prepare = prepare(qubits),
    )
    .expect("write region");
    let packets = measure_objects(&mut body, &objects);
    Case { body, qubits, trips: random.random_range(0..=3), packets, expectation: Expectation::Clean }
}

fn repeated_store(random: &mut ChaCha8Rng) -> Case {
    let qubits = random.random_range(2..=3);
    let initial = random.random::<u16>();
    let first_scale = random.random_range(1..=255);
    let second_scale = random.random::<u16>();
    let first = if random.random_bool(0.5) {
        format!("a = {first_scale}U * q + 5U;")
    } else {
        format!("if ({}) a = {first_scale}U * q + 5U;", condition(random, qubits))
    };
    let mut body = format!(
        "uint16_t a = {initial}U;\nuint8_t b = 9;\n\
         packet entry(unsigned prep, unsigned n, unsigned column) {{\n{prepare}    \
             {first}\n    if ({condition}) {{ a ^= {second_scale}U + q; b += q; }}\n",
        prepare = prepare(qubits),
        condition = condition(random, qubits),
    );
    let packets = measure_objects(&mut body, &[("a".into(), 2), ("b".into(), 1)]);
    // Both forms know a's initial value and the second region only XORs into a.
    Case { body, qubits, trips: 0, packets, expectation: Expectation::Clean }
}

fn retained_store(random: &mut ChaCha8Rng) -> Case {
    let qubits = random.random_range(2..=3);
    let bits = if random.random_bool(0.5) { 8 } else { 16 };
    let divisor = random.random_range(2..=5);
    let bit = random.random_range(0..=1);
    let body = format!(
        "packet entry(unsigned prep, unsigned n, unsigned column) {{\n    \
             uint{bits}_t x = 0;\n{prepare}    \
             x = 5U + q;\n    XBIT(x, {bit});\n    x /= {divisor}U;\n    \
             packet key = bennett_measure(&q);\n    return key | ((packet)bennett_measure(&x) << 8);\n}}\n",
        prepare = prepare(qubits),
    );
    Case { body, qubits, trips: 0, packets: 1, expectation: Expectation::Retained(bits) }
}

fn measured_bytes(random: &mut ChaCha8Rng) -> Case {
    let qubits = random.random_range(2..=3);
    let first = random.random_range(0..2);
    let second = 1 - first;
    let body = format!(
        "uint8_t a[2] = {{ {a}, {b} }};\n\
         packet entry(unsigned prep, unsigned n, unsigned column) {{\n{prepare}    \
             if ({condition}) {{ a[0] ^= 9U * q; a[1] += 13U + q; }}\n    \
             packet before = bennett_measure_bytes(&a[{first}], 1);\n    \
             a[{first}] += {delta}U;\n    packet after = a[{first}];\n    \
             packet other = bennett_measure_bytes(&a[{second}], 1);\n    \
             packet key = bennett_measure(&q);\n                 return key | (before << 8) | (after << 16) | (other << 24);\n}}\n",
        a = random.random::<u8>(),
        b = random.random::<u8>(),
        delta = random.random_range(1..=255),
        prepare = prepare(qubits),
        condition = condition(random, qubits),
    );
    Case { body, qubits, trips: 0, packets: 1, expectation: Expectation::Clean }
}
