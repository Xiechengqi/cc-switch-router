-- A Host may be rented again while an earlier rental's compensation is disputed.
-- Workbench identity and pagination therefore use the original termination, not
-- the Host's current installation. No financial records are rewritten.
CREATE INDEX idx_client_termination_host_history
    ON client_market_terminations(host_id, cutoff_at DESC, id DESC);
CREATE INDEX idx_client_termination_provider_work
    ON client_market_terminations(provider_user_id, status, cutoff_at DESC, id DESC);
