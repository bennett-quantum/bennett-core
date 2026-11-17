//! Independent exact device addresses, including each configuration's frames in a region.
use bennett_exec::{Executor, Options, Outcome};

fn options() -> Options {
    Options { reversible: true, budget: Some(10000), ..Options::default() }
}

#[test]
fn generated_frame_levels_direct_and_indirect_have_exact_addresses_and_un_run() {
    let mut count = 0;
    for depth in 0_u32..=8 {
        for level in 0..=depth + 2 {
            let mut text = format!("memory 512\nfunc @f0()->i64 {{\nentry:\n%r = retaddr {level}\nret %r\n}}\n");
            for i in 1..=depth + 1 {
                // In each frame: one instruction + branch in the first block; one instruction,
                // function-address instruction and indirect call in the second. Direct calls have
                // one fewer instruction. Counts are written here independently of code_address_after.
                let call = if i % 2 == 0 {
                    format!("%fp = faddr @f{}\n%r = call_indirect fn() -> i64 %fp()", i - 1)
                } else {
                    format!("%r = call @f{}()", i - 1)
                };
                text += &format!(
                    "func @f{i}()->i64 {{\nentry:\n%pad = add i8 1, 2\nbr next\nnext:\n%pad2 = add i8 3, 4\n{call}\nret %r\n}}\n"
                );
            }
            let p = bennett_ir::load(&text).expect("valid generated frame program");
            let name = format!("f{}", depth + 1);
            let mut e = Executor::new(&p, p.function_named(&name).expect("entry"), &[], options()).expect("start");
            let start = e.snapshot();
            let expected = if level == depth + 1 {
                0xffff_ffff_0000_0000_u64
            } else if level > depth + 1 {
                0
            } else {
                let caller = level + 1;
                ((u64::from(caller) + 1) << 32) | if caller % 2 == 0 { 5 } else { 4 }
            };
            assert_eq!(e.run(), Outcome::Returned(Some(expected.into())), "depth {depth}, level {level}");
            e.reverse().expect("reverse");
            assert_eq!(e.snapshot().classical, start.classical);
            count += 1;
        }
    }
    assert_eq!(count, 63);
    println!("{count} generated direct/indirect frame-level programs, independent encoding, reference and reverse");
}

const REGION: &str = "memory 512
func @leaf()->i64 {
entry:
%r = retaddr 0
%up = retaddr 3
%beyond = retaddr 4
%entry = icmp eq i64 %up, -4294967296
%zero = icmp eq i64 %beyond, 0
%good = and i1 %entry, %zero
%out = select i64 %good, %r, 0
ret %out
}
func @work(i1 %x)->i64 {
entry:
br %x, left, right
left:
%a = call @leaf()
ret %a
right:
%fp = faddr @leaf
%b = call_indirect fn() -> i64 %fp()
ret %b
}
func @inner() {
entry:
%q = qalloc 1
%out = qalloc 64
%fault = qalloc 5
h %q
apply @work(%q) -> %out, %fault budget 100
ret
}
func @main() {
entry:
call @inner()
ret
}
";

#[test]
fn each_region_branch_reads_its_own_call_site_and_the_frames_below_it() {
    let p = bennett_ir::load(REGION).expect("valid region program");
    let mut e = Executor::new(&p, p.function_named("main").expect("main"), &[], options()).expect("start");
    let start = e.snapshot();
    assert_eq!(e.run(), Outcome::Returned(None));
    // @work is function index 1; block-order positions: br=0, direct call=1, ret=2,
    // faddr=3, indirect call=4, ret=5. The address is just after the corresponding call.
    let a = (2_u64 << 32) | 2;
    let b = (2_u64 << 32) | 5;
    let s = e.snapshot();
    for (config, amp) in &s.quantum.amplitudes {
        assert_eq!(config[1], if config[0] == 1 { a } else { b });
        assert_eq!(config[2], 0);
        assert!((amp.re - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-12);
    }
    assert_eq!(s.quantum.amplitudes.len(), 2);
    e.reverse().expect("reverse region");
    assert_eq!(e.snapshot().classical, start.classical);
    assert!(e.snapshot().quantum.distance(&start.quantum).is_some_and(|d| d < 1e-12));
}
