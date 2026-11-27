//! A released register's `qN` value never names a later allocation, also when the storage reuses the
//! register's column with the same generation: after 2^32 reuses the column's 32-bit generation wraps, and
//! a `qN` value that held the simulator's handle (generation and column) named the new register. Here the
//! wrap is seeded: the column's generation is set back so that the next allocation gets the generation of
//! the released one.

use bennett_ir::{Fault, FuncId};

use crate::executor::{Executor, Options, Outcome};

const PROGRAM: &str = "memory 64
func @main() -> i1 {
entry:
    %a = qalloc 1
    qfree %a
    %b = qalloc 1
    x %a
    %m = measure %b
    ret %m
}";

#[test]
fn a_released_register_does_not_name_a_register_made_after_its_generation_wraps() {
    let module = bennett_ir::load(PROGRAM).expect("valid");
    let options = Options { budget: Some(100), ..Options::default() };
    let mut executor = Executor::new(&module, FuncId(0), &[], options).expect("starts");
    assert_eq!(executor.step(), None, "qalloc");
    let generation = executor.quantum.registers()[0].generation;
    assert_eq!(executor.step(), None, "qfree");
    // The next allocation takes column 0 again and adds one to its generation: seed it so that the sum is
    // the released register's generation, as 2^32 reuses would.
    executor.quantum.seed_generation(0, generation.wrapping_sub(1));
    assert_eq!(executor.step(), None, "qalloc");
    assert_eq!(executor.quantum.registers()[0].generation, generation, "the generation of the released register");
    assert_eq!(executor.run(), Outcome::Faulted(Fault::DanglingRegister));
}
