//! Channel limits apply to the payment total, not the wallet credit. Invert
//! the same cent-rounded fee calculation used by quotes; never use floats.
use super::{AppError, MAX_TOPUP_MINOR, MIN_TOPUP_MINOR, StatusCode};
use crate::market_wallet_policy::topup_amounts;
use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TopupAmountLimits {
    pub min_topup_minor: i64,
    pub max_topup_minor: i64,
    pub min_credit_minor: Option<i64>,
    pub max_credit_minor: Option<i64>,
    pub maintenance_fee_bps: i64,
    pub policy_version: i64,
}

impl TopupAmountLimits {
    pub fn new(min: i64, max: i64, fee_bps: i64, policy_version: i64) -> Result<Self, AppError> {
        // Validate even an empty range so corrupt fee policies fail closed.
        topup_amounts(MIN_TOPUP_MINOR, fee_bps)?;
        let first = |after: &dyn Fn(i64) -> bool| -> Result<i64, AppError> {
            let (mut low, mut high) = (MIN_TOPUP_MINOR, MAX_TOPUP_MINOR + 1);
            while low < high {
                let mid = low + (high - low) / 2;
                let (_, pay) = topup_amounts(mid, fee_bps)?;
                if after(pay) {
                    high = mid;
                } else {
                    low = mid + 1;
                }
            }
            Ok(low)
        };
        let lower = first(&|pay| pay >= min)?;
        let upper = first(&|pay| pay > max)? - 1;
        let feasible = min <= max && lower <= upper;
        Ok(Self {
            min_topup_minor: min,
            max_topup_minor: max,
            min_credit_minor: feasible.then_some(lower),
            max_credit_minor: feasible.then_some(upper),
            maintenance_fee_bps: fee_bps,
            policy_version,
        })
    }

    pub fn validate_credit(&self, credit: i64) -> Result<(i64, i64), AppError> {
        let amounts = (MIN_TOPUP_MINOR..=MAX_TOPUP_MINOR)
            .contains(&credit)
            .then(|| topup_amounts(credit, self.maintenance_fee_bps))
            .transpose()?;
        if let (Some(min), Some(max), Some(amounts)) =
            (self.min_credit_minor, self.max_credit_minor, amounts)
            && (min..=max).contains(&credit)
        {
            return Ok(amounts);
        }
        Err(AppError::Coded {
            status: StatusCode::BAD_REQUEST,
            code: "MARKET_TOPUP_AMOUNT_OUT_OF_RANGE",
            message: "wallet credit is outside this channel's fee-adjusted range".into(),
            details: serde_json::json!({
                "minTopupMinor": self.min_topup_minor, "maxTopupMinor": self.max_topup_minor,
                "minCreditMinor": self.min_credit_minor, "maxCreditMinor": self.max_credit_minor,
                "maintenanceFeeBps": self.maintenance_fee_bps, "policyVersion": self.policy_version,
                "payMinor": amounts.map(|(_, pay)| pay),
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topup_credit_limits_include_fees_without_raising_channel_limits() {
        for (fee_bps, expected_max) in [(0, 100_000), (500, 95_238), (10_000, 50_000)] {
            let limits = TopupAmountLimits::new(100, 100_000, fee_bps, 7).unwrap();
            assert_eq!(limits.min_credit_minor, Some(100));
            assert_eq!(limits.max_credit_minor, Some(expected_max));
            assert!(limits.validate_credit(expected_max).is_ok());
            assert!(limits.validate_credit(expected_max + 1).is_err());
        }
        let limits = TopupAmountLimits::new(100, 100_000, 500, 7).unwrap();
        assert_eq!(limits.validate_credit(95_238).unwrap(), (4_762, 100_000));
        assert!(limits.validate_credit(95_239).is_err());
        assert!(limits.validate_credit(96_000).is_err());
    }

    #[test]
    fn topup_credit_limits_match_exact_cent_rounding_at_both_edges() {
        for fee_bps in [0, 1, 49, 50, 499, 500, 9_999, 10_000] {
            for (min, max) in [(100, 100_000), (1_000, 5_000), (104, 105), (199, 201)] {
                let limits = TopupAmountLimits::new(min, max, fee_bps, 1).unwrap();
                // An independent exhaustive oracle also covers one-cent gaps
                // where no principal produces a payment inside a narrow range.
                let mut eligible = (MIN_TOPUP_MINOR..=MAX_TOPUP_MINOR).filter(|credit| {
                    let pay = credit + (credit * fee_bps + 5_000) / 10_000;
                    (min..=max).contains(&pay)
                });
                let first = eligible.next();
                let last = eligible.last().or(first);
                assert_eq!(
                    (limits.min_credit_minor, limits.max_credit_minor),
                    (first, last)
                );
            }
        }
        assert_eq!(
            TopupAmountLimits::new(105, 105, 500, 1)
                .unwrap()
                .validate_credit(100)
                .unwrap(),
            (5, 105)
        );
    }

    #[test]
    fn topup_credit_limits_preserve_empty_ranges_and_reject_invalid_policy() {
        for (min, max, fee) in [(100, 100, 500), (201, 201, 10_000), (500, 100, 0)] {
            let limits = TopupAmountLimits::new(min, max, fee, 1).unwrap();
            assert_eq!(
                (limits.min_credit_minor, limits.max_credit_minor),
                (None, None)
            );
            assert!(limits.validate_credit(100).is_err());
        }
        assert!(TopupAmountLimits::new(100, 100_000, -1, 1).is_err());
        assert!(TopupAmountLimits::new(100, 100_000, 10_001, 1).is_err());
    }
}
