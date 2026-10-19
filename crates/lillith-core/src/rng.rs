//! Tiny seedable PRNG (xorshift64*), so idle timing is deterministic in tests
//! without pulling in `rand`.

/// xorshift64* generator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rng(u64);

impl Rng {
    /// Seeded generator (a zero seed is remapped; xorshift can't use 0).
    pub fn new(seed: u64) -> Self {
        Self(if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        })
    }

    /// Seed from the system clock (for the real app).
    pub fn from_time() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1);
        Self::new(nanos)
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in `[0, 1)`.
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Uniform in `[lo, hi]` (order-insensitive).
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        let (a, b) = if lo <= hi { (lo, hi) } else { (hi, lo) };
        a + (b - a) * self.next_f32()
    }

    /// Uniform index in `0..n` (`n > 0`).
    pub fn index(&mut self, n: usize) -> usize {
        (self.next_u64() % n.max(1) as u64) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_in_range() {
        let mut a = Rng::new(7);
        let mut b = Rng::new(7);
        for _ in 0..1000 {
            let x = a.range(3.5, 4.5);
            assert_eq!(x, b.range(3.5, 4.5));
            assert!((3.5..=4.5).contains(&x));
            assert!(a.index(3) < 3);
            b.index(3);
        }
    }
}
