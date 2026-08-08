CREATE INDEX idx_market_credit_accounts_buyer_currency_updated
    ON market_credit_accounts (buyer_user_id, currency, updated_at DESC);

CREATE INDEX idx_market_credit_accounts_supplier_currency_updated
    ON market_credit_accounts (supplier_user_id, currency, updated_at DESC);
