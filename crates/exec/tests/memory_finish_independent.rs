//! Dense two-control calculation of changed memory accesses. For byte j the inverse circuit is
//! cell[j] = old[j] XOR (delta[j] * c), wire[j] = delta[j] * c; the keep sets k=1.
//! Taking back the wires with an outer rewind restores cell=old, k=0. This is one linear map
//! for zero, one, plus and complex inputs; it does not select a rule from amplitude support.
mod common;
use bennett_exec::{Options, Outcome, Snapshot};
use bennett_ir::Fault;
use num_complex::Complex64 as C;
use std::collections::BTreeMap;

const HALF: f64 = std::f64::consts::FRAC_1_SQRT_2;
type State = BTreeMap<Vec<u64>, C>;

fn preparations() -> Vec<(&'static str, [C; 2])> {
    vec![
        ("", [C::new(1.0, 0.0), C::new(0.0, 0.0)]),
        ("x %c", [C::new(0.0, 0.0), C::new(1.0, 0.0)]),
        ("h %c", [C::new(HALF, 0.0), C::new(HALF, 0.0)]),
        ("h %c\nz %c", [C::new(HALF, 0.0), C::new(-HALF, 0.0)]),
        ("h %c\ns %c", [C::new(HALF, 0.0), C::new(0.0, HALF)]),
        ("ry(0.74) %c\nsdg %c", [C::new(0.37f64.cos(), 0.0), C::new(0.0, -0.37f64.sin())]),
    ]
}

fn h(state: State) -> State {
    let mut out = State::new();
    for (config, a) in state {
        for c in 0..2 {
            let mut next = config.clone();
            next[0] = c;
            *out.entry(next).or_default() += a * if c & config[0] == 1 { -HALF } else { HALF };
        }
    }
    out
}

fn vector(snapshot: &Snapshot, expected: &State, columns: usize) {
    assert!(snapshot.worlds.is_empty(), "{snapshot:?}");
    let mut actual = State::new();
    for (config, a) in &snapshot.quantum.amplitudes {
        assert!(config.len() <= columns, "{snapshot:?}");
        let mut config = config.clone();
        config.resize(columns, 0);
        *actual.entry(config).or_default() += a;
    }
    for c in actual.keys().chain(expected.keys()) {
        assert!(
            (actual.get(c).copied().unwrap_or_default() - expected.get(c).copied().unwrap_or_default()).norm() < 1e-11,
            "{c:?}: actual {actual:?}; expected {expected:?}"
        );
    }
}

fn reversible(text: &str, expected: &State, columns: usize) {
    let p = common::load(text);
    let mut e =
        common::executor(&p, "main", &[], Options { reversible: true, machine_meaning: true, ..common::bounded() });
    let entry = e.snapshot();
    let mut snapshots = vec![];
    while e.outcome().is_none() {
        snapshots.push(e.snapshot());
        e.step();
        assert!(snapshots.len() < 300, "straight-line circuit\n{text}");
    }
    assert_eq!(e.outcome(), Some(Outcome::Returned(None)), "{text}");
    vector(&e.snapshot(), expected, columns);
    for snapshot in snapshots.into_iter().rev() {
        e.step_back().expect("every inverse step exists");
        whole_state(&e.snapshot(), &snapshot, "captured inverse step");
    }
    whole_state(&e.snapshot(), &entry, "entry restored");
    assert_eq!(e.run(), Outcome::Returned(None));
    vector(&e.snapshot(), expected, columns);
    e.reverse().expect("second run reverses");
    whole_state(&e.snapshot(), &entry, "second reversal");
}

fn access_program(width: u32, access: &str, prepare: &str, outer: bool) -> (String, Vec<u8>) {
    let bytes = width.div_ceil(8) as usize;
    let old: Vec<_> = (0..bytes).map(|j| 0x93u8.wrapping_add((j as u8).wrapping_mul(7))).collect();
    let stores: String = old
        .iter()
        .enumerate()
        .map(|(j, b)| format!("store i8 {b}, {}\nx [{}:7]\nx [{}:7]\n", 128 + j, 128 + j, 128 + j))
        .collect();
    let gates: String = (0..bytes).map(|j| format!("x [{}:{}] ctrl %c, %k\n", 128 + j, j % 8)).collect();
    (
        format!(
            "memory 8192\nfunc @main() {{\nentry:\n%c = qalloc 1\n{prepare}\n%k = qalloc 1\n{stores}{}%m = mark budget 200\n{access}\n{gates}rewind %m keep (i1 1 -> %k)\n{}h %c\nret\n}}",
            if outer { "%outer = mark budget 500\n" } else { "" },
            if outer { "rewind %outer\n" } else { "" }
        ),
        old,
    )
}

fn access_expected(input: [C; 2], old: &[u8], outer: bool) -> State {
    let mut state = State::new();
    for (c, a) in input.into_iter().enumerate() {
        let c = c as u64;
        let mut config = vec![c, u64::from(!outer)];
        config.extend(old.iter().enumerate().map(|(j, b)| u64::from(*b) ^ if outer { 0 } else { c << (j % 8) }));
        if !outer {
            config.extend((0..old.len()).map(|j| c << (j % 8)));
        }
        *state.entry(config).or_default() += a;
    }
    h(state)
}

#[test]
fn all_memory_access_widths_keep_exact_wires_and_reverse_each_step() {
    let mut runs = 0;
    for width in 1u32..=128 {
        for access in [format!("%v = load i{width} 128"), format!("store i{width} {}, 128", u128::MAX >> (128 - width))]
        {
            for (prepare, input) in preparations() {
                for outer in [false, true] {
                    let (text, old) = access_program(width, &access, prepare, outer);
                    let expected = access_expected(input, &old, outer);
                    reversible(&text, &expected, 2 + old.len() * if outer { 1 } else { 2 });
                    runs += 1;
                }
            }
        }
    }
    assert_eq!(runs, 3072);
}

#[test]
fn a_changed_setjmp_record_retains_and_takes_back_all_32_byte_wires() {
    for (prepare, input) in preparations() {
        for outer in [false, true] {
            let (text, old) = access_program(256, "%v = setjmp 128", prepare, outer);
            let expected = access_expected(input, &old, outer);
            reversible(&text, &expected, 2 + old.len() * if outer { 1 } else { 2 });
        }
    }
}

#[test]
fn memory_keeps_at_every_integer_width_preserve_the_other_byte_bits() {
    for width in 1u32..=128 {
        let bytes = width.div_ceil(8);
        let mask = u128::MAX >> (128 - width);
        let value = 0xa97b_205d_dcb1_409e_e356_98a0_7112_f4d3u128 & mask;
        let promotions: String =
            (0..bytes).map(|j| format!("store i8 165, {}\nx [{}:0]\nx [{}:0]\n", 128 + j, 128 + j, 128 + j)).collect();
        let text = format!(
            "memory 8192\nfunc @main() {{\nentry:\n{promotions}%m = mark budget 10\n%v = xor i{width} {value}, 0\nrewind %m keep (%v -> [128])\nret\n}}"
        );
        let expected: Vec<_> = (0..bytes).map(|j| 165 ^ ((value >> (8 * j)) as u64 & 255)).collect();
        reversible(&text, &BTreeMap::from([(expected, C::new(1.0, 0.0))]), bytes as usize);
    }
}

#[test]
fn strict_memory_access_diagnostics_remain_atomic() {
    for access in ["%v = load i16 128", "store i16 429, 128", "%v = setjmp 128"] {
        let width = if access.contains("setjmp") { 256 } else { 16 };
        let (text, _) = access_program(width, access, "h %c\ns %c", false);
        let p = common::load(&text);
        let mut e = common::executor(&p, "main", &[], Options { reversible: true, ..common::bounded() });
        let initial = e.snapshot();
        let mut before = initial.clone();
        while e.outcome().is_none() {
            before = e.snapshot();
            e.step();
        }
        assert_eq!(e.outcome(), Some(Outcome::Faulted(Fault::BadRewind)));
        let mut after = e.snapshot();
        after.classical.outcome = None;
        whole_state(&after, &before, "strict rewind is atomic");
        e.reverse().expect("atomic diagnostic reverses");
        whole_state(&e.snapshot(), &initial, "strict inverse");
    }
}

#[test]
fn overlapping_load_store_and_setjmp_wires_replay_in_their_original_order() {
    let accesses = "%a = load i16 128\nstore i8 17, 128\nstore i16 907, 129\n%b = load i32 128\n%j = setjmp 128\n%d = load i16 130";
    // Inverse order: last load bytes 2,3; setjmp bytes 0..32; prior load 0..4;
    // the two stores at bytes 1,2 and 0; first load bytes 0,1.
    let wires: Vec<usize> = [vec![2, 3], (0..32).collect(), (0..4).collect(), vec![1, 2], vec![0], vec![0, 1]].concat();
    for (prepare, input) in preparations() {
        for outer in [false, true] {
            let (text, old) = access_program(256, accesses, prepare, outer);
            let mut expected = State::new();
            for (c, a) in input.into_iter().enumerate() {
                let c = c as u64;
                let mut config = vec![c, u64::from(!outer)];
                config
                    .extend(old.iter().enumerate().map(|(j, b)| u64::from(*b) ^ if outer { 0 } else { c << (j % 8) }));
                if !outer {
                    config.extend(wires.iter().map(|j| c << (j % 8)));
                }
                *expected.entry(config).or_default() += a;
            }
            reversible(&text, &h(expected), 34 + if outer { 0 } else { wires.len() });
        }
    }
}

#[test]
fn whole_world_reference_takes_back_memory_accesses_and_promotions() {
    use common::oracle_support::worlds::{Model, basis};
    use common::oracle_support::worlds_executor::assert_snapshot;
    for width in [1, 16, 128, 256] {
        let access = if width == 256 {
            "%v = setjmp 128".to_owned()
        } else {
            format!("%v = load i{width} 128\nstore i{width} 1, 128")
        };
        for (prepare, _) in preparations() {
            for outer in [false, true] {
                let (text, _) = access_program(width, &access, prepare, outer);
                let p = common::load(&text);
                let mut m = Model::new(&p, "main");
                m.machine_meaning = true;
                let initial = basis(m.initial("main", &[]));
                let expected = m.run(initial.clone(), 300);
                let mut e = common::executor(
                    &p,
                    "main",
                    &[],
                    Options { reversible: true, machine_meaning: true, ..common::bounded() },
                );
                assert_eq!(e.run(), Outcome::Returned(None));
                assert_snapshot(&m, &expected, &e);
                let reversed = m.reverse_joined(expected).expect("reference memory wires reverse");
                assert_eq!(reversed.keys().collect::<Vec<_>>(), initial.keys().collect::<Vec<_>>());
                assert!((reversed.values().next().expect("entry amplitude") - C::new(1.0, 0.0)).norm() < 1e-11);
                e.reverse().expect("production memory wires reverse");
                assert_snapshot(&m, &initial, &e);
            }
        }
    }
    for outer in ["", "rewind %outer"] {
        let text = format!(
            "memory 1024\nfunc @main() {{\nentry:\n%c = qalloc 1\nh %c\ns %c\n%k = qalloc 1\nstore i8 147, 128\n%outer = mark budget 500\n%m = mark budget 100\nx [128:2] ctrl %c, %k\nrewind %m keep (i1 1 -> %k)\n{outer}\nh %c\nret\n}}"
        );
        let p = common::load(&text);
        let mut m = Model::new(&p, "main");
        m.machine_meaning = true;
        let initial = basis(m.initial("main", &[]));
        let expected = m.run(initial.clone(), 100);
        let mut e =
            common::executor(&p, "main", &[], Options { reversible: true, machine_meaning: true, ..common::bounded() });
        assert_eq!(e.run(), Outcome::Returned(None));
        assert_snapshot(&m, &expected, &e);
        let reversed = m.reverse_joined(expected).expect("reference promotions reverse");
        assert_eq!(reversed.keys().collect::<Vec<_>>(), initial.keys().collect::<Vec<_>>());
        assert!((reversed.values().next().expect("entry amplitude") - C::new(1.0, 0.0)).norm() < 1e-11);
        e.reverse().expect("production promotions reverse");
        assert_snapshot(&m, &initial, &e);
    }
}

fn whole_state(actual: &bennett_exec::Snapshot, expected: &bennett_exec::Snapshot, context: &str) {
    common::assert_same_state(actual, expected, context);
    assert_eq!(actual.worlds.len(), expected.worlds.len(), "{context}: other worlds");
    for (actual, expected) in actual.worlds.iter().zip(&expected.worlds) {
        whole_state(actual, expected, context);
    }
}
