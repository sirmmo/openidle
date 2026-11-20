//! Big numbers for idle games: `m × 2^e`, with `1 ≤ |m| < 2` and a 64-bit exponent, so amounts
//! can grow far past `1e308`.
//!
//! Every operation uses only IEEE-754 addition, subtraction, multiplication, division, square
//! root and floor, which are exact to the last bit on every machine and in WebAssembly, and
//! scaling by powers of two, which is exact. Within the range of a double the results are those of
//! plain double arithmetic. Logarithms and powers are computed here from those operations, not by
//! the platform's maths library, so the same game gives the same numbers in a browser and on a
//! server, bit for bit.
//!
//! Written out (saves, views), a number in the range of a double is its shortest exact decimal
//! (`1024`, `1.5e300`); beyond, the exact binary form `<m>p<e>` (`1.25p2000` is 1.25 × 2^2000).

use std::cmp::Ordering;
use std::fmt;
use std::ops::{Add, Div, Mul, Neg, Sub};
use std::str::FromStr;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A number `m × 2^e`. Zero is `m = 0, e = 0`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Num {
    m: f64,
    e: i64,
}

/// Exponents beyond this saturate (a number this large is "infinite" for any game).
const MAX_E: i64 = 1_000_000_000_000_000;
const ALMOST_2: f64 = 1.999_999_999_999_999_8;

const LN_2: f64 = std::f64::consts::LN_2;
const LN_10: f64 = std::f64::consts::LN_10;
const LOG2_10: f64 = std::f64::consts::LOG2_10;

/// `2^k` for `-1022 ≤ k ≤ 1023`, exactly.
fn two(k: i64) -> f64 {
    f64::from_bits(((k + 1023) as u64) << 52)
}

impl Num {
    pub const ZERO: Num = Num { m: 0.0, e: 0 };
    pub const ONE: Num = Num { m: 1.0, e: 0 };

    fn huge(negative: bool) -> Num {
        Num {
            m: if negative { -ALMOST_2 } else { ALMOST_2 },
            e: MAX_E,
        }
    }

    /// `m × 2^e`, normalised.
    pub fn new(m: f64, e: i64) -> Num {
        if m == 0.0 || m.is_nan() {
            return Num::ZERO;
        }
        if m.is_infinite() {
            return Num::huge(m < 0.0);
        }
        let (mut m, mut e) = (m, e);
        if (m.to_bits() >> 52) & 0x7ff == 0 {
            // subnormal
            m *= two(54);
            e = e.saturating_sub(54);
        }
        let bits = m.to_bits();
        let k = ((bits >> 52) & 0x7ff) as i64 - 1023;
        let m = f64::from_bits((bits & !(0x7ffu64 << 52)) | (1023u64 << 52));
        let e = e.saturating_add(k);
        if e > MAX_E {
            return Num::huge(m < 0.0);
        }
        if e < -MAX_E {
            return Num::ZERO;
        }
        Num { m, e }
    }

    pub fn from_f64(x: f64) -> Num {
        Num::new(x, 0)
    }

    pub fn from_u64(x: u64) -> Num {
        Num::new(x as f64, 0)
    }

    /// `10^n`, by repeated products (exact while it fits a double's 53 bits).
    fn ten_pow(n: i64) -> Num {
        Num::from_f64(10.0).pow_int(n.unsigned_abs(), n < 0)
    }

    pub fn is_zero(self) -> bool {
        self.m == 0.0
    }

    pub fn is_positive(self) -> bool {
        self.m > 0.0
    }

    pub fn is_negative(self) -> bool {
        self.m < 0.0
    }

    /// The nearest double (infinite beyond `1.8e308`).
    pub fn to_f64(self) -> f64 {
        if self.m == 0.0 {
            return 0.0;
        }
        if self.e > 1023 {
            return if self.m > 0.0 { f64::INFINITY } else { f64::NEG_INFINITY };
        }
        if self.e < -1074 {
            return 0.0;
        }
        if self.e < -1022 {
            return self.m * two(self.e + 64) / two(64);
        }
        self.m * two(self.e)
    }

    /// The whole part as a count (0 for negatives, `u64::MAX` when too large).
    pub fn to_u64(self) -> u64 {
        if self.m <= 0.0 {
            return 0;
        }
        if self.e >= 64 {
            return u64::MAX;
        }
        self.to_f64().floor() as u64
    }

    pub fn abs(self) -> Num {
        Num {
            m: self.m.abs(),
            e: self.e,
        }
    }

    pub fn max(self, o: Num) -> Num {
        if self >= o {
            self
        } else {
            o
        }
    }

    pub fn min(self, o: Num) -> Num {
        if self <= o {
            self
        } else {
            o
        }
    }

    pub fn floor(self) -> Num {
        if self.e >= 52 {
            return self;
        }
        if self.e < 0 {
            return if self.m < 0.0 { Num::from_f64(-1.0) } else { Num::ZERO };
        }
        Num::from_f64(self.to_f64().floor())
    }

    pub fn ceil(self) -> Num {
        -(-self).floor()
    }

    pub fn round(self) -> Num {
        (self + Num::from_f64(0.5)).floor()
    }

    pub fn sqrt(self) -> Num {
        if self.m <= 0.0 {
            return Num::ZERO;
        }
        let (mut m, mut e) = (self.m, self.e);
        if e.rem_euclid(2) != 0 {
            m *= 2.0;
            e -= 1;
        }
        Num::new(m.sqrt(), e / 2)
    }

    /// Base-10 logarithm of the absolute value (`-inf` for zero).
    pub fn log10(self) -> f64 {
        self.ln() / LN_10
    }

    /// Natural logarithm of the absolute value (`-inf` for zero).
    pub fn ln(self) -> f64 {
        if self.m == 0.0 {
            return f64::NEG_INFINITY;
        }
        self.e as f64 * LN_2 + ln(self.m.abs())
    }

    /// `10^x`.
    pub fn pow10(x: f64) -> Num {
        Num::pow2(x * LOG2_10)
    }

    /// `2^x`.
    pub fn pow2(x: f64) -> Num {
        if x.is_nan() {
            return Num::ZERO;
        }
        if x > MAX_E as f64 {
            return Num::huge(false);
        }
        if x < -(MAX_E as f64) {
            return Num::ZERO;
        }
        let e = x.floor();
        Num::new(exp((x - e) * LN_2), e as i64)
    }

    fn pow_int(self, mut n: u64, invert: bool) -> Num {
        let mut base = self;
        let mut acc = Num::ONE;
        while n > 0 {
            if n & 1 == 1 {
                acc = acc * base;
            }
            n >>= 1;
            if n > 0 {
                base = base * base;
            }
        }
        if invert {
            Num::ONE / acc
        } else {
            acc
        }
    }

    /// `self^y`. Whole exponents are repeated products; others go through logarithms.
    /// Negative bases with a fractional exponent give zero.
    pub fn pow(self, y: Num) -> Num {
        if y.is_zero() {
            return Num::ONE;
        }
        if self.is_zero() {
            return if y.is_positive() { Num::ZERO } else { Num::huge(false) };
        }
        if y.floor() == y && y.e < 53 {
            return self.pow_int(y.abs().to_u64(), y.is_negative());
        }
        if self.is_negative() {
            return Num::ZERO;
        }
        Num::pow2(y.to_f64() * self.ln() / LN_2)
    }
}

/// Natural logarithm of a positive double, from basic operations only.
pub fn ln(x: f64) -> f64 {
    if x <= 0.0 || x.is_nan() {
        return f64::NEG_INFINITY;
    }
    if x.is_infinite() {
        return f64::INFINITY;
    }
    let (mut r, mut k) = (x, 0i64);
    while r > 1e16 {
        r /= 65_536.0;
        k += 16;
    }
    while r < 1e-16 {
        r *= 65_536.0;
        k -= 16;
    }
    while r > std::f64::consts::SQRT_2 {
        r /= 2.0;
        k += 1;
    }
    while r < std::f64::consts::FRAC_1_SQRT_2 {
        r *= 2.0;
        k -= 1;
    }
    // ln r = 2 atanh(s), s = (r - 1) / (r + 1), |s| < 0.172
    let s = (r - 1.0) / (r + 1.0);
    let s2 = s * s;
    let mut term = s;
    let mut sum = 0.0;
    let mut n = 1.0;
    for _ in 0..16 {
        sum += term / n;
        term *= s2;
        n += 2.0;
    }
    2.0 * sum + k as f64 * LN_2
}

/// `e^x` for moderate `x`, from basic operations only.
pub fn exp(x: f64) -> f64 {
    if x.is_nan() {
        return 0.0;
    }
    if x > 709.0 {
        return f64::INFINITY;
    }
    if x < -745.0 {
        return 0.0;
    }
    let k = (x / LN_2 + 0.5).floor();
    let r = x - k * LN_2;
    let mut term = 1.0;
    let mut sum = 1.0;
    for i in 1..=22 {
        term *= r / i as f64;
        sum += term;
    }
    let mut k = k as i64;
    while k > 0 {
        sum *= 2.0;
        k -= 1;
    }
    while k < 0 {
        sum /= 2.0;
        k += 1;
    }
    sum
}

impl Add for Num {
    type Output = Num;
    fn add(self, o: Num) -> Num {
        if self.m == 0.0 {
            return o;
        }
        if o.m == 0.0 {
            return self;
        }
        let (big, small) = if self.e >= o.e { (self, o) } else { (o, self) };
        let d = big.e - small.e;
        if d > 60 {
            return big;
        }
        Num::new(big.m + small.m * two(-d), big.e)
    }
}

impl Sub for Num {
    type Output = Num;
    fn sub(self, o: Num) -> Num {
        self + (-o)
    }
}

impl Neg for Num {
    type Output = Num;
    fn neg(self) -> Num {
        if self.m == 0.0 {
            self
        } else {
            Num { m: -self.m, e: self.e }
        }
    }
}

impl Mul for Num {
    type Output = Num;
    fn mul(self, o: Num) -> Num {
        if self.m == 0.0 || o.m == 0.0 {
            return Num::ZERO;
        }
        Num::new(self.m * o.m, self.e.saturating_add(o.e))
    }
}

/// Division by zero gives zero: a game should not stop on it.
impl Div for Num {
    type Output = Num;
    fn div(self, o: Num) -> Num {
        if self.m == 0.0 || o.m == 0.0 {
            return Num::ZERO;
        }
        Num::new(self.m / o.m, self.e.saturating_sub(o.e))
    }
}

impl PartialOrd for Num {
    fn partial_cmp(&self, o: &Num) -> Option<Ordering> {
        Some(self.total(o))
    }
}

impl Num {
    fn total(&self, o: &Num) -> Ordering {
        let sign = |n: &Num| {
            if n.m > 0.0 {
                1
            } else if n.m < 0.0 {
                -1
            } else {
                0
            }
        };
        let (a, b) = (sign(self), sign(o));
        if a != b || a == 0 {
            return a.cmp(&b);
        }
        let mag = self
            .e
            .cmp(&o.e)
            .then(self.m.abs().partial_cmp(&o.m.abs()).unwrap_or(Ordering::Equal));
        if a > 0 {
            mag
        } else {
            mag.reverse()
        }
    }
}

impl From<f64> for Num {
    fn from(x: f64) -> Num {
        Num::from_f64(x)
    }
}

impl From<u64> for Num {
    fn from(x: u64) -> Num {
        Num::from_u64(x)
    }
}

/// The exact form, which reads back to the same number: `42`, `1.5e300`, `-2.5e-3`, and beyond the
/// range of a double `1.25p2000` (1.25 × 2^2000).
impl fmt::Display for Num {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.m == 0.0 {
            return f.write_str("0");
        }
        if (-1022..=1023).contains(&self.e) {
            let x = self.to_f64();
            if x.abs() < 1e16 && x == x.floor() {
                return write!(f, "{}", x as i64);
            }
            let t = format!("{x:e}");
            return f.write_str(t.strip_suffix("e0").unwrap_or(&t));
        }
        write!(f, "{}p{}", self.m, self.e)
    }
}

impl FromStr for Num {
    type Err = String;
    fn from_str(s: &str) -> Result<Num, String> {
        let t: String = s.trim().chars().filter(|&c| c != '_').collect();
        let bad = || format!("`{s}` is not a number");
        if let Some((m, e)) = t.split_once('p') {
            let m: f64 = m.parse().map_err(|_| bad())?;
            let e: i64 = e.parse().map_err(|_| bad())?;
            if !m.is_finite() {
                return Err(bad());
            }
            return Ok(Num::new(m, e));
        }
        let x: f64 = t.parse().map_err(|_| bad())?;
        if x.is_finite()
            && (x != 0.0
                || !t
                    .bytes()
                    .take_while(|b| *b != b'e' && *b != b'E')
                    .any(|b| (b'1'..=b'9').contains(&b)))
        {
            return Ok(Num::from_f64(x));
        }
        if x.is_nan() {
            return Err(bad());
        }
        // Beyond a double: decimal mantissa times a power of ten.
        let (m, e) = match t.find(['e', 'E']) {
            Some(i) => (&t[..i], t[i + 1..].parse::<i64>().map_err(|_| bad())?),
            None => return Err(bad()),
        };
        let m: f64 = m.parse().map_err(|_| bad())?;
        Ok(Num::from_f64(m) * Num::ten_pow(e))
    }
}

impl Serialize for Num {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Num {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Num, D::Error> {
        struct V;
        impl Visitor<'_> for V {
            type Value = Num;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a number, or a string like \"1.5e300\"")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Num, E> {
                v.parse().map_err(E::custom)
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Num, E> {
                Ok(Num::from_f64(v))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Num, E> {
                Ok(Num::from_u64(v))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Num, E> {
                Ok(Num::from_f64(v as f64))
            }
        }
        d.deserialize_any(V)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(s: &str) -> Num {
        s.parse().unwrap()
    }

    #[test]
    fn arithmetic() {
        assert_eq!(n("2") + n("3"), n("5"));
        assert!(((n("1e400") * n("1e400")).log10() - 800.0).abs() < 1e-9);
        assert_eq!(n("5") - n("5"), Num::ZERO);
        assert!(n("1e500") > n("9e499"));
        assert!(n("-1e500") < n("-9e499"));
        assert!(n("-1") < Num::ZERO);
        assert_eq!(n("1e30") + n("1"), n("1e30"));
        assert_eq!(n("7") / Num::ZERO, Num::ZERO);
        assert_eq!(n("1234.5").floor(), n("1234"));
        assert_eq!(n("-0.5").floor(), n("-1"));
        assert_eq!(n("2").pow(n("10")), n("1024"));
        assert_eq!(n("1e4").sqrt(), n("100"));
        assert_eq!(n("1e5").sqrt().to_f64(), 1e5f64.sqrt());
        assert_eq!(n("0.1") + n("0.2"), Num::from_f64(0.1 + 0.2));
        assert_eq!(n("1.1") * n("3.3"), Num::from_f64(1.1 * 3.3));
        assert!(n("1e400") > n("1e399") && n("1e400").to_f64().is_infinite());
    }

    #[test]
    fn logs_and_powers() {
        assert!((ln(10.0) - LN_10).abs() < 1e-15);
        assert!((exp(1.0) - std::f64::consts::E).abs() < 1e-15);
        assert!((n("1e300").log10() - 300.0).abs() < 1e-12);
        let p = n("1e100").pow(n("0.5"));
        assert!((p.log10() - 50.0).abs() < 1e-12);
        let q = n("1.15").pow(n("100"));
        assert!((q.to_f64() / 1_174_313.450_700_4 - 1.0).abs() < 1e-12);
    }

    #[test]
    fn round_trip() {
        assert_eq!(n("1024").to_string(), "1024");
        assert_eq!(n("1.5e300").to_string(), "1.5e300");
        assert!((n("1e1000").log10() - 1000.0).abs() < 1e-9);
        for s in [
            "0",
            "1",
            "-2.5e-3",
            "1.2345678901234567e300",
            "9.999e999999",
            "1.25p2000",
            "5e-324",
        ] {
            let x = n(s);
            assert_eq!(x.to_string().parse::<Num>().unwrap(), x);
            let j = serde_json::to_string(&x).unwrap();
            assert_eq!(serde_json::from_str::<Num>(&j).unwrap(), x);
        }
        assert_eq!(serde_json::from_str::<Num>("12.5").unwrap(), n("12.5"));
    }
}
