//! Native glibc is the independent oracle, called through volatile function pointers so native
//! optimization cannot replace its software with a conversion. The device uses LLVM intrinsics.
#![cfg(all(target_arch = "x86_64", target_env = "gnu"))]

use bennett_exec::Outcome;
use bennett_front_tests::{CLANG_FLAGS, CLANG_O2_FLAGS, lower, native_build_with, out_dir, run, write_c};
use std::collections::BTreeSet;
use std::fs::File;
use std::process::{Command, Stdio};

const SOURCE: &str = r#"
#include <stdint.h>
#include <string.h>
#include <math.h>
#ifdef NATIVE
#include <stdio.h>
#include <inttypes.h>
static long (*volatile native_lround)(long double) = lroundl;
static long long (*volatile native_llround)(long double) = llroundl;
#endif
uint64_t probe(uint64_t significand, unsigned exponent, unsigned op) {
    uint64_t words[2] = {significand, exponent};
    long double x;
    memcpy(&x, words, sizeof x);
#ifdef NATIVE
    return op ? (uint64_t)native_llround(x) : (uint64_t)native_lround(x);
#else
    return op ? (uint64_t)__builtin_llroundl(x) : (uint64_t)__builtin_lroundl(x);
#endif
}
#ifdef NATIVE
int main(void) {
    uint64_t s;
    unsigned e, op;
    while (scanf("%" SCNx64 " %x %u", &s, &e, &op) == 3)
        printf("%016" PRIx64 "\n", probe(s, e, op));
    return 0;
}
#endif
"#;

fn cases() -> Vec<(u64, u16, u64)> {
    let mut inputs = BTreeSet::new();
    // Both signs, every software shift regime, subnormals, pseudo-denormals, unnormals,
    // canonical and pseudo infinities/NaNs, half-way values and their immediate neighbors.
    for exponent in [
        0, 1, 2, 0x3ffd, 0x3ffe, 0x3fff, 0x4000, 0x401c, 0x401d, 0x401e, 0x401f, 0x403c, 0x403d, 0x403e, 0x403f,
        0x4040, 0x7ffe, 0x7fff,
    ] {
        for significand in [
            0,
            1,
            0x3fff_ffff_ffff_ffff,
            0x4000_0000_0000_0000,
            0x7fff_ffff_ffff_ffff,
            0x8000_0000_0000_0000,
            0x8000_0000_0000_0001,
            0xbfff_ffff_ffff_ffff,
            0xc000_0000_0000_0000,
            u64::MAX,
        ] {
            inputs.insert((significand, exponent));
        }
    }
    for point in 0..63 {
        let half = 1_u64 << (62 - point);
        for significand in [half - 1, half, half + 1, u64::MAX - half, u64::MAX - half + 1] {
            inputs.insert((significand, 0x3fff + point));
            inputs.insert((significand | (1 << 63), 0x3fff + point));
        }
    }
    inputs
        .into_iter()
        .flat_map(|(s, e)| [0, 0x8000].into_iter().flat_map(move |sign| [0, 1].map(move |op| (s, e | sign, op))))
        .collect()
}

#[test]
fn long_double_integer_rounding_matches_glibc_at_both_optimization_levels() {
    let cases = cases();
    let source = write_c("sites-rounding-independent", SOURCE);
    let input = out_dir().join("sites-rounding-independent-input.txt");
    std::fs::write(&input, cases.iter().map(|(s, e, op)| format!("{s:x} {e:x} {op}\n")).collect::<String>())
        .expect("native input");
    for (opt, flags) in [("-O0", CLANG_FLAGS), ("-O2", CLANG_O2_FLAGS)] {
        let name = format!("sites-rounding-independent{opt}");
        let native = native_build_with(&source, &name, &[opt, "-DNATIVE", "-fno-builtin"], &["-lm"]);
        let output =
            Command::new(native).stdin(Stdio::from(File::open(&input).expect("input"))).output().expect("glibc runs");
        assert!(output.status.success());
        let expected: Vec<u64> = String::from_utf8(output.stdout)
            .expect("hex text")
            .lines()
            .map(|s| u64::from_str_radix(s, 16).expect("hex word"))
            .collect();
        assert_eq!(expected.len(), cases.len());
        let mut flags = flags.to_vec();
        flags.push("-fno-math-errno");
        let module = lower(&source, &name, &flags, &[]);
        for ((s, e, op), want) in cases.iter().zip(expected) {
            assert_eq!(
                run(&module, "probe", &[*s, u64::from(*e), *op], 0),
                Outcome::Returned(Some(u128::from(want))),
                "{opt} sig={s:016x} exp={e:04x} op={op}"
            );
        }
        eprintln!("{opt}: {} exact lroundl/llroundl comparisons with native glibc", cases.len());
    }
}
