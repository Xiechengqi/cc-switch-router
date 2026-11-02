-- Personal history is scoped by immutable user ID, never by a supplied email.
-- No new financial/history table and no rewrite of historical identity evidence.
CREATE INDEX idx_client_rental_history_buyer
    ON client_market_rental_identities(buyer_user_id, created_at DESC, id DESC);
