#![warn(rust_2018_idioms)]
#![cfg_attr(not(feature = "std"), no_std)]
use core::convert::TryFrom;
use core::str::FromStr;
use core::{fmt, i64, marker::PhantomData, mem};

use derive_more::Display;
#[cfg(feature = "std")]
use derive_more::Error;

use serde::{Deserialize, Serialize};
use typenum::Unsigned;

#[cfg(feature = "i128")]
use crate::i256::I256;
use crate::ops::{
    CheckedAdd, CheckedMul, CheckedSub, Numeric, RoundMode, RoundingDiv, RoundingMul,
};

#[cfg(feature = "i128")]
mod i256;
pub mod ops;
mod power_table;
#[cfg(test)]
mod tests;

type Result<T, E = ArithmeticError> = core::result::Result<T, E>;
pub use typenum;

/// Abstraction over fixed point floating numbers.
///
/// The internal representation is a fixed point decimal number,
/// i.e. a value pre-multiplied by 10^N, where N is a pre-defined number.
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct FixedPoint<I, P> {
    inner: I,
    _marker: PhantomData<P>,
}

pub trait Precision: Unsigned {}
impl<U: Unsigned> Precision for U {}

/////////////////////////////////////////////////////////

#[cfg_attr(feature = "std", derive(Error))]
#[derive(Debug, derive_more::Display, PartialEq)]
pub enum ArithmeticError {
    #[cfg_attr(feature = "std", display(fmt = "overflow"))]
    Overflow,
    #[cfg_attr(feature = "std", display(fmt = "division by zero"))]
    DivisionByZero,
}

#[cfg_attr(feature = "std", derive(Error))]
#[derive(Debug, Display, PartialEq)]
pub enum FromDecimalError {
    #[cfg_attr(feature = "std", display(fmt = "unsupported exponent"))]
    UnsupportedExponent,
    #[cfg_attr(feature = "std", display(fmt = "too big mantissa"))]
    TooBigMantissa,
}

#[cfg_attr(feature = "std", derive(Error))]
#[derive(Debug, Display, PartialEq)]
pub enum ConvertError {
    #[cfg_attr(feature = "std", display(fmt = "overflow"))]
    Overflow,
    #[cfg_attr(feature = "std", display(fmt = "other: {}", _0))]
    Other(#[error(not(source))] &'static str),
}

macro_rules! pow10 {
    ($convert:expr, $rhs:expr) => {{
        let mut result = 1;
        let mut i = $rhs;
        while i > 0 {
            result *= 10;
            i -= 1;
        }
        $convert(result)
    }};
}

macro_rules! impl_fixed_point {
    (
        inner = $layout:tt;
        promoted_to = $promotion:tt;
        convert = $convert:expr;
        from = [$($from:ty),*];
        try_from = [$($try_from:ty),*];
    ) => {
        impl<P> FixedPoint<$layout, P> {
            pub const fn from_bits(inner: $layout) -> Self {
                FixedPoint {
                    inner,
                    _marker: PhantomData,
                }
            }
        }

        impl<P: Precision> FixedPoint<$layout, P> {
            pub const PRECISION: i32 = P::I32;
            pub const EPSILON: Self = Self::from_bits(1);

            const COEF: $layout = pow10!(identity, Self::PRECISION);
            const COEF_PROMOTED: $promotion = pow10!($convert, Self::PRECISION);

            // TODO
            //pub const HALF: Self = Self::from_bits(Self::COEF / 2);
            //pub const MAX_MINUS_ONE: Self = Self::from_bits(i64::MAX - 1);
            //pub const MINUS_ONE: Self = Self::from_bits(-COEF);
        }

        impl<P: Precision> Numeric for FixedPoint<$layout, P> {
            const ZERO: Self = Self::from_bits(0);
            const ONE: Self = Self::from_bits(Self::COEF);
            const MIN: Self = Self::from_bits($layout::MIN);
            const MAX: Self = Self::from_bits($layout::MAX);
        }

        impl<P: Precision> RoundingMul for FixedPoint<$layout, P> {
            type Output = FixedPoint<$layout, P>;
            type Error = ArithmeticError;

            #[inline]
            fn rmul(self, rhs: Self, mode: RoundMode) -> Result<Self> {
                // TODO(loyd): avoid 128bit arithmetic when possible,
                //      because LLVM doesn't replace 128bit division by const with multiplication.

                let value = $promotion::from(self.inner) * $promotion::from(rhs.inner);
                // TODO: replace with multiplication by constant
                let result = value / Self::COEF_PROMOTED;
                let loss = value - result * Self::COEF_PROMOTED;
                let sign = self.inner.signum() * rhs.inner.signum();

                let mut result =
                    $layout::try_from(result).map_err(|_| ArithmeticError::Overflow)?;

                if loss != $convert(0) && mode as i32 == sign as i32 {
                    result = result.checked_add(sign).ok_or(ArithmeticError::Overflow)?;
                }

                Ok(Self::from_bits(result))
            }
        }

        impl<P: Precision> RoundingDiv for FixedPoint<$layout, P> {
            type Output = FixedPoint<$layout, P>;
            type Error = ArithmeticError;

            #[inline]
            fn rdiv(self, rhs: Self, mode: RoundMode) -> Result<Self> {
                // TODO(loyd): avoid 128bit arithmetic when possible,
                //      because LLVM doesn't replace 128bit division by const with multiplication.

                if rhs.inner == 0 {
                    return Err(ArithmeticError::DivisionByZero);
                }

                let numerator = $promotion::from(self.inner) * Self::COEF_PROMOTED;
                let denominator = $promotion::from(rhs.inner);
                let result = numerator / denominator;
                let loss = numerator - result * denominator;

                let mut result =
                    $layout::try_from(result).map_err(|_| ArithmeticError::Overflow)?;

                if loss != $convert(0) {
                    let sign = self.inner.signum() * rhs.inner.signum();

                    if mode as i32 == sign as i32 {
                        result = result.checked_add(sign).ok_or(ArithmeticError::Overflow)?;
                    }
                }

                Ok(Self::from_bits(result))
            }
        }

        impl<P: Precision> RoundingDiv<$layout> for FixedPoint<$layout, P> {
            type Output = FixedPoint<$layout, P>;
            type Error = ArithmeticError;

            #[inline]
            fn rdiv(self, rhs: $layout, mode: RoundMode) -> Result<FixedPoint<$layout, P>> {
                if rhs == 0 {
                    return Err(ArithmeticError::DivisionByZero);
                }

                let numerator = self.inner;
                let denominator = rhs;
                let mut result = numerator / denominator;
                let loss = numerator - result * denominator;

                if loss != 0 {
                    let sign = numerator.signum() * denominator.signum();

                    if mode as i32 == sign as i32 {
                        result = result.checked_add(sign).ok_or(ArithmeticError::Overflow)?;
                    }
                }

                Ok(Self::from_bits(result))
            }
        }

        impl<P: Precision> CheckedAdd for FixedPoint<$layout, P> {
            type Output = FixedPoint<$layout, P>;
            type Error = ArithmeticError;

            #[inline]
            fn cadd(self, rhs: FixedPoint<$layout, P>) -> Result<FixedPoint<$layout, P>> {
                self.inner
                    .checked_add(rhs.inner)
                    .map(Self::from_bits)
                    .ok_or(ArithmeticError::Overflow)
            }
        }

        impl<P: Precision> CheckedSub for FixedPoint<$layout, P> {
            type Output = FixedPoint<$layout, P>;
            type Error = ArithmeticError;

            #[inline]
            fn csub(self, rhs: FixedPoint<$layout, P>) -> Result<FixedPoint<$layout, P>> {
                self.inner
                    .checked_sub(rhs.inner)
                    .map(Self::from_bits)
                    .ok_or(ArithmeticError::Overflow)
            }
        }

        impl<P: Precision> CheckedMul<$layout> for FixedPoint<$layout, P> {
            type Output = FixedPoint<$layout, P>;
            type Error = ArithmeticError;

            #[inline]
            fn cmul(self, rhs: $layout) -> Result<FixedPoint<$layout, P>> {
                self.inner
                    .checked_mul(rhs)
                    .map(Self::from_bits)
                    .ok_or(ArithmeticError::Overflow)
            }
        }

        impl<P: Precision> FixedPoint<$layout, P> {
            #[inline]
            pub fn recip(self, mode: RoundMode) -> Result<FixedPoint<$layout, P>> {
                Self::ONE.rdiv(self, mode)
            }

            #[inline]
            pub fn cneg(self) -> Result<FixedPoint<$layout, P>> {
                self.inner
                    .checked_neg()
                    .map(Self::from_bits)
                    .ok_or_else(|| ArithmeticError::Overflow)
            }

            #[inline]
            pub fn half_sum(
                a: FixedPoint<$layout, P>,
                b: FixedPoint<$layout, P>,
            ) -> FixedPoint<$layout, P> {
                if a.inner.signum() != b.inner.signum() {
                    Self::from_bits((a.inner + b.inner) / 2)
                } else {
                    let min = a.inner.min(b.inner);
                    let max = a.inner.max(b.inner);
                    Self::from_bits(min + (max - min) / 2)
                }
            }

            #[inline]
            pub fn integral(self, mode: RoundMode) -> $layout {
                let sign = self.inner.signum();
                let (int, frac) = (self.inner / Self::COEF, self.inner.abs() % Self::COEF);

                if mode as i32 == sign as i32 && frac > 0 {
                    int + sign
                } else {
                    int
                }
            }

            #[inline]
            pub fn round_towards_zero_by(
                self,
                precision: FixedPoint<$layout, P>,
            ) -> FixedPoint<$layout, P> {
                self.inner
                    .checked_div(precision.inner)
                    .and_then(|v| v.checked_mul(precision.inner))
                    .map_or(self, Self::from_bits)
            }

            pub fn next_power_of_ten(self) -> Result<FixedPoint<$layout, P>> {
                if self.inner < 0 {
                    return self.cneg()?.next_power_of_ten()?.cneg();
                }

                let lz = self.inner.leading_zeros() as usize;
                assert!(lz > 0, "unexpected negative value");

                let value = Self::power_of_ten_by_leading_zeros(lz);

                let value = if self.inner > value {
                    Self::power_of_ten_by_leading_zeros(lz - 1)
                } else {
                    value
                };

                if value == 0 {
                    return Err(ArithmeticError::Overflow);
                }

                // TODO
                Ok(Self::from_bits(value as $layout))
            }

            fn power_of_ten_by_leading_zeros(lz: usize) -> $layout {
                use crate::power_table::POWER_TABLE;
                let value = POWER_TABLE[mem::size_of::<$layout>() * 8 - lz];
                const LAYOUT_MAX: i128 = $layout::MAX as i128;
                if value > LAYOUT_MAX { 0 } else { value as $layout }
            }

            pub fn rounding_from_f64(value: f64) -> Result<FixedPoint<$layout, P>> {
                let x = (value * Self::COEF as f64).round();
                if x >= ($layout::MIN as f64) && x <= ($layout::MAX as f64) {
                    Ok(Self::from_bits(x as $layout))
                } else {
                    Err(ArithmeticError::Overflow)
                }
            }

            pub fn to_f64(self) -> f64 {
                (self.inner as f64) / (Self::COEF as f64)
            }

            // TODO
            pub fn rounding_to_i64(self) -> i64 {
                let x = if self.inner > 0 {
                    self.inner + Self::COEF / 2
                } else {
                    self.inner - Self::COEF / 2
                };
                (x / Self::COEF) as i64
            }
        }

        impl<P: Precision> fmt::Debug for FixedPoint<$layout, P> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self)
            }
        }

        impl<P: Precision> fmt::Display for FixedPoint<$layout, P> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                let sign = self.inner.signum();
                let integral = (self.inner / Self::COEF).abs();
                let mut fractional = (self.inner % Self::COEF).abs();
                let mut frac_width = if fractional > 0 {
                    Self::PRECISION as usize
                } else {
                    0
                };

                while fractional > 0 && fractional % 10 == 0 {
                    fractional /= 10;
                    frac_width -= 1;
                }

                write!(
                    f,
                    "{}{}.{:0width$}",
                    if sign < 0 { "-" } else { "" },
                    integral,
                    fractional,
                    width = frac_width
                )
            }
        }

        impl<P: Precision> FixedPoint<$layout, P> {
            pub fn from_decimal(
                mantissa: $layout,
                exponent: i32,
            ) -> Result<FixedPoint<$layout, P>, FromDecimalError> {
                if exponent < -Self::PRECISION || exponent > 10 {
                    return Err(FromDecimalError::UnsupportedExponent);
                }

                let ten: $layout = 10;
                let multiplier = ten.pow((exponent + Self::PRECISION) as u32);

                mantissa
                    .checked_mul(multiplier)
                    .map(Self::from_bits)
                    .map_or_else(|| Err(FromDecimalError::TooBigMantissa), Ok)
            }
        }

        $(
            impl<P: Precision> TryFrom<$try_from> for FixedPoint<$layout, P> {
                type Error = ConvertError;

                fn try_from(value: $try_from) -> Result<Self, Self::Error> {
                    $layout::try_from(value)
                        .map_err(|_| ConvertError::Overflow)?
                        .checked_mul(Self::COEF)
                        .map(Self::from_bits)
                        .ok_or(ConvertError::Overflow)
                }
            }
        )*
        $(
            /// Returns `FixedPoint<$layout, P>` corresponding to the integer `value`.
            impl<P: Precision> From<$from> for FixedPoint<$layout, P> {
                fn from(value: $from) -> Self {
                    Self::from_bits($layout::from(value) * Self::COEF)
                }
            }
        )*

        impl<P: Precision> FromStr for FixedPoint<$layout, P> {
            type Err = ConvertError;

            fn from_str(str: &str) -> Result<Self, Self::Err> {
                let str = str.trim();
                let coef = Self::COEF;

                let index = match str.find('.') {
                    Some(index) => index,
                    None => {
                        let integral: $layout = str.parse().map_err(|_| {
                            ConvertError::Other("can't parse integral part of the str")
                        })?;
                        return integral
                            .checked_mul(coef)
                            .ok_or(ConvertError::Overflow)
                            .map(Self::from_bits);
                    }
                };

                let integral: $layout = str[0..index]
                    .parse()
                    .map_err(|_| ConvertError::Other("can't parse integral part"))?;
                let fractional_str = &str[index + 1..];

                if !fractional_str.chars().all(|c| c.is_digit(10)) {
                    return Err(ConvertError::Other("can't parse fractional part: must contain digits only"));
                }

                if fractional_str.len() > Self::PRECISION.abs() as usize {
                    return Err(ConvertError::Other("requested precision is too high"));
                }

                let ten: $layout = 10;
                let exp = ten.pow(fractional_str.len() as u32);

                if exp > coef {
                    return Err(ConvertError::Other("requested precision is too high"));
                }

                let fractional: $layout = fractional_str.parse().map_err(|_| {
                    ConvertError::Other("can't parse fractional part")
                })?;

                let final_integral = integral.checked_mul(coef).ok_or(ConvertError::Overflow)?;
                let signum = if str.as_bytes()[0] == b'-' { -1 } else { 1 };
                let final_fractional = signum * coef / exp * fractional;

                final_integral
                    .checked_add(final_fractional)
                    .map(Self::from_bits)
                    .ok_or(ConvertError::Overflow)
            }
        }
    };
}

const fn identity<T>(x: T) -> T {
    x
}

impl_fixed_point!(
    inner = i16;
    promoted_to = i32;
    convert = identity;
    from = [i8, u8];
    try_from = [i16, u16, i32, u32, i64, u64, i128, u128, isize, usize];
);
impl_fixed_point!(
    inner = i32;
    promoted_to = i64;
    convert = identity;
    from = [i8, u8, i16, u16];
    try_from = [i32, u32, i64, u64, i128, u128, isize, usize];
);
impl_fixed_point!(
    inner = i64;
    promoted_to = i128;
    convert = identity;
    from = [i8, u8, i16, u16, i32, u32];
    try_from = [i64, u64, i128, u128, isize, usize];
);
#[cfg(feature = "i128")]
impl_fixed_point!(
    inner = i128;
    promoted_to = I256;
    convert = I256::from_i128;
    from = [i8, u8, i16, u16, i32, u32, i64, u64];
    try_from = [i128, u128, isize, usize];
);
