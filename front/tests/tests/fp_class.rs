//! `llvm.is.fpclass` on long double against clang 18's native x86-64 code: every one of the 1,024 masks, on
//! encodings of every class of both signs (the x87 encodings no arithmetic produces included), in a function
//! that is `strictfp` and in one that is not (the back end compares for some masks only in the second).
//! The input is LLVM IR, so that each mask reaches the back end as written; the encodings are integers in
//! memory, because LLVM's constant folder would rewrite a noncanonical `x86_fp80` constant.
#![cfg(target_arch = "x86_64")]

use std::fmt::Write;
use std::process::Command;

use bennett_exec::Outcome;
use bennett_front_tests::{lower, native_build_with, out_dir, run, write_c};

/// Exponent words (sign clear) and significands: zero, subnormals, normals, infinity, NaNs, then the
/// pseudo-denormals, unnormals, unnormal zeros, pseudo-infinity and pseudo-NaNs.
const ENCODINGS: [(u16, u64); 24] = [
    (0, 0),
    (0, 1),
    (0, 0x4000_0000_0000_0000),
    (0, 0x7fff_ffff_ffff_ffff),
    (1, 0x8000_0000_0000_0000),
    (0x3fff, 0x8000_0000_0000_0000),
    (0x7ffe, 0xffff_ffff_ffff_ffff),
    (0x7fff, 0x8000_0000_0000_0000),
    (0x7fff, 0xc000_0000_0000_0000),
    (0x7fff, 0x8000_0000_0000_0001),
    (0x7fff, 0xffff_ffff_ffff_ffff),
    (0, 0x8000_0000_0000_0000),
    (0, 0x8000_0000_0000_0001),
    (0, 0xffff_ffff_ffff_ffff),
    (1, 1),
    (0x3fff, 0x4000_0000_0000_0000),
    (0x7ffe, 0x7fff_ffff_ffff_ffff),
    (1, 0),
    (0x3fff, 0),
    (0x7ffe, 0),
    (0x7fff, 0),
    (0x7fff, 0x4000_0000_0000_0000),
    (0x7fff, 1),
    (0x7fff, 0x7fff_ffff_ffff_ffff),
];

const GROUPS: u64 = 1024 / 64;

/// `@result(i)`: for encoding `i / 32`, the 64 masks of group `i / 2 % 16` as the bits of an `i64`, from the
/// function without `strictfp` for even `i` and from the `strictfp` one for odd `i`.
fn module(encodings: &[(u16, u64)]) -> String {
    let words: Vec<String> =
        encodings.iter().map(|&(e, m)| format!("i128 {}", (u128::from(e) << 64) | u128::from(m))).collect();
    let n = encodings.len();
    let mut ir = format!("@enc = global [{n} x i128] [{}], align 16\n", words.join(", "));
    writeln!(ir, "define i64 @results() {{\n  ret i64 {}\n}}", n as u64 * GROUPS * 2).unwrap();
    for (name, attribute) in [("loose", ""), ("strict", " strictfp")] {
        writeln!(ir, "define i64 @{name}(i64 %i){attribute} {{\nentry:").unwrap();
        writeln!(ir, "  %e = udiv i64 %i, {GROUPS}\n  %g = urem i64 %i, {GROUPS}").unwrap();
        writeln!(ir, "  %p = getelementptr [{n} x i128], ptr @enc, i64 0, i64 %e").unwrap();
        writeln!(ir, "  %x = load volatile x86_fp80, ptr %p").unwrap();
        let cases: Vec<String> = (1..GROUPS).map(|g| format!("i64 {g}, label %g{g}")).collect();
        writeln!(ir, "  switch i64 %g, label %g0 [{}]", cases.join(" ")).unwrap();
        for g in 0..GROUPS {
            writeln!(ir, "g{g}:").unwrap();
            let mut sum = "0".to_string();
            for k in 0..64 {
                let m = g * 64 + k;
                writeln!(ir, "  %c{m} = call i1 @llvm.is.fpclass.f80(x86_fp80 %x, i32 {m}){attribute}").unwrap();
                writeln!(ir, "  %z{m} = zext i1 %c{m} to i64\n  %s{m} = shl i64 %z{m}, {k}").unwrap();
                writeln!(ir, "  %a{m} = or i64 {sum}, %s{m}").unwrap();
                sum = format!("%a{m}");
            }
            writeln!(ir, "  ret i64 {sum}").unwrap();
        }
        ir.push_str("}\n");
    }
    ir.push_str(
        "define i64 @result(i64 %i) {\n  %h = udiv i64 %i, 2\n  %r = urem i64 %i, 2\n  %b = icmp eq i64 %r, 0\n\
         \x20 br i1 %b, label %l, label %s\nl:\n  %x = call i64 @loose(i64 %h)\n  ret i64 %x\n\
         s:\n  %y = call i64 @strict(i64 %h)\n  ret i64 %y\n}\ndeclare i1 @llvm.is.fpclass.f80(x86_fp80, i32)\n",
    );
    ir
}

#[test]
fn every_class_mask_on_every_long_double_encoding_agrees_with_native() {
    let encodings: Vec<(u16, u64)> =
        ENCODINGS.iter().copied().chain(ENCODINGS.iter().map(|&(e, m)| (e | 0x8000, m))).collect();
    let source = out_dir().join("fp-class.ll");
    std::fs::write(&source, module(&encodings)).expect("write the IR");
    let driver = write_c(
        "fp-class-driver",
        "#include <stdio.h>\nlong long results(void);\nunsigned long long result(long long);\n\
         int main(void) { for (long long i = 0; i < results(); i++) printf(\"%016llx\\n\", result(i)); return 0; }\n",
    );
    let native = native_build_with(&source, "fp-class", &["-O0", "-Wno-override-module"], &[driver.to_str().unwrap()]);
    let output = Command::new(native).output().expect("native program runs");
    assert!(output.status.success());
    let expected: Vec<u64> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| u64::from_str_radix(line, 16).expect("native bits"))
        .collect();
    assert_eq!(expected.len() as u64, encodings.len() as u64 * GROUPS * 2);
    let module = lower(&source, "fp-class", &["-O0", "-Wno-override-module"], &[]);
    let mut differ = Vec::new();
    for (i, &native) in expected.iter().enumerate() {
        let Outcome::Returned(Some(device)) = run(&module, "result", &[i as u64], 0) else {
            panic!("result({i}) did not return");
        };
        let (h, strict) = (i / 2, i % 2 == 1);
        let ((e, m), group) = (encodings[h / GROUPS as usize], h as u64 % GROUPS);
        let device = device as u64;
        for k in (0..64).filter(|k| (device ^ native) >> k & 1 == 1) {
            differ.push(format!(
                "{e:04x} {m:016x} mask {} strictfp {strict}: native {}",
                group * 64 + k,
                native >> k & 1
            ));
        }
    }
    assert!(differ.is_empty(), "{} differ:\n{}", differ.len(), differ.join("\n"));
}
