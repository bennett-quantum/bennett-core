//! Writes the global-write workloads (`journal_global.c`, with `jg_setup`'s tables as the initial
//! static image) for the independent check in `bennett-exec` (`recompute_independent_tests.rs`,
//! `c_globals_check_independently`). It builds the image its own way: lowered with the same flags,
//! `jg_setup` run once classically, the static bytes kept, the stack left clean.
use bennett_exec::{Executor, Outcome};
use bennett_front_tests::{CLANG_FLAGS, c_dir, entry, lower, options, out_dir};
use bennett_ir::{DataItem, DataSegment};

#[test]
#[ignore = "writes target/front-tests/journal_global_base.bn for the exec crate's ignored global check"]
fn write_global_base() {
    let m = lower(&c_dir().join("journal_global.c"), "journal_global_check", CLANG_FLAGS, &["--entry=jg_setup"]);
    let mut e = Executor::new(&m, entry(&m, "jg_setup"), &[], options(0)).expect("setup");
    assert_eq!(e.run(), Outcome::Returned(None));
    let image = e.snapshot().classical.memory[..m.static_end() as usize].to_vec();
    let mut module = m.into_module();
    module.data = vec![DataSegment { address: 0, items: vec![DataItem::Bytes(image)] }];
    bennett_ir::validate(module.clone()).expect("initialized static image");
    let text = bennett_ir::write(&module);
    std::fs::write(out_dir().join("journal_global_base.bn"), text).expect("write");
}
