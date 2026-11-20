//! Seeded generated applied functions: the independent reference against the executor.
//!
//! Each generated module has an applied function `@f(i3 %x, i3 %y) -> i8` over the instruction families
//! the gate emitter covers (integer operations at several widths, comparisons, selections, casts,
//! shifts by constant and register amounts, quotient and remainder with `divide_by_zero`, jumps,
//! `switch`, loops, direct and indirect calls with their frames), plus byte loads and stores, explicit
//! faults, `effect` and `exit`. For every one of the 64 argument values the reference's own run gives
//! when it finishes; the executor is then compared at budget zero, at one step before, at and after each
//! finish, and at a bound beyond all of them. At a few budgets the whole reversible construction runs
//! on all 64 inputs and on a superposition with phases, and the executor's amplitudes are compared.

mod common;
#[allow(dead_code, reason = "this binary does not use the store-by-exchange helpers")]
mod compute_copy_reference;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use bennett_exec::{Options, Outcome};
use bennett_ir::{Fault, ValidModule};
use common::{bounded, executor};
use compute_copy_reference::{Reference, Vector};
use num_complex::Complex;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

/// Inputs: both 3-bit arguments.
const INPUTS: u64 = 64;

/// A bound on the reference's own runs, beyond which an input counts as never finishing. Generated
/// loops run at most 255 passes of a few steps, so some inputs exhaust it, which gives `budget`.
const BOUND: usize = 400;

/// The largest budget at which the whole reversible construction runs.
const CONSTRUCTED: usize = 48;

/// Explicit faults: codes no generated instruction gives otherwise, then two that others also give.
const FAULTS: [&str; 6] =
    ["unreachable", "dangling_register", "qubit_alias", "exit_type_mismatch", "out_of_bounds", "budget"];
const EXPLICIT: [Fault; 4] = [Fault::Unreachable, Fault::DanglingRegister, Fault::QubitAlias, Fault::ExitTypeMismatch];
/// Every operation once, then the signed, division and shift operations again: their sign, rounding
/// and wide-amount cases are the ones a run reaches least often.
const BINARY: [&str; 20] = [
    "add", "sub", "mul", "udiv", "sdiv", "urem", "srem", "and", "or", "xor", "shl", "lshr", "ashr", "sdiv", "srem",
    "sdiv", "udiv", "shl", "shl", "ashr",
];
const PREDICATES: [&str; 10] = ["eq", "ne", "ult", "ule", "ugt", "uge", "slt", "sle", "sgt", "sge"];
const WIDTHS: [u32; 6] = [1, 3, 5, 16, 32, 64];

/// The applied function, its helpers and the caller around them, in the text form.
struct Generated {
    text: String,
}

struct Function {
    blocks: Vec<String>,
    current: String,
    names: usize,
}

struct Generator {
    rng: ChaCha8Rng,
    /// Helpers `@h0 ...`, each `(i8, i8) -> i8`; helper `j` may call helpers below `j`.
    helpers: usize,
    /// Whether `@v(i8)`, a helper without a result that stores its argument, exists.
    void_helper: bool,
}

impl Function {
    fn new() -> Self {
        Self { blocks: Vec::new(), current: "entry:\n".into(), names: 0 }
    }

    fn name(&mut self, prefix: &str) -> String {
        self.names += 1;
        format!("{prefix}{}", self.names)
    }

    fn line(&mut self, line: &str) {
        writeln!(self.current, "    {line}").expect("string");
    }

    fn close(&mut self, terminator: &str, next: &str) {
        self.line(terminator);
        self.blocks.push(std::mem::replace(&mut self.current, format!("{next}:\n")));
    }

    fn finish(mut self, terminator: &str) -> String {
        self.line(terminator);
        self.blocks.push(self.current);
        self.blocks.concat()
    }
}

impl Generator {
    fn constant(&mut self) -> String {
        match self.rng.random_range(0..4) {
            0 => "0".into(),
            1 => self.rng.random_range(-128..0).to_string(),
            _ => self.rng.random_range(0..256).to_string(),
        }
    }

    fn operand(&mut self, values: &[String]) -> String {
        if self.rng.random_bool(0.75) {
            values[self.rng.random_range(0..values.len())].clone()
        } else {
            self.constant()
        }
    }

    fn value(&mut self, values: &[String]) -> String {
        values[self.rng.random_range(0..values.len())].clone()
    }

    fn pick<'a>(&mut self, from: &[&'a str]) -> &'a str {
        from[self.rng.random_range(0..from.len())]
    }

    /// One straight-line item, defining at most a few names; the last becomes available.
    fn item(&mut self, f: &mut Function, values: &mut Vec<String>, calls_below: usize, applied: bool) {
        let d = f.name("v");
        match self.rng.random_range(0..16) {
            0..=3 => {
                let (op, a, b) = (self.pick(&BINARY), self.operand(values), self.operand(values));
                f.line(&format!("%{d} = {op} i8 {a}, {b}"));
            }
            4 | 5 => {
                // The same operation at another width, through casts each way.
                let w = WIDTHS[self.rng.random_range(0..WIDTHS.len())];
                let op = self.pick(&BINARY);
                let mut wide = Vec::new();
                for _ in 0..2 {
                    let (n, v) = (f.name("w"), self.value(values));
                    let cast = if w > 8 { self.pick(&["zext", "sext"]) } else { "trunc" };
                    f.line(&format!("%{n} = {cast} i8 {v} to i{w}"));
                    wide.push(n);
                }
                let r = f.name("w");
                f.line(&format!("%{r} = {op} i{w} %{}, %{}", wide[0], wide[1]));
                let cast = if w > 8 { "trunc" } else { self.pick(&["zext", "sext"]) };
                f.line(&format!("%{d} = {cast} i{w} %{r} to i8"));
            }
            6 | 7 => {
                let (pred, a, b) = (self.pick(&PREDICATES), self.value(values), self.operand(values));
                let c = f.name("c");
                f.line(&format!("%{c} = icmp {pred} i8 {a}, {b}"));
                if self.rng.random_bool(0.5) {
                    let (t, e) = (self.operand(values), self.operand(values));
                    f.line(&format!("%{d} = select i8 %{c}, {t}, {e}"));
                } else {
                    let cast = self.pick(&["zext", "sext"]);
                    f.line(&format!("%{d} = {cast} i1 %{c} to i8"));
                }
            }
            8 => {
                // A shift amount from a register, possibly at least the width.
                let (op, a, b) = (self.pick(&["shl", "lshr", "ashr"]), self.value(values), self.value(values));
                let amount = f.name("s");
                f.line(&format!("%{amount} = and i8 {b}, 15"));
                f.line(&format!("%{d} = {op} i8 {a}, %{amount}"));
            }
            9 => {
                // Private memory: a byte store, then a load of one or two bytes at a computed address
                // that can lie beyond the end of memory.
                let (v, a) = (self.value(values), self.value(values));
                let at = self.rng.random_range(0..16);
                f.line(&format!("store i8 {v}, {at}"));
                let (p, q, l) = (f.name("p"), f.name("p"), f.name("l"));
                let offset = self.rng.random_range(0..400);
                f.line(&format!("%{p} = zext i8 {a} to i64"));
                f.line(&format!("%{q} = add i64 %{p}, {offset}"));
                if self.rng.random_bool(0.5) {
                    f.line(&format!("%{d} = load i8 %{q}"));
                } else {
                    f.line(&format!("%{l} = load i16 %{q}"));
                    f.line(&format!("%{d} = trunc i16 %{l} to i8"));
                }
            }
            10 | 11 if calls_below > 0 => {
                let (j, a, b) = (self.rng.random_range(0..calls_below), self.operand(values), self.operand(values));
                f.line(&format!("%{d} = call @h{j}({}, {})", typed(&a), typed(&b)));
            }
            12 if applied && self.helpers > 0 => {
                // An indirect call through one of two addresses: a helper's, or one that is no
                // function of this signature (`bad_indirect`).
                let j = self.rng.random_range(0..self.helpers);
                let (good, bad, c, p) = (f.name("a"), f.name("a"), f.name("c"), f.name("a"));
                f.line(&format!("%{good} = faddr @h{j}"));
                match self.rng.random_range(0..3) {
                    0 => f.line(&format!("%{bad} = faddr @f")),
                    1 => f.line(&format!("%{bad} = add i64 0, {}", self.rng.random_range(0..12))),
                    _ => f.line(&format!("%{bad} = faddr @h{}", self.rng.random_range(0..self.helpers))),
                }
                let (v, k) = (self.value(values), self.constant());
                f.line(&format!("%{c} = icmp ult i8 {v}, {k}"));
                f.line(&format!("%{p} = select i64 %{c}, %{good}, %{bad}"));
                let (a, b) = (self.operand(values), self.operand(values));
                f.line(&format!("%{d} = call_indirect fn(i8, i8) -> i8 %{p}({}, {})", typed(&a), typed(&b)));
            }
            13 if self.void_helper => {
                let a = self.operand(values);
                let at = self.rng.random_range(0..16);
                f.line(&format!("call @v({})", typed(&a)));
                f.line(&format!("%{d} = load i8 {at}"));
            }
            14 | 15 => {
                // A reservation of up to 63 bytes (rounded down to eight), a byte stored in it and read
                // back, then sometimes a restore: to the saved pointer, a few bytes below it, or eight
                // above it, which is in range only if the frame reserved more before the save.
                let (save, size, at, low, back) = (f.name("p"), f.name("p"), f.name("p"), f.name("p"), f.name("p"));
                let (v, n) = (self.value(values), self.value(values));
                f.line(&format!("%{save} = stacksave"));
                let masked = f.name("p");
                f.line(&format!("%{masked} = and i8 {n}, 63"));
                f.line(&format!("%{size} = zext i8 %{masked} to i64"));
                f.line(&format!("%{at} = alloca %{size}"));
                f.line(&format!("store i8 {v}, %{at}"));
                let byte = f.name("l");
                f.line(&format!("%{byte} = load i8 %{at}"));
                let target = match self.rng.random_range(0..4) {
                    0 => None,
                    1 => Some(save.clone()),
                    2 => {
                        f.line(&format!("%{low} = sub i64 %{save}, {}", self.rng.random_range(1..9)));
                        Some(low)
                    }
                    _ => {
                        f.line(&format!("%{low} = add i64 %{save}, 8"));
                        Some(low)
                    }
                };
                if let Some(target) = target {
                    f.line(&format!("stackrestore %{target}"));
                }
                f.line(&format!("%{back} = stacksave"));
                let (distance, narrow) = (f.name("p"), f.name("l"));
                f.line(&format!("%{distance} = sub i64 %{save}, %{back}"));
                f.line(&format!("%{narrow} = trunc i64 %{distance} to i8"));
                f.line(&format!("%{d} = xor i8 %{narrow}, %{byte}"));
            }
            _ => {
                let (a, b) = (self.operand(values), self.operand(values));
                f.line(&format!("%{d} = xor i8 {a}, {b}"));
            }
        }
        values.push(format!("%{d}"));
    }

    /// A value depending on every name of `values` from `from` on (on one name if there are none), so that
    /// what a region computes reaches the result instead of dying unread.
    fn fold(&mut self, f: &mut Function, values: &[String], from: usize) -> String {
        let fresh = &values[from.min(values.len() - 1)..];
        let mut acc = fresh[0].clone();
        for v in &fresh[1..] {
            let t = f.name("f");
            let op = self.pick(&["add", "xor", "sub"]);
            f.line(&format!("%{t} = {op} i8 {acc}, {v}"));
            acc = format!("%{t}");
        }
        acc
    }

    /// A sequence of items and control constructs; on return the current block is open and every
    /// name in `values` is available in it.
    fn items(&mut self, f: &mut Function, values: &mut Vec<String>, depth: u32, calls_below: usize, applied: bool) {
        for _ in 0..self.rng.random_range(1..5) {
            match self.rng.random_range(0..10) {
                0 if depth > 0 => self.diamond(f, values, depth, calls_below, applied),
                1 if depth > 0 => self.switch(f, values, depth, calls_below, applied),
                2 if depth > 0 => self.repeat(f, values, depth, calls_below, applied),
                3 => self.early_stop(f, values),
                _ => self.item(f, values, calls_below, applied),
            }
        }
    }

    fn diamond(&mut self, f: &mut Function, values: &mut Vec<String>, depth: u32, calls_below: usize, applied: bool) {
        let (c, t, e, join, p) = (f.name("c"), f.name("t"), f.name("e"), f.name("j"), f.name("v"));
        let (pred, a, b) = (self.pick(&PREDICATES), self.value(values), self.operand(values));
        f.line(&format!("%{c} = icmp {pred} i8 {a}, {b}"));
        f.close(&format!("br %{c}, {t}, {e}"), &t);
        for next in [&e, &format!("{join}(i8 %{p})")] {
            let mut inner = values.clone();
            self.items(f, &mut inner, depth - 1, calls_below, applied);
            let v = self.fold(f, &inner, values.len());
            f.close(&format!("br {join}({})", typed(&v)), next);
        }
        values.push(format!("%{p}"));
    }

    fn switch(&mut self, f: &mut Function, values: &mut Vec<String>, depth: u32, calls_below: usize, applied: bool) {
        let (s, join, p) = (f.name("s"), f.name("j"), f.name("v"));
        let a = self.value(values);
        f.line(&format!("%{s} = and i8 {a}, 7"));
        let arms: Vec<String> = (0..self.rng.random_range(1..4)).map(|_| f.name("k")).collect();
        let mut cases: Vec<u64> = (0..8).collect();
        let mut text = format!("switch i8 %{s}");
        for arm in &arms {
            let k = cases.remove(self.rng.random_range(0..cases.len()));
            write!(text, ", {k}: {arm}").expect("string");
        }
        // A case straight to the join, passing a value, and a default.
        let k = cases.remove(self.rng.random_range(0..cases.len()));
        let direct = self.value(values);
        write!(text, ", {k}: {join}({}), default: {join}(i8 {})", typed(&direct), self.constant()).expect("string");
        let first = arms[0].clone();
        f.close(&text, &first);
        for (i, _) in arms.iter().enumerate() {
            let mut inner = values.clone();
            self.items(f, &mut inner, depth - 1, calls_below, applied);
            let v = self.fold(f, &inner, values.len());
            let next = arms.get(i + 1).cloned().unwrap_or_else(|| format!("{join}(i8 %{p})"));
            f.close(&format!("br {join}({})", typed(&v)), &next);
        }
        values.push(format!("%{p}"));
    }

    /// A loop carrying a counter, a value and the value of the previous pass; the trip count is at most 7
    /// or at most 255 passes. The jump back passes the value parameter on to the third parameter, so a
    /// jump that assigned parameters one after another would pass the new value instead of the old.
    fn repeat(&mut self, f: &mut Function, values: &mut Vec<String>, depth: u32, calls_below: usize, applied: bool) {
        let (n, head, body, exit) = (f.name("n"), f.name("l"), f.name("b"), f.name("x"));
        let (i, acc, more, next, out) = (f.name("i"), f.name("v"), f.name("c"), f.name("i"), f.name("v"));
        let old = f.name("v");
        let (a, init) = (self.value(values), self.value(values));
        let bound = if self.rng.random_bool(0.7) { 7 } else { 255 };
        f.line(&format!("%{n} = and i8 {a}, {bound}"));
        f.close(&format!("br {head}(i8 0, {}, i8 0)", typed(&init)), &format!("{head}(i8 %{i}, i8 %{acc}, i8 %{old})"));
        f.line(&format!("%{more} = icmp ult i8 %{i}, %{n}"));
        f.close(&format!("br %{more}, {body}, {exit}(%{acc})"), &body);
        let mut inner = values.clone();
        inner.extend([format!("%{i}"), format!("%{acc}"), format!("%{old}")]);
        self.items(f, &mut inner, depth - 1, calls_below, applied);
        let v = self.fold(f, &inner, values.len());
        f.line(&format!("%{next} = add i8 %{i}, 1"));
        f.close(&format!("br {head}(%{next}, {}, %{acc})", typed(&v)), &format!("{exit}(i8 %{out})"));
        values.push(format!("%{out}"));
    }

    /// A test that leaves the function on one side: a return, an explicit fault, `effect` or `exit`.
    fn early_stop(&mut self, f: &mut Function, values: &[String]) {
        let (c, stop, go) = (f.name("c"), f.name("t"), f.name("g"));
        let (pred, a, b) = (self.pick(&PREDICATES), self.value(values), self.operand(values));
        f.line(&format!("%{c} = icmp {pred} i8 {a}, {b}"));
        f.close(&format!("br %{c}, {stop}, {go}"), &stop);
        let v = self.fold(f, values, 0);
        let terminator = match self.rng.random_range(0..5) {
            0 => format!("ret {v}"),
            1 | 2 => format!("fault {}", self.pick(&FAULTS)),
            3 => {
                f.line(&format!("effect {v}"));
                format!("ret {v}")
            }
            _ => format!("exit {}", typed(&v)),
        };
        f.close(&terminator, &go);
    }

    fn module(&mut self) -> Generated {
        self.helpers = self.rng.random_range(0..3);
        self.void_helper = self.rng.random_bool(0.4);
        let mut text = String::new();
        let mut slots = Vec::new();
        for j in 0..self.helpers {
            let mut f = Function::new();
            let mut values = vec!["%a".to_string(), "%b".to_string()];
            self.items(&mut f, &mut values, 1, j, false);
            let v = self.fold(&mut f, &values, 0);
            slots.push(f.names + 2);
            write!(text, "func @h{j}(i8 %a, i8 %b) -> i8 {{\n{}}}\n", f.finish(&format!("ret {v}"))).expect("string");
        }
        if self.void_helper {
            let at = self.rng.random_range(0..16);
            text.push_str(&format!(
                "func @v(i8 %a) {{\nentry:\n    %b = add i8 %a, 1\n    store i8 %b, {at}\n    ret\n}}\n"
            ));
        }
        let mut f = Function::new();
        let (x, y) = (f.name("v"), f.name("v"));
        f.line(&format!("%{x} = zext i3 %x to i8"));
        f.line(&format!("%{y} = sext i3 %y to i8"));
        let mut values = vec![format!("%{x}"), format!("%{y}")];
        self.items(&mut f, &mut values, 2, self.helpers, true);
        let v = self.fold(&mut f, &values, 0);
        write!(text, "func @f(i3 %x, i3 %y) -> i8 {{\n{}}}\n", f.finish(&format!("ret {v}"))).expect("string");
        let data: Vec<String> = (0..16).map(|_| format!("i8 {}", self.rng.random_range(0..256))).collect();
        let text = format!("data 0 = {}\n{text}{CALLERS}", data.join(", "));
        // Static data ends at 16 and the caller takes five slots. A tight memory, in a quarter of the
        // modules, leaves room for the applied frame and for some, none or all of its callees' frames,
        // so `stack_overflow` occurs at a call; rarely not even the applied frame fits.
        let sized = bennett_ir::load(&format!("memory 4096\n{text}")).expect("generated text loads");
        let slots = |name: &str| 8 * sized.function(sized.function_named(name).expect("function")).slot_count() as u64;
        let applied = slots("f");
        let callees: u64 = (0..self.helpers).map(|j| slots(&format!("h{j}"))).sum::<u64>() + 8;
        // The stack pointer may meet the floor exactly: after the applied frame, or after one callee's.
        let helpers = self.helpers;
        let callee = |j: usize| if j < helpers { slots(&format!("h{j}")) } else { 8 };
        let extra = match self.rng.random_range(0..16) {
            0 => self.rng.random_range(0..applied + 1),
            1 => applied,
            2 => applied + callee(self.rng.random_range(0..self.helpers + 1)),
            3 => applied + self.rng.random_range(0..callees + 1),
            _ => applied + callees + 64,
        };
        let text = format!("memory {}\n{text}", 16 + 40 + extra);
        Generated { text }
    }
}

/// A constant operand in an argument list names its type.
fn typed(operand: &str) -> String {
    if operand.starts_with('%') { operand.to_string() } else { format!("i8 {operand}") }
}

/// Both callers have five slots, so the applied function's frame starts at the same stack pointer.
const CALLERS: &str = "
func @definite(i3 %x, i3 %y, i64 %budget) {
entry:
    %o = qalloc 8
    %e = qalloc 5
    apply @f(%x, %y) -> %o, %e budget %budget
    ret
}
func @main(i64 %budget) {
entry:
    %x = qalloc 3
    %y = qalloc 3
    %o = qalloc 8
    %e = qalloc 5
    h %x[0]
    h %x[1]
    h %x[2]
    h %y[0]
    h %y[1]
    h %y[2]
    h %o[0]
    s %x[0]
    z %y[1]
    s %o[0]
    x %o[7]
    x %e[2]
    x %e[4]
    apply @f(%x, %y) -> %o, %e budget %budget
    ret
}
";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Basis {
    x: u64,
    y: u64,
    out: u64,
    fault: u64,
}

/// The state `@main` prepares, with amplitudes in units of 1/sqrt(128) = 1/(8 sqrt 2).
fn prepared() -> Vector<Basis> {
    let i = Complex::new(0, 1);
    let mut vector = Vector::new();
    for x in 0..8 {
        for y in 0..8 {
            for o in [0, 1] {
                let mut amplitude = Complex::new(1, 0);
                if x & 1 == 1 {
                    amplitude *= i;
                }
                if y & 2 == 2 {
                    amplitude = -amplitude;
                }
                if o == 1 {
                    amplitude *= i;
                }
                vector.insert(Basis { x, y, out: 128 | o, fault: 20 }, amplitude);
            }
        }
    }
    vector
}

fn arguments(input: u64) -> [u64; 2] {
    [input & 7, input >> 3]
}

/// The executor's `(out, fault)` after `@definite(x, y, budget)`.
fn executor_outcome(module: &ValidModule, [x, y]: [u64; 2], budget: usize) -> (u64, u64) {
    let options = Options { allow_region_faults: true, ..bounded() };
    let mut run = executor(module, "definite", &[x, y, budget as u64], options);
    assert_eq!(run.run(), Outcome::Returned(None));
    let state = run.snapshot();
    assert_eq!(state.quantum.amplitudes.len(), 1);
    let config = &state.quantum.amplitudes[0].0;
    (config[0], config[1])
}

/// The construction at `budget` on all inputs (blank targets) and on `@main`'s superposition, against
/// the reference's own finishes and the executor's amplitudes.
fn construction(module: &ValidModule, reference: &Reference, budget: usize, expected: &[(u64, u64)], context: &str) {
    let blank: Vector<[u64; 4]> = (0..INPUTS).map(|i| ([i & 7, i >> 3, 0, 0], Complex::new(1, 0))).collect();
    let (after, outcomes) = reference.apply(
        blank,
        budget,
        |r| [r[0], r[1]],
        |r, (o, e)| {
            r[2] ^= o;
            r[3] ^= e;
        },
    );
    for (input, &want) in expected.iter().enumerate() {
        assert_eq!(outcomes[&arguments(input as u64)], want, "{context}: construction at input {input}");
        let [x, y] = arguments(input as u64);
        assert_eq!(after[&[x, y, want.0, want.1]], Complex::new(1, 0), "{context}");
    }
    let (after, _) = reference.apply(
        prepared(),
        budget,
        |b| [b.x, b.y],
        |b, (o, e)| {
            b.out ^= o;
            b.fault ^= e;
        },
    );
    let mut device = executor(module, "main", &[budget as u64], Options { allow_region_faults: true, ..bounded() });
    assert_eq!(device.run(), Outcome::Returned(None));
    let quantum = device.snapshot().quantum;
    assert_eq!(quantum.amplitudes.len(), after.len(), "{context}: configurations");
    for (basis, amplitude) in &after {
        let actual = quantum.amplitude(&[basis.x, basis.y, basis.out, basis.fault]);
        let scale = 128f64.sqrt();
        let (re, im) = (amplitude.re as f64 / scale, amplitude.im as f64 / scale);
        assert!((actual.re - re).abs() < 1e-12 && (actual.im - im).abs() < 1e-12, "{context}: {basis:?} {actual}");
    }
}

/// What the comparisons of some seeds covered: the number of (input, budget) comparisons, and the
/// outcomes of the inputs' finished runs by fault code (0 for a return).
#[derive(Default)]
struct Tally {
    comparisons: usize,
    outcomes: BTreeMap<u64, usize>,
    constructions: usize,
}

/// Generates the module of `seed` and compares, adding to `tally`.
fn check_seed(seed: u64, thorough: bool, tally: &mut Tally) {
    let mut generator = Generator { rng: ChaCha8Rng::seed_from_u64(seed), helpers: 0, void_helper: false };
    let generated = generator.module();
    let module = bennett_ir::load(&generated.text)
        .unwrap_or_else(|error| panic!("seed {seed}: generated text does not load: {error}\n{}", generated.text));
    let reference = Reference::new(&module, "f", "definite");
    let finishes: Vec<_> = (0..INPUTS).map(|input| reference.finish(&arguments(input), BOUND)).collect();
    let mut budgets = BTreeSet::from([0, BOUND]);
    for finish in &finishes {
        if let Some(t) = finish.ticks {
            budgets.extend([t.saturating_sub(1), t, t + 1]);
        }
    }
    let expected_at = |budget: usize| -> Vec<(u64, u64)> {
        finishes
            .iter()
            .map(|f| match f.ticks {
                Some(t) if t <= budget => f.output,
                _ => (0, Fault::Budget.code()),
            })
            .collect()
    };
    for finish in finishes.iter().filter(|f| f.ticks.is_some()) {
        *tally.outcomes.entry(finish.output.1).or_default() += 1;
    }
    for &budget in &budgets {
        for (input, &want) in expected_at(budget).iter().enumerate() {
            let got = executor_outcome(&module, arguments(input as u64), budget);
            assert_eq!(
                got,
                want,
                "seed {seed}, input {input} {:?}, budget {budget}: executor {got:?}, reference {want:?}\n{}",
                arguments(input as u64),
                generated.text
            );
            tally.comparisons += 1;
        }
    }
    // The whole construction at budget zero and one step beyond the last finish; a thorough sweep adds
    // the first and last finish. Records cost a machine per step, so long runs are left to the sweep.
    let finished: Vec<usize> = finishes.iter().filter_map(|f| f.ticks).collect();
    let mut constructed = BTreeSet::from([0]);
    if let (Some(&low), Some(&high)) = (finished.iter().min(), finished.iter().max()) {
        constructed.insert(if high < CONSTRUCTED { high + 1 } else { low });
        if thorough {
            constructed.extend([low, high]);
        }
    }
    for budget in constructed.into_iter().filter(|&b| b <= CONSTRUCTED) {
        construction(
            &module,
            &reference,
            budget,
            &expected_at(budget),
            &format!("seed {seed}, budget {budget}\n{}", generated.text),
        );
        tally.constructions += 1;
    }
}

fn sweep(seeds: std::ops::Range<u64>, thorough: bool) -> Tally {
    let mut tally = Tally::default();
    for seed in seeds {
        check_seed(seed, thorough, &mut tally);
    }
    println!(
        "{} comparisons, {} constructions, finished outcomes by fault code: {:?}",
        tally.comparisons, tally.constructions, tally.outcomes
    );
    tally
}

#[test]
fn generated_functions_agree_with_the_executor_around_every_finish() {
    let tally = sweep(0..24, false);
    // The generator reaches a return and each fault the families can give; a generator change that
    // stopped reaching one would otherwise leave that family silently untested.
    for fault in [
        0,
        Fault::DivideByZero.code(),
        Fault::OutOfBounds.code(),
        Fault::BadIndirect.code(),
        Fault::StackOverflow.code(),
        Fault::EffectInRegion.code(),
        Fault::ExitInRegion.code(),
        Fault::BadStackRestore.code(),
    ] {
        assert!(tally.outcomes.get(&fault).is_some_and(|&n| n > 0), "no finished run ends with code {fault}");
    }
    assert!(EXPLICIT.iter().any(|f| tally.outcomes.contains_key(&f.code())), "no explicit fault");
    assert!(tally.constructions >= 24, "{}", tally.constructions);
}

/// A longer sweep: `cargo test -p bennett-exec --test compute_copy_generated -- --ignored --nocapture`,
/// with `BENNETT_REFERENCE_SEEDS` seeds (default 1000).
#[test]
#[ignore = "longer; run by hand after a change to the executor or the reference"]
fn generated_functions_long_sweep() {
    let seeds = std::env::var("BENNETT_REFERENCE_SEEDS").ok().and_then(|s| s.parse().ok()).unwrap_or(1000);
    sweep(0..seeds, true);
}
