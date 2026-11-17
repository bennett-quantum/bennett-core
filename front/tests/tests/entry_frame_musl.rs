//! The entry frame with musl startup: a C program linked by `rt/musl/link.py`, run with every stack size
//! around the two edges that matter, where `__bennett_entry`'s own frame stops fitting and where the whole
//! run stops fitting. Below the first the run is `stack_overflow` before its first step; between them it is
//! `stack_overflow` later, its output a prefix of the native output; from the second on it is the native
//! output and status.
use bennett_exec::{Executor, Options, Outcome};
use bennett_front_tests::{native_build, out_dir, root, write_c};
use bennett_ir::{Fault, Module, ValidModule};
use std::process::Command;

const SOURCE: &str = r#"#include <stdio.h>
static int depth(int n) {
  volatile char pad[24];
  pad[0] = (char)n;
  return n ? depth(n - 1) + pad[0] : 0;
}
int main(void) {
  printf("depth %d\n", depth(3));
  return 3;
}
"#;
/// A bound on the steps of one run, far above the program's.
const STEP_BUDGET: u64 = 10_000_000;

fn with_stack(image: &Module, stack: u64) -> ValidModule {
    let mut trial = image.clone();
    trial.memory_size = image.static_end() + stack;
    bennett_ir::validate(trial).expect("a valid memory size")
}

fn run(module: &ValidModule) -> (Option<Outcome>, Outcome, Vec<u8>, u64) {
    let entry = module.function_named("__bennett_entry").expect("entry");
    let options = Options { budget: Some(STEP_BUDGET), ..Options::default() };
    let mut e = Executor::new(module, entry, &[], options).expect("start");
    let at_start = e.outcome();
    let outcome = e.run();
    (at_start, outcome, e.tape().to_vec(), e.stats().steps)
}

#[test]
fn every_stack_size_at_the_entry_and_run_edges_agrees_with_native_and_the_reference() {
    let source = write_c("entry_frame_musl", SOURCE);
    let native = Command::new(native_build(&source, "entry_frame_musl", &[])).output().expect("native");
    assert_eq!(native.status.code(), Some(3));
    let workspace = root().canonicalize().expect("workspace");
    let build = Command::new("python3").arg(workspace.join("rt/musl/build.py")).output().expect("musl build");
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    let file = out_dir().join("entry_frame_musl.bn");
    let link = Command::new("python3")
        .arg(workspace.join("rt/musl/link.py"))
        .arg(&source)
        .arg("-o")
        .arg(&file)
        .output()
        .expect("musl link");
    assert!(link.status.success(), "{}", String::from_utf8_lossy(&link.stderr));
    let image = bennett_ir::load(&std::fs::read_to_string(&file).expect("device text")).expect("valid").into_module();
    let entry_frame = {
        let module = with_stack(&image, 0);
        8 * module.function(module.function_named("__bennett_entry").expect("entry")).slot_count() as u64
    };
    assert!(entry_frame > 0);
    // The smallest stack the whole run fits in, by bisection over a monotone outcome checked below.
    let (mut low, mut high) = (entry_frame, 1 << 16);
    assert_eq!(run(&with_stack(&image, high)).1, Outcome::Returned(Some(3)));
    while high - low > 1 {
        let middle = (low + high) / 2;
        if run(&with_stack(&image, middle)).1 == Outcome::Returned(Some(3)) {
            high = middle;
        } else {
            low = middle;
        }
    }
    let minimum = high;
    let edges: Vec<u64> = (0..=entry_frame + 16).chain(minimum - 16..=minimum + 16).collect();
    for &stack in &edges {
        let (at_start, outcome, tape, steps) = run(&with_stack(&image, stack));
        let context = format!("stack {stack} (entry frame {entry_frame}, minimum {minimum})");
        if stack < entry_frame {
            assert_eq!(at_start, Some(Outcome::Faulted(Fault::StackOverflow)), "{context}");
            assert_eq!((outcome, steps, tape.len()), (Outcome::Faulted(Fault::StackOverflow), 0, 0), "{context}");
        } else if stack < minimum {
            assert_eq!((at_start, outcome), (None, Outcome::Faulted(Fault::StackOverflow)), "{context}");
            // A first instruction that faults counts no step, as with ample memory (`startup.rs`).
            assert!(native.stdout.starts_with(&tape), "{context}: {steps} steps");
        } else {
            assert_eq!((at_start, outcome), (None, Outcome::Returned(Some(3))), "{context}");
            assert_eq!(tape, native.stdout, "{context}");
        }
    }
    println!("entry frame {entry_frame} bytes; whole run from {minimum} bytes of stack");
}
