//! CDK Amount
//!
//! Is any unit and will be treated as the unit of the wallet

use std::fmt;

use serde::{Deserialize, Serialize};

/// Amount can be any unit
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Amount(u64);

impl Amount {
    /// Amount zero
    pub const ZERO: Amount = Amount(0);

    /// Split into parts that are powers of two
    pub fn split(&self) -> Vec<Self> {
        let sats = self.0;
        (0_u64..64)
            .rev()
            .filter_map(|bit| {
                let part = 1 << bit;
                ((sats & part) == part).then_some(Self::from(part))
            })
            .collect()
    }

    /// Split into parts that are powers of two by target
    pub fn split_targeted(&self, target: &SplitTarget) -> Vec<Self> {
        let mut parts = match *target {
            SplitTarget::None => self.split(),
            SplitTarget::Value(amount) => {
                if self.le(&amount) {
                    return self.split();
                }

                let mut parts_total = Amount::ZERO;
                let mut parts = Vec::new();

                // The powers of two that are need to create target value
                let parts_of_value = amount.split();

                while parts_total.lt(self) {
                    for part in parts_of_value.iter().copied() {
                        if (part + parts_total).le(self) {
                            parts.push(part);
                        } else {
                            let amount_left = *self - parts_total;
                            parts.extend(amount_left.split());
                        }

                        parts_total = parts.clone().iter().copied().sum::<Amount>();

                        if parts_total.eq(self) {
                            break;
                        }
                    }
                }

                parts
            }
        };

        parts.sort();
        parts
    }
}

impl Default for Amount {
    fn default() -> Self {
        Amount::ZERO
    }
}

impl Default for &Amount {
    fn default() -> Self {
        &Amount::ZERO
    }
}

impl fmt::Display for Amount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<u64> for Amount {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

impl From<&u64> for Amount {
    fn from(value: &u64) -> Self {
        Self(*value)
    }
}

impl From<Amount> for u64 {
    fn from(value: Amount) -> Self {
        value.0
    }
}

impl AsRef<u64> for Amount {
    fn as_ref(&self) -> &u64 {
        &self.0
    }
}

impl std::ops::Add for Amount {
    type Output = Amount;

    fn add(self, rhs: Amount) -> Self::Output {
        Amount(self.0 + rhs.0)
    }
}

impl std::ops::AddAssign for Amount {
    fn add_assign(&mut self, rhs: Self) {
        self.0 += rhs.0;
    }
}

impl std::ops::Sub for Amount {
    type Output = Amount;

    fn sub(self, rhs: Amount) -> Self::Output {
        Amount(self.0 - rhs.0)
    }
}

impl std::ops::SubAssign for Amount {
    fn sub_assign(&mut self, other: Self) {
        self.0 -= other.0;
    }
}

impl std::ops::Mul for Amount {
    type Output = Self;

    fn mul(self, other: Self) -> Self::Output {
        Amount(self.0 * other.0)
    }
}

impl std::ops::Div for Amount {
    type Output = Self;

    fn div(self, other: Self) -> Self::Output {
        Amount(self.0 / other.0)
    }
}

impl core::iter::Sum for Amount {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        let sats: u64 = iter.map(|amt| amt.0).sum();
        Amount::from(sats)
    }
}

/// Kinds of targeting that are supported
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Serialize, Deserialize,
)]
pub enum SplitTarget {
    /// Default target; least amount of proofs
    #[default]
    None,
    /// Target amount for wallet to have most proofs that add up to value
    Value(Amount),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_amount() {
        assert_eq!(Amount::from(1).split(), vec![Amount::from(1)]);
        assert_eq!(Amount::from(2).split(), vec![Amount::from(2)]);
        assert_eq!(
            Amount::from(3).split(),
            vec![Amount::from(2), Amount::from(1)]
        );
        let amounts: Vec<Amount> = [8, 2, 1].iter().map(|a| Amount::from(*a)).collect();
        assert_eq!(Amount::from(11).split(), amounts);
        let amounts: Vec<Amount> = [128, 64, 32, 16, 8, 4, 2, 1]
            .iter()
            .map(|a| Amount::from(*a))
            .collect();
        assert_eq!(Amount::from(255).split(), amounts);
    }

    #[test]
    fn test_split_target_amount() {
        let amount = Amount(65);

        let split = amount.split_targeted(&SplitTarget::Value(Amount(32)));
        assert_eq!(vec![Amount(1), Amount(32), Amount(32)], split);

        let amount = Amount(150);

        let split = amount.split_targeted(&SplitTarget::Value(Amount::from(50)));
        assert_eq!(
            vec![
                Amount(2),
                Amount(2),
                Amount(2),
                Amount(16),
                Amount(16),
                Amount(16),
                Amount(32),
                Amount(32),
                Amount(32)
            ],
            split
        );

        let amount = Amount::from(63);

        let split = amount.split_targeted(&SplitTarget::Value(Amount::from(32)));
        assert_eq!(
            vec![
                Amount(1),
                Amount(2),
                Amount(4),
                Amount(8),
                Amount(16),
                Amount(32)
            ],
            split
        );
    }
}
