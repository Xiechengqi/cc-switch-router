CREATE INDEX IF NOT EXISTS idx_market_external_receipts_account_history
    ON market_external_payment_receipts(payment_account_id, confirmed_at DESC, id DESC);
