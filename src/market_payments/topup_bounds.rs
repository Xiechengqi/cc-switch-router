//! Top-up limits apply to wallet credit. The cent-rounded maintenance fee is
//! added afterwards and never reduces the permitted principal range.
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
        let lower = min.max(MIN_TOPUP_MINOR);
        let upper = max.min(MAX_TOPUP_MINOR);
        let feasible = lower <= upper;
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
            message:
                "wallet credit is outside this channel's range; the maintenance fee is additional"
                    .into(),
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
    fn topup_credit_limits_exclude_fees_at_every_supported_rate() {
        for fee_bps in [0, 500, 10_000] {
            let limits = TopupAmountLimits::new(100, 100_000, fee_bps, 7).unwrap();
            assert_eq!(limits.min_credit_minor, Some(100));
            assert_eq!(limits.max_credit_minor, Some(100_000));
            for credit in [100, 95_238, 95_239, 96_000, 100_000] {
                assert_eq!(
                    limits.validate_credit(credit).unwrap(),
                    topup_amounts(credit, fee_bps).unwrap()
                );
            }
            assert!(limits.validate_credit(99).is_err());
            assert!(limits.validate_credit(100_001).is_err());
        }
        let limits = TopupAmountLimits::new(100, 100_000, 500, 7).unwrap();
        assert_eq!(limits.validate_credit(96_000).unwrap(), (4_800, 100_800));
        assert_eq!(limits.validate_credit(100_000).unwrap(), (5_000, 105_000));
    }

    #[test]
    fn topup_credit_limits_preserve_channel_principal_bounds_and_round_fees_separately() {
        for fee_bps in [0, 1, 49, 50, 499, 500, 9_999, 10_000] {
            for (min, max) in [(100, 100_000), (1_000, 5_000), (104, 105), (199, 201)] {
                let limits = TopupAmountLimits::new(min, max, fee_bps, 1).unwrap();
                assert_eq!(
                    (limits.min_credit_minor, limits.max_credit_minor),
                    (Some(min), Some(max))
                );
                for credit in [min, max] {
                    let fee = (credit * fee_bps + 5_000) / 10_000;
                    assert_eq!(limits.validate_credit(credit).unwrap(), (fee, credit + fee));
                }
                assert!(limits.validate_credit(min - 1).is_err());
                assert!(limits.validate_credit(max + 1).is_err());
            }
        }
        assert_eq!(
            TopupAmountLimits::new(100, 100, 500, 1)
                .unwrap()
                .validate_credit(100)
                .unwrap(),
            (5, 105)
        );
    }

    #[test]
    fn topup_credit_limits_preserve_empty_ranges_and_reject_invalid_policy() {
        for (min, max, fee) in [(1, 99, 500), (100_001, 200_000, 10_000), (500, 100, 0)] {
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
