//! Phase-sensitive swap-in checks against both the specified map and device programs.

use bennett_exec::{Executor, Outcome};
use bennett_ir::{Fault, ValidModule};
use num_complex::Complex;

use crate::common::{allowing_region_faults, executor, load};
use crate::compute_copy_reference::swap_in::{
    Density, Registers, StoreVector, apply_to_spare, clear_known, exchange, release,
};

type Exact = Complex<i64>;

const PHASES: [Exact; 4] = [Complex::new(1, 0), Complex::new(0, 1), Complex::new(-1, 0), Complex::new(0, -1)];
const PREPARE_ARGUMENT: &str = "h %a[0]\nh %a[1]\ns %a[0]\nz %a[1]\n";
const PREPARE_TARGET: &str = "h %x[0]\nh %x[1]\ns %x[0]\nz %x[1]\n";
const PREPARE_SPARE: &str = "h %n[0]\nh %n[1]\nz %n[1]\n";

const ADD: &str = "
func @g(i2 %a, i2 %x) -> i2 {
entry:
    %u = add i2 %x, %a
    ret %u
}
";

fn exchange_gates(width: u32, target_bit: impl Fn(u32) -> String) -> String {
    (0..width)
        .map(|bit| {
            let target = target_bit(bit);
            format!("x %n[{bit}] ctrl {target}\nx {target} ctrl %n[{bit}]\nx %n[{bit}] ctrl {target}\n")
        })
        .collect()
}

// The layout is (a, x, n, e) in both the reference and the register device program.
fn register_program(functions: &str, preparation: &str, after_swap: &str) -> ValidModule {
    let swap = exchange_gates(2, |bit| format!("%x[{bit}]"));
    load(&format!(
        "memory 128
{functions}
func @main() {{
entry:
    %a = qalloc 2
    %x = qalloc 2
    %n = qalloc 2
    %e = qalloc 5
{PREPARE_ARGUMENT}{preparation}    apply @g(%a, %x) -> %n, %e budget 4
{swap}{after_swap}    ret
}}"
    ))
}

fn correlated_input(old: impl Fn(u64) -> u64) -> StoreVector {
    (0..4).map(|a| ([a, old(a), 0, 0], PHASES[a as usize])).collect()
}

fn independent_input(spare_values: u64) -> StoreVector {
    let mut state = StoreVector::new();
    for a in 0..4 {
        for x in 0..4 {
            for n in 0..spare_values {
                state.insert([a, x, n, 0], PHASES[a as usize] * PHASES[x as usize] * PHASES[(n & 2) as usize]);
            }
        }
    }
    state
}

fn image(input: &StoreVector, labels: impl Fn(Registers) -> Registers) -> StoreVector {
    input.iter().map(|(&basis, &amplitude)| (labels(basis), amplitude)).collect()
}

fn scaled(amplitude: Exact, denominator: f64) -> Complex<f64> {
    Complex::new(amplitude.re as f64, amplitude.im as f64) / denominator
}

struct Comparison<'m> {
    module: &'m ValidModule,
    device: Executor<'m>,
    exact: StoreVector,
    denominator: f64,
    columns: [usize; 4],
}

impl<'m> Comparison<'m> {
    fn prepared(
        module: &'m ValidModule,
        steps: usize,
        exact: StoreVector,
        denominator: f64,
        columns: [usize; 4],
    ) -> Self {
        let mut comparison = Self {
            module,
            device: executor(module, "main", &[], allowing_region_faults()),
            exact,
            denominator,
            columns,
        };
        comparison.step_and_compare(steps, "preparation");
        comparison
    }

    fn step_and_compare(&mut self, steps: usize, stage: &str) {
        for _ in 0..steps {
            assert_eq!(self.device.step(), None, "{stage}");
        }
        let quantum = self.device.snapshot().quantum;
        assert_eq!(quantum.registers.len(), self.columns.len(), "{stage}");
        assert_eq!(quantum.amplitudes.len(), self.exact.len(), "{stage}");
        for (config, actual) in quantum.amplitudes {
            let basis = self.columns.map(|column| config[column]);
            let expected = self.exact.get(&basis).unwrap_or_else(|| panic!("{stage}: unexpected {basis:?}"));
            let expected = scaled(*expected, self.denominator);
            assert!((actual - expected).norm() < 1e-14, "{stage} at {basis:?}: {actual} != {expected}");
        }
    }

    fn apply<const N: usize>(&mut self, function: &str, arguments: impl Fn(&Registers) -> [u64; N]) {
        self.exact = apply_to_spare(self.module, function, 4, std::mem::take(&mut self.exact), arguments);
        self.step_and_compare(1, function);
    }

    fn exchange(&mut self, width: u32) {
        self.exact = exchange(std::mem::take(&mut self.exact), width);
        self.step_and_compare(3 * width as usize, "exchange");
    }

    fn clear_known(&mut self, value: u64) {
        self.exact = clear_known(std::mem::take(&mut self.exact), value);
        self.step_and_compare(value.count_ones() as usize, "clear known old value");
    }

    fn check_released(mut self, expected: &Density) {
        let exact = release(&self.exact);
        let before = self.device.snapshot().quantum;
        assert_eq!(self.device.run(), Outcome::Returned(None));
        let quantum = self.device.snapshot().quantum;
        for &column in &self.columns[2..] {
            let released = &quantum.registers[column];
            assert!(released.width.is_none() || released.generation != before.registers[column].generation);
        }
        // The executor snapshot keeps amplitudes of its purification. Form each density
        // matrix entry directly, with matching values in every column outside the kept pair.
        let [argument, target, ..] = self.columns;
        let mut actual = [[Complex::<f64>::default(); 16]; 16];
        for (row, alpha) in &quantum.amplitudes {
            for (column, beta) in &quantum.amplitudes {
                if (0..row.len()).all(|i| i == argument || i == target || row[i] == column[i]) {
                    let r = (4 * row[argument] + row[target]) as usize;
                    let c = (4 * column[argument] + column[target]) as usize;
                    actual[r][c] += alpha * beta.conj();
                }
            }
        }
        for (r, entries) in actual.iter().enumerate() {
            for (c, &actual) in entries.iter().enumerate() {
                let index = ([(r / 4) as u64, (r % 4) as u64], [(c / 4) as u64, (c % 4) as u64]);
                let coefficient = expected.get(&index).copied().unwrap_or_default();
                assert_eq!(exact.get(&index).copied().unwrap_or_default(), coefficient, "reference rho{index:?}");
                let expected = scaled(coefficient, self.denominator * self.denominator);
                assert!((actual - expected).norm() < 1e-14, "executor rho{index:?}: {actual} != {expected}");
            }
        }
    }
}

#[test]
fn apply_and_exchange_is_the_full_permutation_even_with_a_dirty_spare() {
    let module = register_program(ADD, &format!("{PREPARE_TARGET}{PREPARE_SPARE}"), "");
    let input = independent_input(4);
    // Check each basis image separately so a permutation within a set of equal
    // coefficients cannot hide in the superposed comparison.
    for (&basis, &amplitude) in &input {
        let [a, x, n, _] = basis;
        let definite = StoreVector::from([(basis, amplitude)]);
        let applied = apply_to_spare(&module, "g", 4, definite, |&[a, x, ..]| [a, x]);
        let expected = StoreVector::from([([a, n ^ ((a + x) & 3), x, 0], amplitude)]);
        assert_eq!(exchange(applied, 2), expected, "basis {basis:?}");
    }
    let mut check = Comparison::prepared(&module, 15, input.clone(), 8.0, [0, 1, 2, 3]);
    check.apply("g", |&[a, x, ..]| [a, x]);
    check.exchange(2);
    assert_eq!(check.exact, image(&input, |[a, x, n, _]| [a, n ^ ((a + x) & 3), x, 0]));
    assert_eq!(check.device.run(), Outcome::Returned(None));
}

#[test]
fn known_old_byte_clears_by_x_gates() {
    compare_known_old_byte(512);
}

// With 128 bytes, `@main`'s four slots leave the stack pointer at 96 and `@g`'s four slots would take
// it to 64, below the floor of 65 (data ends after byte 64): the entry reservation faults
// `stack_overflow` before any budget test, for every argument value.
#[test]
fn byte_swap_with_insufficient_space_for_the_applied_frame() {
    compare_known_old_byte(128);
}

fn compare_known_old_byte(memory_bytes: u64) {
    let swap = exchange_gates(8, |bit| format!("[64:{bit}]"));
    let module = load(&format!(
        "memory {memory_bytes}
data 64 = i8 165
func @g(i2 %a, i8 %x) -> i8 {{
entry:
    %w = zext i2 %a to i8
    %u = add i8 %x, %w
    ret %u
}}
func @main() {{
entry:
    %old = load i8 64
    %a = qalloc 2
    %n = qalloc 8
    %e = qalloc 5
{PREPARE_ARGUMENT}    x [64:0]
    x [64:0]
    apply @g(%a, %old) -> %n, %e budget 4
{swap}    x %n[0]
    x %n[2]
    x %n[5]
    x %n[7]
    ret
}}"
    ));
    let input = correlated_input(|_| 165);
    // The byte's column is allocated by the first addressed gate. The two X gates
    // in preparation leave its value unchanged.
    let mut check = Comparison::prepared(&module, 10, input.clone(), 2.0, [0, 3, 1, 2]);
    check.apply("g", |&[a, x, ..]| [a, x]);
    if memory_bytes < 64 + 1 + 8 * (4 + 4) {
        assert_eq!(check.exact, image(&input, |[a, x, ..]| [a, x, 0, Fault::StackOverflow.code()]));
        return;
    }
    check.exchange(8);
    check.clear_known(165);
    assert_eq!(check.exact, image(&input, |[a, ..]| [a, 165 + a, 0, 0]));
    assert_eq!(check.device.run(), Outcome::Returned(None));
    assert_eq!(check.device.snapshot().quantum.cells, [(64, 3)]);
}

#[test]
fn old_value_recomputed_from_the_unchanged_argument_clears_the_spare() {
    let module = register_program(
        "
func @g(i2 %a, i2 %x) -> i2 {
entry:
    %high = and i2 %x, 2
    %u = xor i2 %a, %high
    ret %u
}
func @old(i2 %a) -> i2 {
entry:
    %v = and i2 %a, 2
    ret %v
}",
        "x %x[1] ctrl %a[1]\n",
        "apply @old(%a) -> %n, %e budget 4\n",
    );
    let input = correlated_input(|a| a & 2);
    let mut check = Comparison::prepared(&module, 9, input.clone(), 2.0, [0, 1, 2, 3]);
    check.apply("g", |&[a, x, ..]| [a, x]);
    check.exchange(2);
    check.apply("old", |&[a, ..]| [a]);
    assert_eq!(check.exact, image(&input, |[a, ..]| [a, a & 1, 0, 0]));
    assert_eq!(check.device.run(), Outcome::Returned(None));
}

#[test]
fn inverse_on_the_new_target_clears_the_spare() {
    let functions = format!(
        "{ADD}
func @inverse(i2 %a, i2 %u) -> i2 {{
entry:
    %v = sub i2 %u, %a
    ret %v
}}"
    );
    let module = register_program(&functions, PREPARE_TARGET, "apply @inverse(%a, %x) -> %n, %e budget 4\n");
    let input = independent_input(1);
    let mut check = Comparison::prepared(&module, 12, input.clone(), 4.0, [0, 1, 2, 3]);
    check.apply("g", |&[a, x, ..]| [a, x]);
    check.exchange(2);
    check.apply("inverse", |&[a, x, ..]| [a, x]);
    assert_eq!(check.exact, image(&input, |[a, x, ..]| [a, (a + x) & 3, 0, 0]));
    assert_eq!(check.device.run(), Outcome::Returned(None));
}

#[test]
fn retained_old_value_keeps_only_equal_old_value_coherences() {
    let module = register_program(
        "
func @g(i2 %a, i2 %x) -> i2 {
entry:
    %low = and i2 %x, 1
    %u = xor i2 %a, %low
    ret %u
}",
        PREPARE_TARGET,
        "qfree %n\nqfree %e\n",
    );
    let input = independent_input(1);
    let mut check = Comparison::prepared(&module, 12, input.clone(), 4.0, [0, 1, 2, 3]);
    check.apply("g", |&[a, x, ..]| [a, x]);
    check.exchange(2);
    assert_eq!(check.exact, image(&input, |[a, x, ..]| [a, a ^ (x & 1), x, 0]));

    // Old values with the same low bit give the same kept output for each a.
    // Each such pair contributes twice to that diagonal and to equal-old
    // coherences across different a. Coherences between the low-bit groups vanish.
    let mut expected = Density::new();
    for a in 0..4 {
        for b in 0..4 {
            for low in 0..2 {
                expected.insert(([a, a ^ low], [b, b ^ low]), 2 * PHASES[a as usize] * PHASES[b as usize].conj());
            }
        }
    }
    check.check_released(&expected);
}

#[test]
fn fault_code_distinguishes_released_pairs_even_with_a_zero_spare() {
    let module = register_program(
        "
func @g(i2 %a, i2 %x) -> i2 {
entry:
    %low = and i2 %a, 1
    %u = udiv i2 2, %low
    ret %u
}",
        "",
        "qfree %n\nqfree %e\n",
    );
    let input = correlated_input(|_| 0);
    let mut check = Comparison::prepared(&module, 8, input.clone(), 2.0, [0, 1, 2, 3]);
    check.apply("g", |&[a, x, ..]| [a, x]);
    check.exchange(2);

    let targets = [0, 2, 0, 2];
    let divide_by_zero = Fault::DivideByZero.code();
    let faults = [divide_by_zero, 0, divide_by_zero, 0];
    assert_eq!(check.exact, image(&input, |[a, ..]| [a, targets[a as usize], 0, faults[a as usize]]));
    let mut expected = Density::new();
    for (a, b) in [(0, 0), (0, 2), (2, 0), (2, 2), (1, 1), (1, 3), (3, 1), (3, 3)] {
        expected.insert(([a as u64, targets[a]], [b as u64, targets[b]]), PHASES[a] * PHASES[b].conj());
    }
    check.check_released(&expected);
}
