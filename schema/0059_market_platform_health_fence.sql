-- A fresh service-health sample is required after an entitlement is first
-- activated or resumed. Old healthy samples must not be reused as proof of
-- service after a billing pause.
ALTER TABLE market_platform_contracts ADD COLUMN health_fence_at TEXT;
