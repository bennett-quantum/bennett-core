//! Reference semantics from DESIGN.md, not the front or executor. Host clang
//! executes each update once per basis. Only a documented coherent permutation
//! (or a restored region with a keep) admits adding complex amplitudes. Otherwise
//! expose immediate measurement probabilities, never invent a garbage policy.
use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;

use bennett_front_tests::{c_dir, native_build, root};
use num_complex::Complex64 as C;

/// The fixture's twelve-byte image (`fixture.h`): four guard bytes, the four
/// cells the bodies write, four guard bytes.
pub const INITIAL_IMAGE: [u8; 12] = [0xa5, 0x5a, 0xc3, 0x3c, 1, 2, 3, 0, 0x96, 0x69, 0xf0, 0x0f];
/// Where the cells lie in the image.
pub const CELLS: std::ops::Range<usize> = 4..8;
pub type Vector = [C; 8];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub bits: usize,
    pub memory: [u8; 12],
}

impl Image {
    /// q, r, y in bits 0 to 2 and the whole image above them: guard bytes are
    /// part of every compared result, so a stray write outside the cells counts.
    pub fn measured(&self) -> u128 {
        self.memory.iter().enumerate().fold(self.bits as u128, |v, (i, b)| v | (u128::from(*b) << (3 + 8 * i)))
    }

    pub fn cells(&self) -> [u8; 4] {
        self.memory[CELLS].try_into().expect("four cells")
    }
}

pub struct Native {
    pub images: Vec<Image>,
}

impl Native {
    pub fn compile(name: &str, level: &str) -> Self {
        Self::compile_with(name, level, &[level])
    }

    /// A native build with `flags` (an optimisation level, sanitizers), named by `tag`.
    pub fn compile_with(name: &str, tag: &str, flags: &[&str]) -> Self {
        let include = format!("-I{}", root().join("front/include").display());
        let mut all = vec!["-DNATIVE", &include];
        all.extend_from_slice(flags);
        let exe =
            native_build(&c_dir().join(format!("fallback_oracle/{name}.c")), &format!("fallback.{name}.{tag}"), &all);
        let images = (0..8)
            .map(|basis| {
                let output = Command::new(&exe).arg(basis.to_string()).output().expect("native basis run");
                assert!(output.status.success(), "{name} basis {basis}: native failed: {output:?}");
                assert!(output.stderr.is_empty(), "{name} basis {basis}: native diagnostics: {output:?}");
                let words: Vec<u8> = String::from_utf8(output.stdout)
                    .expect("native text")
                    .split_whitespace()
                    .map(|word| word.parse().expect("native byte"))
                    .collect();
                assert_eq!(words.len(), 3 + INITIAL_IMAGE.len(), "{name} basis {basis}");
                assert!(words[..3].iter().all(|&b| b <= 1));
                Image {
                    bits: usize::from(words[0] | words[1] << 1 | words[2] << 2),
                    memory: words[3..].try_into().expect("the whole image"),
                }
            })
            .collect();
        Self { images }
    }

    fn permutation(&self) -> Vec<usize> {
        assert!(self.images.iter().all(|image| image.memory == INITIAL_IMAGE), "coherent map must restore memory");
        let map: Vec<_> = self.images.iter().map(|image| image.bits).collect();
        assert_eq!(map.iter().collect::<BTreeSet<_>>().len(), 8, "coherent update must be injective");
        map
    }

    pub fn coherent(&self, input: Vector) -> Vector {
        scatter(input, &self.permutation())
    }

    /// U† Z_y U followed by H_q. The device fixture repeats U, so require U=U†.
    pub fn phase_round_trip(&self, input: Vector) -> Vector {
        let map = self.permutation();
        assert!((0..8).all(|b| map[map[b]] == b), "fixture repeats an involution");
        let forward = scatter(input, &map);
        let marked = std::array::from_fn(|b| if b & 4 == 0 { forward[b] } else { -forward[b] });
        let inverse = (0..8).map(|b| map.iter().position(|&v| v == b).expect("bijection")).collect::<Vec<_>>();
        hadamard_q(scatter(marked, &inverse))
    }

    pub fn measured(&self, input: &Vector) -> BTreeMap<u128, f64> {
        let mut probabilities = BTreeMap::new();
        for (image, amplitude) in self.images.iter().zip(input) {
            *probabilities.entry(image.measured()).or_default() += amplitude.norm_sqr();
        }
        probabilities.retain(|_, p| *p > 1e-14);
        probabilities
    }

    /// The ordinary region driver returns bit 1 of cell 0, then everything it
    /// did is un-run. Native memory is NOT kept in the coherent output.
    pub fn kept(&self) -> [usize; 4] {
        std::array::from_fn(|b| usize::from((self.images[b].cells()[0] >> 1) & 1))
    }

    /// The whole image after the region on each (q, r), guard bytes included,
    /// little-endian, as the region driver returns it; y is 0 there.
    pub fn region_image(&self) -> [u128; 4] {
        std::array::from_fn(|b| self.images[b].memory.iter().rev().fold(0, |v, byte| v << 8 | u128::from(*byte)))
    }
}

fn scatter(input: Vector, image: &[usize]) -> Vector {
    let mut result = [C::new(0.0, 0.0); 8];
    for (b, amplitude) in input.into_iter().enumerate() {
        result[image[b]] += amplitude;
    }
    result
}

pub fn hadamard_q(input: Vector) -> Vector {
    std::array::from_fn(|b| {
        (input[b & !1] + if b & 1 == 0 { input[b | 1] } else { -input[b | 1] }) * std::f64::consts::FRAC_1_SQRT_2
    })
}

/// Preparation is tabulated analytically, separately from the dense gate oracle:
/// columns 0..7, the complex product state 8 and the real product state 9 of
/// `driver.h` (Ry(theta)|0> = cos(theta/2)|0> + sin(theta/2)|1>).
pub fn prepared(basis: usize) -> Vector {
    if basis < 8 {
        return std::array::from_fn(|b| C::new(f64::from(b == basis), 0.0));
    }
    if basis == 9 {
        let half = |theta: f64, bit: bool| if bit { (theta / 2.0).sin() } else { (theta / 2.0).cos() };
        return std::array::from_fn(|b| {
            C::new(half(0.7, b & 1 != 0) * half(1.3, b & 2 != 0) * half(2.2, b & 4 != 0), 0.0)
        });
    }
    assert_eq!(basis, 8);
    std::array::from_fn(|b| {
        let q = if b & 1 == 0 { 0.35_f64.cos() } else { 0.35_f64.sin() };
        let r = if b & 2 == 0 { C::new(1.0, 0.0) } else { C::new(0.0, 1.0) };
        let y = if b & 4 == 0 { C::new(1.0, 0.0) } else { C::from_polar(1.0, std::f64::consts::FRAC_PI_4) };
        q * r * y / 2.0
    })
}

/// S_y† X_y^f S_y after an ordinary marked C region. No read is changed:
/// q/r are retained, y predates the mark, and all region memory is restored.
pub fn unrun_keep(input: Vector, kept: [usize; 4]) -> Vector {
    let mut output = [C::new(0.0, 0.0); 8];
    for (b, amplitude) in input.into_iter().enumerate() {
        let target = b ^ (kept[b & 3] << 2);
        let forward = if b & 4 == 0 { C::new(1.0, 0.0) } else { C::new(0.0, 1.0) };
        let inverse = if target & 4 == 0 { C::new(1.0, 0.0) } else { C::new(0.0, -1.0) };
        output[target] += amplitude * forward * inverse;
    }
    output
}
