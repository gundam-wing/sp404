//! Tiny deterministic generator. The same seed always yields the same stream.

#[derive(Debug, Clone)]
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        // SplitMix64. A zero seed still produces a non-zero state.
        let mut state = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        state = (state ^ (state >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        state = (state ^ (state >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        state ^= state >> 31;
        if state == 0 {
            state = 0xA5A5_A5A5_A5A5_A5A5;
        }
        Self { state }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    /// Uniform in `0..n`. `n == 0` returns 0.
    pub fn index(&mut self, n: usize) -> usize {
        if n == 0 {
            return 0;
        }
        (self.next_u64() as usize) % n
    }

    /// Uniform in `[0, 1)`.
    pub fn unit(&mut self) -> f64 {
        let mantissa = self.next_u64() >> 11;
        mantissa as f64 / (1u64 << 53) as f64
    }
}
