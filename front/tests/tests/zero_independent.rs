//! Independent native and explicit-byte comparisons for zero-run emission.

use bennett_exec::{Executor, Outcome};
use bennett_front_tests::{CLANG_FLAGS, CLANG_O2_FLAGS, entry, lower_text, native_exit_status, options, write_c};
use bennett_ir::{DataItem, ValidModule};

fn compare_with_explicit_bytes(module: &ValidModule) {
    let mut reference = (**module).clone();
    for segment in &mut reference.data {
        for item in &mut segment.items {
            if let DataItem::Zero(n) = item {
                *item = DataItem::Bytes(vec![0; *n as usize]);
            }
        }
    }
    let reference = bennett_ir::load(&bennett_ir::write(&reference)).expect("explicit bytes load");
    assert_eq!(module.memory_size, reference.memory_size);
    assert_eq!(module.static_end(), reference.static_end());
    let run_options = bennett_exec::Options { budget: Some(100_000_000), ..options(0) };
    let mut compact = Executor::new(module, entry(module, "main"), &[], run_options).expect("starts");
    let mut spelled = Executor::new(&reference, entry(&reference, "main"), &[], run_options).expect("starts");
    assert_eq!(compact.snapshot(), spelled.snapshot(), "entire initial state");
    assert_eq!(compact.run(), Outcome::Returned(Some(0)), "native program returns zero");
    assert_eq!(compact.snapshot().classical.outcome, Some(spelled.run()));
    assert_eq!(compact.snapshot(), spelled.snapshot(), "entire final state, including step count");
}

fn global_items<'a>(text: &str, module: &'a ValidModule, name: &str) -> &'a [DataItem] {
    let mut lines = text.lines().skip_while(|line| *line != format!("; @{name}"));
    let line = lines.nth(1).unwrap_or_else(|| panic!("missing @{name}"));
    let address: u64 =
        line.strip_prefix("data ").expect("data").split(' ').next().expect("address").parse().expect("u64");
    &module.data.iter().find(|segment| segment.address == address).expect("global segment").items
}

#[test]
fn every_run_length_through_33_at_each_global_boundary_agrees_with_native() {
    let mut source = String::new();
    let mut checks = String::new();
    let mut globals = Vec::new();
    for n in 0..=33 {
        for shape in ["head", "tail", "middle", "whole"] {
            if n == 0 && shape == "whole" {
                continue;
            }
            let mut bytes = vec![0; n];
            if shape == "tail" || shape == "middle" {
                bytes.insert(0, 61);
            }
            if shape == "head" || shape == "middle" {
                bytes.push(123);
            }
            let name = format!("run_{shape}_{n}");
            let init = bytes.iter().map(u8::to_string).collect::<Vec<_>>().join(",");
            source.push_str(&format!(
                "static volatile unsigned char {name}[{}] __attribute__((aligned(1))) = {{{init}}};\n",
                bytes.len()
            ));
            for (i, byte) in bytes.iter().enumerate() {
                checks.push_str(&format!("if ({name}[{i}] != {byte}) return 1;\n"));
            }
            globals.push((name, n, bytes));
        }
    }
    source.push_str(&format!("int main(void) {{\n{checks}return 0;\n}}\n"));
    let source = write_c("independent_zero_matrix", &source);
    assert_eq!(native_exit_status(&source, "independent_zero_matrix"), 0);
    for (suffix, flags) in [("o0", CLANG_FLAGS), ("o2", CLANG_O2_FLAGS)] {
        let text = lower_text(&source, &format!("independent_zero_matrix.{suffix}"), flags, &[]).expect("lowers");
        let module = bennett_ir::load(&text).expect("loads");
        for (name, n, expected) in &globals {
            let items = global_items(&text, &module, name);
            let zeros: Vec<_> =
                items.iter().filter_map(|i| if let DataItem::Zero(n) = i { Some(*n) } else { None }).collect();
            assert_eq!(zeros, if *n >= 16 { vec![*n as u64] } else { vec![] }, "@{name}");
            let segment = bennett_ir::DataSegment { address: 0, items: items.to_vec() };
            assert_eq!(&segment.encode(), expected, "@{name}");
        }
        assert_eq!(bennett_ir::read(&bennett_ir::write(&module)).as_ref(), Ok(&*module));
        compare_with_explicit_bytes(&module);
    }
    assert_eq!(globals.len(), 135);
}

#[test]
fn separate_globals_alignment_and_function_relocations_preserve_zero_runs() {
    let source = write_c(
        "independent_zero_layout",
        r#"
static int first(void) { return 3; }
static int second(void) { return 5; }
static volatile unsigned char left[9] __attribute__((aligned(1))) = {11};
static volatile unsigned char right[9] __attribute__((aligned(1))) = {[8] = 13};
static volatile unsigned char aligned[33] __attribute__((aligned(256))) = {[16] = 17};
static volatile struct __attribute__((packed)) {
  unsigned char before[15]; int (*f)(void);
  unsigned char between[16]; int (*g)(void);
  unsigned char after[17]; unsigned char end;
} relocations = {{0}, first, {0}, second, {0}, 19};
static int check(volatile unsigned char *p, unsigned n) {
  for (unsigned i = 0; i < n; ++i) if (p[i]) return 0;
  return 1;
}
int main(void) {
  if (left[0] != 11 || !check(left + 1, 8)) return 1;
  if (!check(right, 8) || right[8] != 13) return 2;
  if ((unsigned long)aligned % 256 || !check(aligned, 16) || aligned[16] != 17 || !check(aligned + 17, 16)) return 3;
  if (!check(relocations.before, 15) || !check(relocations.between, 16) || !check(relocations.after, 17)) return 4;
  if (relocations.f() != 3 || relocations.g() != 5 || relocations.end != 19) return 5;
  return 0;
}
"#,
    );
    assert_eq!(native_exit_status(&source, "independent_zero_layout"), 0);
    for (suffix, flags) in [("o0", CLANG_FLAGS), ("o2", CLANG_O2_FLAGS)] {
        let text = lower_text(&source, &format!("independent_zero_layout.{suffix}"), flags, &[]).expect("lowers");
        let module = bennett_ir::load(&text).expect("loads");
        // Two neighbouring eight-byte runs remain separate; alignment gaps contain no data item.
        for name in ["left", "right"] {
            assert!(global_items(&text, &module, name).iter().all(|item| matches!(item, DataItem::Bytes(_))));
        }
        assert!(
            matches!(global_items(&text, &module, "aligned"), [DataItem::Zero(16), DataItem::Bytes(b), DataItem::Zero(16)] if b == &[17])
        );
        assert!(
            matches!(global_items(&text, &module, "relocations"), [DataItem::Bytes(b), DataItem::FuncAddr { .. }, DataItem::Zero(16), DataItem::FuncAddr { .. }, DataItem::Zero(17), DataItem::Bytes(end)] if b == &[0; 15] && end == &[19])
        );
        compare_with_explicit_bytes(&module);
    }
}

#[test]
fn a_megabyte_zero_global_keeps_its_image_and_stack_floor() {
    let source = write_c(
        "independent_zero_large",
        r#"
static volatile unsigned char all_zero[1048576];
static volatile unsigned char sparse[1048579] = {23, [524289] = 29, [1048578] = 31};
int main(void) {
  for (unsigned i = 0; i < sizeof all_zero; ++i) if (all_zero[i]) return 1;
  for (unsigned i = 0; i < sizeof sparse; ++i) {
    unsigned char want = i == 0 ? 23 : i == 524289 ? 29 : i == 1048578 ? 31 : 0;
    if (sparse[i] != want) return 2;
  }
  all_zero[1048575] = 37;
  return all_zero[1048575] == 37 ? 0 : 3;
}
"#,
    );
    assert_eq!(native_exit_status(&source, "independent_zero_large"), 0);
    let text = lower_text(&source, "independent_zero_large", CLANG_FLAGS, &[]).expect("lowers");
    let module = bennett_ir::load(&text).expect("loads");
    assert_eq!(global_items(&text, &module, "all_zero"), [DataItem::Zero(1048576)]);
    let zeros: Vec<_> = global_items(&text, &module, "sparse")
        .iter()
        .filter_map(|item| if let DataItem::Zero(n) = item { Some(*n) } else { None })
        .collect();
    assert_eq!(zeros, [524288, 524288]);
    compare_with_explicit_bytes(&module);
}
