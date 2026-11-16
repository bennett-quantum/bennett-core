//! Native oracle, compiled by the same explicit rustc as the bitcode.
#[path = "scalars.rs"]
mod scalars;
use std::io::{self, Write};

fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args.len() == 4 {
        let a = args[2].parse().expect("u8 a");
        let b = args[3].parse().expect("u8 b");
        println!("{}", scalars::ordinary_add(a, b));
        return;
    }
    let mut out = io::BufWriter::new(io::stdout().lock());
    for name in ["arithmetic", "branchy", "counted_loop", "checked", "ordinary_add"] {
        for a in 0..=255u8 {
            let bs = if matches!(name, "branchy" | "counted_loop") { 1 } else { 256 };
            for b in 0..bs {
                // On a checked build, represent a native panic as 256. Probe those
                // cases separately in child processes: panic=abort cannot be caught.
                let value = match name {
                    "arithmetic" => scalars::arithmetic(a, b as u8) as u16,
                    "branchy" => scalars::branchy(a) as u16,
                    "counted_loop" => scalars::counted_loop(a) as u16,
                    "checked" => scalars::checked(a, b as u8) as u16,
                    "ordinary_add" if cfg!(overflow_checks_probe) && a as u16 + b > 255 => 256,
                    "ordinary_add" => scalars::ordinary_add(a, b as u8) as u16,
                    _ => unreachable!(),
                };
                out.write_all(&value.to_le_bytes()).expect("oracle output");
            }
        }
    }
}
