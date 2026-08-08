ALTER TABLE client_market_batches ADD COLUMN idempotency_key TEXT;
ALTER TABLE client_market_batches ADD COLUMN request_fingerprint TEXT NOT NULL DEFAULT '';

CREATE UNIQUE INDEX uq_client_market_batch_idempotency
    ON client_market_batches(client_user_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;

CREATE INDEX idx_client_market_batches_owner_created
    ON client_market_batches(client_user_id, created_at DESC);
CREATE INDEX idx_provisioning_jobs_batch_status
    ON provisioning_jobs(batch_id, status, created_at);
