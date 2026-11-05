//! Python 3.9's `random.Random`, bit for bit: MT19937, `init_by_array` seeding from an int or a str
//! (version 2: str -> utf-8 bytes + their SHA-512, read as one big-endian int), `random()`,
//! `getrandbits`, `_randbelow` (getrandbits with rejection), `randrange`, `randint`, `choice` and
//! `shuffle`. Kern's property tests, mutation scores, assumed-handler values, the host's SimNet and
//! its audit sampling all draw from it, so the Rust toolchain's numbers match the Python oracle's.
use sha2::{Digest, Sha512};

const N: usize = 624;
const M: usize = 397;

#[derive(Clone)]
pub struct Random {
    mt: [u32; N],
    i: usize,
}

/// A seed: Python seeds `Random(int)` from all the bits of |n| and `Random(str)` from the string.
pub enum Seed<'a> {
    Int(i128),
    Str(&'a str),
}

impl Random {
    pub fn new(seed: Seed) -> Random {
        let key = match seed {
            Seed::Int(n) => words_of(&n.unsigned_abs().to_be_bytes()),
            Seed::Str(s) => {
                let mut b = s.as_bytes().to_vec();
                b.extend_from_slice(&Sha512::digest(s.as_bytes()));
                words_of(&b)
            }
        };
        let mut r = Random { mt: [0; N], i: N };
        r.init_by_array(&key);
        r
    }

    pub fn from_int(n: i128) -> Random {
        Random::new(Seed::Int(n))
    }

    pub fn from_str(s: &str) -> Random {
        Random::new(Seed::Str(s))
    }

    fn init_genrand(&mut self, s: u32) {
        self.mt[0] = s;
        for i in 1..N {
            let p = self.mt[i - 1];
            self.mt[i] = 1812433253u32.wrapping_mul(p ^ (p >> 30)).wrapping_add(i as u32);
        }
        self.i = N;
    }

    fn init_by_array(&mut self, key: &[u32]) {
        self.init_genrand(19650218);
        let (mut i, mut j) = (1usize, 0usize);
        for _ in 0..N.max(key.len()) {
            let p = self.mt[i - 1];
            self.mt[i] = (self.mt[i] ^ (p ^ (p >> 30)).wrapping_mul(1664525))
                .wrapping_add(key[j])
                .wrapping_add(j as u32);
            i += 1;
            j += 1;
            if i >= N {
                self.mt[0] = self.mt[N - 1];
                i = 1;
            }
            if j >= key.len() {
                j = 0;
            }
        }
        for _ in 0..N - 1 {
            let p = self.mt[i - 1];
            self.mt[i] = (self.mt[i] ^ (p ^ (p >> 30)).wrapping_mul(1566083941)).wrapping_sub(i as u32);
            i += 1;
            if i >= N {
                self.mt[0] = self.mt[N - 1];
                i = 1;
            }
        }
        self.mt[0] = 0x8000_0000;
    }

    fn genrand_u32(&mut self) -> u32 {
        if self.i >= N {
            for k in 0..N {
                let y = (self.mt[k] & 0x8000_0000) | (self.mt[(k + 1) % N] & 0x7fff_ffff);
                let mut v = self.mt[(k + M) % N] ^ (y >> 1);
                if y & 1 != 0 {
                    v ^= 0x9908_b0df;
                }
                self.mt[k] = v;
            }
            self.i = 0;
        }
        let mut y = self.mt[self.i];
        self.i += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^ (y >> 18)
    }

    /// `random.random()`: a float in [0, 1) from 53 random bits.
    pub fn random(&mut self) -> f64 {
        let a = (self.genrand_u32() >> 5) as f64;
        let b = (self.genrand_u32() >> 6) as f64;
        (a * 67108864.0 + b) * (1.0 / 9007199254740992.0)
    }

    /// `random.getrandbits(k)` for k <= 128: 32-bit words, least significant first.
    pub fn getrandbits(&mut self, k: u32) -> u128 {
        assert!(k <= 128, "getrandbits: k > 128 is not needed by Kern");
        if k == 0 {
            return 0;
        }
        if k <= 32 {
            return (self.genrand_u32() >> (32 - k)) as u128;
        }
        let (mut out, mut left, mut shift) = (0u128, k, 0u32);
        while left > 0 {
            let mut r = self.genrand_u32();
            if left < 32 {
                r >>= 32 - left;
            }
            out |= (r as u128) << shift;
            shift += 32;
            left = left.saturating_sub(32);
        }
        out
    }

    /// `_randbelow(n)`: uniform in [0, n), by rejection; 0 when n == 0.
    pub fn randbelow(&mut self, n: u128) -> u128 {
        if n == 0 {
            return 0;
        }
        let k = 128 - n.leading_zeros();
        loop {
            let r = self.getrandbits(k);
            if r < n {
                return r;
            }
        }
    }

    /// `randrange(start, stop)`; panics on an empty range, as Python raises ValueError.
    pub fn randrange(&mut self, start: i64, stop: i64) -> i64 {
        let width = stop as i128 - start as i128;
        assert!(width > 0, "empty range for randrange() ({start}, {stop}, {width})");
        (start as i128 + self.randbelow(width as u128) as i128) as i64
    }

    /// `randint(a, b)`: both ends included.
    pub fn randint(&mut self, a: i64, b: i64) -> i64 {
        let width = b as i128 + 1 - a as i128;
        assert!(width > 0, "empty range for randint({a}, {b})");
        (a as i128 + self.randbelow(width as u128) as i128) as i64
    }

    /// `choice(seq)`; panics on an empty sequence, as Python raises IndexError.
    pub fn choice<'a, T>(&mut self, seq: &'a [T]) -> &'a T {
        assert!(!seq.is_empty(), "Cannot choose from an empty sequence");
        &seq[self.randbelow(seq.len() as u128) as usize]
    }

    /// `sample(population, k)`: k distinct elements in selection order, by CPython 3.9's algorithm (a pool for small
    /// populations, rejection against a set otherwise). Panics if k > len, as Python raises ValueError.
    pub fn sample<T: Clone>(&mut self, population: &[T], k: usize) -> Vec<T> {
        let n = population.len();
        assert!(k <= n, "Sample larger than population or is negative");
        let mut setsize = 21usize;
        if k > 5 {
            let e = ((k * 3) as f64).log(4.0).ceil() as u32;
            setsize += 4usize.pow(e);
        }
        let mut out = Vec::with_capacity(k);
        if n <= setsize {
            let mut pool = population.to_vec();
            for i in 0..k {
                let j = self.randbelow((n - i) as u128) as usize;
                out.push(pool[j].clone());
                pool[j] = pool[n - i - 1].clone();
            }
        } else {
            let mut selected = std::collections::BTreeSet::new();
            for _ in 0..k {
                let mut j = self.randbelow(n as u128) as usize;
                while selected.contains(&j) {
                    j = self.randbelow(n as u128) as usize;
                }
                selected.insert(j);
                out.push(population[j].clone());
            }
        }
        out
    }

    /// `shuffle(x)` in place (Fisher-Yates from the end, as CPython).
    pub fn shuffle<T>(&mut self, x: &mut [T]) {
        for i in (1..x.len()).rev() {
            let j = self.randbelow(i as u128 + 1) as usize;
            x.swap(i, j);
        }
    }
}

/// Big-endian bytes of a non-negative int -> its 32-bit words, least significant first (CPython's
/// `init_by_array` key); at least one word, so 0 seeds as [0].
fn words_of(be: &[u8]) -> Vec<u32> {
    let mut words = vec![];
    let mut end = be.len();
    while end > 0 {
        let start = end.saturating_sub(4);
        let mut w = 0u32;
        for &b in &be[start..end] {
            w = (w << 8) | b as u32;
        }
        words.push(w);
        end = start;
    }
    while words.len() > 1 && *words.last().unwrap() == 0 {
        words.pop();
    }
    if words.is_empty() {
        words.push(0);
    }
    words
}
