//! Native glibc is the decided reference for copied sigjmp_bufs, including where musl differs.
//! Build bitcode and native musl as documented in rt/musl/README.md, then:
//! cargo test -p bennett-front-tests --test sigjmp_copy copied_sigjmp_buffers -- --ignored --nocapture
use std::process::Command;

use bennett_front_tests::{front, root};

#[test]
#[ignore = "heavy: needs separately built bitcode and native musl; see rt/musl/README.md, copied jump buffers"]
fn copied_sigjmp_buffers_match_glibc_at_o0_and_o2() {
    compare(&[]);
}

#[test]
#[ignore = "defect: a partial sigjmp_buf copy overwrites the device continuation but preserves glibc’s"]
fn a_partial_record_copy_preserves_the_glibc_continuation() {
    compare(&["--partial-record"]);
}

fn compare(arguments: &[&str]) {
    let result = Command::new("python3")
        .arg(root().join("rt/musl/test-sigjmp.py"))
        .arg("--front")
        .arg(front())
        .args(arguments)
        .output()
        .expect("runs the three-way comparison");
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    eprint!("{}", String::from_utf8_lossy(&result.stdout));
}
