-- Channels are never physically deleted: old orders retain their credential
-- revision and reconciliation continues after the channel is archived.
ALTER TABLE market_payment_channels
    ADD COLUMN version INTEGER NOT NULL DEFAULT 1 CHECK (version > 0);
ALTER TABLE market_payment_channels
    ADD COLUMN api_reverify_required INTEGER NOT NULL DEFAULT 0
    CHECK (api_reverify_required IN (0, 1));
ALTER TABLE market_payment_channels ADD COLUMN archived_at TEXT;
ALTER TABLE market_payment_channels ADD COLUMN webhook_verified_at TEXT;
ALTER TABLE market_payment_channels ADD COLUMN webhook_verified_revision INTEGER;
ALTER TABLE market_payment_channels ADD COLUMN previous_webhook_revision INTEGER;
ALTER TABLE market_payment_channels ADD COLUMN previous_webhook_valid_until TEXT;
CREATE INDEX idx_market_payment_channels_archived
    ON market_payment_channels(archived_at, sort_order, id);
